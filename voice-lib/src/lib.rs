use std::num::NonZeroU64;
use crate::packet::PacketKind;

pub use serde::{Deserialize, Serialize};
use std::mem;

pub mod network;
pub mod packet;
pub mod serialization;
#[cfg(windows)]
pub mod winlib;

#[derive(Debug, Serialize, Deserialize)]
pub struct PacketHeader {
    pub id: NonZeroU64,
    pub reply_to: Option<NonZeroU64>,
    pub kind: PacketKind,
    pub body_len: u16,
}
pub const PACKET_HEADER_SIZE: usize = mem::size_of::<PacketHeader>();

