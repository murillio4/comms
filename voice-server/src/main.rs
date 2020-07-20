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
use mio::{Poll, Token, Events, Interest, Waker};
use voice_lib::packet;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use std::io::Error;
use std::cell::UnsafeCell;
use std::sync::Arc;
use crate::task::{SyncUnsafeCell, EndpointError};
use voice_lib::packet::ClientId;
use voice_lib::serialization::DeserializeError;
use crate::network::{NetworkHandle, SendPacket, Target, CommunicationFailure};
use crate::client::{Clients, ConnectingClient, Connection};
use crate::endpoints::{handle_udp_packet, HandlePacketError, handle_tcp_stream, spawn_close_connection, StreamStatus, CloseConnectionReason};

mod client;
mod endpoints;
mod network;
mod task;

pub async fn on_connect(network_handle: NetworkHandle, client_id: ClientId) -> Result<(), EndpointError> {
    let outgoing = SendPacket::tcp(Target::One(client_id), None, packet::Hello {
        client_id
    })?;
    outgoing.send(&network_handle).await;
    Ok(())
}

fn main() -> Result<(), CommsServerError> {
    #[cfg(windows)] bump_priority();

    let mut raw_next_client_id: ClientId = 0;
    let mut next_client_id = move || {
        let id = raw_next_client_id;
        raw_next_client_id += 1;
        id
    };

    let mut poll = Poll::new()?;

    let task_executor_token = Token(next_client_id() as usize);
    let waker = Waker::new(poll.registry(), task_executor_token).unwrap();
    let (task_executor, spawner) = task::new(waker);

    let mut udp_socket = UdpSocket::bind("0.0.0.0:18847".parse().unwrap())?;
    let udp_token = Token(next_client_id() as usize);
    let mut tcp_listener = TcpListener::bind("0.0.0.0:18847".parse().unwrap())?;
    let tcp_listener_token = Token(next_client_id() as usize);

    let outgoing_token = Token(next_client_id() as usize);
    let waker = Waker::new(poll.registry(), outgoing_token).unwrap();
    let (network_handle, mut outgoing_queue) = network::queue_and_handle(waker);

    poll.registry().register(&mut udp_socket, udp_token, Interest::READABLE | Interest::WRITABLE)?;
    poll.registry().register(&mut tcp_listener, tcp_listener_token, Interest::READABLE)?;

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
                if event.is_readable() {
                    loop {
                        match udp_socket.recv_from(&mut buffer) {
                            Ok((size, remote_addr)) => {
                                if size != 0 {
                                    let buffer = &buffer[0..size];
                                    match handle_udp_packet(remote_addr.clone(), buffer, clients.clone(), &spawner, network_handle.clone()) {
                                        Err(HandlePacketError::CommunicationFailure) => {
                                            if let Some(client_id) = {
                                                let clients = unsafe { &mut *clients.get() };
                                                clients.id_by_socket_addr(&remote_addr)
                                            } {
                                                spawn_close_connection(client_id, CloseConnectionReason::MalformedData, &network_handle, &clients, &spawner);
                                            }
                                        }
                                        _ => ()
                                    };
                                }
                            },
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => (),
                            Err(_) => return Err(CommsServerError::IoError),
                        };
                    }
                }
                if event.is_writable() {
                    udp_socket_writable = true;
                    if let Err(CommunicationFailure(client_id)) = {
                        let mut clients = unsafe { &mut *clients.get() };
                        outgoing_queue.attempt_write_udp(&udp_socket, &mut udp_socket_writable, &mut clients)
                    } {
                        spawn_close_connection(client_id, CloseConnectionReason::MalformedData, &network_handle, &clients, &spawner);
                    }
                }
            } else if token == outgoing_token {
                if let Err(failing_clients) = {
                    let mut clients = unsafe { &mut *clients.get() };
                    outgoing_queue.sort_from_handle(&mut clients);
                    outgoing_queue.attempt_write_all(&udp_socket, &mut udp_socket_writable, &mut clients)
                } {
                    for failure in failing_clients {
                        spawn_close_connection(failure.0, CloseConnectionReason::MalformedData, &network_handle, &clients, &spawner);
                    }
                }
            } else if token == tcp_listener_token {
                loop {
                    let mut stream = match tcp_listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            break;
                        },
                        Err(e) => panic!(e),
                    };
                    let client_id = next_client_id() as ClientId;
                    let token = Token(client_id as usize);
                    poll.registry().register(&mut stream, token, Interest::READABLE | Interest::WRITABLE)?;

                    let clients = unsafe { &mut *clients.get() };
                    clients.connecting.insert(client_id, ConnectingClient {
                        connection: Connection::new(stream),
                        udp_addr: None,
                    });

                    spawner.spawn(on_connect(network_handle.clone(), client_id));
                }
            } else { // it's a TCP stream event
                if event.is_readable() {
                    let client_id = token.0 as ClientId;
                    match handle_tcp_stream(client_id, clients.clone(), &spawner, &network_handle) {
                        Ok(StreamStatus::Closed) => {
                            spawn_close_connection(client_id, CloseConnectionReason::SocketClosed, &network_handle, &clients, &spawner);
                        }
                        Err(HandlePacketError::CommunicationFailure) => {
                            spawn_close_connection(client_id, CloseConnectionReason::MalformedData, &network_handle, &clients, &spawner);
                        }
                        _ => (),
                    }
                }
                if event.is_writable() {
                    let client_id = token.0 as ClientId;
                    let clients = unsafe { &mut *clients.get() };
                    clients.connection_by_id(&client_id).map(|c| {
                        c.tcp_writable = true;
                        outgoing_queue.attempt_write_tcp_for_client(c)
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