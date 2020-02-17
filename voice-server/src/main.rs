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
use voice_lib::DeserializeError;
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

mod task;
mod tcp;
mod udp;

pub struct ConnectingClient {
    next_packet_id: NonZeroU64,
    tcp_token: Token,
    tcp_stream: TcpStream,
    udp_addr: Option<SocketAddr>,
}

pub struct Client {
    next_packet_id: NonZeroU64,
    tcp_stream: TcpStream,
    udp_addr: SocketAddr,
    username: String,
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

    let poll = Poll::new()?;
    poll.register(&task_executor, task_executor_token, Ready::readable(), PollOpt::edge())?;
    poll.register(&udp_socket, udp_token, Ready::all(), PollOpt::edge())?;
    poll.register(&tcp_listener, tcp_listener_token, Ready::readable(), PollOpt::edge())?;
    poll.register(&udp_outgoing_queue, udp_queue_token, Ready::readable(), PollOpt::edge())?;

    let mut events = Events::with_capacity(1024);
    let mut buffer = [0u8; 65535];

    let connecting_clients = Arc::new(SyncUnsafeCell(UnsafeCell::new(HashMap::<ClientId, ConnectingClient>::new())));
    let clients = Arc::new(SyncUnsafeCell(UnsafeCell::new(HashMap::<ClientId, Client>::new())));

    let mut udp_socket_writable = false;

    loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            let token = event.token();
            if token == task_executor_token {
                println!("Async time y'all :) :) :) fugg");
                task_executor.run();
            } else if token == udp_token {
                let readiness = event.readiness();
                if readiness.is_readable() {
                    println!("Attempting to read UDP packet");
                    let (size, remote_addr) = match udp_socket.recv_from(&mut buffer) {
                        Ok(tuple) => tuple,
                        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                        Err(_) => return Err(CommsServerError::IoError),
                    };

                    if size == 0 {
                        continue;
                    }

                    let buffer = &buffer[0..size];
                    udp::handle_packet(buffer, &clients, &connecting_clients, &spawner, remote_addr, &udp_handle).unwrap();
                }
                if readiness.is_writable() {
                    println!("Attempting to output UDP packet (socket writable)");
                    let mut clients = unsafe { &mut *clients.get() };
                    let mut connecting_clients = unsafe { &mut *connecting_clients.get() };
                    udp::attempt_write_packet(&udp_socket, &mut udp_outgoing_queue, &mut udp_socket_writable, &mut clients, &mut connecting_clients);
                }
            } else if token == udp_queue_token {
                println!("Attempting to output UDP packet (packet pushed)");
                if udp_socket_writable {
                    let mut clients = unsafe { &mut *clients.get() };
                    let mut connecting_clients = unsafe { &mut *connecting_clients.get() };
                    udp::attempt_write_packet(&udp_socket, &mut udp_outgoing_queue, &mut udp_socket_writable, &mut clients, &mut connecting_clients);
                }
            } else if token == tcp_listener_token {
                let (stream, _) = tcp_listener.accept()?;
                let id = next_client_id();
                let token = Token(id as usize);
                poll.register(&stream, token, Ready::all(), PollOpt::edge())?;

                println!("TCP Connection opened with id {}", &id);

                let connecting_clients = unsafe { &mut *connecting_clients.get() };
                connecting_clients.insert(id, ConnectingClient {
                    next_packet_id: NonZeroU64::new(1).unwrap(),
                    tcp_token: token,
                    tcp_stream: stream,
                    udp_addr: None,
                });
            } else { // it's a TCP stream event
                println!("Some other event with id {} \\_:)_/", token.0);
//                if let Some(client) = clients.get_mut(&token) {
//                    println!("handle packet to client");
//                } else if let Some((id, connecting_client)) = connecting_clients
//                    .iter_mut()
//                    .find(|(id, c)| c.tcp_token == token) {
//                    match read_from_tcp_stream(&mut connecting_client.tcp_stream)? {
//                        TcpRead::ConnectionClosed => connecting_clients.remove(id),
//                        TcpRead::Data(buffer) => {
//                            let (packet_type, packet) = (buffer[0], &buffer[1..]);
//                            if packet_type == packet::Connect::KIND_ID {
//
//                            } else {
//                                let response_packet = packet::ErrorResponse;
//                                // todo respond with packet
////                                outgoing_queue.push((Rc::new(serialize(packet::AssociateUdpAccepted)?), remote_addr));
////                                attempt_write(&mut outgoing_queue, &udp_socket, &mut udp_writable);
//                            }
//                        }
//                    }
//                }
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