//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

use crate::client::{Clients, ConnectingClient, Connection};
use crate::endpoints::{
    handle_tcp_stream, handle_udp_packet, spawn_close_connection, CloseConnectionReason,
    HandlePacketError, StreamStatus,
};
use crate::network::{NetworkHandle, SendPacket, Target};
use crate::task::EndpointResult;
use comms_lib::network::{CommsNetworkError, ServerUdpReadIter, UdpConnection};
use comms_lib::packet;
use comms_lib::packet::ClientId;
use mio::net::TcpListener;
use mio::net::UdpSocket;
use mio::{Events, Interest, Poll, Token, Waker};
use std::cell::RefCell;
use std::io;
use std::io::Error;
use std::sync::Arc;

mod client;
mod endpoints;
mod network;
mod task;

pub async fn on_connect(network_handle: NetworkHandle, client_id: ClientId) -> EndpointResult {
    let outgoing = SendPacket::tcp(
        Target::One(client_id),
        None,
        packet::Hello {
            client_id,
            protocol_version: packet::PROTOCOL_VERSION,
        },
    )?;
    outgoing.send(&network_handle).await;
    Ok(())
}

fn main() -> Result<(), CommsServerError> {
    let mut raw_next_client_id: ClientId = 0;
    let mut next_client_id = move || {
        let id = raw_next_client_id;
        raw_next_client_id += 1;
        id
    };

    let mut poll = Poll::new()?;

    let waker_token = Token(next_client_id() as usize);
    let waker = Arc::new(Waker::new(poll.registry(), waker_token).unwrap());
    let (task_executor, spawner) = task::new(waker.clone());

    let mut udp_socket = UdpSocket::bind("0.0.0.0:18847".parse().unwrap())?;
    let udp_token = Token(next_client_id() as usize);
    let mut tcp_listener = TcpListener::bind("0.0.0.0:18847".parse().unwrap())?;
    let tcp_listener_token = Token(next_client_id() as usize);

    let (network_handle, mut outgoing_queue) = network::queue_and_handle(waker);

    poll.registry().register(
        &mut udp_socket,
        udp_token,
        Interest::READABLE | Interest::WRITABLE,
    )?;
    poll.registry()
        .register(&mut tcp_listener, tcp_listener_token, Interest::READABLE)?;

    let mut events = Events::with_capacity(1024);
    let mut udp = UdpConnection::new(udp_socket);
    let clients = Arc::new(RefCell::new(Clients::new()));

    loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            let token = event.token();
            if token == waker_token {
                // Waker has been triggered, this could either be because a future is ready to move
                // or because there's new packets to send.
                task_executor.run();
                let failed_client_ids = {
                    let mut clients = clients.borrow_mut();
                    outgoing_queue.sort_from_handle(&mut clients, &mut udp);
                    outgoing_queue.attempt_write_all(&mut udp, &mut clients)
                };
                for client_id in failed_client_ids {
                    spawn_close_connection(
                        client_id,
                        CloseConnectionReason::MalformedData,
                        &network_handle,
                        clients.clone(),
                        &spawner,
                    );
                }
            } else if token == udp_token {
                if event.is_readable() {
                    udp.read_new_packets::<'_, ServerUdpReadIter>()
                        .try_process(|packet, addr| {
                            handle_udp_packet(
                                addr,
                                packet,
                                clients.clone(),
                                &spawner,
                                network_handle.clone(),
                            )
                            .map_err(|e| e.into())
                        })
                        .unwrap();
                }
                if event.is_writable() {
                    udp.notify_writable().unwrap();
                }
            } else if token == tcp_listener_token {
                loop {
                    let mut stream = match tcp_listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            break;
                        }
                        Err(e) => panic!("{}", e),
                    };
                    let client_id = next_client_id() as ClientId;
                    let token = Token(client_id as usize);
                    poll.registry().register(
                        &mut stream,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                    )?;

                    let mut clients = clients.borrow_mut();
                    clients.connecting.insert(
                        client_id,
                        ConnectingClient {
                            connection: Connection::new(stream),
                            udp_association: None,
                        },
                    );

                    spawner.spawn(on_connect(network_handle.clone(), client_id));
                }
            } else {
                // it's a TCP stream event
                if event.is_readable() {
                    let client_id = token.0 as ClientId;
                    match handle_tcp_stream(client_id, clients.clone(), &spawner, &network_handle) {
                        Ok(StreamStatus::Closed) => {
                            spawn_close_connection(
                                client_id,
                                CloseConnectionReason::SocketClosed,
                                &network_handle,
                                clients.clone(),
                                &spawner,
                            );
                        }
                        Err(HandlePacketError::CommunicationFailure) => {
                            spawn_close_connection(
                                client_id,
                                CloseConnectionReason::MalformedData,
                                &network_handle,
                                clients.clone(),
                                &spawner,
                            );
                        }
                        _ => (),
                    }
                }
                if event.is_writable() {
                    let client_id = token.0 as ClientId;
                    let mut clients = clients.borrow_mut();
                    if let Some(c) = clients.connection_by_id(&client_id) {
                        c.tcp.notify_writable().unwrap();
                    };
                }
            }
        }
    }
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

impl From<CommsNetworkError> for CommsServerError {
    fn from(_: CommsNetworkError) -> Self {
        CommsServerError::DeserializeError
    }
}
