use crate::serialization::{IncomingPacketFrame, SerializeError};
use crate::{PacketHeader, PACKET_HEADER_SIZE};
use mio::net::{TcpStream, UdpSocket};
use slice_deque::SliceDeque;
use std::collections::VecDeque;
use std::io;
use std::io::{Read, Write};
use std::net::SocketAddr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Protocol {
    Tcp,
    Udp,
}

pub struct TcpConnection {
    stream: TcpStream,
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

    pub fn write_until_blocked(&mut self) -> Result<(), io::Error> {
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
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn push_outgoing(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.outgoing_stream.extend_from_slice(bytes);
        self.write_until_blocked()
    }

    pub fn push_outgoing_without_send(&mut self, bytes: &[u8]) {
        self.outgoing_stream.extend_from_slice(bytes);
    }

    pub fn notify_writable(&mut self) -> Result<(), io::Error> {
        self.is_writable = true;
        self.write_until_blocked()
    }

    fn read_from_network(&mut self) -> Result<(), CommsNetworkError> {
        loop {
            let mut buf = [0; 4096];
            match self.stream.read(&mut buf) {
                Ok(0) => return Err(CommsNetworkError::Closed),
                Ok(amount) => self.incoming_stream.extend_from_slice(&buf[0..amount]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {
                    return Err(CommsNetworkError::Closed)
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn read_new_packets(&mut self) -> TcpReadIter {
        let read_result = self.read_from_network();
        TcpReadIter {
            read_error: read_result.err(),
            incoming_stream: &mut self.incoming_stream,
            next_incoming_header: &mut self.next_incoming_header,
        }
    }

    pub fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        self.stream.peer_addr()
    }
}

pub trait UdpPacket {
    type IncomingPacket;

    fn send(&self, socket: &mut UdpSocket) -> io::Result<usize>;
    fn from_recv(socket: &mut UdpSocket) -> io::Result<Option<Self>>
    where
        Self: Sized;
    fn into_incoming_packet(self) -> (Result<Self::IncomingPacket, SerializeError>, SocketAddr);
}

pub struct ServerUdpPacket {
    client_addr: SocketAddr,
    buffer: Vec<u8>,
}

impl ServerUdpPacket {
    pub fn new(client_addr: SocketAddr, buffer: Vec<u8>) -> Self {
        Self {
            client_addr,
            buffer,
        }
    }
}

impl UdpPacket for ServerUdpPacket {
    type IncomingPacket = IncomingPacketFrame;

    fn send(&self, socket: &mut UdpSocket) -> io::Result<usize> {
        socket.send_to(&self.buffer, self.client_addr)
    }

    fn from_recv(socket: &mut UdpSocket) -> io::Result<Option<Self>> {
        let mut buffer = [0u8; 65536];
        let (size, address) = socket.recv_from(&mut buffer)?;
        if size == 0 {
            return Ok(None);
        }

        Ok(Some(Self {
            client_addr: address,
            buffer: buffer[..size].to_vec(),
        }))
    }

    fn into_incoming_packet(self) -> (Result<Self::IncomingPacket, SerializeError>, SocketAddr) {
        let incoming_packet_result = IncomingPacketFrame::from_bytes(&self.buffer);
        (incoming_packet_result, self.client_addr)
    }
}

pub struct ClientUdpPacket {
    client_addr: SocketAddr,
    buffer: Vec<u8>,
}

impl ClientUdpPacket {
    pub fn new(buffer: Vec<u8>, client_addr: SocketAddr) -> Self {
        Self {
            buffer,
            client_addr,
        }
    }
}

impl UdpPacket for ClientUdpPacket {
    type IncomingPacket = IncomingPacketFrame;

    fn send(&self, socket: &mut UdpSocket) -> io::Result<usize> {
        socket.send_to(&self.buffer, self.client_addr)
    }

    fn from_recv(socket: &mut UdpSocket) -> io::Result<Option<Self>> {
        let mut buffer = [0u8; 65536];
        let (size, address) = socket.recv_from(&mut buffer)?;

        if size == 0 {
            return Ok(None);
        }

        Ok(Some(Self {
            client_addr: address,
            buffer: buffer[..size].to_vec(),
        }))
    }

    fn into_incoming_packet(self) -> (Result<Self::IncomingPacket, SerializeError>, SocketAddr) {
        (
            IncomingPacketFrame::from_bytes(&self.buffer),
            self.client_addr,
        )
    }
}

pub struct UdpConnection<U> {
    socket: UdpSocket,
    is_writable: bool,
    incoming_packets: VecDeque<U>,
    outgoing_packets: VecDeque<U>,
}

impl<U: UdpPacket> UdpConnection<U> {
    pub fn new(socket: UdpSocket) -> Self {
        Self {
            socket,
            is_writable: false,
            incoming_packets: VecDeque::with_capacity(256),
            outgoing_packets: VecDeque::with_capacity(256),
        }
    }

    pub fn write_until_blocked(&mut self) -> Result<(), io::Error> {
        if !self.is_writable {
            return Ok(());
        }

        while let Some(packet) = self.outgoing_packets.pop_front() {
            match packet.send(&mut self.socket) {
                Ok(_) => (),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.is_writable = false;
                    self.outgoing_packets.push_front(packet);
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn push_outgoing(&mut self, packet: U) -> Result<(), io::Error> {
        self.outgoing_packets.push_back(packet);
        self.write_until_blocked()
    }

    pub fn push_outgoing_without_send(&mut self, packet: U) {
        self.outgoing_packets.push_back(packet);
    }

    pub fn notify_writable(&mut self) -> Result<(), io::Error> {
        self.is_writable = true;
        self.write_until_blocked()
    }

    fn read_from_network(&mut self) -> Result<(), CommsNetworkError> {
        loop {
            match U::from_recv(&mut self.socket) {
                Ok(Some(packet)) => self.incoming_packets.push_back(packet),
                Ok(None) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn read_new_packets(&mut self) -> UdpReadIter<U> {
        let read_result = self.read_from_network();
        UdpReadIter {
            read_error: read_result.err(),
            incoming_packets: &mut self.incoming_packets,
        }
    }
}

pub struct TcpReadIter<'a> {
    read_error: Option<CommsNetworkError>,
    incoming_stream: &'a mut SliceDeque<u8>,
    next_incoming_header: &'a mut Option<PacketHeader>,
}

impl TcpReadIter<'_> {
    pub fn try_process(
        &mut self,
        mut f: impl FnMut(IncomingPacketFrame) -> Result<(), CommsNetworkError>,
    ) -> Result<(), CommsNetworkError> {
        if let Some(e) = self.read_error.take() {
            return Err(e);
        }
        for result in self {
            if let Err(e) = result.map_err(|e| e.into()).and_then(&mut f) {
                return Err(e);
            }
        }
        Ok(())
    }
}

impl<'a> Iterator for TcpReadIter<'a> {
    type Item = Result<IncomingPacketFrame, SerializeError>;

    fn next(&mut self) -> Option<Self::Item> {
        let header = if let Some(header) = self.next_incoming_header.take() {
            header
        } else {
            if self.incoming_stream.len() < PACKET_HEADER_SIZE {
                return None;
            }
            let bytes = &self.incoming_stream.as_slice()[0..PACKET_HEADER_SIZE];
            let header = match PacketHeader::from_bytes(bytes) {
                Ok(header) => header,
                Err(e) => return Some(Err(e)),
            };
            unsafe { self.incoming_stream.move_head(PACKET_HEADER_SIZE as isize) };
            header
        };

        if self.incoming_stream.len() < header.body_len as usize {
            *self.next_incoming_header = Some(header);
            return None;
        }

        let body_bytes: Vec<u8> =
            self.incoming_stream.as_slice()[..header.body_len as usize].to_vec();
        unsafe { self.incoming_stream.move_head(header.body_len as isize) };
        Some(Ok(IncomingPacketFrame::from_header_and_bytes(
            header, body_bytes,
        )))
    }
}

pub struct UdpReadIter<'a, U> {
    read_error: Option<CommsNetworkError>,
    incoming_packets: &'a mut VecDeque<U>,
}

impl<U: UdpPacket> UdpReadIter<'_, U> {
    // TODO join with TCP impl into a trait
    pub fn try_process(
        &mut self,
        mut f: impl FnMut(U::IncomingPacket, SocketAddr) -> Result<(), CommsNetworkError>,
    ) -> Result<(), CommsNetworkError> {
        if let Some(e) = self.read_error.take() {
            return Err(e);
        }
        for (result, addr) in self {
            if let Err(e) = result.map_err(|e| e.into()).and_then(|p| f(p, addr)) {
                return Err(e);
            }
        }
        Ok(())
    }
}

impl<'a, U: UdpPacket> Iterator for UdpReadIter<'a, U> {
    type Item = (Result<U::IncomingPacket, SerializeError>, SocketAddr);

    fn next(&mut self) -> Option<Self::Item> {
        self.incoming_packets
            .pop_front()
            .map(|udp_packet| udp_packet.into_incoming_packet())
    }
}

#[derive(PartialEq)]
pub enum UpdateFromStream {
    Success,
    ConnectionClosed,
}

#[derive(thiserror::Error, Debug)]
pub enum CommsNetworkError {
    #[error("Connection is now closed")]
    Closed,
    #[error("IO error")]
    Io(#[from] io::Error),
    #[error("(De)serializeError")]
    Deserialize(#[from] bincode::Error),
}
