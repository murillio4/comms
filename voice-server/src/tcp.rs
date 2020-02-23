use voice_lib::packet::{ClientId, Packet, PacketBody, FinishConnect, FinishConnectAccepted};
use std::collections::{HashMap, VecDeque};
use crate::{Clients, Client, CurrentClient};
use std::num::NonZeroU64;
use mio::{Registration, Evented, Token, Ready, PollOpt, SetReadiness, Poll};
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use futures::future::join_all;
use crate::task::{PacketFuture, PacketFinish, SyncUnsafeCell, Spawner};
use crate::udp::{UdpHandle, HandleReadableStreamError, Context};
use voice_lib::serialization::WritePacketBuilder;
use voice_lib::Serialize;
use voice_lib::network::{UpdateFromStream, StreamPacket};
use std::io::Write;
use std::cell::UnsafeCell;

async fn finish_connect(context: Context, packet: Packet<FinishConnect>) {
    // TODO respond with packet saying "you need to associate udp first" if failed
    let users = {
        let clients = unsafe { &mut *context.clients.get() };
        if let Some((addr, client)) = clients.connecting.remove(&context.client_id)
            .and_then(|mut client| client.udp_addr.take().map(|addr| (addr, client)))
        {
            clients.established.insert(context.client_id, Client {
                udp_addr: addr,
                tcp_connection: client.tcp_connection,
                next_packet_id: client.next_packet_id,
                username: packet.body.username,
            });
            clients.established.values()
                .map(|c| c.username.clone())
                .collect()
        } else {
            return;
        }
    };

    send_packet(&context.tcp_handle, context.client_id, Some(packet.id), FinishConnectAccepted {
        users,
    }).await;
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
        CurrentClient::Established(client) => &mut client.tcp_connection,
        CurrentClient::Connecting(client) => &mut client.tcp_connection,
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

pub async fn send_packet<B>(handle: &TcpHandle, dest: ClientId, reply_to: Option<NonZeroU64>, packet: B)
    where B: PacketBody + Serialize
{
    let builder = WritePacketBuilder::new(reply_to, packet).unwrap();
    handle.send(dest, Arc::new(SyncUnsafeCell(UnsafeCell::new(builder)))).await.unwrap();
}

pub async fn send_packet_to_many<B>(handle: &TcpHandle, clients: Vec<ClientId>, reply_to: Option<NonZeroU64>, packet: B)
    where B: PacketBody + Serialize
{
    let builder = WritePacketBuilder::new(reply_to, packet).unwrap();
    let raw = Arc::new(SyncUnsafeCell(UnsafeCell::new(builder)));

    let futures: Vec<PacketFuture> = clients.into_iter()
        .map(|c| handle.send(c, raw.clone()))
        .collect();
    for result in join_all(futures).await {
        result.unwrap();
    }
}

pub enum WriteAttempt {
    NoPacketAvailable,
    WriteAttempted,
}

enum PacketSendStatus {
    Done,
    SocketUnavailable,
    SkipPacket,
}

fn attempt_write_packet(client_id: ClientId, builder: &mut WritePacketBuilder, clients: &mut Clients) -> PacketSendStatus {
    let (packet_id, connection) = if let Some(client) = clients.established.get_mut(&client_id) {
        let packet_id = client.next_packet_id;
        client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        (packet_id, &mut client.tcp_connection)
    } else if let Some(client) = clients.connecting.get_mut(&client_id) {
        let packet_id = client.next_packet_id;
        client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        (packet_id, &mut client.tcp_connection)
    } else {
        eprintln!("Attempted to send TCP packet to client {} that doesn't exist", client_id);
        return PacketSendStatus::SkipPacket;
    };

    if !connection.tcp_writable {
        return PacketSendStatus::SocketUnavailable;
    }

    let raw = builder.as_raw(packet_id).unwrap();
    match connection.stream.internal.write(&raw) {
        Ok(_) => {
            println!("Sent TCP packet of kind {} to {}", builder.kind, client_id);
            connection.tcp_writable = false;
            return PacketSendStatus::Done;
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            connection.tcp_writable = false;
            return PacketSendStatus::SocketUnavailable;
        }
        Err(e) => panic!(e),
    }
}

pub fn attempt_write_client_packet(client_id: ClientId, outgoing: &mut TcpOutgoingQueue, clients: &mut Clients) {
    outgoing.next_packet(client_id, |builder| {
        attempt_write_packet(client_id, builder, clients)
    });
}

pub fn attempt_write_any_packet(outgoing: &mut TcpOutgoingQueue, clients: &mut Clients) {
    outgoing.any_next_packet(|destination_id, builder| {
        attempt_write_packet(destination_id, builder, clients)
    });
}

struct OutgoingPacket {
    dest: ClientId,
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
    finish: PacketFinish,
}

pub struct TcpOutgoingQueue {
    queue_per_connection: HashMap<ClientId, VecDeque<OutgoingPacket>>,
    packet_order: VecDeque<ClientId>,
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
}

impl TcpOutgoingQueue {
    fn sort_incoming_packets(&mut self) {
        // Clear out the incoming queue
        for packet in self.receiver.try_iter() {
            let queue = self.queue_per_connection.entry(packet.dest).or_insert(VecDeque::new());
            self.packet_order.push_back(packet.dest);
            queue.push_back(packet);
        }
    }

    // TODO we could iterate through the packets, trying to send until either
    // one sends _OR_ we've exhausted all potential clients/sockets
    fn any_next_packet<F>(&mut self, mut op: F) -> WriteAttempt
        where F: FnMut(ClientId, &mut WritePacketBuilder) -> PacketSendStatus
    {
        self.sort_incoming_packets();

        // There might be a cleaner way of doing this.
        // We need the loop to try a new packet if the one first attempted
        // targets a client that is no longer connected.
        loop {
            let client_id = match self.packet_order.pop_front() {
                Some(client_id) => client_id,
                None => return WriteAttempt::NoPacketAvailable,
            };
            let client_queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
            let packet = client_queue.pop_front()
                .expect("packet_order queue is out of sync with queue_per_connection");

            let builder = unsafe { &mut *packet.builder.get() };
            match op(packet.dest, builder) {
                PacketSendStatus::SocketUnavailable => {
                    client_queue.push_front(packet);
                    self.packet_order.push_back(client_id); // Socket is busy, don't use it for a while
                    return WriteAttempt::WriteAttempted;
                }
                PacketSendStatus::Done => {
                    packet.finish.done(Ok(()));
                    return WriteAttempt::WriteAttempted;
                }
                PacketSendStatus::SkipPacket => {
                    continue;
                }
            }
        }
    }

    // There might be a cleaner way of doing this.
    // We need the loop to try a new packet if the one first attempted
    // targets a client that is no longer connected.
    fn next_packet<F>(&mut self, client_id: ClientId, mut op: F) -> WriteAttempt
        where F: FnMut(&mut WritePacketBuilder) -> PacketSendStatus
    {
        self.sort_incoming_packets();

        loop {
            let client_queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
            let packet = match client_queue.pop_front() {
                Some(packet) => packet,
                None => return self.any_next_packet(|_, body| op(body)),
            };

            let builder = unsafe { &mut *packet.builder.get() };
            match op(builder) {
                PacketSendStatus::SocketUnavailable => {
                    client_queue.push_front(packet);
                    return WriteAttempt::WriteAttempted;
                }
                PacketSendStatus::Done => {
                    let position = self.packet_order.iter()
                        .position(|id| id == &client_id)
                        .expect("packet_order queue is out of sync with queue_per_connection");
                    self.packet_order.remove(position);
                    packet.finish.done(Ok(()));
                    return WriteAttempt::WriteAttempted;
                }
                PacketSendStatus::SkipPacket => {
                    continue;
                }
            }
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
            packet_order: VecDeque::new(),
            queue_per_connection: HashMap::new(),
        }
    );
}

