use crate::packet::{PacketBody, PacketKind, Packet};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use std::{mem, fmt};
use serde::{Serialize, Deserialize};
use std::io::Cursor;
use std::fmt::{Formatter, Debug};
use serde::de::DeserializeOwned;
use std::num::NonZeroU64;

pub fn serialize<B>(id: NonZeroU64, reply_to: Option<NonZeroU64>, packet: B) -> Result<Vec<u8>, SerializeError>
    where B: PacketBody + Serialize
{
    WritePacketBuilder::new(reply_to, &packet)?.into_raw(id)
}

pub struct WritePacketBuilder {
    raw: Vec<u8>,
    pub kind: PacketKind,
    pub reply_to: Option<NonZeroU64>,
}

impl WritePacketBuilder {
    pub fn new<B>(reply_to: Option<NonZeroU64>, body: &B) -> Result<Self, SerializeError>
        where B: PacketBody + Serialize
    {
        let mut raw = vec![0u8; mem::size_of::<PacketHeader>()];
        bincode::serialize_into(&mut raw, body).map_err(|_| SerializeError)?;
        Ok(WritePacketBuilder {
            raw,
            kind: B::KIND_ID,
            reply_to,
        })
    }

    pub fn as_raw(&mut self, id: NonZeroU64) -> Result<&[u8], SerializeError> {
        let body_len = self.raw.len() as u16;
        let mut cursor = Cursor::new(&mut self.raw);
        cursor.set_position(0);
        bincode::serialize_into(cursor, &PacketHeader {
            id,
            reply_to: self.reply_to,
            kind: self.kind,
            body_len: body_len - PACKET_HEADER_SIZE as u16,
        }).map_err(|_| SerializeError)?;
        Ok(&self.raw)
    }

    pub fn into_raw(mut self, id: NonZeroU64) -> Result<Vec<u8>, SerializeError> {
        self.as_raw(id)?;
        Ok(self.raw)
    }
}

pub struct HeaderParsedPacket<'a> {
    pub header: PacketHeader,
    raw_body: &'a [u8],
}

impl<'a> HeaderParsedPacket<'a> {
    pub fn from_header(header: PacketHeader, raw_body: &'a [u8]) -> Self {
        HeaderParsedPacket {
            header,
            raw_body,
        }
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

    pub fn kind(&self) -> PacketKind {
        return self.header.kind
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: PacketBody + Deserialize<'a>
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
    pub fn new(header: PacketHeader, raw_body: Vec<u8>) -> Self {
        Self {
            header,
            raw_body,
        }
    }

    pub fn kind(&self) -> PacketKind {
        return self.header.kind
    }
    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: PacketBody + DeserializeOwned
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