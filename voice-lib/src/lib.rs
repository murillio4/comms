use std::io::{Write, ErrorKind, Read};
use std::io;
pub use serde::{Deserialize, Serialize};
use crate::packet::Packet;
use std::num::NonZeroU64;
use std::cmp::min;

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

#[derive(Debug)]
pub struct Marker(pub usize);
pub struct MarkerRead<'a>(&'a [u8], &'a mut Marker);

impl Read for MarkerRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let marker = (self.1).0;
        let target_slice = &self.0[marker..(marker + buf.len())];
        let len = min(target_slice.len(), buf.len());
        buf[..len].copy_from_slice(target_slice);
        (self.1).0 += buf.len();
        Ok(len)
    }
}

pub fn deserialize_header(raw_packet: &[u8]) -> Result<(usize, PacketHeader), DeserializeError> {
    let mut marker = Marker(0);
    let marker_reader = MarkerRead(raw_packet, &mut marker);
    bincode::deserialize_from(marker_reader)
        .map_err(|_| DeserializeError)
        .map(|header| (marker.0, header))
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