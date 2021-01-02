use crate::packet::{Packet, PacketBody};
use crate::serialization::{DeserializeError, OwnedHeaderParsedPacket};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use mio::net::TcpStream;
use serde::de::DeserializeOwned;
use slice_deque::SliceDeque;
use std::io;
use std::io::Read;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Protocol {
    Tcp,
    Udp,
}

pub struct CommsTcpStream {
    pub internal: TcpStream,
    next_header: Option<PacketHeader>,
    slice_deque: SliceDeque<u8>,
}

impl CommsTcpStream {
    pub fn new(tcp_stream: TcpStream) -> Self {
        Self {
            internal: tcp_stream,
            slice_deque: SliceDeque::with_capacity(48_000),
            next_header: None,
        }
    }

    pub fn update_from_stream(&mut self) -> Result<UpdateFromStream, io::Error> {
        loop {
            let mut buf = [0; 256];
            match self.internal.read(&mut buf) {
                Ok(0) => return Ok(UpdateFromStream::ConnectionClosed),
                Ok(amount) => self.slice_deque.extend_from_slice(&buf[0..amount]),
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(UpdateFromStream::Success)
                }
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(ref err) if err.kind() == io::ErrorKind::ConnectionReset => {
                    return Ok(UpdateFromStream::ConnectionClosed)
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub fn next_packet(&mut self) -> Result<Option<TcpHeaderParsedPacket<'_>>, DeserializeError> {
        let header = if let Some(header) = self.next_header.take() {
            header
        } else {
            if self.slice_deque.len() < PACKET_HEADER_SIZE {
                return Ok(None);
            }
            let bytes = &self.slice_deque.as_slice()[0..PACKET_HEADER_SIZE];
            let header = bincode::deserialize(bytes).map_err(|_| DeserializeError)?;
            unsafe { self.slice_deque.move_head(PACKET_HEADER_SIZE as isize) };
            header
        };

        if self.slice_deque.len() < header.body_len as usize {
            self.next_header = Some(header);
            return Ok(None);
        }
        Ok(Some(TcpHeaderParsedPacket::new(header, self)))
    }

    fn take_packet_body<R, F>(&mut self, body_len: usize, parse: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let slice = &self.slice_deque.as_slice()[0..body_len];
        let return_value: R = parse(slice);
        unsafe { self.slice_deque.move_head(body_len as isize) };
        return_value
    }

    fn ignore_packet_body(&mut self, body_len: usize) {
        unsafe { self.slice_deque.move_head(body_len as isize) };
    }
}

pub struct TcpBodyInStream<'a> {
    len: usize,
    stream: &'a mut CommsTcpStream,
    has_consumed_stream: bool,
}

impl<'a> TcpBodyInStream<'a> {
    pub fn new(len: usize, stream: &'a mut CommsTcpStream) -> Self {
        Self {
            len,
            stream,
            has_consumed_stream: false,
        }
    }

    fn into_vec(mut self) -> Vec<u8> {
        self.has_consumed_stream = true;
        self.stream
            .take_packet_body(self.len, |bytes| bytes.to_vec())
    }

    fn into_body<B>(mut self) -> Result<B, Box<bincode::ErrorKind>>
    where
        B: DeserializeOwned,
    {
        self.has_consumed_stream = true;
        self.stream
            .take_packet_body(self.len, |bytes| bincode::deserialize(bytes))
    }
}

impl<'a> Drop for TcpBodyInStream<'a> {
    fn drop(&mut self) {
        if !self.has_consumed_stream {
            self.stream.ignore_packet_body(self.len);
        }
    }
}

pub struct TcpHeaderParsedPacket<'a> {
    pub header: PacketHeader,
    stream_body: TcpBodyInStream<'a>,
}

impl<'a> TcpHeaderParsedPacket<'a> {
    pub fn new(header: PacketHeader, stream: &'a mut CommsTcpStream) -> Self {
        TcpHeaderParsedPacket {
            stream_body: TcpBodyInStream::new(header.body_len as usize, stream),
            header,
        }
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
    where
        B: PacketBody + DeserializeOwned,
    {
        let body: B = self.stream_body.into_body().map_err(|_| DeserializeError)?;
        Ok(Packet::new(self.header.id, self.header.reply_to, body))
    }

    pub fn into_owned(self) -> OwnedHeaderParsedPacket {
        let raw_body: Vec<u8> = self.stream_body.into_vec();
        OwnedHeaderParsedPacket::new(self.header, raw_body)
    }
}

#[derive(PartialEq)]
pub enum UpdateFromStream {
    Success,
    ConnectionClosed,
}
