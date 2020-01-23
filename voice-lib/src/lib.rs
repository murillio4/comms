use std::io::{Write, ErrorKind};
use std::io;
use serde::{Deserialize, Serialize};

pub mod packet;
#[cfg(windows)]
pub mod winlib;

pub fn serialize<'a, P>(packet: P) -> Result<Vec<u8>, SerializeError>
    where P: packet::Packet<'a> + Serialize {
    let mut buffer = Vec::<u8>::new();
    loop {
        match buffer.write(&[P::HEADER_TYPE]) {
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

pub fn deserialize<'a, P>(packet_bytes: &'a [u8]) -> bincode::Result<P>
    where P: packet::Packet<'a> + Deserialize<'a> {
    bincode::deserialize(packet_bytes)
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