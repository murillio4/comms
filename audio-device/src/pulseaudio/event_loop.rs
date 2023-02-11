use crate::audio_connection::{EventLoop, EventLoopNotify};
use crate::buffer::InterruptReceiver;
use crate::pulseaudio::audio_connection::PulseAudioConnection;
use libpulse_binding::mainloop::api::{Mainloop, MainloopInner};
use libpulse_binding::mainloop::events::io;
use libpulse_binding::mainloop::events::io::IoEvent;
use libpulse_binding::mainloop::standard::{Mainloop as MainloopImpl, MainloopInternal};
use std::cell::RefCell;
use std::mem::size_of_val;
use std::rc::Rc;

#[derive(Clone)]
pub struct PulseEventLoopNotify {
    pub fd: libc::c_int,
}

unsafe impl Send for PulseEventLoopNotify {}

impl EventLoopNotify for PulseEventLoopNotify {
    fn set_ready(&self) {
        let value = 1u64;
        let ptr = &value as *const u64;
        let size = size_of_val(&value);
        let result = unsafe { libc::write(self.fd, ptr as _, size) };
        assert_eq!(result as usize, size);
    }
}

pub struct PulseEventLoopRegistration {
    pub fd: libc::c_int,
}

pub struct PulseEventLoopBuilder {
    mainloop: MainloopImpl,
    connection: Option<PulseAudioConnection>,
    io_event_handle: Option<IoEvent<MainloopInner<MainloopInternal>>>,
}

impl PulseEventLoopBuilder {
    pub fn new(mainloop: MainloopImpl, connection: PulseAudioConnection) -> Self {
        Self {
            mainloop,
            connection: Some(connection),
            io_event_handle: None,
        }
    }

    pub fn with_interrupt(
        mut self,
        receiver: impl InterruptReceiver<PulseEventLoopRegistration> + 'static,
    ) -> Self {
        let fd = receiver.registration().fd;
        let receiver = Rc::new(RefCell::new(receiver));
        let connection = Rc::new(self.connection.take().unwrap());
        let io_event_handle = {
            self.mainloop.new_io_event(
                fd,
                io::FlagSet::INPUT,
                Box::new(move |_, fd, _| {
                    unsafe { libc::read(fd, std::ptr::null() as *const u64 as _, 8) };
                    let mut receiver = receiver.borrow_mut();
                    receiver.invoke(&*connection);
                }),
            )
        };
        self.io_event_handle = io_event_handle;
        self
    }

    pub fn create_event_loop(self) -> PulseEventLoop {
        PulseEventLoop {
            mainloop: self.mainloop,
            _connection: self.connection,
            _io_event_handle: self.io_event_handle,
        }
    }
}

pub struct PulseEventLoop {
    mainloop: MainloopImpl,
    // Need to keep Context in scope, otherwise mainloop.run() doesn't do anything
    _connection: Option<PulseAudioConnection>,
    // Keep IO handle in scope while mainloop.run() is invoked
    _io_event_handle: Option<IoEvent<MainloopInner<MainloopInternal>>>,
}

impl EventLoop for PulseEventLoop {
    type Registration = PulseEventLoopRegistration;
    type Notify = PulseEventLoopNotify;
    type AudioConnection = PulseAudioConnection;

    fn run(mut self) -> ! {
        self.mainloop.run().unwrap();
        panic!("Pulse event loop should never exit on its own");
    }
}
