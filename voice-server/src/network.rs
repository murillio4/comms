use crate::client::{AnyClientRef, Clients};
use crate::task::EndpointError;
use mio::Waker;
use std::cell::RefCell;
use std::fmt::Debug;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use voice_lib::network::{Protocol, ServerUdpPacket, UdpConnection};
use voice_lib::packet::{ClientId, PacketBody, PacketId};
use voice_lib::serialization::WritePacketBuilder;
use voice_lib::Serialize;

pub struct OutgoingPacket {
    pub dest: ClientId,
    pub builder: Arc<RefCell<WritePacketBuilder>>,
}

pub struct OutgoingQueue {
    receiver: Receiver<(Protocol, OutgoingPacket)>,
}

impl OutgoingQueue {
    pub fn sort_from_handle(
        &mut self,
        clients: &mut Clients,
        udp: &mut UdpConnection<ServerUdpPacket>,
    ) {
        // Sort the incoming queue of packets into their respective socket buckets
        for (protocol, packet) in self.receiver.try_iter() {
            match protocol {
                Protocol::Udp => {
                    let (packet_id, addr) = match clients.client_by_id(&packet.dest) {
                        Some(AnyClientRef::Established(c)) => {
                            (c.connection.next_packet_id(), c.udp_addr)
                        },
                        Some(AnyClientRef::Connecting(c)) => (
                            c.connection.next_packet_id(),
                            c.udp_addr.expect("Attempting to send UDP packet to client that hasn't associated yet"),
                        ),
                        _ => continue,
                    };
                    let mut builder = packet.builder.borrow_mut();
                    udp.push_outgoing_without_send(ServerUdpPacket::new(
                        addr,
                        builder.as_raw(packet_id).unwrap().to_vec(),
                    ))
                }
                Protocol::Tcp => {
                    if let Some(connection) = clients.connection_by_id(&packet.dest) {
                        let mut builder = packet.builder.borrow_mut();
                        let bytes = builder.as_raw(connection.next_packet_id()).unwrap();
                        connection.tcp.push_outgoing_without_send(bytes);
                    } else {
                        println!("connection could not be found :)");
                    }
                }
            }
        }
    }

    pub fn attempt_write_all(
        &mut self,
        udp: &mut UdpConnection<ServerUdpPacket>,
        clients: &mut Clients,
    ) -> Vec<ClientId> {
        udp.write_until_blocked().unwrap();

        let mut failures = Vec::new();
        for (client_id, client) in clients.established.iter_mut() {
            if client.connection.tcp.write_until_blocked().is_err() {
                failures.push(*client_id);
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
    pub fn send(
        &self,
        protocol: Protocol,
        dest: ClientId,
        builder: Arc<RefCell<WritePacketBuilder>>,
    ) {
        self.sender
            .send((protocol, OutgoingPacket { dest, builder }))
            .unwrap();
        self.waker.wake().unwrap();
    }
}

pub fn queue_and_handle(waker: Arc<Waker>) -> (NetworkHandle, OutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    (NetworkHandle { sender, waker }, OutgoingQueue { receiver })
}

pub enum Target {
    One(ClientId),
    Many(Vec<ClientId>),
}

pub struct SendPacket {
    destination: Target,
    protocol: Protocol,
    builder: Arc<RefCell<WritePacketBuilder>>,
}

impl SendPacket {
    pub fn udp<B>(
        destination: Target,
        reply_to: Option<PacketId>,
        packet: B,
    ) -> Result<Self, EndpointError>
    where
        B: PacketBody + Serialize + Debug + 'static,
    {
        let builder = WritePacketBuilder::new(reply_to, &packet).map_err(|_| {
            EndpointError::SerializeFailed {
                kind: B::KIND_ID,
                packet: Box::new(packet),
            }
        })?;
        Ok(Self {
            protocol: Protocol::Udp,
            destination,
            builder: Arc::new(RefCell::new(builder)),
        })
    }

    pub fn tcp<B>(
        destination: Target,
        reply_to: Option<PacketId>,
        packet: B,
    ) -> Result<Self, EndpointError>
    where
        B: PacketBody + Serialize + Debug + 'static,
    {
        let builder = WritePacketBuilder::new(reply_to, &packet).map_err(|_| {
            EndpointError::SerializeFailed {
                kind: B::KIND_ID,
                packet: Box::new(packet),
            }
        })?;
        Ok(Self {
            destination,
            protocol: Protocol::Tcp,
            builder: Arc::new(RefCell::new(builder)),
        })
    }

    pub async fn send(self, handle: &NetworkHandle) {
        let protocol = self.protocol;
        let builder = self.builder;
        match self.destination {
            Target::One(dest) => handle.send(protocol, dest, builder),
            Target::Many(clients) => {
                for client in clients {
                    handle.send(protocol.clone(), client, builder.clone())
                }
            }
        }
    }
}
