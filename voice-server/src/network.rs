use voice_lib::serialization::WritePacketBuilder;
use crate::task::{SyncUnsafeCell, EndpointError, PacketFuture, PacketFinish};
use std::sync::Arc;
use voice_lib::packet::{PacketId, PacketBody, ClientId, AudioToClient};
use std::fmt::Debug;
use std::cell::UnsafeCell;
use voice_lib::network::Protocol;
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender, Receiver};
use mio::{Registration, Ready, SetReadiness, Token, PollOpt, Evented};
use std::io;
use mio::net::UdpSocket;
use voice_lib::Serialize;
use futures::future::join_all;
use crate::client::{AnyClientRef, Clients, Connection};
use std::io::Write;

pub struct OutgoingPacket {
    pub dest: ClientId,
    pub builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
    pub finish: PacketFinish,
}

pub struct OutgoingQueue {
    receiver: Receiver<(Protocol, OutgoingPacket)>,
    registration: Registration,
    udp_queue: VecDeque<OutgoingPacket>,
}

pub struct CommunicationFailure(pub ClientId);

impl OutgoingQueue {
    pub fn sort_from_handle(&mut self, clients: &mut Clients) {
        // Sort the incoming queue of packets into their respective socket buckets
        for (protocol, packet) in self.receiver.try_iter() {
            match protocol {
                Protocol::Udp => self.udp_queue.push_back(packet),
                Protocol::Tcp => {
                    let kind = {
                        let builder = unsafe { &mut *packet.builder.get() };
                        builder.kind
                    };
                    println!("Sorting incoming packet of kind {} for dest {}", kind, &packet.dest);

                    if let Some(connection) = clients.connection_by_id(&packet.dest) {
                        connection.outgoing_queue.push_back(packet);
                    }
                }
            }
        }
    }

    pub fn attempt_write_udp(&mut self, socket: &UdpSocket, socket_writable: &mut bool, clients: &mut Clients) -> Result<(), CommunicationFailure> {
        if !*socket_writable {
            return Ok(());
        }

        let (packet_id, packet, addr) = loop {
            let packet = match self.udp_queue.pop_front() {
                None => return Ok(()),
                Some(p) => p,
            };

            break match clients.client_by_id(&packet.dest) {
                None => continue,
                Some(AnyClientRef::Established(c)) => (c.connection.next_packet_id(), packet, c.udp_addr),
                Some(AnyClientRef::Connecting(c)) => (c.connection.next_packet_id(), packet, c.udp_addr.expect("attempting to send UDP packet to client that hasn't associated yet")),
            };
        };

        *socket_writable = false;
        let builder = unsafe { &mut *packet.builder.get() };
        let kind = builder.kind;
        let raw = builder.as_raw(packet_id).expect(&format!("Serialization failed for packet id {} kind {}", packet_id, kind));
        match socket.send_to(&raw, &addr) {
            Ok(_) => {
                if kind != AudioToClient::KIND_ID {
                    println!("Sent UDP packet of kind {} to {}", kind, &packet.dest);
                }
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.udp_queue.push_front(packet),
            Err(_) => return Err(CommunicationFailure(packet.dest)),
        }
        Ok(())
    }

    pub fn attempt_write_tcp_for_client(&mut self, client_id: ClientId, connection: &mut Connection) -> Result<(), CommunicationFailure> {
        println!("Attempt to write tcp for client {}", &client_id);
        let packet = match connection.outgoing_queue.pop_front() {
            None => return Ok(()),
            Some(p) => p,
        };

        connection.tcp_writable = false;
        let packet_id = connection.next_packet_id();
        let builder = unsafe { &mut *packet.builder.get() };
        let kind = builder.kind;
        let raw = builder.as_raw(packet_id.clone()).expect(&format!("Serialization failed for packet id {} kind {}", packet_id, kind));
        match connection.stream.internal.write(&raw) {
            Ok(bytes) => {
                // TODO if not all bytes written to buffer, try to re-send later
                println!("Sent {} bytes of TCP stream of kind {} to {}", bytes, kind, client_id);
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => connection.outgoing_queue.push_front(packet),
            Err(_) => return Err(CommunicationFailure(packet.dest)),
        }
        Ok(())
    }

    pub fn attempt_write_all(&mut self, socket: &UdpSocket, udp_socket_writable: &mut bool, clients: &mut Clients) -> Result<(), Vec<CommunicationFailure>> {
        let mut failures = Vec::new();
        if let Err(e) = self.attempt_write_udp(socket, udp_socket_writable, clients) {
            failures.push(e);
        }

        let writable_connections = clients.writable_connections();
        for (client_id, connection) in writable_connections.into_iter() {
            if let Err(e) = self.attempt_write_tcp_for_client(client_id, connection) {
                failures.push(e);
            }
        }

        match failures.is_empty() {
            true => Ok(()),
            false => Err(failures)
        }
    }
}

impl Evented for OutgoingQueue {
    fn register(&self, poll: &mio::Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &mio::Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        poll.deregister(self)
    }
}

#[derive(Clone)]
pub struct NetworkHandle {
    sender: SyncSender<(Protocol, OutgoingPacket)>,
    set_queue_readiness: SetReadiness,
}

impl NetworkHandle {
    pub fn send(&self, protocol: Protocol, dest: ClientId, builder: Arc<SyncUnsafeCell<WritePacketBuilder>>) -> PacketFuture {
        let (future, finish) = PacketFuture::new();
        self.sender.send((protocol, OutgoingPacket {
            dest,
            builder,
            finish,
        })).unwrap();
        self.set_queue_readiness.set_readiness(Ready::readable()).unwrap();
        future
    }
}

pub fn queue_and_handle() -> (NetworkHandle, OutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    let (registration, set_queue_readiness) = Registration::new2();
    return (
        NetworkHandle {
            sender,
            set_queue_readiness,
        },
        OutgoingQueue {
            receiver,
            registration,
            udp_queue: VecDeque::new(),
        }
    );
}

pub enum Target {
    One(ClientId),
    Many(Vec<ClientId>),
}

pub struct SendPacket {
    destination: Target,
    protocol: Protocol,
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
}

impl SendPacket {
    pub fn udp<B>(destination: Target, reply_to: Option<PacketId>, packet: B) -> Result<Self, EndpointError>
        where B: PacketBody + Serialize + Debug + 'static
    {
        let builder = WritePacketBuilder::new(reply_to, &packet)
            .map_err(|_| EndpointError::SerializeFailed { kind: B::KIND_ID, packet: Box::new(packet) })?;
        Ok(Self {
            protocol: Protocol::Udp,
            destination,
            builder: Arc::new(SyncUnsafeCell(UnsafeCell::new(builder))),
        })
    }

    pub fn tcp<B>(destination: Target, reply_to: Option<PacketId>, packet: B) -> Result<Self, EndpointError>
        where B: PacketBody + Serialize + Debug + 'static
    {
        let builder = WritePacketBuilder::new(reply_to, &packet)
            .map_err(|_| EndpointError::SerializeFailed { kind: B::KIND_ID, packet: Box::new(packet) })?;
        Ok(Self {
            destination,
            protocol: Protocol::Tcp,
            builder: Arc::new(SyncUnsafeCell(UnsafeCell::new(builder))),
        })
    }

    pub async fn send(self, handle: &NetworkHandle) {
        let protocol = self.protocol;
        let builder = self.builder;
        let kind = {
            let builder = unsafe { &mut *builder.get() };
            builder.kind
        };
        match self.destination {
            Target::One(dest) => handle.send(protocol, dest.clone(), builder).await,
            Target::Many(clients) => {
                let futures: Vec<PacketFuture> = clients.into_iter()
                    .map(|c| {
                        if protocol == Protocol::Tcp {
                            println!("Sending {} to many, one of which is client {}", kind, c);
                        }
                        handle.send(protocol.clone(), c, builder.clone())
                    })
                    .collect();
                join_all(futures).await;
            }
        }
    }
}