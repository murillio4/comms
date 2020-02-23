use std::net::SocketAddr;
use crate::task::{Spawner, SyncUnsafeCell, EndpointError};
use std::io;
use voice_lib::packet;
use std::sync::Arc;
use std::fmt::Debug;
use voice_lib::packet::{ClientId, Packet, AssociateUdp, AssociateUdpAccepted, AudioFromClient, PacketBody, AudioToClient};
use voice_lib::serialization::{DeserializeError, HeaderParsedPacket};
use crate::network::{NetworkHandle, SendPacket};
use crate::client::Clients;
use crate::tcp::sent_wrong_packet;

pub struct Context {
    pub clients: Arc<SyncUnsafeCell<Clients>>,
    pub client_id: ClientId,
    pub network_handle: NetworkHandle,
}

async fn associate_udp(udp_handle: NetworkHandle, clients: Arc<SyncUnsafeCell<Clients>>, addr: SocketAddr, packet: Packet<AssociateUdp>) -> Result<(), EndpointError> {
    let outgoing = {
        let clients = unsafe { &mut *clients.get() };
        let mut client = match clients.connecting.get_mut(&packet.body.client_id) {
            Some(client) => client,
            None => return Ok(()),
        };
        client.udp_addr = Some(addr);
        SendPacket::new(Some(packet.id), AssociateUdpAccepted)?
    };
    outgoing.udp(&udp_handle, packet.body.client_id).await;
    Ok(())
}

async fn audio_from_client(context: Context, packet: Packet<AudioFromClient>) -> Result<(), EndpointError> {
    let client_ids = {
        let clients = unsafe { &*context.clients.get() };
        clients.established.iter()
            .map(|(id, _)| id.clone())
            .filter(|id| id != &context.client_id)
            .collect()
    };
    let outgoing = SendPacket::new(None, AudioToClient {
        author: context.client_id,
        bytes: packet.body.0,
    })?;
    outgoing.udp_many(&context.network_handle, client_ids).await;
    Ok(())
}

#[derive(Debug)]
pub enum HandlePacketError {
    CommunicationFailure,
}

impl From<DeserializeError> for HandlePacketError {
    fn from(_: DeserializeError) -> Self {
        HandlePacketError::CommunicationFailure
    }
}

impl From<io::Error> for HandlePacketError {
    fn from(_: io::Error) -> Self {
        HandlePacketError::CommunicationFailure
    }
}

fn packet_to_endpoint(spawner: &Spawner, packet: HeaderParsedPacket, context: Context) -> Result<(), HandlePacketError> {
    let kind = packet.kind();
    if kind != AudioFromClient::KIND_ID {
        println!("Received UDP packet of type {} from {}", kind, context.client_id);
    }

    match kind {
        AudioFromClient::KIND_ID => spawner.spawn(audio_from_client(context, packet.finish_parse()?)),
        _ => spawner.spawn(sent_wrong_packet(context, packet.header.id))
    };
    Ok(())
}

pub fn handle_raw_packet(
    addr: SocketAddr,
    raw_packet: &[u8],
    clients: Arc<SyncUnsafeCell<Clients>>,
    spawner: &Spawner,
    network_handle: NetworkHandle,
) -> Result<(), HandlePacketError> {
    let parsed_packet = HeaderParsedPacket::from_raw(raw_packet)?;
    let kind = parsed_packet.kind();
    if kind == packet::AssociateUdp::KIND_ID {
        // This is the only packet that doesn't need to be sent from a client
        println!("Received UDP packet of type {}", kind);
        spawner.spawn(associate_udp(network_handle, clients, addr, parsed_packet.finish_parse()?));
        return Ok(());
    }

    let client_id = {
        let clients = unsafe { &mut *clients.get() };
        match clients.id_by_socket_addr(&addr) {
            Some(client_id) => client_id,
            None => return Ok(())
        }
    };

    packet_to_endpoint(spawner, parsed_packet, Context {
        client_id,
        clients,
        network_handle,
    })
}