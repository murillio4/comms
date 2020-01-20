use std::sync::{Arc, Mutex};
use std::task::{Waker, Context, Poll};
use std::future::Future;
use std::pin::Pin;
use futures::channel::oneshot;
use bincode::deserialize_from;
use voice_lib::packet;
use crate::network;
use std::io::Cursor;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;

pub async fn connect_to_server(handle: &network::Handle, username: &str) -> Vec<String> {
    let connect_accepted: packet::ConnectAccepted = request(
        handle,
        packet::Connect {
            username
        }
    ).await;
    connect_accepted.users
}

pub async fn request<'a, Response: packet::Packet<'a> + DeserializeOwned>(handle: &network::Handle, request: impl packet::Packet<'_> + Serialize) -> Response {
    RequestFuture::new(handle, request).await
}

pub struct SendResponse {
    sender: oneshot::Sender<Vec<u8>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl SendResponse {
    pub fn done(self, response_packet: Vec<u8>) {
        self.sender.send(response_packet).unwrap();
        let mut waker = self.waker.lock().unwrap();
        if let Some(waker) = waker.take() {
            waker.wake()
        }
    }
}

pub struct RequestFuture<R> {
    phantom: PhantomData<R>,
    receiver: Arc<Mutex<oneshot::Receiver<Vec<u8>>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl<'a, R: packet::Packet<'a> + DeserializeOwned> Future for RequestFuture<R> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut receiver = self.receiver.lock().unwrap();
        match receiver.try_recv() {
            Ok(Some(response)) => {
                //let packet = deserialize(&response).unwrap();
                let packet = deserialize_from(Cursor::new(response)).unwrap();
                Poll::Ready(packet)
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

impl<'a, R: packet::Packet<'a>> RequestFuture<R> {
    pub fn new<'outgoing>(handle: &network::Handle, outgoing_packet: impl packet::Packet<'outgoing> + Serialize) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(Mutex::new(None));
        handle.request(outgoing_packet, R::HEADER_TYPE, SendResponse {sender, waker: waker.clone() }).unwrap();

        RequestFuture {
            receiver: Arc::new(Mutex::new(receiver)),
            waker,
            phantom: PhantomData,
        }
    }
}