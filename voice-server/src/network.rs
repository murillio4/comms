use voice_lib::serialization::WritePacketBuilder;
use crate::task::{SyncUnsafeCell, EndpointError, PacketFuture, PacketFinish};
use std::sync::Arc;
use voice_lib::packet::{PacketId, PacketBody, ClientId, AudioToClient};
use std::fmt::Debug;
use std::cell::UnsafeCell;
use voice_lib::network::Protocol;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{sync_channel, SyncSender, Receiver};
use mio::{Registration, Ready, SetReadiness, Token, PollOpt, Evented};
use std::io;
use mio::net::UdpSocket;
use voice_lib::Serialize;
use futures::future::join_all;
use crate::client::{AnyClientRef, Clients, Connection};
use std::io::Write;

struct OutgoingPacket {
    pub dest: ClientId,
    pub builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
    pub finish: PacketFinish,
}

pub struct OutgoingQueue {
    receiver: Receiver<(Protocol, OutgoingPacket)>,
    registration: Registration,
    udp_queue: VecDeque<OutgoingPacket>,
    queue_per_connection: HashMap<ClientId, VecDeque<OutgoingPacket>>,
}

impl OutgoingQueue {
    pub fn sort_incoming_packets(&mut self) {
        // Sort the incoming queue of packets into their respective socket buckets
        for (protocol, packet) in self.receiver.try_iter() {
            match protocol {
                Protocol::Udp => self.udp_queue.push_back(packet),
                Protocol::Tcp => {
                    let queue = self.queue_per_connection.entry(packet.dest).or_insert(VecDeque::new());
                    queue.push_back(packet);
                }
            }
        }
    }

    pub fn attempt_write_udp(&mut self, socket: &UdpSocket, socket_writable: &mut bool, clients: &mut Clients) {
        if !*socket_writable {
            return;
        }

        let (packet_id, packet, addr) = loop {
            let packet = match self.udp_queue.pop_front() {
                None => return,
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
        let raw = builder.as_raw(packet_id).unwrap();
        match socket.send_to(&raw, &addr) {
            Ok(_) => {
                if builder.kind != AudioToClient::KIND_ID {
                    println!("Sent UDP packet of kind {} to {}", builder.kind, &packet.dest);
                }
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.udp_queue.push_front(packet),
            Err(e) => panic!(e),
        }
    }

    pub fn attempt_write_tcp_for_client(&mut self, client_id: ClientId, connection: &mut Connection) {
        let queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
        let packet = match queue.pop_front() {
            None => return,
            Some(p) => p,
        };

        connection.tcp_writable = false;
        let builder = unsafe { &mut *packet.builder.get() };
        let raw = builder.as_raw(connection.next_packet_id()).unwrap();
        match connection.stream.internal.write(&raw) {
            Ok(_) => {
                println!("Sent TCP packet of kind {} to {}", builder.kind, client_id);
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => queue.push_front(packet),
            Err(e) => panic!(e),
        }
    }

    pub fn attempt_write_all(&mut self, socket: &UdpSocket, udp_socket_writable: &mut bool, clients: &mut Clients) {
        self.attempt_write_udp(socket, udp_socket_writable, clients);
        let writable_connections = clients.writable_connections();
        for (client_id, connection) in writable_connections.into_iter() {
            self.attempt_write_tcp_for_client(client_id, connection);
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
            queue_per_connection: HashMap::new(),
        }
    );
}

pub struct SendPacket {
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
}

impl SendPacket {
    pub fn new<B>(reply_to: Option<PacketId>, packet: B) -> Result<Self, EndpointError>
        where B: PacketBody + Serialize + Debug + 'static
    {
        let builder = WritePacketBuilder::new(reply_to, &packet)
            .map_err(|_| EndpointError::SerializeFailed { kind: B::KIND_ID, packet: Box::new(packet) })?;
        Ok(Self {
            builder: Arc::new(SyncUnsafeCell(UnsafeCell::new(builder))),
        })
    }

    pub async fn tcp(self, handle: &NetworkHandle, dest: ClientId) {
        handle.send(Protocol::Tcp, dest, self.builder).await
    }

    pub async fn udp(self, handle: &NetworkHandle, dest: ClientId) {
        handle.send(Protocol::Udp, dest, self.builder).await
    }

    pub async fn tcp_many(self, handle: &NetworkHandle, clients: Vec<ClientId>) {
        let arc = self.builder;

        let futures: Vec<PacketFuture> = clients.into_iter()
            .map(|c| handle.send(Protocol::Tcp, c, arc.clone()))
            .collect();
        join_all(futures).await;
    }

    pub async fn udp_many(self, handle: &NetworkHandle, clients: Vec<ClientId>) {
        let arc = self.builder;

        let futures: Vec<PacketFuture> = clients.into_iter()
            .map(|c| handle.send(Protocol::Udp, c, arc.clone()))
            .collect();
        join_all(futures).await;
    }
}