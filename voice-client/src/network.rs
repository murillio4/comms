use std::io;
use std::net::SocketAddr;

use mio::{Evented, Events, Poll, PollOpt, Ready, Registration, SetReadiness, Token};
use mio::net::UdpSocket;
use mio::unix::UnixReady;
use voice_lib::{packet, serialize};
use std::sync::mpsc::{Receiver, Sender, SendError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, AtomicUsize};
use serde::Serialize;
use ringbuf::Producer;
use voice_lib::packet::{Packet, AudioToClient};
use bincode::deserialize;

pub fn run(server_address: SocketAddr, receiver: Receiver<Vec<u8>>, mut producer: Producer<u8>, registration: Registration, size: Arc<AtomicUsize>) -> io::Result<()> {
    let socket = UdpSocket::bind(&"127.0.0.1:0".parse().unwrap())?;
    socket.connect(server_address)?;

    let outgoing_queue = OutgoingQueue::new(receiver, registration, size);

    let network_token = Token(0);
    let audio_token = Token(1);
    let poll = Poll::new()?;
    poll.register(&socket, network_token, Ready::all(), PollOpt::edge())?;
    poll.register(&outgoing_queue, audio_token, Ready::readable(), PollOpt::edge())?;

    let mut buffer = [0; 65535];
    let mut events = Events::with_capacity(1024);
    let mut failed = false;

    while !failed {
        poll.poll(&mut events, None)?;

        for event in events.iter() {
            // todo make cross-platform, how can tell if can't send on windows?
            let readiness = UnixReady::from(event.readiness());
            // println!("{:?} {:?}", &event.token(), readiness);
            let token = event.token();
            if token == network_token {
                if readiness.is_readable() {
                    loop {
                        match socket.recv(&mut buffer) {
                            Ok(size) => {
                                if size == 0 {
                                    println!("received packet with size 0, ignoring");
                                    continue;
                                }

                                let buffer = &buffer[0..size];
                                let header_type = buffer[0];
                                match header_type {
                                    packet::Connect::HEADER_TYPE => {
                                        println!("Thanks for sending back the connect packet");
                                    },
                                    packet::AudioToClient::HEADER_TYPE => {
                                        let audio_to_client: AudioToClient = match deserialize(&buffer[1..size]) {
                                            Ok(packet) => packet,
                                            Err(_) => {
                                                println!("Failed to parse AudioToClient packet, ignoring");
                                                continue;
                                            }
                                        };
                                        producer.push_slice(audio_to_client.bytes);
                                    },
                                    _ => ()
                                }
                            }
                            Err(e) => {
                                if e.kind() == io::ErrorKind::WouldBlock {
                                    break;
                                } else {
                                    return Err(e);
                                }
                            }
                        }
                    }
                }
                if readiness.is_error() {
                    eprintln!("{}", socket.take_error().unwrap().unwrap());
                    failed = true;
                }
                if readiness.is_writable() {
                    outgoing_queue.socket_writable.store(true, Ordering::Relaxed);
                    if !outgoing_queue.is_empty() {
                        outgoing_queue.send_one(&socket)
                    }
                }
            } else if token == audio_token && readiness.is_readable() {
                outgoing_queue.send_one(&socket);
            }
        }
    };

    Ok(())
}

pub struct AppendToQueue {
    sender: Sender<Vec<u8>>,
    set_readiness: SetReadiness,
    size: Arc<AtomicUsize>,
}

impl AppendToQueue {
    pub fn new(sender: Sender<Vec<u8>>, set_readiness: SetReadiness, size: Arc<AtomicUsize>) -> AppendToQueue {
        AppendToQueue {
            sender,
            set_readiness,
            size,
        }
    }

    pub fn push<P>(&self, packet: P) -> Result<(), AppendQueueError> where P: packet::Packet + Serialize {
        let raw_packet = serialize(packet)?;
        self.sender.send(raw_packet)?;
        self.set_readiness.set_readiness(Ready::readable())?;
        self.size.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug)]
pub enum AppendQueueError {
    SerializeFailed(bincode::Error),
    SendFailed(SendError<Vec<u8>>),
    SetReadinessFailed(io::Error),
}

impl From<bincode::Error> for AppendQueueError {
    fn from(e: bincode::Error) -> Self {
        AppendQueueError::SerializeFailed(e)
    }
}

impl From<SendError<Vec<u8>>> for AppendQueueError {
    fn from(e: SendError<Vec<u8>>) -> Self {
        AppendQueueError::SendFailed(e)
    }
}

impl From<io::Error> for AppendQueueError {
    fn from(e: io::Error) -> Self {
        AppendQueueError::SetReadinessFailed(e)
    }
}

pub struct OutgoingQueue {
    receiver: Receiver<Vec<u8>>,
    registration: Registration,
    socket_writable: AtomicBool,
    size: Arc<AtomicUsize>,
}
unsafe impl Send for OutgoingQueue {}
unsafe impl Sync for OutgoingQueue {}

impl OutgoingQueue {
    pub fn new(receiver: Receiver<Vec<u8>>, registration: Registration, size: Arc<AtomicUsize>) -> OutgoingQueue {
        OutgoingQueue {
            receiver,
            registration,
            socket_writable: AtomicBool::new(false),
            size,
        }
    }

    fn send_one(&self, socket: &UdpSocket) {
        if !self.socket_writable.load(Ordering::Relaxed) {
            return
        }


        let packet = self.receiver.recv().unwrap();
        self.size.fetch_sub(1, Ordering::Relaxed);
        socket.send(&packet).unwrap();
//        println!("[ -> ] Sending packet of type {}", packet[0]);
        self.socket_writable.store(false, Ordering::Relaxed);
    }

    fn is_empty(&self) -> bool {
        self.size.load(Ordering::Relaxed) == 0
    }
}

impl Evented for OutgoingQueue {
    fn register(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        self.registration.deregister(poll)
    }
}