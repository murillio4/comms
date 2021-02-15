use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::mpsc::{Receiver, RecvError, SendError};

use mio::net::TcpStream;
use mio::net::UdpSocket;
use mio::{Events, Interest, Poll, Token};

use comms_lib::network::{
    ClientUdpPacket, CommsNetworkError, Protocol, TcpConnection, UdpConnection,
};
use comms_lib::packet;
use comms_lib::packet::{AudioFromClient, AudioToClient, Hello, Packet, PacketBody};
use comms_lib::serialization::{IncomingPacketFrame, OutgoingPacketFrame};

use crate::audio::ipc::{AudioCommand, AudioHandle};
use crate::network::future::SendResponse;
use crate::network::ipc::{CommandQueue, ConnectError, NetworkCommand, NetworkEvent};
use futures::channel::mpsc::Sender;

pub mod future;
pub mod ipc;

pub struct Connection {
    udp: UdpConnection<ClientUdpPacket>,
    udp_token: Token,
    tcp: TcpConnection,
    tcp_token: Token,
    next_packet_id: NonZeroU64,
    reply_map: HashMap<NonZeroU64, SendResponse<IncomingPacketFrame>>,
    connect_response: Option<SendResponse<Result<Hello, ConnectError>>>,
}

impl Connection {
    fn new(socket: UdpSocket, udp_token: Token, tcp: TcpConnection, tcp_token: Token) -> Self {
        Connection {
            udp: UdpConnection::new(socket),
            udp_token,
            tcp,
            tcp_token,
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

fn handle_udp_packet(
    parsed_packet: IncomingPacketFrame,
    reply_map: &mut HashMap<NonZeroU64, SendResponse<IncomingPacketFrame>>,
    audio_handle: &mut AudioHandle,
    event_sender: &mut Sender<NetworkEvent>,
) -> Result<(), CommsNetworkError> {
    if let Some(reply_to) = parsed_packet.header.reply_to {
        if let Some(sender) = reply_map.remove(&reply_to) {
            sender.done(parsed_packet);
            return Ok(());
        }
    }

    let kind = parsed_packet.header.kind;
    match kind {
        packet::AudioToClient::KIND_ID => {
            let audio_to_client: Packet<AudioToClient> = parsed_packet.finish_parse()?;
            audio_handle.send_audio(audio_to_client.body.author, audio_to_client.body.bytes);
        }
        _ => {
            event_sender
                .try_send(NetworkEvent::Packet(parsed_packet))
                .unwrap();
        }
    }
    Ok(())
}

fn handle_tcp_packet(
    stream_packet: IncomingPacketFrame,
    reply_map: &mut HashMap<NonZeroU64, SendResponse<IncomingPacketFrame>>,
    await_hello: &mut Option<SendResponse<Result<Hello, ConnectError>>>,
    event_sender: &mut Sender<NetworkEvent>,
) -> Result<(), CommsNetworkError> {
    if let Some(reply_to) = stream_packet.header.reply_to {
        if let Some(sender) = reply_map.remove(&reply_to) {
            sender.done(stream_packet);
            return Ok(());
        }
    }

    let kind = stream_packet.header.kind;
    if kind == Hello::KIND_ID && await_hello.is_some() {
        if let Some(send_response) = await_hello.take() {
            send_response.done(Ok(stream_packet.finish_parse()?.body));
        }
    } else {
        event_sender
            .try_send(NetworkEvent::Packet(stream_packet))
            .unwrap();
    }
    Ok(())
}

fn connect(
    connection: &mut Option<Connection>,
    poll: &Poll,
    destination: SocketAddr,
    next_poll_token: &mut NextPollToken,
) -> Result<(), ConnectError> {
    if connection.is_some() {
        return Err(ConnectError::AlreadyConnected);
    }

    let mut tcp = TcpStream::connect(destination)?;
    let mut udp = UdpSocket::bind("0.0.0.0:0".parse().unwrap())?;
    udp.connect(destination)?;

    let udp_token = next_poll_token.create();
    let tcp_token = next_poll_token.create();
    poll.registry()
        .register(&mut udp, udp_token, Interest::READABLE | Interest::WRITABLE)
        .unwrap();
    poll.registry()
        .register(&mut tcp, tcp_token, Interest::READABLE | Interest::WRITABLE)
        .unwrap();
    *connection = Some(Connection::new(
        udp,
        udp_token,
        TcpConnection::new(tcp),
        tcp_token,
    ));
    Ok(())
}

impl Worker {
    pub fn new() -> io::Result<Self> {
        Ok(Worker { connection: None })
    }

    pub fn run(mut self, worker_init: WorkerInit) -> Result<(), NetworkError> {
        let command_queue = CommandQueue::new(worker_init.command_receiver);
        let mut audio_handle = worker_init.audio_handle;
        let mut event_sender = worker_init.event_sender;

        let mut next_poll_token = worker_init.next_poll_token;
        let mut poll = worker_init.poll;
        let command_token = worker_init.command_token;

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
                                packet: outgoing,
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
                                                if let Err(e) = connection.tcp.push_outgoing(&raw) {
                                                    println!("Failed to send TCP data: {:?}", e);
                                                    if let Some(send_response) =
                                                        connection.connect_response.take()
                                                    {
                                                        send_response
                                                            .done(Err(ConnectError::NetworkError))
                                                    } else {
                                                        unimplemented!(
                                                            "Should gracefully disconnect"
                                                        );
                                                    }
                                                }
                                            }
                                            Protocol::Udp => {
                                                match connection
                                                    .udp
                                                    .push_outgoing(ClientUdpPacket::new(raw))
                                                {
                                                    Ok(_) => (),
                                                    Err(e)
                                                        if e.kind()
                                                            == io::ErrorKind::ConnectionRefused =>
                                                    {
                                                        if let Some(send_response) =
                                                            connection.connect_response.take()
                                                        {
                                                            send_response.done(Err(
                                                                ConnectError::NetworkError,
                                                            ));
                                                        }
                                                    }
                                                    Err(e) => return Err(e.into()),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            NetworkCommand::SendAudio(chunk) => {
                                if let Some(connection) = &mut self.connection {
                                    let bytes = chunk
                                        .iter()
                                        .flat_map(|sample| sample.to_le_bytes().to_vec())
                                        .collect();
                                    let packet = OutgoingPacketFrame::from_packet(
                                        None,
                                        &AudioFromClient(bytes),
                                    )
                                    .unwrap();
                                    let id = connection.next_packet_id();
                                    if let Ok(raw) = packet.into_raw(id) {
                                        if let Err(e) =
                                            connection.udp.push_outgoing(ClientUdpPacket::new(raw))
                                        {
                                            return Err(e.into());
                                        }
                                    }
                                }
                            }
                            NetworkCommand::Connect {
                                destination,
                                send_response,
                            } => {
                                let mut connection = &mut self.connection;
                                if let Err(e) = connect(
                                    &mut connection,
                                    &poll,
                                    destination,
                                    &mut next_poll_token,
                                ) {
                                    send_response.done(Err(e));
                                } else if let Some(connection) = &mut connection {
                                    connection.connect_response = Some(send_response);
                                }
                            }
                            NetworkCommand::Disconnect { send_response } => {
                                self.connection.take();
                                send_response.done(());
                            }
                        }
                    }
                } else if let Some(connection) = &mut self.connection {
                    if token == connection.udp_token {
                        if event.is_readable() {
                            let reply_map = &mut connection.reply_map;
                            match connection.udp.read_new_packets().try_process(|packet| {
                                handle_udp_packet(
                                    packet,
                                    reply_map,
                                    &mut audio_handle,
                                    &mut event_sender,
                                )
                            }) {
                                Err(CommsNetworkError::Closed) => {
                                    event_sender.try_send(NetworkEvent::SocketClosed).unwrap();
                                }
                                Err(_) => {
                                    event_sender.try_send(NetworkEvent::NetworkError).unwrap();
                                }
                                _ => {}
                            }
                        }
                        if event.is_writable() && connection.udp.notify_writable().is_err() {
                            event_sender.try_send(NetworkEvent::NetworkError).unwrap();
                        }
                    } else if token == connection.tcp_token {
                        if event.is_readable() {
                            let reply_map = &mut connection.reply_map;
                            let connect_response = &mut connection.connect_response;

                            match connection.tcp.read_new_packets().try_process(|packet| {
                                handle_tcp_packet(
                                    packet,
                                    reply_map,
                                    connect_response,
                                    &mut event_sender,
                                )
                            }) {
                                Err(CommsNetworkError::Closed) => {
                                    event_sender.try_send(NetworkEvent::SocketClosed).unwrap();
                                }
                                Err(_) => {
                                    event_sender.try_send(NetworkEvent::NetworkError).unwrap();
                                }
                                _ => {}
                            }
                        }
                        if event.is_writable() && connection.tcp.notify_writable().is_err() {
                            event_sender.try_send(NetworkEvent::NetworkError).unwrap();
                        }
                    }
                }
            }
        }
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
    NetworkingFailed(CommsNetworkError),
    SendFailed,
    SetReadinessFailed(io::Error),
}

impl From<CommsNetworkError> for SendToNetworkThreadError {
    fn from(e: CommsNetworkError) -> Self {
        SendToNetworkThreadError::NetworkingFailed(e)
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
