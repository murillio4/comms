use crate::packet::{Packet, PacketBody};
use crate::serialization::{DeserializeError, OwnedHeaderParsedPacket};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use mio::net::TcpStream;
use serde::de::DeserializeOwned;
use slice_deque::SliceDeque;
use std::io;
use std::io::{Read, Write};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Protocol {
    Tcp,
    Udp,
}

pub struct TcpConnection {
    pub stream: TcpStream,
    is_writable: bool,
    incoming_stream: SliceDeque<u8>,
    outgoing_stream: SliceDeque<u8>,
    next_incoming_header: Option<PacketHeader>,
}

impl TcpConnection {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            is_writable: false,
            incoming_stream: SliceDeque::with_capacity(48_000),
            outgoing_stream: SliceDeque::with_capacity(48_000),
            next_incoming_header: None,
        }
    }

    pub fn write_packets_until_blocked(&mut self) -> Result<(), io::Error> {
        if !self.is_writable {
            return Ok(())
        }

        while !self.outgoing_stream.is_empty() {
            match self.stream.write(&self.outgoing_stream) {
                Ok(count) => unsafe { self.outgoing_stream.move_head_unchecked(count as isize) },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.is_writable = false;
                    return Ok(())
                },
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn push_outgoing(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.outgoing_stream.extend_from_slice(bytes);
        self.write_packets_until_blocked()
    }

    pub fn push_outgoing_without_send(&mut self, bytes: &[u8]) {
        self.outgoing_stream.extend_from_slice(bytes);
    }

    pub fn notify_writable(&mut self) -> Result<(), io::Error> {
        self.is_writable = true;
        self.write_packets_until_blocked()?;
        Ok(())
    }

    pub fn read_until_blocked(&mut self) -> Result<UpdateFromStream, io::Error> {
        loop {
            let mut buf = [0; 4096];
            match self.stream.read(&mut buf) {
                Ok(0) => return Ok(UpdateFromStream::ConnectionClosed),
                Ok(amount) => self.incoming_stream.extend_from_slice(&buf[0..amount]),
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

    pub fn get_next_incoming(&mut self) -> Result<Option<TcpHeaderParsedPacket<'_>>, DeserializeError> {
        let header = if let Some(header) = self.next_incoming_header.take() {
            header
        } else {
            if self.incoming_stream.len() < PACKET_HEADER_SIZE {
                return Ok(None);
            }
            let bytes = &self.incoming_stream.as_slice()[0..PACKET_HEADER_SIZE];
            let header = bincode::deserialize(bytes).map_err(|_| DeserializeError)?;
            unsafe { self.incoming_stream.move_head(PACKET_HEADER_SIZE as isize) };
            header
        };

        if self.incoming_stream.len() < header.body_len as usize {
            self.next_incoming_header = Some(header);
            return Ok(None);
        }
        Ok(Some(TcpHeaderParsedPacket::new(header, self)))
    }

    fn take_packet_body<R, F>(&mut self, body_len: usize, parse: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let slice = &self.incoming_stream.as_slice()[0..body_len];
        let return_value: R = parse(slice);
        unsafe { self.incoming_stream.move_head(body_len as isize) };
        return_value
    }

    fn ignore_packet_body(&mut self, body_len: usize) {
        unsafe { self.incoming_stream.move_head(body_len as isize) };
    }
}

pub struct TcpBodyInStream<'a> {
    len: usize,
    stream: &'a mut TcpConnection,
    has_consumed_stream: bool,
}

impl<'a> TcpBodyInStream<'a> {
    pub fn new(len: usize, stream: &'a mut TcpConnection) -> Self {
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
    // TODO merge with Owned...Packet?
    pub fn new(header: PacketHeader, stream: &'a mut TcpConnection) -> Self {
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
