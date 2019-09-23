//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

use std::net::UdpSocket;

use libpulse_binding::mainloop::standard::{Mainloop, IterateResult};
use libpulse_binding::context::Context;
use libpulse_binding::context::flags::NOAUTOSPAWN;
use libpulse_binding::error::PAErr;
use libpulse_binding::time::MicroSeconds;
use std::error::Error;
use std::fmt;
use std::fmt::Formatter;
use libpulse_binding::context;
use libpulse_binding::sample::{Spec, Format};
use libpulse_binding::stream::{Stream, SeekMode};
use libpulse_binding::def::BufferAttr;
use std::rc::Rc;
use std::cell::RefCell;
use libpulse_binding::stream::flags::ADJUST_LATENCY;
use std::sync::mpsc::Receiver;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};
const MAX_PACKET_SIZE: usize = 2000;

fn play(context: &mut Context, rx: Receiver<u8>) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(context, "playback", &SPEC, None).unwrap()
    ));
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    stream.set_overflow_callback(Some(Box::new(move || {
        println!("[playback] overflow");
    })));
    stream.set_underflow_callback(Some(Box::new(move || {
        println!("[playback] underflow");
    })));
    {
        let stream_rc = stream_rc.clone();
        stream.set_started_callback(Some(Box::new(move || {
            let mut stream = stream_rc.borrow_mut();
            println!("[playback] started -> {:?}", stream.get_buffer_attr().unwrap());
        })));
    }
    stream.set_suspended_callback(Some(Box::new(move || {
        println!("[playback] suspended");
    })));
    stream.set_event_callback(Some(Box::new(move |_, _| {
        println!("[playback] event???");
    })));
    stream.set_moved_callback(Some(Box::new(move || {
        println!("[playback] it moved how neat is that");
    })));

    let buffer_attr = BufferAttr {
        fragsize: std::u32::MAX, // only matters for recording
        maxlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        tlength: SPEC.usec_to_bytes(MicroSeconds(30000)) as u32,
    };
    stream.connect_playback(None, Some(&buffer_attr), ADJUST_LATENCY, None, None).unwrap();

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect("127.0.0.1:18847").unwrap();
    stream.set_write_callback(Some(Box::new(move |bytes| {
        let mut stream = stream_rc.borrow_mut();
        let buffer = stream.begin_write(Some(bytes)).unwrap().unwrap();

        for i in 0..buffer.len() {
            let sample = match rx.try_recv() {
                Ok(sample) => {
                    sample
                },
                Err(_) => 0u8,
            };
            buffer[i] = sample;
        }

        stream.write_copy(&buffer, 0, SeekMode::Relative).unwrap();
    })));
}

fn main() {
    let (tx, rx) = std::sync::mpsc::sync_channel((SPEC.rate / 16) as usize);
    std::thread::spawn(move || {
        let mut mainloop = Mainloop::new().unwrap();
        let mut context = Context::new(&mainloop, "voip-client").unwrap();
        context.connect(Some("/run/user/1000/pulse/native"), NOAUTOSPAWN, None).unwrap();
        context.wait(&mut mainloop).unwrap();
        play(&mut context, rx);
        mainloop.run().unwrap();
    });
    let socket = UdpSocket::bind("127.0.0.1:18847").unwrap();
    let mut buf = [0; MAX_PACKET_SIZE];
    loop {
        let packet_length = socket.recv_from(&mut buf).unwrap().0;
        for i in 0..packet_length {
            tx.send(buf[i]).unwrap();
        }
    }
}

#[derive(Debug)]
enum WaitError {
    Failed(Option<PAErr>),
    Cancelled,
}

impl Error for WaitError {}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), ::std::fmt::Error> {
        match *self {
            WaitError::Failed(_) => f.write_str("Failed"),
            WaitError::Cancelled => f.write_str("Cancelled"),
        }
    }
}

trait WaitableOperation {
    fn wait(&self, mainloop: &mut Mainloop) -> Result<(), WaitError>;
}

impl WaitableOperation for Context {
    fn wait(&self, mainloop: &mut Mainloop) -> Result<(), WaitError> {
        loop {
            match mainloop.iterate(false) {
                IterateResult::Quit(_) => {
                    return Err(WaitError::Cancelled);
                }
                IterateResult::Err(err) => {
                    return Err(WaitError::Failed(Some(err)));
                }
                IterateResult::Success(_) => {}
            };
            match self.get_state() {
                context::State::Ready => { break; }
                context::State::Failed |
                context::State::Terminated => {
                    return Err(WaitError::Failed(None));
                }
                _ => {}
            }
        }
        Ok(())
    }
}