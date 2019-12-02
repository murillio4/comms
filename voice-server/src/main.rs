//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

use mio::net::UdpSocket;
use std::io;
use mio::{Poll, Token, Ready, PollOpt, Events};
use voice_lib::{packet, deserialize, serialize};
use voice_lib::packet::{Packet, AudioFromClient};
use std::net::SocketAddr;
use std::rc::Rc;
use std::collections::HashSet;

// TODO handle keep-alive

fn main() -> io::Result<()> {
    let socket = UdpSocket::bind(&"0.0.0.0:18847".parse().unwrap())?;

    let poll = Poll::new()?;
    poll.register(&socket, Token(0), Ready::all(), PollOpt::edge())?;

    let mut events = Events::with_capacity(1024);
    let mut buffer = [0u8; 65535];

    let mut clients = HashSet::<SocketAddr>::new();
    let mut outgoing_queue = Vec::<(Rc<Vec<u8>>, SocketAddr)>::new();
    let mut socket_writable = false;

    loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            let readiness = event.readiness();
            if readiness.is_readable() {
                let (size, remote_addr) = match socket.recv_from(&mut buffer) {
                    Ok(tuple) => tuple,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => {
                        return Err(e);
                    }
                };

                if size == 0 {
                    continue;
                }

                let buffer = &buffer[0..size];
                let header_type = buffer[0];
                match header_type {
                    packet::Connect::HEADER_TYPE => {
                        clients.insert(remote_addr);
                        println!("[{:?}] added client", &remote_addr);
                        outgoing_queue.push((Rc::new(buffer.to_vec()), remote_addr));
                        attempt_write(&mut outgoing_queue, &socket, &mut socket_writable);
                    }
                    packet::AudioFromClient::HEADER_TYPE => {
                        let audio_from_client: AudioFromClient = match deserialize(&buffer[1..size]) {
                            Ok(packet) => packet,
                            Err(_) => {
                                println!("[{:?}] Failed to parse packet, ignoring", &remote_addr);
                                continue;
                            }
                        };
                        clients.insert(remote_addr.clone());
                        println!("[{:?}] received audio [{}]", remote_addr, audio_from_client.0.len());
                        let audio_to_client = packet::AudioToClient {
                            bytes: audio_from_client.0,
                            author: remote_addr.to_string(),
                        };
                        let packet = Rc::new(serialize(audio_to_client).unwrap());

                        for client in clients.iter()
                            .filter(|c| c != &&remote_addr)
                            {
                            outgoing_queue.push((packet.clone(), client.clone()))
                        }
                        attempt_write(&mut outgoing_queue, &socket, &mut socket_writable);
                    }
                    packet::Disconnect::HEADER_TYPE => {
                        clients.retain(|c| c != &remote_addr);
                        println!("[{:?}] removed client", &remote_addr);
                    }
                    _ => {
                        println!("Received unknown packet type {} from {:?}", header_type, remote_addr);
                    }
                }
            }
            if readiness.is_writable() {
                socket_writable = true;
                attempt_write(&mut outgoing_queue, &socket, &mut socket_writable)
            }
        }
    };
}

fn attempt_write(outgoing_queue: &mut Vec<(Rc<Vec<u8>>, SocketAddr)>, socket: &UdpSocket, socket_writable: &mut bool) {
    if !outgoing_queue.is_empty() && *socket_writable {
        let (packet, client) = outgoing_queue.remove(0);
        match socket.send_to(&packet, &client) {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                outgoing_queue.insert(0, (packet, client));
                return;
            },
            Err(e) => panic!(e),
        };
        *socket_writable = false;
        println!("[ -> {:?}] Sending packet of type {}", &client, packet[0]);
    }
}