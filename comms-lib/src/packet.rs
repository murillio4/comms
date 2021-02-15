use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

pub type ClientId = u16;
pub type PacketKind = u8;
pub type PacketId = NonZeroU64;

#[derive(Debug, Serialize, Deserialize)]
pub struct Packet<T: PacketBody> {
    pub id: PacketId,
    pub reply_to: Option<PacketId>,
    pub body: T,
}

impl<T: PacketBody> Packet<T> {
    pub fn new(id: PacketId, reply_to: Option<PacketId>, body: T) -> Self {
        Packet { id, reply_to, body }
    }
}

pub trait PacketBody: Serialize {
    const KIND_ID: PacketKind;
    fn kind_id() -> PacketKind {
        Self::KIND_ID
    }
}

#[derive(Serialize, Deserialize)]
pub struct AssociateUdp {
    pub client_id: ClientId,
}

impl PacketBody for AssociateUdp {
    const KIND_ID: PacketKind = 0u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AssociateUdpAccepted;

impl PacketBody for AssociateUdpAccepted {
    const KIND_ID: PacketKind = 1u8;
}

#[derive(Serialize, Deserialize)]
pub struct FinishConnect {
    pub name: String,
}

impl PacketBody for FinishConnect {
    const KIND_ID: PacketKind = 2u8;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AudioFromClient(#[serde(with = "serde_bytes")] pub Vec<u8>);

impl PacketBody for AudioFromClient {
    const KIND_ID: PacketKind = 3u8;
}

#[derive(Serialize, Deserialize)]
pub struct KeepAlive;

impl PacketBody for KeepAlive {
    const KIND_ID: PacketKind = 4u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AudioToClient {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    pub author: ClientId,
}

impl PacketBody for AudioToClient {
    const KIND_ID: PacketKind = 5u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FinishConnectAccepted {
    pub users: Vec<(ClientId, String)>,
}

impl PacketBody for FinishConnectAccepted {
    const KIND_ID: PacketKind = 6u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ErrorResponse {
    AssociateUdpFirst,
    UnexpectedPacket(PacketId),
}

impl PacketBody for ErrorResponse {
    const KIND_ID: PacketKind = 7u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub client_id: ClientId,
}

impl PacketBody for Hello {
    const KIND_ID: PacketKind = 8u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserJoined {
    pub client_id: ClientId,
    pub name: String,
}

impl PacketBody for UserJoined {
    const KIND_ID: PacketKind = 9u8;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserLeft {
    pub client_id: ClientId,
}

impl PacketBody for UserLeft {
    const KIND_ID: PacketKind = 10u8;
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum DisconnectReason {
    Leaving,
    MalformedData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Disconnect {
    pub reason: DisconnectReason,
}

impl PacketBody for Disconnect {
    const KIND_ID: PacketKind = 11u8;
}
