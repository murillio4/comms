use std::io;
use std::net::SocketAddr;

use mio::{Evented, Events, Poll, PollOpt, Ready, Registration, Token, SetReadiness};
use mio::net::UdpSocket;
use voice_lib::{packet, SerializeError, serialize};
use std::sync::mpsc::{Receiver, Sender, SendError, RecvError};
use std::sync::{Arc, mpsc};
use std::sync::atomic::{AtomicBool, Ordering, AtomicUsize};
use ringbuf::{Producer, Consumer, RingBuffer};
use voice_lib::packet::{Packet, AudioToClient};
use bincode::deserialize;
use std::collections::HashMap;

pub struct NetworkConnectionWorker {
    server_address: SocketAddr,
    audio_sender: Sender<AudioCommand>,
    packet_queue: OutgoingPacketQueue,
}

impl NetworkConnectionWorker {
    pub fn run(self) -> Result<(), NetworkError> {
        let socket = UdpSocket::bind(&"0.0.0.0:0".parse().unwrap())?;
        socket.connect(self.server_address)?;

        let packet_queue = self.packet_queue;
        let mut send_to_audio_thread = SendToAudioThread::new(self.audio_sender);

        let network_token = Token(0);
        let audio_token = Token(1);
        let poll = Poll::new()?;
        poll.register(&socket, network_token, Ready::all(), PollOpt::edge())?;
        poll.register(&packet_queue, audio_token, Ready::readable(), PollOpt::edge())?;

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

                                    let buffer = &buffer[0..size];
                                    let header_type = buffer[0];
                                    match header_type {
                                        packet::Connect::HEADER_TYPE => {
                                            println!("-> Connected!");
                                        },
                                        packet::AudioToClient::HEADER_TYPE => {
                                            let audio_to_client: AudioToClient = match deserialize(&buffer[1..size]) {
                                                Ok(packet) => packet,
                                                Err(_) => {
                                                    println!("Failed to parse AudioToClient packet, ignoring");
                                                    continue;
                                                }
                                            };
                                            send_to_audio_thread.send(audio_to_client.author, audio_to_client.bytes)?;
                                        },
                                        _ => ()
                                    }
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
                        packet_queue.socket_writable.store(true, Ordering::Relaxed);
                        if !packet_queue.is_empty() {
                            packet_queue.send_one(&socket)?;
                        }
                    }
                } else if token == audio_token && readiness.is_readable() {
                    packet_queue.send_one(&socket)?;
                }
            }
        }
    }
}

pub struct NetworkConnectionHandle {
    sender: Sender<Vec<u8>>, // todo make Vec<NetworkCommand> (where NetworkCommand is probably "send a packet")
    set_readiness: SetReadiness,
    queue_size: Arc<AtomicUsize>,
}

impl NetworkConnectionHandle {
    pub fn new_handle_and_worker(server_address: SocketAddr, audio_sender: Sender<AudioCommand>) -> (Self, NetworkConnectionWorker) {
        let (sender, receiver) = mpsc::channel();
        let (registration, set_readiness) = Registration::new2();
        let queue_size = Arc::new(AtomicUsize::new(0));
        return (
            NetworkConnectionHandle {
                sender,
                set_readiness,
                queue_size: queue_size.clone(),
            },
            NetworkConnectionWorker {
                server_address,
                audio_sender,
                packet_queue: OutgoingPacketQueue::new(receiver, registration, queue_size),
            }
        )
    }

    pub fn send<'a>(&self, packet: impl Packet<'a>) -> Result<(), SendToNetworkThreadError> {
        let raw_packet = serialize(packet)?;
        self.sender.send(raw_packet)?;
        self.set_readiness.set_readiness(Ready::readable())?;
        self.queue_size.fetch_add(1, Ordering::Relaxed);
        Ok(())
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

pub struct OutgoingPacketQueue {
    audio_thread_receiver: Receiver<Vec<u8>>,
    registration: Registration,
    socket_writable: AtomicBool,
    size: Arc<AtomicUsize>,
}
unsafe impl Send for OutgoingPacketQueue {}

impl OutgoingPacketQueue {
    pub fn new(receiver: Receiver<Vec<u8>>, registration: Registration, size: Arc<AtomicUsize>) -> OutgoingPacketQueue {
        OutgoingPacketQueue {
            audio_thread_receiver: receiver,
            registration,
            socket_writable: AtomicBool::new(false),
            size,
        }
    }

    fn send_one(&self, socket: &UdpSocket)-> Result<(), NetworkError> {
        if !self.socket_writable.load(Ordering::Relaxed) {
            return Ok(())
        }

        let packet = match self.audio_thread_receiver.try_recv() {
            Ok(packet) => packet,
            Err(_) => {
                // todo Audio thread doesn't actually have data, ignoring missing packet
                return Ok(());
            }
        };

        self.size.fetch_sub(1, Ordering::Relaxed);
        match socket.send(&packet) {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                println!("EWouldBlock on OutgoingPacketQueue, dropping packet");
                return Ok(());
            },
            Err(e) => return Err(e.into())
        };
        self.socket_writable.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.size.load(Ordering::Relaxed) == 0
    }
}

impl Evented for OutgoingPacketQueue {
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
    SendFailed(SendError<Vec<u8>>),
    SetReadinessFailed(io::Error),
}

impl From<SerializeError> for SendToNetworkThreadError {
    fn from(e: SerializeError) -> Self {
        SendToNetworkThreadError::SerializeFailed(e)
    }
}

impl From<SendError<Vec<u8>>> for SendToNetworkThreadError {
    fn from(e: SendError<Vec<u8>>) -> Self {
        SendToNetworkThreadError::SendFailed(e)
    }
}

impl From<io::Error> for SendToNetworkThreadError {
    fn from(e: io::Error) -> Self {
        SendToNetworkThreadError::SetReadinessFailed(e)
    }
}