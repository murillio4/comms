use crate::{udp, Clients};
use std::net::SocketAddr;
use mio::net::UdpSocket;
use crate::task::{PacketFinish, Spawner, PacketFuture, SyncUnsafeCell};
use std::io;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel, SyncSender};
use mio::{SetReadiness, Ready, Registration, Evented, Token, PollOpt, IoVec};
use voice_lib::packet::{AssociateUdp, PacketBody, AudioFromClient, Packet, ClientId};
use voice_lib::{serialize_body, PacketHeader, serialize_head, deserialize_remaining, DeserializeError, deserialize_header};
use voice_lib::packet;
use futures::future::join_all;
use voice_lib::Serialize;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::fmt::Debug;
use crate::tcp::TcpHandle;

pub struct Context {
    pub clients: Arc<SyncUnsafeCell<Clients>>,
    pub client_id: ClientId,
    pub udp_handle: UdpHandle,
    pub tcp_handle: TcpHandle,
}

async fn associate_udp(context: Context, packet: Packet<AssociateUdp>, addr: SocketAddr) {
    let client_id = {
        let clients = unsafe { &mut *context.clients.get() };
        let mut client = match clients.connecting.get_mut(&packet.body.client_id) {
            Some(client) => client,
            None => return,
        };
        client.udp_addr = Some(addr);
        client.client_id
    };
    udp::send_packet(&context.udp_handle, client_id, packet::AssociateUdpAccepted).await;
}

async fn audio_from_client(context: Context, packet: Packet<AudioFromClient>) {
    let client_ids = {
        let clients = unsafe { &*context.clients.get() };
        clients.established.iter()
            .map(|(id, _)| id.clone())
            .filter(|id| id != &context.client_id)
            .collect()
    };
    let audio_to_client = packet::AudioToClient {
        bytes: packet.body.0,
    };
    udp::send_packet_to_many(&context.udp_handle, client_ids, audio_to_client).await;
}

#[derive(Debug)]
pub enum HandlePacketError {
    DeserializeFailed,
    InvalidPacketType(u8),
    NeededToBeClient,
}

impl From<DeserializeError> for HandlePacketError {
    fn from(_: DeserializeError) -> Self {
        HandlePacketError::DeserializeFailed
    }
}

pub fn handle_packet(
    addr: SocketAddr,
    raw_packet: &[u8],
    clients: Arc<SyncUnsafeCell<Clients>>,
    spawner: &Spawner,
    udp_handle: UdpHandle,
    tcp_handle: TcpHandle,
) -> Result<(), HandlePacketError> {
    let (body_offset, header) = deserialize_header(raw_packet)?;
    let raw_body = &raw_packet[body_offset..];

    let client_id = {
        let clients = unsafe { &mut *clients.get() };
        match clients.id_by_socket_addr(&addr) {
            Some(client_id) => client_id,
            None => return Err(HandlePacketError::NeededToBeClient)
        }
    };

    let context = Context {
        client_id,
        clients,
        udp_handle,
        tcp_handle,
    };

    match header.kind {
        packet::AssociateUdp::KIND_ID => spawner.spawn(associate_udp(context, deserialize_remaining(raw_body, header)?, addr)),
        packet::AudioFromClient::KIND_ID => spawner.spawn(audio_from_client(context, deserialize_remaining(raw_body, header)?)),
        _ => return Err(HandlePacketError::InvalidPacketType(header.kind)),
    }
    Ok(())
}

pub async fn send_packet<B>(handle: &UdpHandle, dest: ClientId, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());
    handle.send(dest, B::KIND_ID, raw_body).await.unwrap();
}

pub async fn send_packet_to_many<B>(handle: &UdpHandle, clients: Vec<ClientId>, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());

    let futures: Vec<PacketFuture> = clients.into_iter()
        .map(|c| handle.send(c, B::KIND_ID, raw_body.clone()))
        .collect();
    for result in join_all(futures).await {
        result.unwrap();
    }
}

pub enum WriteAttempt {
    NoPacketAvailable,
    WriteAttempted,
}

pub fn attempt_write_packet(socket: &UdpSocket, outgoing: &mut UdpOutgoingQueue, socket_writable: &mut bool, clients: &mut Clients) {
    match outgoing.next_packet(|destination_id, packet_kind, body| {
        let (packet_id, addr) = if let Some(client) = clients.established.get_mut(&destination_id) {
            let packet_id = client.next_packet_id;
            client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
            (packet_id, client.udp_addr.clone())
        } else if let Some(client) = clients.connecting.get_mut(&destination_id) {
            let packet_id = client.next_packet_id;
            client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
            (packet_id, client.udp_addr.expect("attempting to send UDP packet to client that hasn't associated yet").clone())
        } else {
            eprintln!("Attempted to send UDP packet to client {} that doesn't exist", destination_id);
            return PacketSendStatus::SkipPacket;
        };

        let packet_header = PacketHeader {
            id: packet_id,
            reply_to: None,
            kind: packet_kind,
        };

        socket.connect(addr).unwrap();
        let header = serialize_head(packet_header).unwrap();
        let buffers: Vec<&IoVec> = vec![
            IoVec::from_bytes(&header),
            IoVec::from_bytes(body),
        ].into_iter()
            .filter_map(|o| o)
            .collect();

        match socket.send_bufs(&buffers) {
            Ok(_) => {
                println!("Sent UDP packet of kind {} to {}", packet_kind, destination_id);
                return PacketSendStatus::Done
            },
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return PacketSendStatus::SocketUnavailable,
            Err(e) => panic!(e),
        }
    }) {
        WriteAttempt::NoPacketAvailable => *socket_writable = true,
        WriteAttempt::WriteAttempted => *socket_writable = false,
    }
}

struct OutgoingPacket {
    dest: ClientId,
    kind: u8,
    raw_body: Arc<Vec<u8>>,
    finish: PacketFinish,
}

pub struct UdpOutgoingQueue {
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
    peek_item: Option<OutgoingPacket>,
}

impl UdpOutgoingQueue {
    fn next_packet<F>(&mut self, mut op: F) -> WriteAttempt
        where F: FnMut(ClientId, u8, &Arc<Vec<u8>>) -> PacketSendStatus
    {
        // There might be a cleaner way of doing this.
        // We need the loop to try a new packet if the one first attempted
        // targets a client that is no longer connected.
        loop {
            let packet = if let Some(body) = self.peek_item.take() {
                body
            } else {
                match self.receiver.try_recv() {
                    Ok(packet) => packet,
                    Err(e) if e == TryRecvError::Empty => return WriteAttempt::NoPacketAvailable,
                    Err(e) => panic!(e),
                }
            };

            match op(packet.dest, packet.kind, &packet.raw_body) {
                PacketSendStatus::SocketUnavailable => {
                    self.peek_item = Some(packet);
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::Done => {
                    packet.finish.done(Ok(()));
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::SkipPacket => {
                    continue;
                }
            }
        }
    }
}

impl Evented for UdpOutgoingQueue {
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

enum PacketSendStatus {
    Done,
    SocketUnavailable,
    SkipPacket,
}

#[derive(Clone)]
pub struct UdpHandle {
    sender: SyncSender<OutgoingPacket>,
    set_queue_readiness: SetReadiness,
}

impl UdpHandle {
    pub fn send(&self, dest: ClientId, kind: u8, raw_body: Arc<Vec<u8>>) -> PacketFuture {
        let (future, finish) = PacketFuture::new();
        self.sender.send(OutgoingPacket {
            dest,
            kind,
            raw_body,
            finish,
        }).unwrap();
        self.set_queue_readiness.set_readiness(Ready::readable()).unwrap();
        future
    }
}

pub fn queue_and_handle() -> (UdpHandle, UdpOutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    let (registration, set_queue_readiness) = Registration::new2();
    return (
        UdpHandle {
            sender,
            set_queue_readiness,
        },
        UdpOutgoingQueue {
            receiver,
            registration,
            peek_item: None,
        }
    )
}