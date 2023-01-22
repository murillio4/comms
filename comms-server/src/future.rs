use crate::network::NetworkHandle;
use comms_lib::network::Protocol;
use comms_lib::packet::ClientId;
use comms_lib::serialization::{IncomingPacketFrame, OutgoingPacketFrame};
use futures::channel::oneshot;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

pub struct SendResponse {
    sender: oneshot::Sender<IncomingPacketFrame>,
    waker: Arc<RefCell<Option<Waker>>>,
}

impl SendResponse {
    pub fn done(self, response: IncomingPacketFrame) {
        let _ = self.sender.send(response); // Timeouts can cause the result of a future to become irrelevant
        let mut waker = self.waker.borrow_mut();
        if let Some(waker) = waker.take() {
            waker.wake()
        }
    }
}

pub struct NetworkFuture {
    receiver: Arc<RefCell<oneshot::Receiver<IncomingPacketFrame>>>,
    waker: Arc<RefCell<Option<Waker>>>,
}

impl Future for NetworkFuture {
    type Output = IncomingPacketFrame;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut receiver = self.receiver.borrow_mut();
        match receiver.try_recv() {
            Ok(Some(response)) => Poll::Ready(response),
            Ok(None) => {
                let mut waker = self.waker.borrow_mut();
                *waker = Some(cx.waker().clone());
                Poll::Pending
            }
            Err(_) => {
                panic!("NetworkFuture was cancelled (other side didn't send response?)");
            }
        }
    }
}

impl NetworkFuture {
    pub fn new(
        handle: &NetworkHandle,
        outgoing: OutgoingPacketFrame,
        protocol: Protocol,
        dest: ClientId,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(RefCell::new(None));

        handle.send_expect(
            protocol,
            dest,
            outgoing,
            SendResponse {
                sender,
                waker: waker.clone(),
            },
        );

        NetworkFuture {
            receiver: Arc::new(RefCell::new(receiver)),
            waker,
        }
    }
}
