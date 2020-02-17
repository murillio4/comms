use std::collections::HashMap;
use crate::{ConnectingClient, Client, udp};
use std::net::SocketAddr;
use mio::net::UdpSocket;
use crate::task::{UdpFinish, Spawner, UdpFuture, SyncUnsafeCell};
use std::io;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel, SyncSender};
use mio::{SetReadiness, Ready, Registration, Evented, Token, PollOpt, IoVec};
use voice_lib::packet::{AssociateUdp, PacketBody, AudioFromClient, Packet, ClientId};
use voice_lib::{serialize_body, PacketHeader, serialize_head, deserialize_header, deserialize_remaining, DeserializeError};
use voice_lib::packet;
use futures::future::join_all;
use voice_lib::Serialize;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::io::Read;
use std::fmt::Debug;
use futures::io::Error;
use std::cmp::min;

struct Context {
    clients: Arc<SyncUnsafeCell<HashMap<ClientId, Client>>>,
    connecting_clients: Arc<SyncUnsafeCell<HashMap<ClientId, ConnectingClient>>>,
    addr: SocketAddr,
    udp_handle: UdpHandle,
}

impl Context {
    fn get_current_client_id(&self) -> ClientId {
        let connecting_clients = unsafe { &*self.connecting_clients.get() };
        let clients = unsafe { &*self.clients.get() };

        let connecting_iter = connecting_clients.iter()
            .filter(|(_, c)| c.udp_addr == Some(self.addr))
            .map(|(id, _)| id.clone());

        clients.iter()
            .filter(|(_, c)| c.udp_addr == self.addr)
            .map(|(id, _)| id.clone())
            .chain(connecting_iter)
            .next()
            .unwrap()
    }
}

async fn associate_udp(context: Context, packet: Packet<AssociateUdp>) {
    {
        let connecting_clients = unsafe { &mut *context.connecting_clients.get() };
        let mut client = match connecting_clients.get_mut(&packet.body.client_id) {
            Some(client) => client,
            None => return,
        };
        client.udp_addr = Some(context.addr.clone());
    }
    udp::send_packet(&context.udp_handle, context.get_current_client_id(), packet::AssociateUdpAccepted).await;
}

async fn audio_from_client(context: Context, packet: Packet<AudioFromClient>) {
    let client_ids = {
        let clients = unsafe { &*context.clients.get() };
        clients.iter()
            .map(|(id, _)| id.clone())
            .filter(|id| id != &context.get_current_client_id())
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
    InvalidPacketType(u8, SocketAddr)
}

impl From<DeserializeError> for HandlePacketError {
    fn from(_: DeserializeError) -> Self {
        HandlePacketError::DeserializeFailed
    }
}

#[derive(Debug)]
struct Marker(usize);
struct MarkerRead<'a>(&'a [u8], &'a mut Marker);

impl Read for MarkerRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let marker = (self.1).0;
        let target_slice = &self.0[marker..(marker + buf.len())];
        let len = min(target_slice.len(), buf.len());
        buf[..len].copy_from_slice(target_slice);
        (self.1).0 += buf.len();
        Ok(len)
    }
}

pub fn handle_packet(
    raw_packet: &[u8],
    clients: &Arc<SyncUnsafeCell<HashMap<ClientId, Client>>>,
    connecting_clients: &Arc<SyncUnsafeCell<HashMap<ClientId, ConnectingClient>>>,
    spawner: &Spawner,
    client: SocketAddr,
    udp_handle: &UdpHandle
) -> Result<(), HandlePacketError> {
    let mut marker = Marker(0);
    let marker_reader = MarkerRead(&raw_packet, &mut marker);
    let header = deserialize_header(marker_reader)?;
    let raw_body = &raw_packet[marker.0..];

    let context = Context {
        clients: clients.clone(),
        connecting_clients: connecting_clients.clone(),
        addr: client,
        udp_handle: udp_handle.clone(),
    };
    match header.kind {
        packet::AssociateUdp::KIND_ID => {
            spawner.spawn(associate_udp(context, deserialize_remaining(raw_body, header)?));
        },
        packet::AudioFromClient::KIND_ID => {
            spawner.spawn(audio_from_client(context, deserialize_remaining(raw_body, header)?))
        },
        _ => {
            return Err(HandlePacketError::InvalidPacketType(header.kind, context.addr));
        }
    }
    Ok(())
}

async fn send_packet<B>(handle: &UdpHandle, dest: ClientId, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());
    handle.send(dest, B::KIND_ID, raw_body).await.unwrap();
}

async fn send_packet_to_many<B>(handle: &UdpHandle, clients: Vec<ClientId>, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());

    let futures: Vec<UdpFuture> = clients.into_iter()
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

pub fn attempt_write_packet(socket: &UdpSocket, outgoing: &mut UdpOutgoingQueue, socket_writable: &mut bool, clients: &mut HashMap<ClientId, Client>, connecting_clients: &mut HashMap<ClientId, ConnectingClient>) {
    match outgoing.next_packet(|destination_id, packet_kind, body| {
        let (packet_id, addr) = if let Some(client) = clients.get_mut(&destination_id) {
            let packet_id = client.next_packet_id;
            client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
            (packet_id, client.udp_addr.clone())
        } else if let Some(client) = connecting_clients.get_mut(&destination_id) {
            let packet_id = client.next_packet_id;
            client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
            (packet_id, client.udp_addr.expect("attempting to send UDP packet to client that hasn't associated yet").clone())
        } else {
            unimplemented!("Should not try to send packet to client that doesn't exist");
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
                println!("Sent packet of kind {}", packet_kind);
                return PacketSendStatus::Done
            },
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return PacketSendStatus::TryAgain,
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
    finish: UdpFinish,
}

pub struct UdpOutgoingQueue {
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
    peek_item: Option<OutgoingPacket>,
}

impl UdpOutgoingQueue {
    fn next_packet<F>(&mut self, op: F) -> WriteAttempt
        where F: FnOnce(ClientId, u8, &Arc<Vec<u8>>) -> PacketSendStatus
    {
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
            PacketSendStatus::TryAgain => self.peek_item = Some(packet),
            PacketSendStatus::Done => packet.finish.done(Ok(())),
        }

        WriteAttempt::WriteAttempted
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
    TryAgain,
}

#[derive(Clone)]
pub struct UdpHandle {
    sender: SyncSender<OutgoingPacket>,
    set_queue_readiness: SetReadiness,
}

impl UdpHandle {
    pub fn send(&self, dest: ClientId, kind: u8, raw_body: Arc<Vec<u8>>) -> UdpFuture {
        let (future, finish) = UdpFuture::new();
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