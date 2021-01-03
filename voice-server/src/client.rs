use crate::network::OutgoingPacket;
use crate::task::PacketFinish;
use mio::net::TcpStream;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use voice_lib::network::CommsTcpStream;
use voice_lib::packet::{ClientId, PacketId};

pub struct NextPacketId(PacketId);

impl NextPacketId {
    pub fn new() -> Self {
        Self(PacketId::new(1).unwrap())
    }

    pub fn next(&mut self) -> PacketId {
        let packet_id = self.0;
        self.0 = PacketId::new(self.0.get() + 1).unwrap();
        packet_id
    }
}

pub struct PartiallySentPacket {
    pub bytes: Vec<u8>,
    pub finish: PacketFinish,
}

pub struct Connection {
    next_packet_id: NextPacketId,
    pub stream: CommsTcpStream,
    pub tcp_writable: bool,
    pub outgoing_queue: VecDeque<OutgoingPacket>,
    pub partially_sent_packet: Option<PartiallySentPacket>,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Self {
        Connection {
            stream: CommsTcpStream::new(stream),
            tcp_writable: false,
            next_packet_id: NextPacketId::new(),
            outgoing_queue: VecDeque::new(),
            partially_sent_packet: None,
        }
    }

    pub fn next_packet_id(&mut self) -> PacketId {
        self.next_packet_id.next()
    }

    pub fn clear_queue_for_disconnect(&mut self) {
        for packet in self.outgoing_queue.drain(..) {
            packet.finish.done();
        }
    }
}

pub struct ConnectingClient {
    pub connection: Connection,
    pub udp_addr: Option<SocketAddr>,
}

pub struct Client {
    pub connection: Connection,
    pub udp_addr: SocketAddr,
    pub username: String,
}

pub struct Clients {
    pub established: HashMap<ClientId, Client>,
    pub connecting: HashMap<ClientId, ConnectingClient>,
}

pub enum AnyClient {
    Established(Client),
    Connecting(ConnectingClient),
}

pub enum AnyClientRef<'a> {
    Established(&'a mut Client),
    Connecting(&'a mut ConnectingClient),
}

impl Clients {
    pub fn new() -> Self {
        Clients {
            established: HashMap::new(),
            connecting: HashMap::new(),
        }
    }

    pub fn writable_connections(&mut self) -> Vec<&mut Connection> {
        let connecting = self.connecting.iter_mut().map(|(_, c)| &mut c.connection);
        self.established
            .iter_mut()
            .map(|(_, c)| &mut c.connection)
            .chain(connecting)
            .filter(|c| c.tcp_writable)
            .collect()
    }

    pub fn id_by_socket_addr(&self, addr: &SocketAddr) -> Option<ClientId> {
        self.established
            .iter()
            .find(|(_, c)| &c.udp_addr == addr)
            .map(|(id, _)| *id)
            .or_else(|| {
                self.connecting
                    .iter()
                    .find(|(_, c)| c.udp_addr == Some(*addr))
                    .map(|(id, _)| *id)
            })
    }

    pub fn connection_by_id(&mut self, client_id: &ClientId) -> Option<&mut Connection> {
        let established_connection = self
            .established
            .get_mut(client_id)
            .map(|c| &mut c.connection);
        let connecting = self
            .connecting
            .get_mut(client_id)
            .map(|c| &mut c.connection);
        established_connection.or(connecting)
    }

    pub fn client_by_id(&mut self, client_id: &ClientId) -> Option<AnyClientRef> {
        match self.established.get_mut(client_id) {
            Some(c) => Some(AnyClientRef::Established(c)),
            None => self
                .connecting
                .get_mut(client_id)
                .map(AnyClientRef::Connecting),
        }
    }

    pub fn remove_client(&mut self, client_id: &ClientId) -> Option<AnyClient> {
        self.established
            .remove(client_id)
            .map(AnyClient::Established)
            .or_else(|| self.connecting.remove(client_id).map(AnyClient::Connecting))
    }

    pub fn insert_client(&mut self, client_id: ClientId, client: AnyClient) {
        match client {
            AnyClient::Established(client) => {
                self.established.insert(client_id, client);
            }
            AnyClient::Connecting(client) => {
                self.connecting.insert(client_id, client);
            }
        };
    }
}
