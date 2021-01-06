use futures::channel::oneshot;
use futures::task::{waker_ref, ArcWake, Context, Poll};
use std::cell::{UnsafeCell, RefCell};
use std::fmt::Debug;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, TryRecvError, SyncSender, sync_channel};
use std::sync::Arc;
use std::task::Waker;
use voice_lib::packet::PacketKind;
use std::rc::{Rc, Weak};

type EndpointFuture = dyn Future<Output = EndpointResult> + 'static;

pub struct PacketFuture {
    kind: PacketKind,
    receiver: Arc<SyncUnsafeCell<oneshot::Receiver<()>>>,
    waker: Arc<SyncUnsafeCell<Option<Waker>>>,
}

impl Future for PacketFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = self.receiver.0.get();
        match unsafe { &mut *receiver }.try_recv() {
            Ok(Some(())) => Poll::Ready(()),
            Ok(None) => {
                let waker = self.waker.0.get();
                unsafe { *waker = Some(cx.waker().clone()) };
                Poll::Pending
            }
            Err(e) => {
                dbg!(self.kind);
                panic!(e)
            }
        }
    }
}

impl PacketFuture {
    pub fn new(kind: PacketKind) -> (Self, PacketFinish) {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(SyncUnsafeCell(UnsafeCell::new(None)));

        (
            PacketFuture {
                kind,
                receiver: Arc::new(SyncUnsafeCell(UnsafeCell::new(receiver))),
                waker: waker.clone(),
            },
            PacketFinish { sender, waker },
        )
    }
}

pub struct PacketFinish {
    sender: oneshot::Sender<()>,
    waker: Arc<SyncUnsafeCell<Option<Waker>>>,
}

impl PacketFinish {
    pub fn done(self) {
        let _ = self.sender.send(());
        let waker = unsafe { &mut *self.waker.get() };
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub enum EndpointError {
    SerializeFailed {
        kind: PacketKind,
        packet: Box<dyn Debug>,
    },
}
pub type EndpointResult = Result<(), EndpointError>;

#[derive(Clone)]
struct ArenaToken(usize);

impl Deref for ArenaToken {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct FuturesArena {
    max_token: ArenaToken,
    futures: Vec<Option<Pin<Box<EndpointFuture>>>>,
    tokens: Vec<ArenaToken>,
}

impl FuturesArena {
    fn new_future(&mut self, future: impl Future<Output = EndpointResult> + 'static) -> ArenaToken {
        let future = Box::pin(future);
        match self.tokens.pop() {
            Some(token) => {
                *self.futures.get_mut(*token).expect("Expected future slot to exist for existing token") = Some(future);
                token
            },
            None => {
                let token = self.max_token.clone();
                self.max_token.0 += 1;
                self.futures.insert(token.0, Some(future));
                token
            }
        }
    }
}

pub struct TaskExecutor {
    scheduler: Scheduler,
    waker_queue: Receiver<Arc<FutureWaker>>,
    arena: Rc<RefCell<FuturesArena>>,
}

impl TaskExecutor {
    pub fn new(mio_waker: Arc<mio::Waker>) -> Self {
        let (waker_sender, waker_queue) = sync_channel(10_000);
        let scheduler = Scheduler {
            future_waker_sender: waker_sender,
            mio_waker,
        };
        Self {
            scheduler,
            waker_queue,
            arena: Rc::new(RefCell::new(FuturesArena {
                max_token: ArenaToken(0),
                futures: Vec::new(),
                tokens: Vec::new(),
            })),
        }
    }

    pub fn spawner(&self) -> Spawner {
        Spawner {
            scheduler: self.scheduler.clone(),
            arena: Rc::downgrade(&self.arena),
        }
    }

    pub fn run(&self) {
        loop {
            match self.waker_queue.try_recv() {
                Err(e) if e == TryRecvError::Empty => break,
                Err(e) => panic!(e),
                Ok(arena_waker) => {
                    let mut future = {
                        let mut arena = self.arena.borrow_mut();
                        arena.futures.get_mut(*arena_waker.token).unwrap().take().expect("Requested obsolete future from arena")
                    };
                    let waker = waker_ref(&arena_waker);
                    let context = &mut Context::from_waker(&*waker);
                    match future.as_mut().poll(context) {
                        Poll::Pending => {
                            let mut arena = self.arena.borrow_mut();
                            *arena.futures.get_mut(*arena_waker.token).unwrap() = Some(future);
                        },
                        Poll::Ready(result) => {
                            let mut arena = self.arena.borrow_mut();
                            arena.tokens.push(arena_waker.token.clone());
                            if let Err(e) = result {
                                panic!(format!("Packet-handling function failed: {:?}", e));
                            }
                        }
                    }
                }
            }
        }
    }
}

struct FutureWaker {
    scheduler: Scheduler,
    token: ArenaToken,
}

impl FutureWaker {
    fn new(token: ArenaToken, scheduler: Scheduler) -> Self {
        Self {
            token,
            scheduler,
        }
    }
}

impl ArcWake for FutureWaker {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.scheduler.schedule(arc_self.clone());
    }
}

pub struct Spawner {
    scheduler: Scheduler,
    arena: Weak<RefCell<FuturesArena>>,
}

impl Spawner {
    pub fn spawn(&self, future: impl Future<Output = EndpointResult> + 'static) {
        let future: Pin<_> = Box::pin(future);
        let arena = self.arena.upgrade().expect("Future spawned after executor shut down");
        let mut arena = arena.borrow_mut();
        let token = arena.new_future(future);
        self.scheduler.schedule(Arc::new(FutureWaker::new(token, self.scheduler.clone())));
    }
}

#[derive(Clone)]
struct Scheduler {
    future_waker_sender: SyncSender<Arc<FutureWaker>>,
    mio_waker: Arc<mio::Waker>,
}

impl Scheduler {
    fn schedule(&self, future_waker: Arc<FutureWaker>) {
        self.future_waker_sender.send(future_waker).unwrap();
        self.mio_waker.wake().unwrap();
    }
}

pub struct SyncUnsafeCell<T>(pub UnsafeCell<T>);
unsafe impl<T> Sync for SyncUnsafeCell<T> {}
unsafe impl<T> Send for SyncUnsafeCell<T> {}
impl<T> Deref for SyncUnsafeCell<T> {
    type Target = UnsafeCell<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn new(mio_waker: Arc<mio::Waker>) -> (TaskExecutor, Spawner) {
    let executor = TaskExecutor::new(mio_waker);
    let spawner = executor.spawner();
    (
        executor,
        spawner,
    )
}
