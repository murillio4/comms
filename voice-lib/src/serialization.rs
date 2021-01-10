use crate::packet::{Packet, PacketBody, PacketId, PacketKind};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::{Debug, Formatter};
use std::io::Cursor;
use std::{fmt, mem};

pub fn serialize<B>(
    id: PacketId,
    reply_to: Option<PacketId>,
    packet: B,
) -> Result<Vec<u8>, SerializeError>
where
    B: PacketBody + Serialize,
{
    WritePacketBuilder::new(reply_to, &packet)?.into_raw(id)
}

pub struct WritePacketBuilder {
    raw: Vec<u8>,
    pub kind: PacketKind,
    pub reply_to: Option<PacketId>,
}

impl WritePacketBuilder {
    pub fn new<B>(reply_to: Option<PacketId>, body: &B) -> Result<Self, SerializeError>
    where
        B: PacketBody + Serialize,
    {
        let mut raw = vec![0u8; mem::size_of::<PacketHeader>()];
        bincode::serialize_into(&mut raw, body).map_err(|_| SerializeError)?;
        Ok(WritePacketBuilder {
            raw,
            kind: B::KIND_ID,
            reply_to,
        })
    }

    pub fn as_raw(&mut self, id: PacketId) -> Result<&[u8], SerializeError> {
        let body_len = self.raw.len() as u16;
        let mut cursor = Cursor::new(&mut self.raw);
        cursor.set_position(0);
        bincode::serialize_into(
            cursor,
            &PacketHeader {
                id,
                reply_to: self.reply_to,
                kind: self.kind,
                body_len: body_len - PACKET_HEADER_SIZE as u16,
            },
        )
        .map_err(|_| SerializeError)?;
        Ok(&self.raw)
    }

    pub fn into_raw(mut self, id: PacketId) -> Result<Vec<u8>, SerializeError> {
        self.as_raw(id)?;
        Ok(self.raw)
    }
}

pub struct IncomingPacket {
    pub header: PacketHeader,
    body_bytes: Vec<u8>,
}

impl Debug for IncomingPacket {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{:?}", self.header))
    }
}

impl IncomingPacket {
    pub fn from_header_and_bytes(header: PacketHeader, body_bytes: Vec<u8>) -> Self {
        Self { header, body_bytes }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeserializeError> {
        let raw_header_size = mem::size_of::<PacketHeader>();
        let header = PacketHeader::from_bytes(&bytes[..raw_header_size])?;
        Ok(Self {
            header,
            body_bytes: bytes[raw_header_size..].to_vec(),
        })
    }

    pub fn kind(&self) -> PacketKind {
        self.header.kind
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
    where
        B: PacketBody + DeserializeOwned,
    {
        let body = bincode::deserialize(&self.body_bytes).map_err(|_| DeserializeError)?;
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
