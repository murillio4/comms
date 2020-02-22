use std::io;

use serde::Serialize;
use mio::{Evented, Events, Poll, PollOpt, Ready, Registration, Token, SetReadiness};
use mio::net::UdpSocket;
use voice_lib::{packet, SerializeError, WritePacketBuilder, PacketHeader, HeaderParsedPacket, OwnedHeaderParsedPacket, DeserializeError};
use std::sync::mpsc::{Receiver, Sender, SendError, RecvError};
use std::sync::mpsc;
use ringbuf::{Producer, Consumer, RingBuffer};
use voice_lib::packet::{PacketBody, AudioToClient, PacketKind, ClientId};
use std::collections::{HashMap, VecDeque};
use crate::network::future::SendResponse;
use std::fmt::{Display, Formatter, Error};
use std::net::SocketAddr;
use std::num::NonZeroU64;

pub mod future;

pub struct Connection {
    socket: UdpSocket,
    token: Token,
    socket_writable: bool,
    next_packet_id: NonZeroU64,
    outgoing_packet_queue: VecDeque<Vec<u8>>,
    reply_map: HashMap<NonZeroU64, SendResponse<OwnedHeaderParsedPacket>>,
}

impl Connection {
    fn new(socket: UdpSocket, connection_token: Token) -> Self {
        Connection {
            socket,
            token: connection_token,
            socket_writable: false,
            next_packet_id: NonZeroU64::new(1).unwrap(),
            outgoing_packet_queue: VecDeque::new(),
            reply_map: HashMap::new(),
        }
    }

    fn next_packet_id(&mut self) -> NonZeroU64 {
        let packet_id = self.next_packet_id;
        self.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        packet_id
    }
}

pub struct WorkerInit {
    audio_sender: Sender<AudioCommand>,
    command_receiver: Receiver<Command>,
    command_registration: Registration,
}

pub struct Worker {
    connection: Option<Connection>,
    poll: Poll,
    next_poll_token: usize,
}

enum ReadUdpPacketError {
    FailedToParse,
    UnexpectedPacket,
}

impl From<DeserializeError> for ReadUdpPacketError {
    fn from(_: DeserializeError) -> Self {
        ReadUdpPacketError::FailedToParse
    }
}

fn read_udp_packet(raw_packet: &[u8], reply_map: &mut HashMap<NonZeroU64, SendResponse<OwnedHeaderParsedPacket>>, audio_handle: &mut AudioHandle) -> Result<(), ReadUdpPacketError> {
    let parsed_packet = HeaderParsedPacket::from_raw(raw_packet)?;
    if let Some(sender) = reply_map.remove(&parsed_packet.header.id) {
        sender.done(parsed_packet.into_owned());
        return Ok(());
    }

    match parsed_packet.kind() {
        packet::AudioToClient::KIND_ID => {
            let audio_to_client: AudioToClient = parsed_packet.finish_parse()?.body;
            audio_handle.send(audio_to_client.author, &audio_to_client.bytes);
        }
        _ => return Err(ReadUdpPacketError::UnexpectedPacket)
    }
    Ok(())
}

impl Worker {
    pub fn new() -> io::Result<Self> {
        Ok(Worker {
            connection: None,
            poll: Poll::new()?,
            next_poll_token: 0,
        })
    }

    pub fn run(mut self, worker_init: WorkerInit) -> Result<(), NetworkError> {
        let command_queue = CommandQueue::new(worker_init.command_receiver, worker_init.command_registration);
        let mut audio_handle = AudioHandle::new(worker_init.audio_sender);

        let command_token = self.create_poll_token();
        self.poll.register(&command_queue, command_token, Ready::readable(), PollOpt::edge())?;

        let mut buffer = [0; 65535];
        let mut events = Events::with_capacity(1024);

        loop {
            self.poll.poll(&mut events, None)?;
            for event in events.iter() {
                let readiness = event.readiness();
                let token = event.token();
                if token == command_token && readiness.is_readable() { // TODO do I need the is_readable check?
                    let command = match command_queue.next() {
                        Some(command) => command,
                        None => {
                            // This should never happen since handle_command is only called when
                            // set_ready is called.
                            // TODO track if/when this happens, to confirm this assumption.
                            return Ok(());
                        }
                    };

                    match command {
                        Command::OneDirection {
                            kind,
                            outgoing,
                        } => {
                            if let Some(connection) = &mut self.connection {
                                let header = PacketHeader {
                                    id: connection.next_packet_id(),
                                    reply_to: None,
                                    kind
                                };
                                if let Ok(raw) = outgoing.into_raw(header) {
                                    connection.outgoing_packet_queue.push_back(raw);
                                    attempt_send_next_packet(&mut connection.outgoing_packet_queue, &connection.socket, &mut connection.socket_writable)?;
                                }
                            }
                        },
                        Command::Request {
                            kind,
                            outgoing,
                            send_response,
                        } => {
                            if let Some(connection) = &mut self.connection {
                                let id = connection.next_packet_id();
                                let header = PacketHeader {
                                    id,
                                    reply_to: None,
                                    kind
                                };
                                if let Ok(raw) = outgoing.into_raw(header) {
                                    connection.outgoing_packet_queue.push_back(raw);
                                    connection.reply_map.insert(id, send_response);
                                    attempt_send_next_packet(&mut connection.outgoing_packet_queue, &connection.socket, &mut connection.socket_writable)?;
                                }
                            }
                        }
                        Command::Connect {
                            destination,
                            send_response,
                        } => {
                            let result: Result<(), ConnectError> = (|| {
                                if self.connection.is_some() {
                                    return Err(ConnectError::AlreadyConnected)
                                }

                                let socket = UdpSocket::bind(&"0.0.0.0:0".parse().unwrap())?;
                                socket.connect(destination)?;

                                let connection_token = self.create_poll_token();
                                self.poll.register(&socket, connection_token, Ready::all(), PollOpt::edge()).unwrap();
                                self.connection = Some(Connection::new(socket, connection_token));
                                Ok(())
                            })();
                            send_response.done(result);
                        },
                        Command::Disconnect {
                            send_response,
                        } => {
                            let result: Result<(), DisconnectError> = (|| {
                                if let Some(connection) = self.connection.take() {
                                    self.poll.deregister(&connection.socket)?;
                                }
                                Ok(())
                            })();
                            send_response.done(result);
                        }
                    }
                } else if let Some(connection) = &mut self.connection {
                    if token == connection.token {
                        if readiness.is_readable() {
                            loop {
                                match connection.socket.recv(&mut buffer) {
                                    Ok(size) => {
                                        if size == 0 {
                                            continue;
                                        }

                                        let raw_packet = &buffer[0..size];
                                        let _ = read_udp_packet(&raw_packet, &mut connection.reply_map, &mut audio_handle);
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                                    Err(e) => return Err(e.into()),
                                }
                            }
                        }
                        if readiness.is_writable() {
                            connection.socket_writable = true;
                            attempt_send_next_packet(&mut connection. outgoing_packet_queue, &connection. socket, &mut connection. socket_writable)?;
                        }
                    }
                }
            }
        }
    }

    fn create_poll_token(&mut self) -> Token {
        let token = self.next_poll_token;
        self.next_poll_token += 1;
        return Token(token);
    }
}

fn attempt_send_next_packet(outgoing_packet_queue: &mut VecDeque<Vec<u8>>, socket: &UdpSocket, socket_writable: &mut bool) -> Result<(), NetworkError> {
    if !*socket_writable {
        return Ok(())
    }

    let packet = match outgoing_packet_queue.pop_front() {
        Some(packet) => packet,
        None => {
            return Ok(());
        }
    };

    *socket_writable = false;
    match socket.send(&packet) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            outgoing_packet_queue.push_front(packet);
            return Ok(());
        },
        Err(e) => return Err(e.into())
    }

}

pub enum Command {
    OneDirection {
        kind: PacketKind,
        outgoing: WritePacketBuilder,
    },
    Request {
        kind: PacketKind,
        outgoing: WritePacketBuilder,
        send_response: SendResponse<OwnedHeaderParsedPacket>,
    },
    Connect {
        destination: SocketAddr,
        send_response: SendResponse<Result<(), ConnectError>>,
    },
    Disconnect {
        send_response: SendResponse<Result<(), DisconnectError>>,
    },
}

impl Display for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match &self {
            &Self::OneDirection { .. } => f.write_str("OneDirection"),
            &Self::Request { .. } => f.write_str("Request"),
            &Self::Connect { .. } => f.write_str("Connect"),
            &Self::Disconnect { .. } => f.write_str("Disconnect"),
        }
    }
}

#[derive(Debug)]
pub enum ConnectError {
    DestinationUnusable,
    NetworkError,
    AlreadyConnected,
}

impl From<io::Error> for ConnectError {
    fn from(_: io::Error) -> Self {
        ConnectError::NetworkError
    }
}

#[derive(Debug)]
pub enum DisconnectError {
    SocketError,
}

impl From<io::Error> for DisconnectError {
    fn from(_: io::Error) -> Self {
        DisconnectError::SocketError
    }
}

#[derive(Clone)]
pub struct Handle {
    sender: Sender<Command>,
    set_readiness: SetReadiness,
}

impl Handle {
    pub fn new_handle_and_worker(audio_sender: Sender<AudioCommand>) -> (Self, WorkerInit) {
        let (sender, receiver) = mpsc::channel();
        let (registration, set_readiness) = Registration::new2();
        return (
            Handle {
                sender,
                set_readiness,
            },
            WorkerInit {
                audio_sender,
                command_receiver: receiver,
                command_registration: registration,
            }
        )
    }

    pub fn command(&self, command: Command) -> Result<(), SendToNetworkThreadError> {
        self.sender.send(command)?;
        self.set_readiness.set_readiness(Ready::readable())?;
        Ok(())
    }

    pub fn send_single<B>(&self, packet: B) -> Result<(), SendToNetworkThreadError>
        where B: PacketBody + Serialize
    {
        self.command(Command::OneDirection {
            kind: B::KIND_ID,
            outgoing: WritePacketBuilder::new(packet)?
        })
    }

    pub fn request<B>(&self, outgoing_packet: B, send_response: SendResponse<OwnedHeaderParsedPacket>) -> Result<(), SendToNetworkThreadError>
        where B: PacketBody + Serialize
    {
        self.command(Command::Request {
            kind: B::KIND_ID,
            outgoing: WritePacketBuilder::new(outgoing_packet)?,
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

pub struct AudioHandle {
    command_tx: Sender<AudioCommand>,
    client_streams: HashMap<ClientId, Producer<i16>>,
}

impl AudioHandle {
    fn new(command_tx: Sender<AudioCommand>) -> AudioHandle {
        AudioHandle {
            command_tx,
            client_streams: HashMap::new(),
        }
    }

    fn send(&mut self, id: ClientId, bytes: &[u8]) {
        let l16le_samples: Vec<i16> = bytes
            .chunks(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        match self.client_streams.get_mut(&id) {
            Some(producer) => {
                producer.push_slice(&l16le_samples);
            },
            None => {
                let ring = RingBuffer::new(4800);
                let (mut producer, consumer) = ring.split();
                self.command_tx.send(AudioCommand::NewClientStream(id.to_string(), consumer)).unwrap();
                producer.push_slice(&l16le_samples);
                self.client_streams.insert(id, producer);
            }
        };
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