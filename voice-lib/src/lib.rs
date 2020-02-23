use crate::packet::{PacketKind, PacketId};

pub use serde::{Deserialize, Serialize};
use std::mem;

pub mod network;
pub mod packet;
pub mod serialization;
#[cfg(windows)]
pub mod winlib;

#[derive(Debug, Serialize, Deserialize)]
pub struct PacketHeader {
    pub id: PacketId,
    pub reply_to: Option<PacketId>,
    pub kind: PacketKind,
    pub body_len: u16,
}
pub const PACKET_HEADER_SIZE: usize = mem::size_of::<PacketHeader>();

