use mio::net::TcpStream;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use voice_lib::network::TcpConnection;
use voice_lib::packet::{ClientId, PacketId};
use voice_lib::serialization::OutgoingPacketFrame;

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

pub struct Connection {
    next_packet_id: NextPacketId,
    pub tcp: TcpConnection,
    pub tcp_writable: bool,
    pub outgoing_queue: VecDeque<(ClientId, OutgoingPacketFrame)>,
    pub partially_sent_packet: Option<Vec<u8>>,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Self {
        Connection {
            tcp: TcpConnection::new(stream),
            tcp_writable: false,
            next_packet_id: NextPacketId::new(),
            outgoing_queue: VecDeque::new(),
            partially_sent_packet: None,
        }
    }

    pub fn next_packet_id(&mut self) -> PacketId {
        self.next_packet_id.next()
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

    pub fn remove_client(&mut self, client_id: &ClientId) {
        self.established
            .remove(client_id)
            .map(|_| ())
            .or_else(|| self.connecting.remove(client_id).map(|_| ()))
            .expect("Tried to close connection with nonexistent client");
    }
}
