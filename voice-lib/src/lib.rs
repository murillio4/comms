use std::io::{Write, ErrorKind, Cursor};
use std::{mem, fmt};
pub use serde::{Deserialize, Serialize};
use crate::packet::{Packet, PacketKind};
use std::num::NonZeroU64;
use std::fmt::Debug;
use serde::export::Formatter;
use serde::de::DeserializeOwned;

pub mod packet;
#[cfg(windows)]
pub mod winlib;

#[derive(Debug, Serialize, Deserialize)]
pub struct PacketHeader {
    pub id: NonZeroU64,
    pub reply_to: Option<NonZeroU64>,
    pub kind: PacketKind,
}

pub fn serialize<B>(header: PacketHeader, packet: B) -> Result<Vec<u8>, SerializeError>
    where B: packet::PacketBody + Serialize
{
    bincode::serialize(&header)
        .and_then(|mut raw_packet| {
            // pad header to minimum size
            let header_size = mem::size_of::<PacketHeader>();
            for _ in raw_packet.len()..header_size {
                raw_packet.push(0);
            }
            bincode::serialize_into(&mut raw_packet, &packet)
                .map(|_| raw_packet)
        })
        .map_err(|e| e.into())

}

pub fn serialize_head(header: PacketHeader) -> Result<Vec<u8>, SerializeError> {
    bincode::serialize(&header)
        .map(|mut vec| {
            // header must be constant size, so pad to that size
            for _ in vec.len()..mem::size_of::<PacketHeader>() {
                vec.push(0);
            }
            vec
        })
        .map_err(|e| e.into())
}

pub fn serialize_body<B>(packet: B) -> Result<Vec<u8>, SerializeError>
    where B: packet::PacketBody + Serialize
{
    bincode::serialize(&packet).map_err(|e| e.into())
}

pub struct WritePacketBuilder(Vec<u8>);

impl WritePacketBuilder {
    pub fn new<B>(body: B) -> Result<Self, SerializeError>
        where B: packet::PacketBody + Serialize
    {
        let mut raw = vec![0u8; mem::size_of::<PacketHeader>()];
        bincode::serialize_into(&mut raw, &body).map_err(|_| SerializeError)?;
        Ok(WritePacketBuilder(raw))
    }

    pub fn as_raw(&mut self, header: PacketHeader) -> Result<&[u8], SerializeError> {
        let mut cursor = Cursor::new(&mut self.0);
        cursor.set_position(0);
        bincode::serialize_into(cursor, &header).map_err(|_| SerializeError)?;
        Ok(&self.0)
    }

    pub fn into_raw(mut self, header: PacketHeader) -> Result<Vec<u8>, SerializeError> {
        self.as_raw(header)?;
        Ok(self.0)
    }
}

pub struct HeaderParsedPacket<'a> {
    pub header: PacketHeader,
    raw_body: &'a [u8],
}

impl<'a> HeaderParsedPacket<'a> {
    pub fn kind(&self) -> PacketKind {
        return self.header.kind
    }

    pub fn from_raw(raw: &'a [u8]) -> Result<Self, DeserializeError> {
        let raw_header_size = mem::size_of::<PacketHeader>();
        let header: PacketHeader = bincode::deserialize(&raw[0..raw_header_size])
            .map_err(|_| DeserializeError)?;
        Ok(Self {
            raw_body: &raw[raw_header_size..],
            header,
        })
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: packet::PacketBody + Deserialize<'a>
    {
        let body = bincode::deserialize(&self.raw_body).map_err(|_| DeserializeError)?;
        Ok(Packet::new(self.header.id, self.header.reply_to, body))
    }

    pub fn into_owned(self) -> OwnedHeaderParsedPacket {
        return OwnedHeaderParsedPacket {
            header: self.header,
            raw_body: self.raw_body.to_vec(),
        }
    }
}

pub struct OwnedHeaderParsedPacket {
    pub header: PacketHeader,
    raw_body: Vec<u8>,
}

impl Debug for OwnedHeaderParsedPacket {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{:?}", self.header))
    }
}

impl OwnedHeaderParsedPacket {
    pub fn kind(&self) -> PacketKind {
        return self.header.kind
    }
    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: packet::PacketBody + DeserializeOwned
    {
        let body = bincode::deserialize(&self.raw_body).map_err(|_| DeserializeError)?;
        Ok(Packet::new(self.header.id, self.header.reply_to, body))
    }
}

#[derive(Debug)]
pub struct SerializeError;

impl From<bincode::Error> for SerializeError {
    fn from(_: bincode::Error) -> Self {
        SerializeError
    }
}

#[derive(Debug)]
pub struct DeserializeError;