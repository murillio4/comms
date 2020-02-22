use std::sync::{Arc, Mutex};
use std::task::{Waker, Context, Poll};
use std::future::Future;
use std::pin::Pin;
use futures::channel::oneshot;
use voice_lib::{packet, OwnedHeaderParsedPacket};
use crate::network;
use serde::{Serialize};
use serde::de::DeserializeOwned;
use crate::network::{Command, ConnectError, DisconnectError};
use std::fmt::{Debug, Formatter, Error};
use std::time::Duration;
use futures_timer::Delay;
use futures::future;
use futures::future::Either;
use futures::pin_mut;
use async_std::net::ToSocketAddrs;
use std::net::SocketAddr;
use voice_lib::packet::{PacketBody, FinishConnectAccepted, Packet};

async fn connect_to_server_without_timeout(handle: network::Handle, destination: String, username: String) -> Result<Vec<String>, ConnectError> {
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

    ConnectFuture::new(&handle, destination).await?;
    let connect_accepted: Packet<FinishConnectAccepted> = request(
        &handle,
        packet::FinishConnect {
            username
        }
    ).await
        .map_err(|_| ConnectError::NetworkError)?;

    Ok(connect_accepted.body.users)
}

pub async fn connect_to_server(handle: network::Handle, destination: String, username: String) -> Result<Vec<String>, ConnectError> {
    let timeout = Delay::new(Duration::from_secs(1)); // TODO timeout should include lookup
    let connect_future = connect_to_server_without_timeout(handle.clone(), destination, username);
    pin_mut!(connect_future);
    match future::select(timeout, connect_future).await {
        Either::Left(_) => {
            let _ = disconnect_from_server(handle).await;
            Err(ConnectError::NetworkError)
        },
        Either::Right((result, _)) => result
    }
}

pub async fn disconnect_from_server(handle: network::Handle) -> Result<(), DisconnectError> {
    DisconnectFuture::new(&handle).await
}

pub async fn request_with_unknown_response<Req>(handle: &network::Handle, request: Req) -> OwnedHeaderParsedPacket
    where Req: PacketBody + Serialize,
{
    RequestFuture::new(handle, request).await
}

pub async fn request<Req, Res>(handle: &network::Handle, request: Req) -> Result<Packet<Res>, RequestError>
    where Req: PacketBody + Serialize,
          Res: PacketBody + DeserializeOwned,
{
    let parsed_packet = request_with_unknown_response(handle, request).await;
    if parsed_packet.kind() != Res::KIND_ID {
        return Err(RequestError::InvalidResponsePacket)
    }
    parsed_packet.finish_parse().map_err(|_| RequestError::DeserializeError)
}

pub enum RequestError {
    DeserializeError,
    InvalidResponsePacket,
}

pub struct SendResponse<T: Debug> {
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

// TODO "there's got to be a better way!" of handling these command futures.
// They're very similar, but due to Command::$Variant taking send_response at the same
// level as the other parameters, it's hard to split up nicely. 🤔 worth thinking about how to improve
struct ConnectFuture {
    receiver: Arc<Mutex<oneshot::Receiver<Result<(), ConnectError>>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Future for ConnectFuture {
    type Output = Result<(), ConnectError>;

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
            Err(e) => {
                panic!(e);
            },
        }
    }
}

impl ConnectFuture {
    pub fn new(handle: &network::Handle, destination: SocketAddr) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(Mutex::new(None));
        handle.command(Command::Connect {
            destination,
            send_response: SendResponse {
                sender,
                waker: waker.clone(),
            }
        }).unwrap();

        ConnectFuture {
            receiver: Arc::new(Mutex::new(receiver)),
            waker,
        }
    }
}

struct DisconnectFuture {
    receiver: Arc<Mutex<oneshot::Receiver<Result<(), DisconnectError>>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Future for DisconnectFuture {
    type Output = Result<(), DisconnectError>;

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
            Err(e) => {
                panic!(e);
            },
        }
    }
}

impl DisconnectFuture {
    pub fn new(handle: &network::Handle) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(Mutex::new(None));
        handle.command(Command::Disconnect {
            send_response: SendResponse {
                sender,
                waker: waker.clone(),
            }
        }).unwrap();

        DisconnectFuture {
            receiver: Arc::new(Mutex::new(receiver)),
            waker,
        }
    }
}

struct RequestFuture {
    receiver: Arc<Mutex<oneshot::Receiver<OwnedHeaderParsedPacket>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Future for RequestFuture {
    type Output = OwnedHeaderParsedPacket;

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
            Err(e) => {
                panic!(e);
            },
        }
    }
}

impl RequestFuture {
    pub fn new(handle: &network::Handle, outgoing_packet: impl packet::PacketBody + Serialize) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(Mutex::new(None));
        handle.request(outgoing_packet, SendResponse {sender, waker: waker.clone() }).unwrap();

        RequestFuture {
            receiver: Arc::new(Mutex::new(receiver)),
            waker,
        }
    }
}