use crate::serialization::{DeserializeError, IncomingPacket};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use mio::net::TcpStream;
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
            return Ok(());
        }

        while !self.outgoing_stream.is_empty() {
            match self.stream.write(&self.outgoing_stream) {
                Ok(count) => unsafe { self.outgoing_stream.move_head_unchecked(count as isize) },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.is_writable = false;
                    return Ok(());
                }
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

    pub fn get_next_incoming(&mut self) -> Result<Option<IncomingPacket>, DeserializeError> {
        let header = if let Some(header) = self.next_incoming_header.take() {
            header
        } else {
            if self.incoming_stream.len() < PACKET_HEADER_SIZE {
                return Ok(None);
            }
            let bytes = &self.incoming_stream.as_slice()[0..PACKET_HEADER_SIZE];
            let header = PacketHeader::from_bytes(bytes)?;
            unsafe { self.incoming_stream.move_head(PACKET_HEADER_SIZE as isize) };
            header
        };

        if self.incoming_stream.len() < header.body_len as usize {
            self.next_incoming_header = Some(header);
            return Ok(None);
        }

        let body_bytes: Vec<u8> =
            self.incoming_stream.as_slice()[..header.body_len as usize].to_vec();
        Ok(Some(IncomingPacket::from_header_and_bytes(
            header, body_bytes,
        )))
    }
}

#[derive(PartialEq)]
pub enum UpdateFromStream {
    Success,
    ConnectionClosed,
}
