use crate::packet::{Packet, PacketBody, PacketId, PacketKind};
use crate::PacketHeader;
use serde::de::DeserializeOwned;
use std::fmt::{Debug, Formatter};
use std::io::Cursor;
use std::{fmt, mem};

#[derive(Clone)]
pub struct OutgoingPacketFrame {
    pub kind: PacketKind,
    pub reply_to: Option<PacketId>,
    pub bytes_with_empty_header: Vec<u8>,
}

impl OutgoingPacketFrame {
    pub fn from_packet<B>(reply_to: Option<PacketId>, body: &B) -> Result<Self, SerializeError>
    where
        B: PacketBody,
    {
        let mut bytes_with_empty_header = vec![0u8; mem::size_of::<PacketHeader>()];
        bincode::serialize_into(&mut bytes_with_empty_header, &body)?;
        Ok(Self {
            kind: B::KIND_ID,
            reply_to,
            bytes_with_empty_header,
        })
    }

    pub fn into_raw(self, id: PacketId) -> Result<Vec<u8>, SerializeError> {
        let body_len = self.bytes_with_empty_header.len() - mem::size_of::<PacketHeader>();
        let mut bytes = self.bytes_with_empty_header;
        let mut cursor = Cursor::new(&mut bytes);
        cursor.set_position(0);
        bincode::serialize_into(
            cursor,
            &PacketHeader {
                id,
                reply_to: self.reply_to,
                kind: self.kind,
                body_len: body_len as u16,
            },
        )?;
        Ok(bytes)
    }
}

pub struct IncomingPacketFrame {
    pub header: PacketHeader,
    body_bytes: Vec<u8>,
}

impl Debug for IncomingPacketFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{:?}", self.header))
    }
}

pub type SerializeError = bincode::Error;
impl IncomingPacketFrame {
    pub fn from_header_and_bytes(header: PacketHeader, body_bytes: Vec<u8>) -> Self {
        Self { header, body_bytes }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SerializeError> {
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

    pub fn finish_parse<B>(self) -> Result<Packet<B>, SerializeError>
    where
        B: PacketBody + DeserializeOwned,
    {
        let body = bincode::deserialize(&self.body_bytes)?;
        Ok(Packet::new(self.header.id, self.header.reply_to, body))
    }
}
