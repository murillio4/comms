use std::io;
use std::net::SocketAddr;

use futures::channel::oneshot;
use mio::{Evented, Events, Poll, PollOpt, Ready, Registration, Token, SetReadiness};
use mio::net::UdpSocket;
use voice_lib::{packet, SerializeError, serialize};
use std::sync::mpsc::{Receiver, Sender, SendError, RecvError};
use std::sync::mpsc;
use ringbuf::{Producer, Consumer, RingBuffer};
use voice_lib::packet::{Packet, AudioToClient};
use bincode::deserialize;
use std::collections::{HashMap, VecDeque};

trait RawPacket {
    fn header_and_data(&self) -> (u8, &[u8]);
}

impl RawPacket for &[u8] {
    fn header_and_data(&self) -> (u8, &[u8]) {
        (self[0], &self[1..])
    }
}

pub struct Worker {
    server_address: SocketAddr,
    audio_sender: Sender<AudioCommand>,
    receiver: Receiver<Command>,
    registration: Registration,
}

impl Worker {
    pub fn run(self) -> Result<(), NetworkError> {
        let socket = UdpSocket::bind(&"0.0.0.0:0".parse().unwrap())?;
        socket.connect(self.server_address)?;

        let command_queue = CommandQueue::new(self.receiver, self.registration);
        let mut send_to_audio_thread = SendToAudioThread::new(self.audio_sender);

        let network_token = Token(0);
        let command_token = Token(1);
        let poll = Poll::new()?;
        poll.register(&socket, network_token, Ready::all(), PollOpt::edge())?;
        poll.register(&command_queue, command_token, Ready::readable(), PollOpt::edge())?;

        let mut socket_writable = false;
        let mut outgoing_packet_queue = VecDeque::<Vec<u8>>::new();
        let mut expected_response_map = HashMap::<u8, oneshot::Sender<Vec<u8>>>::new();

        let mut buffer = [0; 65535];
        let mut events = Events::with_capacity(1024);

        loop {
            poll.poll(&mut events, None)?;
            for event in events.iter() {
                let readiness = event.readiness();
                let token = event.token();
                if token == network_token {
                    if readiness.is_readable() {
                        loop {
                            match socket.recv(&mut buffer) {
                                Ok(size) => {
                                    if size == 0 {
                                        continue;
                                    }

                                    let raw_packet = &buffer[0..size];
                                    let (header, data) = raw_packet.header_and_data();
                                    if header == packet::AudioToClient::HEADER_TYPE {
                                        let audio_to_client: AudioToClient = match deserialize(data) {
                                            Ok(packet) => packet,
                                            Err(_) => {
                                                println!("Failed to parse AudioToClient packet, ignoring");
                                                continue;
                                            }
                                        };
                                        send_to_audio_thread.send(audio_to_client.author, audio_to_client.bytes)?;
                                    } else if let Some(sender) = expected_response_map.remove(&header) {
                                        sender.send(data.to_vec()).unwrap();
                                    } // TODO track unexpected packets
                                }
                                Err(e) => {
                                    if e.kind() == io::ErrorKind::WouldBlock {
                                        break;
                                    } else {
                                        return Err(e.into());
                                    }
                                }
                            }
                        }
                    }
                    if readiness.is_writable() {
                        socket_writable = true;
                        attempt_send_next_packet(&mut outgoing_packet_queue, &socket, &mut socket_writable)?;
                    }
                } else if token == command_token && readiness.is_readable() {
                    let packet = match command_queue.next() {
                        Some(packet) => packet,
                        None => {
                            // This should never happen since handle_command is only called when
                            // set_ready is called.
                            // TODO track if/when this happens, to confirm this assumption.
                            return Ok(());
                        }
                    };

                    match packet {
                        Command::OneDirection(raw_packet) => {
                            outgoing_packet_queue.push_back(raw_packet);
                        },
                        Command::Request {
                            outgoing,
                            expected_response_packet,
                            send_response,
                        } => {
                            outgoing_packet_queue.push_back(outgoing);
                            expected_response_map.insert(expected_response_packet, send_response);
                        }
                    }

                    attempt_send_next_packet(&mut outgoing_packet_queue, &socket, &mut socket_writable)?;
                }
            }
        }
    }
}

fn attempt_send_next_packet(outgoing_packet_queue: &mut VecDeque<Vec<u8>>, socket: &UdpSocket, socket_writable: &mut bool) -> Result<(), NetworkError> {
    if !*socket_writable {
        return Ok(())
    }

    let packet = match outgoing_packet_queue.get(0) {
        Some(packet) => packet,
        None => {
            return Ok(());
        }
    };

    match socket.send(packet) {
        Ok(_) => {},
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // TODO track when this would happen. If it does, what about socket_writable??
            println!("EWouldBlock on OutgoingPacketQueue, dropping packet");
            return Ok(());
        },
        Err(e) => return Err(e.into())
    };

    outgoing_packet_queue.pop_front();
    *socket_writable = false;
    Ok(())
}

#[derive(Debug)]
pub enum Command {
    OneDirection(Vec<u8>),
    Request {
        outgoing: Vec<u8>,
        expected_response_packet: u8,
        send_response: oneshot::Sender<Vec<u8>>,
    }
}

pub struct Handle {
    sender: Sender<Command>,
    set_readiness: SetReadiness,
}

impl Handle {
    pub fn new_handle_and_worker(server_address: SocketAddr, audio_sender: Sender<AudioCommand>) -> (Self, Worker) {
        let (sender, receiver) = mpsc::channel();
        let (registration, set_readiness) = Registration::new2();
        return (
            Handle {
                sender,
                set_readiness,
            },
            Worker {
                server_address,
                audio_sender,
                receiver,
                registration,
            }
        )
    }

    fn send(&self, command: Command) -> Result<(), SendToNetworkThreadError> {
        self.sender.send(command)?;
        self.set_readiness.set_readiness(Ready::readable())?;
        Ok(())
    }

    pub fn send_single<'a>(&self, packet: impl Packet<'a>) -> Result<(), SendToNetworkThreadError> {
        let raw_packet = serialize(packet)?;
        self.send(Command::OneDirection(raw_packet))
    }

    pub fn request<'a>(&self, outgoing_packet: impl Packet<'a>, expected_response_packet: u8, send_response: oneshot::Sender<Vec<u8>>) -> Result<(), SendToNetworkThreadError> {
        let raw_packet = serialize(outgoing_packet)?;
        self.send(Command::Request {
            outgoing: raw_packet,
            expected_response_packet,
            send_response
        })
    }
}

pub struct CommandQueue {
    command_queue: Receiver<Command>,
    registration: Registration,
}

impl CommandQueue {
    fn new(command_queue: Receiver<Command>, registration: Registration) -> CommandQueue {
        CommandQueue {
            command_queue,
            registration,
        }
    }

    fn next(&self) -> Option<Command> {
        self.command_queue.try_recv().ok()
    }
}

impl Evented for CommandQueue {
    fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        poll.deregister(self)
    }
}

pub enum AudioCommand {
    NewClientStream(String, Consumer<i16>),
//    RemoveClientStream(String),
}

pub struct SendToAudioThread {
    command_tx: Sender<AudioCommand>,
    client_streams: HashMap<String, Producer<i16>>,
}

impl SendToAudioThread {
    fn new(command_tx: Sender<AudioCommand>) -> SendToAudioThread {
        SendToAudioThread {
            command_tx,
            client_streams: HashMap::new(),
        }
    }

    fn send(&mut self, id: String, bytes: &[u8]) -> Result<(), NetworkError> {
        let l16le_samples: Vec<i16> = bytes
            .chunks(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        match self.client_streams.get_mut(&id.clone()) {
            Some(producer) => {
                producer.push_slice(&l16le_samples);
            },
            None => {
                let ring = RingBuffer::new(4800);
                let (mut producer, consumer) = ring.split();
                self.command_tx.send(AudioCommand::NewClientStream(id.clone(), consumer))?;
                producer.push_slice(&l16le_samples);
                self.client_streams.insert(id.clone(), producer);
            }
        };
        Ok(())
    }
}

#[derive(Debug)]
pub enum NetworkError {
    Io(Option<io::Error>),
    SendCommandToAudioThread(SendError<AudioCommand>),
    ReceiveFromAudioThread(RecvError),
}

impl From<io::Error> for NetworkError {
    fn from(e: io::Error) -> Self {
        NetworkError::Io(Some(e))
    }
}

impl From<SendError<AudioCommand>> for NetworkError {
    fn from(e: SendError<AudioCommand>) -> Self {
        NetworkError::SendCommandToAudioThread(e)
    }
}

impl From<RecvError> for NetworkError {
    fn from(e: RecvError) -> Self {
        NetworkError::ReceiveFromAudioThread(e)
    }
}

#[derive(Debug)]
pub enum SendToNetworkThreadError {
    SerializeFailed(SerializeError),
    SendFailed(SendError<Command>),
    SetReadinessFailed(io::Error),
}

impl From<SerializeError> for SendToNetworkThreadError {
    fn from(e: SerializeError) -> Self {
        SendToNetworkThreadError::SerializeFailed(e)
    }
}

impl From<SendError<Command>> for SendToNetworkThreadError {
    fn from(e: SendError<Command>) -> Self {
        SendToNetworkThreadError::SendFailed(e)
    }
}

impl From<io::Error> for SendToNetworkThreadError {
    fn from(e: io::Error) -> Self {
        SendToNetworkThreadError::SetReadinessFailed(e)
    }
}