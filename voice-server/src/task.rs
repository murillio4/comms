use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::Arc;
use mio::{Registration, Evented, Token, Ready, PollOpt, SetReadiness};
use std::io;
use futures::future::BoxFuture;
use std::future::Future;
use futures::FutureExt;
use futures::task::{ArcWake, Context, Poll, waker_ref};
use futures::channel::oneshot;
use std::task::Waker;
use std::pin::Pin;
use std::cell::UnsafeCell;
use std::ops::Deref;

pub struct UdpFuture {
    receiver: Arc<SyncUnsafeCell<oneshot::Receiver<Result<(), ()>>>>,
    waker: Arc<SyncUnsafeCell<Option<Waker>>>,
}

impl Future for UdpFuture {
    type Output = Result<(), ()>; // TODO need to be result? Need output at all?

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = self.receiver.0.get();
        match unsafe { &mut *receiver }.try_recv() {
            Ok(Some(response)) => Poll::Ready(response),
            Ok(None) => {
                let waker = self.waker.0.get();
                unsafe { *waker = Some(cx.waker().clone()) };
                Poll::Pending
            },
            Err(e) => panic!(e),
        }
    }
}

impl UdpFuture {
    pub fn new() -> (Self, UdpFinish) {
        let (sender, receiver) = oneshot::channel();
        let waker = Arc::new(SyncUnsafeCell(UnsafeCell::new(None)));

        (
            UdpFuture {
                receiver: Arc::new(SyncUnsafeCell(UnsafeCell::new(receiver))),
                waker: waker.clone(),
            },
            UdpFinish {
                sender,
                waker,
            },
        )
    }
}

pub struct UdpFinish {
    sender: oneshot::Sender<Result<(), ()>>,
    waker: Arc<SyncUnsafeCell<Option<Waker>>>,
}

impl UdpFinish {
    pub fn done(self, response: Result<(), ()>) {
        let _ = self.sender.send(response);
        let waker = unsafe { &mut *self.waker.get() };
        if let Some(waker) = waker.take() {
            waker.wake();
        }
    }
}

pub struct TaskExecutor {
    ready_queue: Receiver<Arc<Task>>, // TODO do this and other needs Arcs?
    registration: Registration,
}

impl TaskExecutor {
    pub fn run(&self) {
        loop {
            match self.ready_queue.try_recv() {
                Err(e) if e == TryRecvError::Empty => break,
                Err(e) => panic!(e),
                Ok(task) => {
                    let future_slot = unsafe { &mut *task.future.0.get() };
                    if let Some(mut future) = future_slot.take() {
                        let waker = waker_ref(&task);
                        let context = &mut Context::from_waker(&*waker);
                        if let Poll::Pending = future.as_mut().poll(context) {
                            *future_slot = Some(future)
                        }
                    }
                }
            }
        }
    }
}

impl Evented for TaskExecutor {
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

pub struct Spawner {
    task_scheduler: Scheduler,
}

impl Spawner {
    pub fn spawn(&self, future: impl Future<Output = ()> + 'static + Send) {
        let future = future.boxed();
        let task = Arc::new(Task {
            future: SyncUnsafeCell(UnsafeCell::new(Some(future))),
            task_scheduler: self.task_scheduler.clone(),
        });
        self.task_scheduler.schedule(task);
    }
}

#[derive(Clone)]
struct Scheduler {
    task_sender: SyncSender<Arc<Task>>,
    set_readiness: SetReadiness
}

impl Scheduler {
    fn schedule(&self, task: Arc<Task>) {
        self.task_sender.send(task).unwrap();
        self.set_readiness.set_readiness(Ready::readable()).unwrap();
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

struct Task {
    future: SyncUnsafeCell<Option<BoxFuture<'static, ()>>>,
    task_scheduler: Scheduler,
}

impl ArcWake for Task {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let cloned = arc_self.clone();
        arc_self.task_scheduler.schedule(cloned);
    }
}

pub fn new() -> (TaskExecutor, Spawner) {
    let (task_sender, ready_queue) = sync_channel(10_000);
    let (registration, set_readiness) = Registration::new2();
    (TaskExecutor { ready_queue, registration }, Spawner { task_scheduler: Scheduler { task_sender, set_readiness } })
}