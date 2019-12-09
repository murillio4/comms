use serde::{Serialize, Deserialize};
use std::io::{Write, ErrorKind};
use std::io;
use winapi::um::processthreadsapi;
use std::os::raw::c_int;

pub mod packet;

pub fn serialize<P>(packet: P) -> Result<Vec<u8>, SerializeError>
    where P: packet::Packet + Serialize {
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
    where P: packet::Packet + Deserialize<'a> {
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

#[cfg(windows)]
pub fn bump_priority() {
    unsafe {
        processthreadsapi::SetThreadPriority(
            processthreadsapi::GetCurrentThread(),
            15 as c_int,
        );
        dbg!(processthreadsapi::GetPriorityClass(processthreadsapi::GetCurrentProcess()));
        dbg!(processthreadsapi::GetThreadPriority(processthreadsapi::GetCurrentThread()));
    }
}