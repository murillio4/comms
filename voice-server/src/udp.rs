use crate::Clients;
use std::net::SocketAddr;
use mio::net::UdpSocket;
use crate::task::{PacketFinish, Spawner, PacketFuture, SyncUnsafeCell, HandlePacketError};
use std::io;
use std::sync::mpsc::{Receiver, sync_channel, SyncSender};
use mio::{SetReadiness, Ready, Registration, Evented, Token, PollOpt};
use voice_lib::packet;
use std::sync::Arc;
use std::fmt::Debug;
use crate::tcp::{TcpHandle, OutgoingPacketBuilder};
use voice_lib::packet::{ClientId, Packet, AssociateUdp, AssociateUdpAccepted, AudioFromClient, PacketBody, AudioToClient};
use voice_lib::serialization::{DeserializeError, HeaderParsedPacket, WritePacketBuilder};

pub struct Context {
    pub clients: Arc<SyncUnsafeCell<Clients>>,
    pub client_id: ClientId,
    pub udp_handle: UdpHandle,
    pub tcp_handle: TcpHandle,
}

async fn associate_udp(udp_handle: UdpHandle, clients: Arc<SyncUnsafeCell<Clients>>, addr: SocketAddr, packet: Packet<AssociateUdp>) -> Result<(), HandlePacketError> {
    let outgoing = {
        let clients = unsafe { &mut *clients.get() };
        let mut client = match clients.connecting.get_mut(&packet.body.client_id) {
            Some(client) => client,
            None => return Ok(()),
        };
        client.udp_addr = Some(addr);
        OutgoingPacketBuilder::new(Some(packet.id), AssociateUdpAccepted)?
    };
    outgoing.send_packet_udp(&udp_handle, packet.body.client_id).await;
    Ok(())
}

async fn audio_from_client(context: Context, packet: Packet<AudioFromClient>) -> Result<(), HandlePacketError> {
    let client_ids = {
        let clients = unsafe { &*context.clients.get() };
        clients.established.iter()
            .map(|(id, _)| id.clone())
            .filter(|id| id != &context.client_id)
            .collect()
    };
    let outgoing = OutgoingPacketBuilder::new(None, AudioToClient {
        author: context.client_id,
        bytes: packet.body.0,
    })?;
    outgoing.send_packet_to_many_udp(&context.udp_handle, client_ids).await;
    Ok(())
}

#[derive(Debug)]
pub enum HandleReadableStreamError {
    Io,
    DeserializeFailed,
    InvalidPacketType(u8),
}

impl From<DeserializeError> for HandleReadableStreamError {
    fn from(_: DeserializeError) -> Self {
        HandleReadableStreamError::DeserializeFailed
    }
}

impl From<io::Error> for HandleReadableStreamError {
    fn from(_: io::Error) -> Self {
        HandleReadableStreamError::Io
    }
}

pub fn handle_packet(
    addr: SocketAddr,
    raw_packet: &[u8],
    clients: Arc<SyncUnsafeCell<Clients>>,
    spawner: &Spawner,
    udp_handle: UdpHandle,
    tcp_handle: TcpHandle,
) -> Result<(), HandleReadableStreamError> {
    let parsed_packet = HeaderParsedPacket::from_raw(raw_packet)?;
    let kind = parsed_packet.kind();
    if kind == packet::AssociateUdp::KIND_ID {
        // This is the only packet that doesn't need to be sent from a client
        println!("Received UDP packet of type {}", kind);
        spawner.spawn(associate_udp(udp_handle, clients, addr, parsed_packet.finish_parse()?));
        return Ok(());
    }

    let client_id = {
        let clients = unsafe { &mut *clients.get() };
        match clients.id_by_socket_addr(&addr) {
            Some(client_id) => client_id,
            None => return Ok(())
        }
    };

    if kind != AudioFromClient::KIND_ID {
        println!("Received UDP packet of type {} from {}", kind, client_id);
    }

    let context = Context {
        client_id,
        clients,
        udp_handle,
        tcp_handle,
    };

    match kind {
        AudioFromClient::KIND_ID => spawner.spawn(audio_from_client(context, parsed_packet.finish_parse()?)),
        _ => return Err(HandleReadableStreamError::InvalidPacketType(kind)),
    }
    Ok(())
}

struct OutgoingPacket {
    dest: ClientId,
    builder: Arc<SyncUnsafeCell<WritePacketBuilder>>,
    finish: PacketFinish,
}

pub struct UdpOutgoingQueue {
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
    peek_item: Option<OutgoingPacket>,
}

impl UdpOutgoingQueue {
    pub fn attempt_write_next(&mut self, socket: &UdpSocket, socket_writable: &mut bool, clients: &mut Clients)
    {
        *socket_writable = true;
        let (packet_id, packet, addr) = match self.peek_item.take()
            .into_iter()
            .chain(self.receiver.try_iter())
            .filter_map(|packet| {
                let (packet_id, addr) = if let Some(client) = clients.established.get_mut(&packet.dest) {
                    let packet_id = client.connection.next_packet_id.next();
                    (packet_id, client.udp_addr.clone())
                } else if let Some(client) = clients.connecting.get_mut(&packet.dest) {
                    let packet_id = client.connection.next_packet_id.next();
                    (packet_id, client.udp_addr.expect("attempting to send UDP packet to client that hasn't associated yet").clone())
                } else {
                    eprintln!("Attempted to send UDP packet to client {} that doesn't exist", &packet.dest);
                    return None;
                };
                Some((packet_id, packet, addr))
            })
            .next() {
            Some(tuple) => tuple,
            None => return,
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
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => self.peek_item = Some(packet),
            Err(e) => panic!(e),
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

#[derive(Clone)]
pub struct UdpHandle {
    sender: SyncSender<OutgoingPacket>,
    set_queue_readiness: SetReadiness,
}

impl UdpHandle {
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
    );
}