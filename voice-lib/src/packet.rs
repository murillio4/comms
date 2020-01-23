use serde::{Deserialize, Serialize};

pub const MAX_AUDIO_BUFFER_SIZE: usize = 2000;

pub trait Packet<'a> {
    const HEADER_TYPE: u8;
}

#[derive(Serialize, Deserialize)]
pub struct Connect<'a> {
    pub username: &'a str,
}

impl<'a> Packet<'a> for Connect<'a> {
    const HEADER_TYPE: u8 = 1u8;
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AudioFromClient<'a>(
    #[serde(with = "serde_bytes")]
    pub &'a [u8]
);

impl<'a> Packet<'a> for AudioFromClient<'a> {
    const HEADER_TYPE: u8 = 2u8;
}


#[derive(Serialize, Deserialize)]
pub struct Disconnect();

impl<'a> Packet<'a> for Disconnect {
    const HEADER_TYPE: u8 = 3u8;
}

#[derive(Serialize, Deserialize)]
pub struct KeepAlive();

impl<'a> Packet<'a> for KeepAlive {
    const HEADER_TYPE: u8 = 4u8;
}

#[derive(Serialize, Deserialize)]
pub struct AudioToClient<'a> {
    #[serde(with = "serde_bytes")]
    pub bytes: &'a [u8],
    pub author: &'a str,
}

impl<'a> Packet<'a> for AudioToClient<'a> {
    const HEADER_TYPE: u8 = 5u8;
}

#[derive(Serialize, Deserialize)]
pub struct ConnectAccepted {
    pub users: Vec<String>,
}

impl<'a> Packet<'a> for ConnectAccepted {
    const HEADER_TYPE: u8 = 6u8;
}