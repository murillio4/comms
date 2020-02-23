use voice_lib::packet::{PacketId, ClientId};
use voice_lib::network::CommsTcpStream;
use std::net::SocketAddr;
use std::collections::HashMap;
use mio::tcp::TcpStream;

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
    pub stream: CommsTcpStream,
    pub tcp_writable: bool,
}

impl Connection {
    pub fn new(stream: TcpStream) -> Self {
        Connection {
            stream: CommsTcpStream::new(stream),
            tcp_writable: false,
            next_packet_id: NextPacketId::new(),
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

    pub fn writable_connections(&mut self) -> Vec<(ClientId, &mut Connection)> {
        let connecting = self.connecting.iter_mut()
            .map(|(id, c)| (id, &mut c.connection));
        self.established.iter_mut()
            .map(|(id, c)| (id, &mut c.connection))
            .chain(connecting)
            .filter_map(|(id, c)| {
                match c.tcp_writable {
                    true => Some((id.clone(), c)),
                    false => None,
                }
            })
            .collect()
    }

    pub fn id_by_socket_addr(&self, addr: &SocketAddr) -> Option<ClientId> {
        self.established.iter()
            .find(|(_, c)| &c.udp_addr == addr)
            .map(|(id, _)| id.clone())
            .or(self.connecting.iter()
                .find(|(_, c)| c.udp_addr == Some(*addr))
                .map(|(id, _)| id.clone()))
    }

    pub fn connection_by_id(&mut self, client_id: &ClientId) -> Option<&mut Connection>
    {
        let established_connection = self.established.get_mut(client_id).map(|c| &mut c.connection);
        let connecting = self.connecting.get_mut(client_id).map(|c| &mut c.connection);
        established_connection.or(connecting)
    }

    pub fn client_by_id(&mut self, client_id: &ClientId) -> Option<AnyClientRef> {
        self.established.get_mut(client_id)
            .map(|c| AnyClientRef::Established(c))
            .or(self.connecting.get_mut(client_id).map(|c| AnyClientRef::Connecting(c)))
    }

    pub fn use_current_client(&mut self, client_id: &ClientId) -> Option<AnyClient> {
        self.established.remove(client_id)
            .map(|c| AnyClient::Established(c))
            .or(self.connecting.remove(client_id).map(|c| AnyClient::Connecting(c)))
    }

    pub fn put_current_client_back(&mut self, client_id: ClientId, client: AnyClient) {
        match client {
            AnyClient::Established(client) => { self.established.insert(client_id, client); },
            AnyClient::Connecting(client) => { self.connecting.insert(client_id, client); },
        };
    }
}