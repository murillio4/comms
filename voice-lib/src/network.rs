use std::io;
use std::io::Read;
use mio::net::TcpStream;
use slice_deque::SliceDeque;
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use crate::serialization::{DeserializeError, OwnedHeaderParsedPacket};
use crate::packet::{Packet, PacketBody};
use serde::de::DeserializeOwned;

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
                Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(UpdateFromStream::Success),
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }

    pub fn next_packet(&mut self) -> Result<Option<IncomingStreamPacket<'_>>, DeserializeError> {
        let header = if let Some(header) = self.next_header.take() {
            header
        } else {
            if self.slice_deque.len() < PACKET_HEADER_SIZE {
                return Ok(None);
            }
            let bytes = &self.slice_deque.as_slice()[0..PACKET_HEADER_SIZE];
            let header = bincode::deserialize(bytes)
                .map_err(|_| DeserializeError)?;
            unsafe { self.slice_deque.move_head(PACKET_HEADER_SIZE as isize) };
            header
        };

        if self.slice_deque.len() < header.body_len as usize {
            self.next_header = Some(header);
            return Ok(None);
        }
        Ok(Some(IncomingStreamPacket::new(header, self)))
    }
}

pub struct IncomingStreamPacket<'a> {
    pub header: PacketHeader,
    stream: &'a mut CommsTcpStream,
}

impl<'a> IncomingStreamPacket<'a> {
    pub fn new(header: PacketHeader, stream: &'a mut CommsTcpStream) -> Self {
        IncomingStreamPacket {
            header,
            stream,
        }
    }

    pub fn finish_parse<B>(self) -> Result<Packet<B>, DeserializeError>
        where B: PacketBody + DeserializeOwned
    {
        let slice_deque = &mut self.stream.slice_deque;
        let body = {
            let slice = &slice_deque.as_slice()[0..self.header.body_len as usize];
            bincode::deserialize(slice).map_err(|_| DeserializeError)?
        };
        unsafe { slice_deque.move_head(self.header.body_len as isize) };
        Ok(Packet::new(self.header.id, self.header.reply_to, body))
    }

    pub fn into_owned(self) -> OwnedHeaderParsedPacket {
        let slice_deque = &mut self.stream.slice_deque;
        let body_slice = &slice_deque.as_slice()[0..self.header.body_len as usize];
        let raw_body = body_slice.to_vec();
        unsafe { slice_deque.move_head(self.header.body_len as isize) };
        OwnedHeaderParsedPacket::new(self.header, raw_body)
    }
}

#[derive(PartialEq)]
pub enum UpdateFromStream {
    Success,
    ConnectionClosed,
}

