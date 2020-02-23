use voice_lib::packet::{ClientId, Packet, PacketBody, FinishConnect, FinishConnectAccepted, ErrorResponse};
use std::collections::{HashMap, VecDeque};
use crate::{Clients, Client, CurrentClient, Connection};
use std::num::NonZeroU64;
use mio::{Registration, Evented, Token, Ready, PollOpt, SetReadiness, Poll};
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use futures::future::join_all;
use crate::task::{PacketFuture, PacketFinish, SyncUnsafeCell, Spawner, HandlePacketError};
use crate::udp::{UdpHandle, HandleReadableStreamError, Context};
use voice_lib::serialization::WritePacketBuilder;
use voice_lib::Serialize;
use voice_lib::network::{UpdateFromStream, StreamPacket};
use std::io::Write;
use std::cell::UnsafeCell;
use std::fmt::Debug;

async fn finish_connect(context: Context, packet: Packet<FinishConnect>) -> Result<(), HandlePacketError> {
    let packet: OutgoingPacketBuilder = {
        let clients = unsafe { &mut *context.clients.get() };
        let mut client = match clients.connecting.remove(&context.client_id) {
            Some(c) => c,
            None => return Ok(()),
        };

        match client.udp_addr.take() {
            None => {
                clients.connecting.insert(context.client_id, client);
                OutgoingPacketBuilder::new(Some(packet.id), ErrorResponse::AssociateUdpFirst)?
            }
            Some(addr) => {
                clients.established.insert(context.client_id, Client {
                    udp_addr: addr,
                    connection: client.connection,
                    username: packet.body.username,
                });
                OutgoingPacketBuilder::new(Some(packet.id), FinishConnectAccepted {
                    users: clients.established.values()
                        .map(|c| c.username.clone())
                        .collect()
                })?
            }
        }
    };

    packet.send_packet_tcp(&context.tcp_handle, context.client_id).await;
    Ok(())
}

fn handle_packet(spawner: &Spawner, context: Context, stream_packet: StreamPacket) -> Result<(), HandleReadableStreamError> {
    let kind = stream_packet.header.kind;
    match kind {
        FinishConnect::KIND_ID => spawner.spawn(finish_connect(context, stream_packet.finish_parse()?)),
        _ => return Err(HandleReadableStreamError::InvalidPacketType(kind)),
    };
    Ok(())
}

pub fn handle_readable_stream(
    client_id: ClientId,
    clients: Arc<SyncUnsafeCell<Clients>>,
    spawner: &Spawner,
    udp_handle: &UdpHandle,
    tcp_handle: &TcpHandle,
    poll: &Poll,
) -> Result<(), HandleReadableStreamError> {
    let mut current_client = {
        let clients = unsafe { &mut *clients.get() };
        if let Some(client) = clients.use_current_client(&client_id) {
            client
        } else {
            return Ok(());
        }
    };

    let connection = match &mut current_client {
        CurrentClient::Established(client) => &mut client.connection,
        CurrentClient::Connecting(client) => &mut client.connection,
    };

    if UpdateFromStream::ConnectionClosed == connection.stream.update_from_stream()? {
        eprintln!("Connection closed for {}. Removing client.", &client_id);
        poll.deregister(&connection.stream.internal).unwrap();
        return Ok(());
    }

    loop {
        match connection.stream.next_packet() {
            Ok(Some(stream_packet)) => {
                println!("Received TCP packet of type {} from {}", &stream_packet.header.kind, &client_id);

                let context = Context {
                    client_id,
                    clients: clients.clone(),
                    udp_handle: udp_handle.clone(),
                    tcp_handle: tcp_handle.clone(),
                };

                handle_packet(&spawner, context, stream_packet)?;
            }
            Ok(None) => break,
            Err(_) => {
                eprintln!("Deserialize error for client {}. Removing client.", &client_id);
                poll.deregister(&connection.stream.internal).unwrap();
                break;
            }
        }
    }

    let clients = unsafe { &mut *clients.get() };
    clients.put_current_client_back(client_id, current_client);
    Ok(())
}

pub struct OutgoingPacketBuilder {
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
}

impl OutgoingPacketBuilder {
    pub fn new<B>(reply_to: Option<NonZeroU64>, packet: B) -> Result<Self, HandlePacketError>
        where B: PacketBody + Serialize + Debug + 'static
    {
        let builder = WritePacketBuilder::new(reply_to, &packet)
            .map_err(|_| HandlePacketError::SerializeFailed { kind: B::KIND_ID, packet: Box::new(packet) })?;
        Ok(Self {
            builder: Arc::new(SyncUnsafeCell(UnsafeCell::new(builder))),
        })
    }

    pub async fn send_packet_tcp(self, handle: &TcpHandle, dest: ClientId) {
        handle.send(dest, self.builder).await
    }

    pub async fn send_packet_udp(self, handle: &UdpHandle, dest: ClientId) {
        handle.send(dest, self.builder).await
    }

    pub async fn send_packet_to_many_tcp(self, handle: &TcpHandle, clients: Vec<ClientId>) {
        let arc = self.builder;

        let futures: Vec<PacketFuture> = clients.into_iter()
            .map(|c| handle.send(c, arc.clone()))
            .collect();
        join_all(futures).await;
    }

    pub async fn send_packet_to_many_udp(self, handle: &UdpHandle, clients: Vec<ClientId>) {
        let arc = self.builder;

        let futures: Vec<PacketFuture> = clients.into_iter()
            .map(|c| handle.send(c, arc.clone()))
            .collect();
        join_all(futures).await;
    }
}

struct OutgoingPacket {
    dest: ClientId,
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
    finish: PacketFinish,
}

pub struct TcpOutgoingQueue {
    queue_per_connection: HashMap<ClientId, VecDeque<OutgoingPacket>>,
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
}

impl TcpOutgoingQueue {
    pub fn sort_incoming_packets(&mut self) {
        // Sort the incoming queue of packets into their respective socket buckets
        for packet in self.receiver.try_iter() {
            let queue = self.queue_per_connection.entry(packet.dest).or_insert(VecDeque::new());
            queue.push_back(packet);
        }
    }

    pub fn attempt_packet_all(&mut self, writable_connections: Vec<(ClientId, &mut Connection)>) {
        for (client_id, connection) in writable_connections.into_iter() {
            self.attempt_packet_for_client(client_id, connection);
        }
    }

    pub fn attempt_packet_for_client(&mut self, client_id: ClientId, connection: &mut Connection) {
        let queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
        let packet = match queue.pop_front() {
            None => return,
            Some(p) => p,
        };

        connection.tcp_writable = false;
        let builder = unsafe { &mut *packet.builder.get() };
        let raw = builder.as_raw(connection.next_packet_id.next()).unwrap();
        match connection.stream.internal.write(&raw) {
            Ok(_) => {
                println!("Sent TCP packet of kind {} to {}", builder.kind, client_id);
                packet.finish.done();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => queue.push_front(packet),
            Err(e) => panic!(e),
        }
    }
}

impl Evented for TcpOutgoingQueue {
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
pub struct TcpHandle {
    sender: SyncSender<OutgoingPacket>,
    set_queue_readiness: SetReadiness,
}

impl TcpHandle {
    pub fn send(&self, dest: ClientId, builder: Arc<SyncUnsafeCell<WritePacketBuilder>>) -> PacketFuture {
        let (future, finish) = PacketFuture::new();
        self.sender.send(OutgoingPacket {
            dest,
            builder,
            finish,
        }).unwrap();
        self.set_queue_readiness.set_readiness(Ready::readable()).unwrap();
        future
    }
}

pub fn queue_and_handle() -> (TcpHandle, TcpOutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    let (registration, set_queue_readiness) = Registration::new2();
    return (
        TcpHandle {
            sender,
            set_queue_readiness,
        },
        TcpOutgoingQueue {
            receiver,
            registration,
            queue_per_connection: HashMap::new(),
        }
    );
}

