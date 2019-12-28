use serde::{Serialize, Deserialize};
use std::io::{Write, ErrorKind};
use std::io;
use winapi::um::{errhandlingapi, processthreadsapi, avrt};
use std::os::raw::c_int;
use winapi::um::winbase::{THREAD_PRIORITY_HIGHEST, REALTIME_PRIORITY_CLASS, THREAD_PRIORITY_TIME_CRITICAL};
use winapi::shared::minwindef::DWORD;

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
        let result = processthreadsapi::SetThreadPriority(
            processthreadsapi::GetCurrentThread(),
            THREAD_PRIORITY_TIME_CRITICAL as c_int,
        );
        if result == 0 {
            println!("Failed to set thread priority: {}", errhandlingapi::GetLastError());
        }
        let task_name = "Pro Audio\0".to_string();
        let mut task_index = 0 as DWORD;
        let handle = avrt::AvSetMmThreadCharacteristicsA(task_name.as_ptr() as _, &mut task_index);
        if handle.is_null() {
            eprintln!("Unable to bump thread priority using mmcss");
        }
        dbg!(processthreadsapi::GetThreadPriority(processthreadsapi::GetCurrentThread()));
    }
}