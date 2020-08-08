use std::collections::{HashMap, VecDeque};
use std::io;
use std::io::Write;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::mpsc::{Receiver, RecvError, SendError};

use mio::{Events, Poll, Token, Interest};
use mio::net::TcpStream;
use mio::net::UdpSocket;

use voice_lib::network::{CommsTcpStream, IncomingStreamPacket, Protocol, UpdateFromStream};
use voice_lib::packet::{AudioToClient, Hello, PacketBody, PacketKind, AudioFromClient,  Packet};
use voice_lib::packet;
use voice_lib::serialization::{HeaderParsedPacket, OwnedHeaderParsedPacket, SerializeError, WritePacketBuilder};

use crate::audio::ipc::{AudioCommand, AudioHandle};
use crate::network::future::SendResponse;
use crate::network::ipc::{CommandQueue, ConnectError, NetworkCommand, NetworkEvent};

pub mod future;
pub mod ipc;

pub struct Connection {
    udp: UdpSocket,
    udp_token: Token,
    udp_writable: bool,
    udp_packet_queue: VecDeque<Vec<u8>>,
    tcp: CommsTcpStream,
    tcp_token: Token,
    tcp_writable: bool,
    next_packet_id: NonZeroU64,
    tcp_packet_queue: VecDeque<Vec<u8>>,
    reply_map: HashMap<NonZeroU64, SendResponse<OwnedHeaderParsedPacket>>,
    connect_response: Option<SendResponse<Result<Hello, ConnectError>>>,
}

impl Connection {
    fn new(udp: UdpSocket, udp_token: Token, tcp: CommsTcpStream, tcp_token: Token) -> Self {
        Connection {
            udp,
            udp_token,
            udp_writable: false,
            udp_packet_queue: VecDeque::new(),
            tcp,
            tcp_token,
            tcp_writable: false,
            tcp_packet_queue: VecDeque::new(),
            next_packet_id: NonZeroU64::new(1).unwrap(),
            reply_map: HashMap::new(),
            connect_response: None,
        }
    }

    fn next_packet_id(&mut self) -> NonZeroU64 {
        let packet_id = self.next_packet_id;
        self.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        packet_id
    }
}

pub struct WorkerInit {
    audio_handle: AudioHandle,
    event_sender: futures::channel::mpsc::Sender<NetworkEvent>,
    command_receiver: Receiver<NetworkCommand>,
    poll: Poll,
    command_token: Token,
    next_poll_token: NextPollToken,
}

struct NextPollToken(usize);

impl NextPollToken {
    fn new() -> NextPollToken {
        NextPollToken(0)
    }

    fn create(&mut self) -> Token {
        let next_id = self.0;
        self.0 += 1;
        Token(next_id)
    }
}

pub struct Worker {
    connection: Option<Connection>,
}

enum HandlePacketError {
    FailedToParse(PacketKind),
    UnexpectedPacket(OwnedHeaderParsedPacket),
}

fn handle_udp_packet(parsed_packet: HeaderParsedPacket, reply_map: &mut HashMap<NonZeroU64, SendResponse<OwnedHeaderParsedPacket>>, audio_handle: &mut AudioHandle) -> Result<(), HandlePacketError> {
    if let Some(reply_to) = parsed_packet.header.reply_to {
        if let Some(sender) = reply_map.remove(&reply_to) {
            sender.done(parsed_packet.into_owned());
            return Ok(());
        }
    }

    let kind = parsed_packet.kind();
    match kind {
        packet::AudioToClient::KIND_ID => {
            let audio_to_client: Packet<AudioToClient> = parsed_packet.finish_parse()
                .map_err(|_| HandlePacketError::FailedToParse(kind))?;
            audio_handle.send_audio(audio_to_client.body.author, &audio_to_client.body.bytes);
        },
        _ => return Err(HandlePacketError::UnexpectedPacket(parsed_packet.into_owned()))
    }
    Ok(())
}

fn handle_tcp_packet(stream_packet: IncomingStreamPacket, reply_map: &mut HashMap<NonZeroU64, SendResponse<OwnedHeaderParsedPacket>>, await_hello: &mut Option<SendResponse<Result<Hello, ConnectError>>>) -> Result<(), HandlePacketError> {
    if let Some(reply_to) = stream_packet.header.reply_to {
        if let Some(sender) = reply_map.remove(&reply_to) {
            sender.done(stream_packet.into_owned());
            return Ok(());
        }
    }

    let kind = stream_packet.header.kind;
    if kind == Hello::KIND_ID && await_hello.is_some() {
        if let Some(send_response) = await_hello.take() {
            send_response.done(Ok(stream_packet.finish_parse()
                .map_err(|_| HandlePacketError::FailedToParse(kind))?.body));
            return Ok(());
        }
    }
    Err(HandlePacketError::UnexpectedPacket(stream_packet.into_owned()))
}

fn connect(connection: &mut Option<Connection>, poll: &Poll, destination: SocketAddr, next_poll_token: &mut NextPollToken) -> Result<(), ConnectError> {
    if connection.is_some() {
        return Err(ConnectError::AlreadyConnected);
    }

    let mut tcp = TcpStream::connect(destination)?;
    let mut udp = UdpSocket::bind("0.0.0.0:0".parse().unwrap())?;
    udp.connect(destination)?;

    let udp_token = next_poll_token.create();
    let tcp_token = next_poll_token.create();
    poll.registry().register(&mut udp, udp_token, Interest::READABLE | Interest::WRITABLE).unwrap();
    poll.registry().register(&mut tcp, tcp_token, Interest::READABLE | Interest::WRITABLE).unwrap();
    *connection = Some(Connection::new(udp, udp_token, CommsTcpStream::new(tcp), tcp_token));
    Ok(())
}

impl Worker {
    pub fn new() -> io::Result<Self> {
        Ok(Worker {
            connection: None,
        })
    }

    pub fn run(mut self, worker_init: WorkerInit) -> Result<(), NetworkError> {
        let command_queue = CommandQueue::new(worker_init.command_receiver);
        let mut audio_handle = worker_init.audio_handle;
        let mut event_sender = worker_init.event_sender;

        let mut next_poll_token = worker_init.next_poll_token;
        let mut poll = worker_init.poll;
        let command_token = worker_init.command_token;

        let mut buffer = [0; 65535];
        let mut events = Events::with_capacity(1024);

        loop {
            poll.poll(&mut events, None)?;
            for event in events.iter() {
                let token = event.token();
                if token == command_token && event.is_readable() {
                    while let Some(command) = command_queue.next() {
                        match command {
                            NetworkCommand::Packet {
                                protocol,
                                outgoing,
                                send_response,
                            } => {
                                if let Some(connection) = &mut self.connection {
                                    let id = connection.next_packet_id();
                                    if let Ok(raw) = outgoing.into_raw(id) {
                                        if let Some(send_response) = send_response {
                                            connection.reply_map.insert(id, send_response);
                                        }

                                        match protocol {
                                            Protocol::Tcp => {
                                                connection.tcp_packet_queue.push_back(raw);
                                                match attempt_send_next_tcp_packet(&mut connection.tcp_packet_queue, &mut connection.tcp.internal, &mut connection.tcp_writable) {
                                                    Ok(_) => (),
                                                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                                                        if let Some(send_response) = connection.connect_response.take() {
                                                            send_response.done(Err(ConnectError::NetworkError))
                                                        }
                                                    }
                                                    Err(e) => return Err(e.into()),
                                                }
                                            }
                                            Protocol::Udp => {
                                                connection.udp_packet_queue.push_back(raw);
                                                match attempt_send_next_udp_packet(&mut connection.udp_packet_queue, &connection.udp, &mut connection.udp_writable) {
                                                    Ok(_) => (),
                                                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                                                        if let Some(send_response) = connection.connect_response.take() {
                                                            send_response.done(Err(ConnectError::NetworkError))
                                                        }
                                                    }
                                                    Err(e) => return Err(e.into()),
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                            NetworkCommand::SendAudio(chunk) => {
                                if let Some(connection) = &mut self.connection {
                                    let bytes = chunk.iter()
                                        .flat_map(|sample| sample.to_le_bytes().to_vec())
                                        .collect();
                                    let outgoing = WritePacketBuilder::new(None, &AudioFromClient(bytes)).unwrap();
                                    let id = connection.next_packet_id();
                                    if let Ok(raw) = outgoing.into_raw(id) {
                                        connection.udp_packet_queue.push_back(raw);
                                        match attempt_send_next_udp_packet(&mut connection.udp_packet_queue, &connection.udp, &mut connection.udp_writable) {
                                            Ok(_) => (),
                                            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => (),
                                            Err(e) => return Err(e.into()),
                                        }
                                    }
                                }
                            },
                            NetworkCommand::Connect {
                                destination,
                                send_response,
                            } => {
                                let mut connection = &mut self.connection;
                                if let Err(e) = connect(&mut connection, &poll, destination, &mut next_poll_token) {
                                    send_response.done(Err(e));
                                } else if let Some(connection) = &mut connection {
                                    connection.connect_response = Some(send_response);
                                }
                            }
                            NetworkCommand::Disconnect {
                                send_response,
                            } => {
                                self.connection.take();
                                send_response.done(());
                            }
                        }
                    }
                } else if let Some(connection) = &mut self.connection {
                    if token == connection.udp_token {
                        if event.is_readable() {
                            loop {
                                match connection.udp.recv(&mut buffer) {
                                    Ok(size) => {
                                        if size == 0 {
                                            continue;
                                        }

                                        let raw_packet = &buffer[0..size];
                                        if let Ok(parsed_packet) = HeaderParsedPacket::from_raw(raw_packet) {
                                            match handle_udp_packet(parsed_packet, &mut connection.reply_map, &mut audio_handle) {
                                                Ok(_) => {}
                                                Err(HandlePacketError::UnexpectedPacket(packet)) => {
                                                    event_sender.try_send(NetworkEvent::Packet(packet)).unwrap();
                                                },
                                                Err(HandlePacketError::FailedToParse(kind)) => eprintln!("Failed to parse a UDP packet of kind {}, ignoring", kind),
                                            }
                                        }
                                    }
                                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                                    Err(e) => return Err(e.into()),
                                }
                            }
                        }
                        if event.is_writable() {
                            connection.udp_writable = true;
                            attempt_send_next_udp_packet(&mut connection.udp_packet_queue, &connection.udp, &mut connection.udp_writable)?;
                        }
                    } else if token == connection.tcp_token {
                        if event.is_readable() {
                            match connection.tcp.update_from_stream() {
                                Ok(UpdateFromStream::Success) => {}
                                Ok(UpdateFromStream::ConnectionClosed) => event_sender.try_send(NetworkEvent::SocketClosed).unwrap(),
                                Err(_) => event_sender.try_send(NetworkEvent::NetworkError).unwrap(),
                            }

                            loop {
                                match connection.tcp.next_packet() {
                                    Ok(Some(stream_packet)) => {
                                        match handle_tcp_packet(stream_packet, &mut connection.reply_map, &mut connection.connect_response) {
                                            Ok(_) => {}
                                            Err(HandlePacketError::UnexpectedPacket(packet)) => {
                                                event_sender.try_send(NetworkEvent::Packet(packet)).unwrap();
                                            },
                                            Err(HandlePacketError::FailedToParse(kind)) => eprintln!("Failed to parse a TCP packet of kind {}, ignoring", kind),
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(_) => {
                                        // TODO do disconnect!
                                        break;
                                    }
                                }
                            }
                        }
                        if event.is_writable() {
                            connection.tcp_writable = true;
                            attempt_send_next_tcp_packet(&mut connection.tcp_packet_queue, &mut connection.tcp.internal, &mut connection.tcp_writable)?;
                        }
                    }
                }
            }
        }
    }
}

fn attempt_send_next_udp_packet(outgoing_packet_queue: &mut VecDeque<Vec<u8>>, socket: &UdpSocket, socket_writable: &mut bool) -> Result<(), io::Error> {
    if !*socket_writable {
        return Ok(());
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
            Ok(())
        }
        Err(e) => Err(e)
    }
}

fn attempt_send_next_tcp_packet(outgoing_packet_queue: &mut VecDeque<Vec<u8>>, socket: &mut TcpStream, socket_writable: &mut bool) -> Result<(), io::Error> {
    if !*socket_writable {
        return Ok(());
    }

    let packet = match outgoing_packet_queue.pop_front() {
        Some(packet) => packet,
        None => {
            return Ok(());
        }
    };

    *socket_writable = false;
    match socket.write(&packet) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            outgoing_packet_queue.push_front(packet);
            Ok(())
        }
        Err(e) => Err(e)
    }
}

#[derive(Debug)]
pub enum NetworkError {
    Io(Option<io::Error>),
    SendCommandToAudioThread,
    ReceiveFromAudioThread(RecvError),
}

impl From<io::Error> for NetworkError {
    fn from(e: io::Error) -> Self {
        NetworkError::Io(Some(e))
    }
}

impl From<SendError<AudioCommand>> for NetworkError {
    fn from(_: SendError<AudioCommand>) -> Self {
        NetworkError::SendCommandToAudioThread
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
    SendFailed,
    SetReadinessFailed(io::Error),
}

impl From<SerializeError> for SendToNetworkThreadError {
    fn from(e: SerializeError) -> Self {
        SendToNetworkThreadError::SerializeFailed(e)
    }
}

impl From<SendError<NetworkCommand>> for SendToNetworkThreadError {
    fn from(_: SendError<NetworkCommand>) -> Self {
        SendToNetworkThreadError::SendFailed
    }
}

impl From<io::Error> for SendToNetworkThreadError {
    fn from(e: io::Error) -> Self {
        SendToNetworkThreadError::SetReadinessFailed(e)
    }
}