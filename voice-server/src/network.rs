use voice_lib::serialization::WritePacketBuilder;
use crate::task::{SyncUnsafeCell, EndpointError, PacketFuture, PacketFinish};
use std::sync::Arc;
use voice_lib::packet::{PacketId, PacketBody, ClientId, PacketKind, Disconnect};
use std::fmt::Debug;
use std::cell::UnsafeCell;
use voice_lib::network::Protocol;
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender, Receiver};
use mio::Waker;
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
                    if let Some(connection) = clients.connection_by_id(&packet.dest) {
                        connection.outgoing_queue.push_back(packet);
                    } else {
                        // I wonder if this will cause issues in higher layers if they expect a
                        // packet to be sent, then wait for a (never coming) response.
                        packet.finish.done();
                        println!("connection could not be found :)");
                    }
                }
            }
        }
    }

    pub fn attempt_write_udp(&mut self, socket: &UdpSocket, socket_writable: &mut bool, clients: &mut Clients) -> Vec<ClientId> {
        let mut failures = Vec::new();

        if !*socket_writable {
            return failures;
        }

        loop {
            let packet = match self.udp_queue.pop_front() {
                None => break,
                Some(p) => p,
            };

            let (packet_id, addr) = match clients.client_by_id(&packet.dest) {
                None => {
                    packet.finish.done();
                    continue;
                },
                Some(AnyClientRef::Established(c)) => (c.connection.next_packet_id(), c.udp_addr),
                Some(AnyClientRef::Connecting(c)) => (c.connection.next_packet_id(), c.udp_addr.expect("attempting to send UDP packet to client that hasn't associated yet")),
            };

            *socket_writable = false;
            let builder = unsafe { &mut *packet.builder.get() };
            let kind = builder.kind;

            let raw = builder.as_raw(packet_id).unwrap_or_else(|_| panic!("Serialization failed for packet id {} kind {}", packet_id, kind));
            match socket.send_to(&raw, addr) {
                Ok(_) => packet.finish.done(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.udp_queue.push_front(packet);
                    break;
                },
                Err(_) =>  {
                    packet.finish.done();
                    if kind != Disconnect::KIND_ID {
                        failures.push(packet.dest)
                    }
                },
            }
        }
        failures
    }

    pub fn attempt_write_tcp_for_client(&mut self, connection: &mut Connection) -> Result<(), CommunicationFailure> {
        let packet = match connection.outgoing_queue.pop_front() {
            None => return Ok(()),
            Some(p) => p,
        };

        let packet_id = connection.next_packet_id();
        let builder = unsafe { &mut *packet.builder.get() };
        let kind = builder.kind;
        let raw = builder.as_raw(packet_id).unwrap_or_else(|_| panic!("Serialization failed for packet id {} kind {}", packet_id, kind));
        match connection.stream.internal.write(&raw) {
            Ok(_) => {
                // TODO if not all bytes written to buffer, try to re-send later
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                println!("WouldBlock on sending {}", kind);
                connection.tcp_writable = false;
                connection.outgoing_queue.push_front(packet)
            },
            Err(_) => {
                // We say we're "done()" with this future, but it's not obvious to the calling
                // future that this packet failed to reach its destination. This could cause
                // issues if there's some back-and-forth between client and server, and the
                // associated logic "waits" for a response, not knowing that a send failed here
                // Perhaps we should be able to "packet.finish.failed()"? Hmm.
                packet.finish.done();
                if kind != Disconnect::KIND_ID {
                    return Err(CommunicationFailure(packet.dest))
                }
            },
        }
        Ok(())
    }

    pub fn attempt_write_all(&mut self, socket: &UdpSocket, udp_socket_writable: &mut bool, clients: &mut Clients) -> Vec<ClientId> {
        let mut failures = self.attempt_write_udp(socket, udp_socket_writable, clients);

        let writable_connections = clients.writable_connections();
        for connection in writable_connections.into_iter() {
            if let Err(e) = self.attempt_write_tcp_for_client(connection) {
                failures.push(e.0);
            }
        }

        failures
    }
}

#[derive(Clone)]
pub struct NetworkHandle {
    sender: SyncSender<(Protocol, OutgoingPacket)>,
    waker: Arc<Waker>,
}

impl NetworkHandle {
    pub fn send(&self, kind: PacketKind, protocol: Protocol, dest: ClientId, builder: Arc<SyncUnsafeCell<WritePacketBuilder>>) -> PacketFuture {
        let (future, finish) = PacketFuture::new(kind);
        self.sender.send((protocol, OutgoingPacket {
            dest,
            builder,
            finish,
        })).unwrap();
        self.waker.wake().unwrap();
        future
    }
}

pub fn queue_and_handle(waker: Arc<Waker>) -> (NetworkHandle, OutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    (
        NetworkHandle {
            sender,
            waker,
        },
        OutgoingQueue {
            receiver,
            udp_queue: VecDeque::new(),
        }
    )
}

pub enum Target {
    One(ClientId),
    Many(Vec<ClientId>),
}

pub struct SendPacket {
    kind: PacketKind,
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
            kind: B::KIND_ID,
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
            kind: B::KIND_ID,
            destination,
            protocol: Protocol::Tcp,
            builder: Arc::new(SyncUnsafeCell(UnsafeCell::new(builder))),
        })
    }

    pub async fn send(self, handle: &NetworkHandle) {
        let protocol = self.protocol;
        let builder = self.builder;
        let kind = self.kind;
        match self.destination {
            Target::One(dest) => handle.send(kind, protocol, dest, builder).await,
            Target::Many(clients) => {
                let futures: Vec<PacketFuture> = clients.into_iter()
                    .map(|c| handle.send(kind, protocol.clone(), c, builder.clone()))
                    .collect();
                join_all(futures).await;
            }
        }
    }
}
