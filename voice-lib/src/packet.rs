use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_BUFFER_SIZE: usize = 2000;

pub trait Packet {
    const HEADER_TYPE: u8;
}

#[derive(Serialize, Deserialize)]
pub struct Connect();

impl Packet for Connect {
    const HEADER_TYPE: u8 = 1u8;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AudioFromClient<'a>(
    #[serde(with = "serde_bytes")]
    pub &'a [u8]
);

impl<'a> Packet for AudioFromClient<'a> {
    const HEADER_TYPE: u8 = 2u8;
}


#[derive(Serialize, Deserialize)]
pub struct Disconnect();

impl Packet for Disconnect {
    const HEADER_TYPE: u8 = 3u8;
}

#[derive(Serialize, Deserialize)]
pub struct KeepAlive();

impl Packet for KeepAlive {
    const HEADER_TYPE: u8 = 4u8;
}

#[derive(Serialize, Deserialize)]
pub struct AudioToClient<'a> {
    #[serde(with = "serde_bytes")]
    pub bytes: &'a [u8],
    pub author: String,
}

impl<'a> Packet for AudioToClient<'a> {
    const HEADER_TYPE: u8 = 5u8;
}