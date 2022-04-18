use crate::packet::{AudioIndex, PacketId, PacketKind};

use crate::serialization::SerializeError;
pub use serde::{Deserialize, Serialize};
use std::mem;

pub mod network;
pub mod packet;
pub mod serialization;

pub struct OpusAudioFrame {
    pub index: AudioIndex,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PacketHeader {
    pub id: PacketId,
    pub reply_to: Option<PacketId>,
    pub kind: PacketKind,
    pub body_len: u16,
}
pub const PACKET_HEADER_SIZE: usize = mem::size_of::<PacketHeader>();

impl PacketHeader {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SerializeError> {
        bincode::deserialize(bytes)
    }
}
