use crate::client::{Client, Clients};
use crate::network::{NetworkHandle, SendPacket, Target};
use crate::task::{EndpointResult, Spawner};
use futures::future;
use std::cell::RefCell;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use voice_lib::network::CommsNetworkError;
use voice_lib::packet::{
    AssociateUdp, AssociateUdpAccepted, AudioFromClient, AudioToClient, ClientId, Disconnect,
    DisconnectReason, ErrorResponse, FinishConnect, FinishConnectAccepted, Packet, PacketBody,
    PacketId, UserJoined, UserLeft,
};
use voice_lib::serialization::{IncomingPacket, SerializeError};

pub struct Context {
    pub clients: Arc<RefCell<Clients>>,
    pub client_id: ClientId,
    pub network_handle: NetworkHandle,
}

async fn associate_udp(
    udp_handle: NetworkHandle,
    clients: Arc<RefCell<Clients>>,
    addr: SocketAddr,
    packet: Packet<AssociateUdp>,
) -> EndpointResult {
    let outgoing = {
        let mut clients = clients.borrow_mut();
        let mut client = match clients.connecting.get_mut(&packet.body.client_id) {
            Some(client) => client,
            None => return Ok(()),
        };
        client.udp_addr = Some(addr);
        SendPacket::udp(
            Target::One(packet.body.client_id),
            Some(packet.id),
            AssociateUdpAccepted,
        )?
    };
    outgoing.send(&udp_handle).await;
    Ok(())
}

async fn audio_from_client(context: Context, packet: Packet<AudioFromClient>) -> EndpointResult {
    let client_ids = {
        let clients = context.clients.borrow();
        clients
            .established
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| id != &context.client_id)
            .collect()
    };
    let outgoing = SendPacket::udp(
        Target::Many(client_ids),
        None,
        AudioToClient {
            author: context.client_id,
            bytes: packet.body.0,
        },
    )?;
    outgoing.send(&context.network_handle).await;
    Ok(())
}

async fn finish_connect(context: Context, packet: Packet<FinishConnect>) -> EndpointResult {
    let packets = {
        let mut clients = context.clients.borrow_mut();
        let mut client = match clients.connecting.remove(&context.client_id) {
            Some(c) => c,
            None => return Ok(()),
        };

        match client.udp_addr.take() {
            None => {
                clients.connecting.insert(context.client_id, client);
                vec![SendPacket::tcp(
                    Target::One(context.client_id),
                    Some(packet.id),
                    ErrorResponse::AssociateUdpFirst,
                )?]
            }
            Some(addr) => {
                let other_clients = clients.established.keys().copied().collect();
                clients.established.insert(
                    context.client_id,
                    Client {
                        udp_addr: addr,
                        connection: client.connection,
                        username: packet.body.name.clone(),
                    },
                );

                vec![
                    SendPacket::tcp(
                        Target::One(context.client_id),
                        Some(packet.id),
                        FinishConnectAccepted {
                            users: clients
                                .established
                                .iter()
                                .map(|(id, c)| (*id, c.username.clone()))
                                .collect(),
                        },
                    )?,
                    SendPacket::tcp(
                        Target::Many(other_clients),
                        None,
                        UserJoined {
                            name: packet.body.name,
                            client_id: context.client_id,
                        },
                    )?,
                ]
            }
        }
    };

    future::join_all(
        packets
            .into_iter()
            .map(|packet| packet.send(&context.network_handle)),
    )
    .await;
    Ok(())
}

async fn sent_wrong_packet(context: Context, packet_id: PacketId) -> EndpointResult {
    let outgoing = SendPacket::tcp(
        Target::One(context.client_id),
        Some(packet_id),
        ErrorResponse::UnexpectedPacket(packet_id),
    )?;
    outgoing.send(&context.network_handle).await;
    Ok(())
}

async fn close_connection(context: Context, reason: CloseConnectionReason) -> EndpointResult {
    if reason == CloseConnectionReason::MalformedData {
        let outgoing = SendPacket::tcp(
            Target::One(context.client_id),
            None,
            Disconnect {
                reason: DisconnectReason::MalformedData,
            },
        )?;
        outgoing.send(&context.network_handle).await;
    }

    let remaining_client_ids = {
        let mut clients = context.clients.borrow_mut();
        clients.remove_client(&context.client_id);
        clients.established.keys().copied().collect()
    };

    let outgoing = SendPacket::tcp(
        Target::Many(remaining_client_ids),
        None,
        UserLeft {
            client_id: context.client_id,
        },
    )?;
    outgoing.send(&context.network_handle).await;
    Ok(())
}

fn tcp_packet_to_endpoint(
    spawner: &Spawner,
    context: Context,
    packet: IncomingPacket,
) -> Result<(), CommsNetworkError> {
    let kind = packet.header.kind;
    match kind {
        FinishConnect::KIND_ID => spawner.spawn(finish_connect(context, packet.finish_parse()?)),
        _ => spawner.spawn(sent_wrong_packet(context, packet.header.id)),
    };
    Ok(())
}

fn udp_packet_to_endpoint(
    spawner: &Spawner,
    packet: IncomingPacket,
    context: Context,
) -> Result<(), SerializeError> {
    match packet.kind() {
        AudioFromClient::KIND_ID => {
            spawner.spawn(audio_from_client(context, packet.finish_parse()?))
        }
        _ => spawner.spawn(sent_wrong_packet(context, packet.header.id)),
    };
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
pub enum CloseConnectionReason {
    SocketClosed,
    MalformedData,
}

pub fn spawn_close_connection(
    client_id: ClientId,
    reason: CloseConnectionReason,
    network_handle: &NetworkHandle,
    clients: Arc<RefCell<Clients>>,
    spawner: &Spawner,
) {
    let context = Context {
        client_id,
        network_handle: network_handle.clone(),
        clients,
    };

    spawner.spawn(close_connection(context, reason));
}

#[derive(Debug)]
pub enum HandlePacketError {
    CommunicationFailure,
}

impl From<CommsNetworkError> for HandlePacketError {
    fn from(_: CommsNetworkError) -> Self {
        HandlePacketError::CommunicationFailure
    }
}

impl From<SerializeError> for HandlePacketError {
    fn from(_: SerializeError) -> Self {
        HandlePacketError::CommunicationFailure
    }
}

impl From<io::Error> for HandlePacketError {
    fn from(_: io::Error) -> Self {
        HandlePacketError::CommunicationFailure
    }
}

pub fn handle_udp_packet(
    addr: SocketAddr,
    packet: IncomingPacket,
    clients: Arc<RefCell<Clients>>,
    spawner: &Spawner,
    network_handle: NetworkHandle,
) -> Result<(), SerializeError> {
    let kind = packet.header.kind;
    if kind == AssociateUdp::KIND_ID {
        // This is the only packet that doesn't need to be sent from a client
        spawner.spawn(associate_udp(
            network_handle,
            clients,
            addr,
            packet.finish_parse()?,
        ));
        return Ok(());
    }

    let client_id = {
        let clients = clients.borrow();
        match clients.id_by_socket_addr(&addr) {
            Some(client_id) => client_id,
            None => return Ok(()),
        }
    };

    udp_packet_to_endpoint(
        spawner,
        packet,
        Context {
            client_id,
            clients,
            network_handle,
        },
    )
}

#[derive(Debug)]
pub enum StreamStatus {
    Normal,
    Closed,
    Nonexistent,
}

pub fn handle_tcp_stream(
    client_id: ClientId,
    clients: Arc<RefCell<Clients>>,
    spawner: &Spawner,
    network_handle: &NetworkHandle,
) -> Result<StreamStatus, HandlePacketError> {
    let clients_arc = clients.clone();
    let mut clients = clients.borrow_mut();
    let connection = match clients.connection_by_id(&client_id) {
        Some(connection) => connection,
        None => return Ok(StreamStatus::Nonexistent),
    };

    match connection
        .tcp
        .read_new_packets()
        .try_process(move |packet| {
            let context = Context {
                client_id,
                clients: clients_arc.clone(),
                network_handle: network_handle.clone(),
            };

            tcp_packet_to_endpoint(&spawner, context, packet)
        }) {
        Err(CommsNetworkError::Closed) => return Ok(StreamStatus::Closed),
        Err(_) => return Err(HandlePacketError::CommunicationFailure),
        _ => {}
    }

    Ok(StreamStatus::Normal)
}
