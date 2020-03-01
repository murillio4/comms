use std::sync::{Arc, Mutex};
use std::task::{Waker, Context, Poll};
use std::future::Future;
use std::pin::Pin;
use futures::channel::oneshot;
use crate::network;
use serde::{Serialize};
use serde::de::DeserializeOwned;
use crate::network::{Command, ConnectError};
use std::fmt::{Debug, Formatter, Error};
use std::time::{Duration};
use futures_timer::Delay;
use futures::future;
use futures::future::Either;
use futures::pin_mut;
use async_std::net::ToSocketAddrs;
use voice_lib::packet::{PacketBody, FinishConnectAccepted, Packet, AssociateUdp, FinishConnect, AssociateUdpAccepted, ClientId};
use voice_lib::serialization::{OwnedHeaderParsedPacket, SerializeError, WritePacketBuilder};
use std::num::NonZeroU64;
use voice_lib::network::Protocol;

async fn connect_without_timeout(handle: &network::Handle, destination: String, username: String) -> Result<Vec<(ClientId, String)>, ConnectError> {
    let destination_with_port = format!("{}:18847", destination);
    let destination = match destination_with_port.to_socket_addrs().await {
        Ok(mut addresses) => {
            if let Some(address) = addresses.next() {
                address
            } else {
                return Err(ConnectError::DestinationUnusable)
            }
        },
        Err(_) => {
            return Err(ConnectError::DestinationUnusable);
        }
    };

    let hello = NetworkFuture::new(handle, |s| Command::Connect {
        destination,
        send_response: s
    }).await?;

    let _: Packet<AssociateUdpAccepted> = request_expect(&handle, Protocol::Udp, None,AssociateUdp {
        client_id: hello.client_id,
    }).await?;

    let connect_accepted: Packet<FinishConnectAccepted> = request_expect(
        &handle,
        Protocol::Tcp,
        None,
        FinishConnect {
            name: username
        }
    ).await?;
    Ok(connect_accepted.body.users)
}

pub async fn connect(handle: &network::Handle, destination: String, username: String) -> Result<Vec<(ClientId, String)>, ConnectError> {
    let timeout = Delay::new(Duration::from_secs(1));
    let connect_future = connect_without_timeout(&handle, destination, username);
    pin_mut!(connect_future);
    match future::select(timeout, connect_future).await {
        Either::Left(_) => {
            disconnect(handle).await;
            Err(ConnectError::NetworkError)
        },
        Either::Right((result, _)) => {
            if result.is_err() {
                disconnect(handle).await;
            }
            result
        }
    }
}

pub async fn disconnect(handle: &network::Handle) {
    NetworkFuture::new(handle, |s| Command::Disconnect {
        send_response: s
    }).await
}

pub async fn request_any<Req>(handle: &network::Handle, packet_type: Protocol, reply_to: Option<NonZeroU64>, request: Req) -> Result<OwnedHeaderParsedPacket, SerializeError>
    where Req: PacketBody + Serialize,
{
    let outgoing = WritePacketBuilder::new(reply_to, &request)?;
    Ok(NetworkFuture::new(handle, |s| Command::Packet {
        protocol: packet_type,
        outgoing,
        send_response: Some(s),
    }).await)
}

pub async fn request_expect<Req, Res>(handle: &network::Handle, packet_type: Protocol, reply_to: Option<NonZeroU64>,request: Req) -> Result<Packet<Res>, RequestError>
    where Req: PacketBody + Serialize,
          Res: PacketBody + DeserializeOwned,
{
    let parsed_packet = request_any(handle, packet_type, reply_to, request).await
        .map_err(|_| RequestError::SerializeError)?;
    if parsed_packet.kind() != Res::KIND_ID {
        return Err(RequestError::InvalidResponsePacket)
    }
    parsed_packet.finish_parse().map_err(|_| RequestError::DeserializeError)
}

pub fn send<Req>(handle: &network::Handle, packet_type: Protocol, reply_to: Option<NonZeroU64>, request: Req) -> Result<(), SerializeError>
    where Req: PacketBody + Serialize,
{
    handle.command(Command::Packet {
        protocol: packet_type,
        outgoing: WritePacketBuilder::new(reply_to, &request)?,
        send_response: None,
    });
    Ok(())
}

#[derive(Debug)]
pub enum RequestError {
    SerializeError,
    DeserializeError,
    InvalidResponsePacket,
}

pub struct SendResponse<T> {
    sender: oneshot::Sender<T>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl<T: Debug> Debug for SendResponse<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        f.write_str("<->")
    }
}

impl<T: Debug> SendResponse<T> {
    pub fn done(self, response: T) {
        let _ = self.sender.send(response); // Timeouts can cause the result of a future to become irrelevant
        let mut waker = self.waker.lock().unwrap();
        if let Some(waker) = waker.take() {
            waker.wake()
        }
    }
}

struct NetworkFuture<T> {
    receiver: Arc<Mutex<oneshot::Receiver<T>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl<T> Future for NetworkFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut receiver = self.receiver.lock().unwrap();
        match receiver.try_recv() {
            Ok(Some(response)) => {
                Poll::Ready(response)
            },
            Ok(None) => {
                let mut waker = self.waker.lock().unwrap();
                *waker = Some(cx.waker().clone());
                Poll::Pending
            },
            Err(_) => {
                panic!("NetworkFuture was cancelled (other side didn't send response?)");
            },
        }
    }
}

impl<T> NetworkFuture<T> {
    pub fn new(handle: &network::Handle, create_command: impl FnOnce(SendResponse<T>) -> Command) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(Mutex::new(None));

        handle.command(create_command(SendResponse {
            sender,
            waker: waker.clone(),
        }));

        NetworkFuture {
            receiver: Arc::new(Mutex::new(receiver)),
            waker,
        }
    }
}