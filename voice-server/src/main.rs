//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

use mio::net::UdpSocket;
use mio::net::TcpListener;
use std::io;
use mio::{Poll, Token, Ready, PollOpt, Events};
use voice_lib::packet;
use std::net::SocketAddr;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use std::collections::HashMap;
use std::io::Error;
use std::cell::UnsafeCell;
use std::sync::Arc;
use crate::task::{SyncUnsafeCell, HandlePacketError};
use voice_lib::packet::{ClientId, PacketId};
use crate::tcp::{TcpHandle, OutgoingPacketBuilder};
use voice_lib::network::{CommsTcpStream};
use voice_lib::serialization::DeserializeError;

mod task;
mod tcp;
mod udp;

pub struct NextPacketId(PacketId);

impl NextPacketId {
    pub fn new() -> Self {
        Self(PacketId::new(1).unwrap())
    }

    pub fn next(&mut self) -> PacketId {
        let packet_id = self.0;
        self.0 = PacketId::new(self.0.get() + 1).unwrap();
        packet_id
    }
}

pub struct Connection {
    next_packet_id: NextPacketId,
    stream: CommsTcpStream,
    tcp_writable: bool,
}

pub struct ConnectingClient {
    connection: Connection,
    udp_addr: Option<SocketAddr>,
}

pub struct Client {
    connection: Connection,
    udp_addr: SocketAddr,
    username: String,
}

pub struct Clients {
    pub established: HashMap<ClientId, Client>,
    pub connecting: HashMap<ClientId, ConnectingClient>,
}

pub enum CurrentClient {
    Established(Client),
    Connecting(ConnectingClient),
}

impl Clients {
    fn new() -> Self {
        Clients {
            established: HashMap::new(),
            connecting: HashMap::new(),
        }
    }

    pub fn writable_connections(&mut self) -> Vec<(ClientId, &mut Connection)> {
        let connecting = self.connecting.iter_mut()
            .map(|(id, c)| (id, &mut c.connection));
        self.established.iter_mut()
            .map(|(id, c)| (id, &mut c.connection))
            .chain(connecting)
            .filter_map(|(id, c)| {
                match c.tcp_writable {
                    true => Some((id.clone(), c)),
                    false => None,
                }
            })
            .collect()
    }

    pub fn id_by_socket_addr(&self, addr: &SocketAddr) -> Option<ClientId> {
        self.established.iter()
            .find(|(_, c)| &c.udp_addr == addr)
            .map(|(id, _)| id.clone())
            .or_else(|| {
                self.connecting.iter()
                    .find(|(_, c)| c.udp_addr == Some(*addr))
                    .map(|(id, _)| id.clone())
            })
    }

    pub fn connection_by_id(&mut self, client_id: &ClientId) -> Option<&mut Connection>
    {
        let established_connection = self.established.get_mut(client_id).map(|c| &mut c.connection);
        let connecting = self.connecting.get_mut(client_id).map(|c| &mut c.connection);
        established_connection.or(connecting)
    }

    pub fn use_current_client(&mut self, client_id: &ClientId) -> Option<CurrentClient> {
        self.established.remove(client_id)
            .map(|c| CurrentClient::Established(c))
            .or_else(|| self.connecting.remove(client_id).map(|c| CurrentClient::Connecting(c)))
    }

    pub fn put_current_client_back(&mut self, client_id: ClientId, client: CurrentClient) {
        match client {
            CurrentClient::Established(client) => { self.established.insert(client_id, client); },
            CurrentClient::Connecting(client) => { self.connecting.insert(client_id, client); },
        };
    }
}

pub async fn on_connect(tcp_handle: TcpHandle, client_id: ClientId) -> Result<(), HandlePacketError> {
    let outgoing = OutgoingPacketBuilder::new(None, packet::Hello {
        client_id
    })?;
    outgoing.send_packet_tcp(&tcp_handle, client_id).await;
    Ok(())
}

fn main() -> Result<(), CommsServerError> {
    #[cfg(windows)]
    bump_priority();

    let mut raw_next_client_id: ClientId = 0;
    let mut next_client_id = move || {
        let id = raw_next_client_id;
        raw_next_client_id += 1;
        id
    };

    let (task_executor, spawner) = task::new();
    let task_executor_token = Token(next_client_id() as usize);

    let udp_socket = UdpSocket::bind(&"0.0.0.0:18847".parse().unwrap())?;
    let udp_token = Token(next_client_id() as usize);
    let tcp_listener = TcpListener::bind(&"0.0.0.0:18847".parse().unwrap())?;
    let tcp_listener_token = Token(next_client_id() as usize);

    let (udp_handle, mut udp_outgoing_queue) = udp::queue_and_handle();
    let udp_queue_token = Token(next_client_id() as usize);

    let (tcp_handle, mut tcp_outgoing_queue) = tcp::queue_and_handle();
    let tcp_queue_token = Token(next_client_id() as usize);

    let poll = Poll::new()?;
    poll.register(&task_executor, task_executor_token, Ready::readable(), PollOpt::edge())?;
    poll.register(&udp_socket, udp_token, Ready::all(), PollOpt::edge())?;
    poll.register(&tcp_listener, tcp_listener_token, Ready::readable(), PollOpt::edge())?;
    poll.register(&udp_outgoing_queue, udp_queue_token, Ready::readable(), PollOpt::edge())?;
    poll.register(&tcp_outgoing_queue, tcp_queue_token, Ready::readable(), PollOpt::edge())?;

    let mut events = Events::with_capacity(1024);
    let mut buffer = [0u8; 65535];
    let mut udp_socket_writable = false;
    let clients = Arc::new(SyncUnsafeCell(UnsafeCell::new(Clients::new())));

    loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            let token = event.token();
            if token == task_executor_token {
                task_executor.run();
            } else if token == udp_token {
                let readiness = event.readiness();
                if readiness.is_readable() {
                    let (size, remote_addr) = match udp_socket.recv_from(&mut buffer) {
                        Ok(tuple) => tuple,
                        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                        Err(_) => return Err(CommsServerError::IoError),
                    };

                    if size == 0 {
                        continue;
                    }

                    let buffer = &buffer[0..size];
                    udp::handle_packet(remote_addr, buffer, clients.clone(), &spawner, udp_handle.clone(), tcp_handle.clone()).unwrap();
                }
                if readiness.is_writable() {
                    let mut clients = unsafe { &mut *clients.get() };
                    udp_outgoing_queue.attempt_write_next(&udp_socket, &mut udp_socket_writable, &mut clients);
                }
            } else if token == udp_queue_token {
                if udp_socket_writable {
                    let mut clients = unsafe { &mut *clients.get() };
                    udp_outgoing_queue.attempt_write_next(&udp_socket, &mut udp_socket_writable, &mut clients);
                }
            } else if token == tcp_queue_token {
                let clients = unsafe { &mut *clients.get() };
                tcp_outgoing_queue.sort_incoming_packets();
                tcp_outgoing_queue.attempt_packet_all(clients.writable_connections());
            } else if token == tcp_listener_token {
                let (stream, _) = tcp_listener.accept()?;
                let client_id = next_client_id() as ClientId;
                let token = Token(client_id as usize);
                poll.register(&stream, token, Ready::all(), PollOpt::edge())?;

                let clients = unsafe { &mut *clients.get() };
                clients.connecting.insert(client_id, ConnectingClient {
                    connection: Connection {
                        next_packet_id: NextPacketId::new(),
                        stream: CommsTcpStream::new(stream),
                        tcp_writable: false,
                    },
                    udp_addr: None,
                });

                spawner.spawn(on_connect(tcp_handle.clone(), client_id));
            } else { // it's a TCP stream event
                let readiness = event.readiness();
                if readiness.is_readable() {
                    let client_id = token.0 as ClientId;
                    tcp::handle_readable_stream(client_id, clients.clone(), &spawner, &udp_handle, &tcp_handle, &poll).unwrap();
                }
                if readiness.is_writable() {
                    let client_id = token.0 as ClientId;

                    let clients = unsafe { &mut *clients.get() };
                    clients.connection_by_id(&client_id).map(|c| {
                        c.tcp_writable = true;
                        tcp_outgoing_queue.attempt_packet_for_client(client_id, c)
                    });
                }
            }
        }
    };
}

#[derive(Debug)]
enum CommsServerError {
    DeserializeError,
    IoError,
}

impl From<io::Error> for CommsServerError {
    fn from(_: Error) -> Self {
        CommsServerError::IoError
    }
}

impl From<DeserializeError> for CommsServerError {
    fn from(_: DeserializeError) -> Self {
        CommsServerError::DeserializeError
    }
}