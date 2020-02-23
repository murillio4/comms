use voice_lib::packet::{ClientId, Packet, PacketBody, FinishConnect, FinishConnectAccepted, ErrorResponse, PacketId};
use std::sync::Arc;
use crate::task::{SyncUnsafeCell, Spawner, EndpointError};
use crate::udp::{HandlePacketError, Context};
use voice_lib::network::{UpdateFromStream, IncomingStreamPacket};
use mio::Poll;
use crate::network::{NetworkHandle, SendPacket};
use crate::client::{AnyClient, Client, Clients};

async fn finish_connect(context: Context, packet: Packet<FinishConnect>) -> Result<(), EndpointError> {
    let packet = {
        let clients = unsafe { &mut *context.clients.get() };
        let mut client = match clients.connecting.remove(&context.client_id) {
            Some(c) => c,
            None => return Ok(()),
        };

        match client.udp_addr.take() {
            None => {
                clients.connecting.insert(context.client_id, client);
                SendPacket::new(Some(packet.id), ErrorResponse::AssociateUdpFirst)?
            }
            Some(addr) => {
                clients.established.insert(context.client_id, Client {
                    udp_addr: addr,
                    connection: client.connection,
                    username: packet.body.username,
                });
                SendPacket::new(Some(packet.id), FinishConnectAccepted {
                    users: clients.established.values()
                        .map(|c| c.username.clone())
                        .collect()
                })?
            }
        }
    };

    packet.tcp(&context.network_handle, context.client_id).await;
    Ok(())
}

pub async fn sent_wrong_packet(context: Context, packet_id: PacketId) -> Result<(), EndpointError> {
    let outgoing = SendPacket::new(Some(packet_id), ErrorResponse::UnexpectedPacket(packet_id))?;
    outgoing.tcp(&context.network_handle, context.client_id).await;
    Ok(())
}

fn packet_to_endpoint(spawner: &Spawner, context: Context, packet: IncomingStreamPacket) -> Result<(), HandlePacketError> {
    let kind = packet.header.kind;
    match kind {
        FinishConnect::KIND_ID => spawner.spawn(finish_connect(context, packet.finish_parse()?)),
        _ => spawner.spawn(sent_wrong_packet(context, packet.header.id)),
    };
    Ok(())
}

pub fn handle_readable_stream(
    client_id: ClientId,
    clients: Arc<SyncUnsafeCell<Clients>>,
    spawner: &Spawner,
    network_handle: &NetworkHandle,
    poll: &Poll,
) -> Result<(), HandlePacketError> {
    let mut current_client = {
        let clients = unsafe { &mut *clients.get() };
        if let Some(client) = clients.use_current_client(&client_id) {
            client
        } else {
            return Ok(());
        }
    };

    let connection = match &mut current_client {
        AnyClient::Established(client) => &mut client.connection,
        AnyClient::Connecting(client) => &mut client.connection,
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
                    network_handle: network_handle.clone(),
                };

                packet_to_endpoint(&spawner, context, stream_packet)?;
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