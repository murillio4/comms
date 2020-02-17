use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

pub const MAX_AUDIO_BUFFER_SIZE: usize = 2000;
pub type ClientId = u16;

#[derive(Serialize, Deserialize)]
pub struct Packet<T: PacketBody> {
    pub id: NonZeroU64,
    pub reply_to: Option<NonZeroU64>,
    pub body: T,
}

impl<T: PacketBody> Packet<T> {
    pub fn new(id: NonZeroU64, reply_to: Option<NonZeroU64>, body: T) -> Self {
        Packet {
            id,
            reply_to,
            body,
        }
    }
}

pub trait PacketBody {
    const KIND_ID: u8;
}

#[derive(Serialize, Deserialize)]
pub struct AssociateUdp {
    pub client_id: u16,
}

impl PacketBody for AssociateUdp {
    const KIND_ID: u8 = 0u8;
}

#[derive(Serialize, Deserialize)]
pub struct AssociateUdpAccepted;

impl PacketBody for AssociateUdpAccepted {
    const KIND_ID: u8 = 1u8;
}

#[derive(Serialize, Deserialize)]
pub struct Connect {
    pub username: String,
}

impl PacketBody for Connect {
    const KIND_ID: u8 = 2u8;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AudioFromClient(
    #[serde(with = "serde_bytes")]
    pub Vec<u8>
);

impl PacketBody for AudioFromClient {
    const KIND_ID: u8 = 3u8;
}


#[derive(Serialize, Deserialize)]
pub struct Disconnect;

impl PacketBody for Disconnect {
    const KIND_ID: u8 = 4u8;
}

#[derive(Serialize, Deserialize)]
pub struct KeepAlive;

impl PacketBody for KeepAlive {
    const KIND_ID: u8 = 5u8;
}

#[derive(Serialize, Deserialize)]
pub struct AudioToClient {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
}

impl PacketBody for AudioToClient {
    const KIND_ID: u8 = 6u8;
}

#[derive(Serialize, Deserialize)]
pub struct ConnectAccepted {
    pub users: Vec<String>,
}

impl PacketBody for ConnectAccepted {
    const KIND_ID: u8 = 7u8;
}

#[derive(Serialize, Deserialize)]
pub enum ErrorResponse {
    ConnectFirst,
}

impl PacketBody for ErrorResponse {
    const KIND_ID: u8 = 8u8;
}

#[derive(Serialize, Deserialize)]
pub struct AssignId {
    pub id: ClientId
}

impl PacketBody for AssignId {
    const KIND_ID: u8 = 9u8;
}