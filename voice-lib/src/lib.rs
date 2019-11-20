use serde::{Serialize, Deserialize};
use std::io::Write;

pub mod packet;

pub fn serialize<P>(packet: P) -> bincode::Result<Vec<u8>>
    where P: packet::Packet + Serialize {
    let mut buffer = Vec::<u8>::new();
    buffer.write(&[P::HEADER_TYPE]).unwrap();
    match bincode::serialize_into(&mut buffer, &packet) {
        Ok(_) => Ok(buffer),
        Err(e) => Err(e),
    }
}

pub fn deserialize<'a, P>(packet_bytes: &'a [u8]) -> bincode::Result<P>
    where P: packet::Packet + Deserialize<'a> {
    bincode::deserialize(packet_bytes)
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
