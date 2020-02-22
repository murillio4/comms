use std::io::{Write, ErrorKind, Cursor};
use std::mem;
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
            Err(_) => return Err(SerializeError)
        }
    }
    match bincode::serialize_into(&mut buffer, &packet) {
        Ok(_) => Ok(buffer),
        Err(e) => Err(e.into()),
    }
}

pub fn serialize_new<B>(header: PacketHeader, packet: B) -> Result<Vec<u8>, SerializeError>
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

pub struct WritePacketBuilder {
    pub raw: Vec<u8>,
    has_header: bool,
}

impl WritePacketBuilder {
    pub fn new<B>(body: B) -> Result<Self, SerializeError>
        where B: packet::PacketBody + Serialize
    {
        let raw = vec![0u8; mem::size_of::<PacketHeader>()];
        let mut instance = WritePacketBuilder {
            raw,
            has_header: false,
        };
        bincode::serialize_into(&mut instance.raw, &body).map_err(|_| SerializeError)?;
        Ok(instance)
    }

    pub fn header(&mut self, header: PacketHeader) -> Result<(), SerializeError> {
        assert!(!self.has_header);
        self.has_header = true;

        let mut cursor = Cursor::new(&mut self.raw);
        cursor.set_position(0);
        bincode::serialize_into(cursor, &header)
            .map_err(|_| SerializeError)
    }
}

pub struct HeaderParsedPacket<'a> {
    pub kind: u8,
    header: PacketHeader,
    raw_body: &'a [u8],
}

impl<'a> HeaderParsedPacket<'a> {
    pub fn from_raw(raw: &'a [u8]) -> Result<Self, DeserializeError> {
        let raw_header_size = mem::size_of::<PacketHeader>();
        let header: PacketHeader = bincode::deserialize(&raw[0..raw_header_size])
            .map_err(|_| DeserializeError)?;
        Ok(Self {
            raw_body: &raw[raw_header_size..],
            kind: header.kind,
            header,
        })
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: packet::PacketBody + Deserialize<'a>
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