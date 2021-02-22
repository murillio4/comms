use crate::client::{AnyClientRef, Clients};
use crate::task::EndpointError;
use comms_lib::network::{Protocol, ServerUdpPacket, UdpConnection};
use comms_lib::packet::{ClientId, PacketBody, PacketId};
use comms_lib::serialization::OutgoingPacketFrame;
use mio::Waker;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

pub struct OutgoingQueue {
    receiver: Receiver<(Protocol, ClientId, OutgoingPacketFrame)>,
}

impl OutgoingQueue {
    pub fn sort_from_handle(
        &mut self,
        clients: &mut Clients,
        udp: &mut UdpConnection<ServerUdpPacket>,
    ) {
        // Sort the incoming queue of packets into their respective socket buckets
        for (protocol, dest, packet) in self.receiver.try_iter() {
            match protocol {
                Protocol::Udp => {
                    let (packet_id, addr) = match clients.client_by_id(&dest) {
                        Some(AnyClientRef::Established(c)) => {
                            (c.connection.next_packet_id(), c.udp_addr)
                        },
                        Some(AnyClientRef::Connecting(c)) => (
                            c.connection.next_packet_id(),
                            c.udp_addr.expect("Attempting to send UDP packet to client that hasn't associated yet"),
                        ),
                        _ => continue,
                    };
                    udp.push_outgoing_without_send(ServerUdpPacket::new(
                        addr,
                        packet.into_raw(packet_id).unwrap(),
                    ))
                }
                Protocol::Tcp => {
                    if let Some(connection) = clients.connection_by_id(&dest) {
                        let bytes = packet.into_raw(connection.next_packet_id()).unwrap();
                        connection.tcp.push_outgoing_without_send(&bytes);
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
        for (client_id, connection) in clients
            .established
            .iter_mut()
            .map(|(client_id, client)| (client_id, &mut client.connection))
            .chain(
                clients
                    .connecting
                    .iter_mut()
                    .map(|(client_id, client)| (client_id, &mut client.connection)),
            )
        {
            if connection.tcp.write_until_blocked().is_err() {
                failures.push(*client_id);
            }
        }

        failures
    }
}

#[derive(Clone)]
pub struct NetworkHandle {
    sender: SyncSender<(Protocol, ClientId, OutgoingPacketFrame)>,
    waker: Arc<Waker>,
}

impl NetworkHandle {
    pub fn send(&self, protocol: Protocol, dest: ClientId, packet: OutgoingPacketFrame) {
        self.sender.send((protocol, dest, packet)).unwrap();
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
    packet: OutgoingPacketFrame,
}

impl SendPacket {
    fn new<B>(
        protocol: Protocol,
        destination: Target,
        reply_to: Option<PacketId>,
        packet: B,
    ) -> Result<Self, EndpointError>
    where
        B: PacketBody,
    {
        let packet = OutgoingPacketFrame::from_packet(reply_to, &packet)
            .map_err(|_| EndpointError::SerializeFailed { kind: B::KIND_ID })?;
        Ok(Self {
            protocol,
            destination,
            packet,
        })
    }

    pub fn udp<B>(
        destination: Target,
        reply_to: Option<PacketId>,
        packet: B,
    ) -> Result<Self, EndpointError>
    where
        B: PacketBody,
    {
        Self::new(Protocol::Udp, destination, reply_to, packet)
    }

    pub fn tcp<B>(
        destination: Target,
        reply_to: Option<PacketId>,
        packet: B,
    ) -> Result<Self, EndpointError>
    where
        B: PacketBody,
    {
        Self::new(Protocol::Tcp, destination, reply_to, packet)
    }

    pub async fn send(self, handle: &NetworkHandle) {
        let protocol = self.protocol;
        match self.destination {
            Target::One(dest) => handle.send(protocol, dest, self.packet),
            Target::Many(clients) => {
                for client in clients {
                    handle.send(protocol.clone(), client, self.packet.clone())
                }
            }
        }
    }
}
