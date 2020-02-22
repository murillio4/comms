//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

use mio::net::UdpSocket;
use mio::net::TcpListener;
use mio::net::TcpStream;
use std::io;
use mio::{Poll, Token, Ready, PollOpt, Events};
use voice_lib::{DeserializeError, packet};
use std::net::SocketAddr;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use std::collections::HashMap;
use std::io::Error;
use std::num::NonZeroU64;
use std::cell::UnsafeCell;
use std::sync::Arc;
use crate::task::SyncUnsafeCell;
use voice_lib::packet::ClientId;
use crate::tcp::{TcpRead, TcpHandle};

mod task;
mod tcp;
mod udp;

pub struct TcpConnection {
    tcp_stream: TcpStream,
    tcp_writable: bool,
}

pub struct ConnectingClient {
    client_id: ClientId,
    next_packet_id: NonZeroU64,
    tcp_connection: TcpConnection,
    udp_addr: Option<SocketAddr>,
}

pub struct Client {
    client_id: ClientId,
    next_packet_id: NonZeroU64,
    tcp_connection: TcpConnection,
    udp_addr: SocketAddr,
    username: String,
}

pub struct Clients {
    pub established: HashMap<ClientId, Client>,
    pub connecting: HashMap<ClientId, ConnectingClient>,
}

impl Clients {
    fn new() -> Self {
        Clients {
            established: HashMap::new(),
            connecting: HashMap::new(),
        }
    }

    pub fn remove(&mut self, client_id: &ClientId) -> Option<TcpConnection> {
        self.established.remove(client_id)
            .map(|c| c.tcp_connection)
            .or_else(|| self.connecting.remove(client_id).map(|c| c.tcp_connection))
    }

    pub fn id_by_socket_addr(&self, addr: &SocketAddr) -> Option<ClientId> {
        self.established.values()
            .find(|c| &c.udp_addr == addr)
            .map(|c| c.client_id)
            .or_else(|| {
                self.connecting.values()
                    .find(|c| c.udp_addr == Some(*addr))
                    .map(|c| c.client_id)
            })
    }

    pub fn tcp_connection_by_id<F, R>(&mut self, client_id: &ClientId, cb: F) -> R
        where F: FnOnce(Option<&mut TcpConnection>) -> R
    {
        let established_connection = self.established.get_mut(client_id).map(|c| &mut c.tcp_connection);
        let connecting = self.connecting.get_mut(client_id).map(|c| &mut c.tcp_connection);

        cb(established_connection.or(connecting))
    }
}

pub async fn on_connect(tcp_handle: TcpHandle, client_id: ClientId) {
    println!("Saying hello to new client {}", &client_id);
    tcp::send_packet(&tcp_handle, client_id, packet::Hello {
        client_id
    }).await;
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
                    println!("UDP socket is writable");
                    udp::attempt_write_packet(&udp_socket, &mut udp_outgoing_queue, &mut udp_socket_writable, &mut clients);
                }
            } else if token == udp_queue_token {
                if udp_socket_writable {
                    let mut clients = unsafe { &mut *clients.get() };
                    udp::attempt_write_packet(&udp_socket, &mut udp_outgoing_queue, &mut udp_socket_writable, &mut clients);
                }
            } else if token == tcp_queue_token {
                let mut clients = unsafe { &mut *clients.get() };
                tcp::attempt_write_any_packet(&mut tcp_outgoing_queue, &mut clients);
            } else if token == tcp_listener_token {
                let (stream, _) = tcp_listener.accept()?;
                let client_id = next_client_id() as ClientId;
                let token = Token(client_id as usize);
                poll.register(&stream, token, Ready::all(), PollOpt::edge())?;

                let clients = unsafe { &mut *clients.get() };
                clients.connecting.insert(client_id, ConnectingClient {
                    client_id,
                    next_packet_id: NonZeroU64::new(1).unwrap(),
                    tcp_connection: TcpConnection {
                        tcp_stream: stream,
                        tcp_writable: false,
                    },
                    udp_addr: None,
                });

                spawner.spawn(on_connect(tcp_handle.clone(), client_id));
            } else { // it's a TCP stream event
                let readiness = event.readiness();
                if readiness.is_readable() {
                    let client_id = token.0 as ClientId;
                    let tcp_read = {
                        let clients = unsafe { &mut *clients.get() };

                        clients.tcp_connection_by_id(&client_id, |connection| {
                            connection
                                .map(|connection| tcp::read_buffer_from_tcp_stream(&mut connection.tcp_stream))
                                .expect("received 'readable' event for non-existant client")
                        })
                    };

                    match tcp_read {
                        Ok(TcpRead::Data(buffer)) => tcp::handle_packet(client_id, &buffer, clients.clone(), &spawner, udp_handle.clone(), tcp_handle.clone()).unwrap(),
                        Ok(TcpRead::ConnectionClosed) => {
                            let clients = unsafe { &mut *clients.get() };
                            let client = clients.remove(&client_id).expect("tried to remove client that doesn't exist");
                            println!("deregistering client: {}", &client_id);
                            poll.deregister(&client.tcp_stream).unwrap();
                        },
                        Err(_) => continue,
                    }
                }
                if readiness.is_writable() {
                    let client_id = token.0 as ClientId;

                    let mut clients = unsafe { &mut *clients.get() };
                    clients.tcp_connection_by_id(&client_id, |c| c.map(|c| c.tcp_writable = true));
                    tcp::attempt_write_client_packet(client_id, &mut tcp_outgoing_queue, &mut clients);
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