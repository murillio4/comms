// TODO impl tcp
//fn read_from_tcp_stream(stream: &mut TcpStream) -> Result<TcpRead, io::Error> {
//    let mut received_data = Vec::new();
//    loop {
//        let mut buf = [0; 256];
//        match stream.read(&mut buf) {
//            Ok(0) => return Ok(TcpRead::ConnectionClosed),
//            Ok(n) => received_data.extend_from_slice(&buf[..n]),
//            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(TcpRead::Data(received_data)),
//            Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
//            Err(err) => return Err(err),
//        }
//    }
//}
//
//enum TcpRead {
//    ConnectionClosed,
//    Data(Vec<u8>),
//}

use voice_lib::packet::ClientId;
use std::collections::{HashMap, VecDeque};
use crate::{Client, ConnectingClient};
use std::num::NonZeroU64;
use voice_lib::{PacketHeader, serialize_head, packet, serialize_body};
use mio::{IoVec, Registration, Evented, Token, Ready, PollOpt, SetReadiness};
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use voice_lib::Serialize;
use futures::future::join_all;
use crate::task::{PacketFuture, PacketFinish};
use crate::udp::UdpHandle;

pub async fn hello_todo(client_id: ClientId, tcp_handle: TcpHandle) {
    println!("queue-ing hello packet");
    send_packet(&tcp_handle, client_id, packet::Hello {
        client_id: client_id,
    }).await;
}

pub async fn send_packet<B>(handle: &TcpHandle, dest: ClientId, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());
    handle.send(dest, B::KIND_ID, raw_body).await.unwrap();
}

pub async fn send_packet_to_many<B>(handle: &UdpHandle, clients: Vec<ClientId>, packet: B)
    where B: packet::PacketBody + Serialize
{
    let raw_body = Arc::new(serialize_body(packet).unwrap());

    let futures: Vec<PacketFuture> = clients.into_iter()
        .map(|c| handle.send(c, B::KIND_ID, raw_body.clone()))
        .collect();
    for result in join_all(futures).await {
        result.unwrap();
    }
}

pub enum WriteAttempt {
    NoPacketAvailable,
    WriteAttempted,
}

enum PacketSendStatus {
    Done,
    SocketUnavailable,
    SkipPacket,
}

fn attempt_write_packet(client_id: ClientId, packet_kind: u8, body: &Arc<Vec<u8>>, clients: &mut HashMap<ClientId, Client>, connecting_clients: &mut HashMap<ClientId, ConnectingClient>) -> PacketSendStatus {
    let (packet_id, connection) = if let Some(client) = clients.get_mut(&client_id) {
        let packet_id = client.next_packet_id;
        client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        (packet_id, &mut client.tcp_connection)
    } else if let Some(client) = connecting_clients.get_mut(&client_id) {
        let packet_id = client.next_packet_id;
        client.next_packet_id = NonZeroU64::new(packet_id.get() + 1).unwrap();
        (packet_id, &mut client.tcp_connection)
    } else {
        eprintln!("Attempted to send TCP packet to client {} that doesn't exist", client_id);
        return PacketSendStatus::SkipPacket;
    };

    if !connection.tcp_writable {
        return PacketSendStatus::SocketUnavailable;
    }

    let packet_header = PacketHeader {
        id: packet_id,
        reply_to: None,
        kind: packet_kind,
    };
    let header = serialize_head(packet_header).unwrap();
    let buffers: Vec<&IoVec> = vec![
        IoVec::from_bytes(&header),
        IoVec::from_bytes(body),
    ].into_iter()
        .filter_map(|o| o)
        .collect();

    match connection.tcp_stream.write_bufs(&buffers) {
        Ok(_) => {
            println!("Sent TCP packet of kind {} to {}", packet_kind, client_id);
            connection.tcp_writable = false;
            return PacketSendStatus::Done;
        },
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            connection.tcp_writable = false;
            return PacketSendStatus::SocketUnavailable
        },
        Err(e) => panic!(e),
    }
}

pub fn attempt_write_client_packet(client_id: ClientId, outgoing: &mut TcpOutgoingQueue, clients: &mut HashMap<ClientId, Client>, connecting_clients: &mut HashMap<ClientId, ConnectingClient>) {
    outgoing.next_packet(client_id, |packet_kind, body| {
        attempt_write_packet(client_id, packet_kind, body, clients, connecting_clients)
    });
}

pub fn attempt_write_any_packet(outgoing: &mut TcpOutgoingQueue, clients: &mut HashMap<ClientId, Client>, connecting_clients: &mut HashMap<ClientId, ConnectingClient>) {
    outgoing.any_next_packet(|destination_id, packet_kind, body| {
        attempt_write_packet(destination_id, packet_kind, body, clients, connecting_clients)
    });
}

struct OutgoingPacket {
    dest: ClientId,
    kind: u8,
    raw_body: Arc<Vec<u8>>,
    finish: PacketFinish,
}

pub struct TcpOutgoingQueue {
    queue_per_connection: HashMap<ClientId, VecDeque<OutgoingPacket>>,
    packet_order: VecDeque<ClientId>,
    receiver: Receiver<OutgoingPacket>,
    registration: Registration,
}

impl TcpOutgoingQueue {
    fn sort_incoming_packets(&mut self) {
        // Clear out the incoming queue
        for packet in self.receiver.try_iter() {
            let queue = self.queue_per_connection.entry(packet.dest).or_insert(VecDeque::new());
            self.packet_order.push_back(packet.dest);
            queue.push_back(packet);
        }
    }

    // TODO we could iterating through the packets, trying to send until either
    // one sends _OR_ we've exhausted all potential clients/sockets
    fn any_next_packet<F>(&mut self, mut op: F) -> WriteAttempt
        where F: FnMut(ClientId, u8, &Arc<Vec<u8>>) -> PacketSendStatus
    {
        self.sort_incoming_packets();

        // There might be a cleaner way of doing this.
        // We need the loop to try a new packet if the one first attempted
        // targets a client that is no longer connected.
        loop {
            let client_id = match self.packet_order.pop_front() {
                Some(client_id) => client_id,
                None => return WriteAttempt::NoPacketAvailable,
            };
            let client_queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
            let packet = client_queue.pop_front()
                .expect("packet_order queue is out of sync with queue_per_connection");

            match op(packet.dest, packet.kind, &packet.raw_body) {
                PacketSendStatus::SocketUnavailable => {
                    client_queue.push_front(packet);
                    self.packet_order.push_back(client_id); // Socket is busy, don't use it for a while
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::Done => {
                    packet.finish.done(Ok(()));
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::SkipPacket => {
                    continue;
                }
            }
        }
    }

    // There might be a cleaner way of doing this.
    // We need the loop to try a new packet if the one first attempted
    // targets a client that is no longer connected.
    fn next_packet<F>(&mut self, client_id: ClientId, mut op: F) -> WriteAttempt
        where F: FnMut(u8, &Arc<Vec<u8>>) -> PacketSendStatus
    {
        self.sort_incoming_packets();

        loop {
            let client_queue = self.queue_per_connection.entry(client_id).or_insert(VecDeque::new());
            let packet = match client_queue.pop_front() {
                Some(packet) => packet,
                None => return self.any_next_packet(|_, kind, body| op(kind, body)),
            };

            match op(packet.kind, &packet.raw_body) {
                PacketSendStatus::SocketUnavailable => {
                    client_queue.push_front(packet);
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::Done => {
                    let position = self.packet_order.iter()
                        .position(|id| id == &client_id)
                        .expect("packet_order queue is out of sync with queue_per_connection");
                    self.packet_order.remove(position);
                    packet.finish.done(Ok(()));
                    return WriteAttempt::WriteAttempted;
                },
                PacketSendStatus::SkipPacket => {
                    continue;
                }
            }
        }
    }

}

impl Evented for TcpOutgoingQueue {
    fn register(&self, poll: &mio::Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.register(poll, token, interest, opts)
    }

    fn reregister(&self, poll: &mio::Poll, token: Token, interest: Ready, opts: PollOpt) -> io::Result<()>
    {
        self.registration.reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        poll.deregister(self)
    }
}

#[derive(Clone)]
pub struct TcpHandle {
    sender: SyncSender<OutgoingPacket>,
    set_queue_readiness: SetReadiness,
}

impl TcpHandle {
    pub fn send(&self, dest: ClientId, kind: u8, raw_body: Arc<Vec<u8>>) -> PacketFuture {
        let (future, finish) = PacketFuture::new();
        self.sender.send(OutgoingPacket {
            dest,
            kind,
            raw_body,
            finish,
        }).unwrap();
        self.set_queue_readiness.set_readiness(Ready::readable()).unwrap();
        future
    }
}

pub fn queue_and_handle() -> (TcpHandle, TcpOutgoingQueue) {
    let (sender, receiver) = sync_channel(10_000);
    let (registration, set_queue_readiness) = Registration::new2();
    return (
        TcpHandle {
            sender,
            set_queue_readiness,
        },
        TcpOutgoingQueue {
            receiver,
            registration,
            packet_order: VecDeque::new(),
            queue_per_connection: HashMap::new(),
        }
    )
}