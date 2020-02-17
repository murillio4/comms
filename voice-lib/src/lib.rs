use std::io::{Write, ErrorKind, Read};
use std::io;
pub use serde::{Deserialize, Serialize};
use crate::packet::Packet;
use std::num::NonZeroU64;

pub mod packet;
#[cfg(windows)]
pub mod winlib;

#[derive(Debug, Serialize, Deserialize)]
pub struct PacketHeader {
    pub id: NonZeroU64,
    pub reply_to: Option<NonZeroU64>,
    pub kind: u8,
}

#[deprecated]
pub fn serialize<B>(packet: Packet<B>) -> Result<Vec<u8>, SerializeError>
    where B: packet::PacketBody + Serialize
{
    let mut buffer = Vec::<u8>::new();
    loop {
        match buffer.write(&[B::KIND_ID]) {
            Ok(_) => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(SerializeError::WritingHeaderTypeFailed(e))
        }
    }
    match bincode::serialize_into(&mut buffer, &packet) {
        Ok(_) => Ok(buffer),
        Err(e) => Err(e.into()),
    }
}

pub fn serialize_head(header: PacketHeader) -> Result<Vec<u8>, SerializeError> {
    bincode::serialize(&header).map_err(|e| e.into())
}

pub fn serialize_body<B>(packet: B) -> Result<Vec<u8>, SerializeError>
    where B: packet::PacketBody + Serialize
{
    bincode::serialize(&packet).map_err(|e| e.into())
}

pub fn deserialize_header(reader: impl Read) -> Result<PacketHeader, DeserializeError> {
    bincode::deserialize_from(reader).map_err(|_| DeserializeError)
}

pub fn deserialize_remaining<'a, B>(bytes: &'a [u8], header: PacketHeader) -> Result<Packet<B>, DeserializeError>
    where B: packet::PacketBody + Deserialize<'a>
{
    let body = bincode::deserialize(bytes).map_err(|_| DeserializeError)?;
    Ok(Packet::new(header.id, header.reply_to, body))
}

#[deprecated]
pub fn deserialize<'a, B>(packet_bytes: &'a [u8]) -> Result<Packet<B>, DeserializeError>
    where B: packet::PacketBody + Deserialize<'a>
{
    bincode::deserialize(packet_bytes).map_err(|_| DeserializeError)
}

#[derive(Debug)]
pub enum SerializeError {
    WritingHeaderTypeFailed(io::Error),
    SerializingFailed(bincode::Error),
}

impl From<bincode::Error> for SerializeError {
    fn from(e: bincode::Error) -> Self {
        SerializeError::SerializingFailed(e)
    }
}

#[derive(Debug)]
pub struct DeserializeError;