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
use libpulse_binding::stream::{Stream, PeekResult};
use libpulse_binding::def::BufferAttr;
use std::rc::Rc;
use std::cell::RefCell;
use libpulse_binding::stream::flags::ADJUST_LATENCY;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};

fn listen(context: &mut Context) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(context, "record", &SPEC, None).unwrap()
    ));
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    stream.set_overflow_callback(Some(Box::new(move || {
        println!("[recording] overflow");
    })));
    stream.set_underflow_callback(Some(Box::new(move || {
        println!("[recording] underflow");
    })));
    stream.set_started_callback(Some(Box::new(move || {
        println!("[recording] started");
    })));
    stream.set_suspended_callback(Some(Box::new(move || {
        println!("[recording] suspended");
    })));
    stream.set_event_callback(Some(Box::new(move |_, _| {
        println!("[recording] event???");
    })));
    stream.set_moved_callback(Some(Box::new(move || {
        println!("[recording] it moved how neat is that");
    })));

    let buffer_attr = BufferAttr {
        tlength: std::u32::MAX, // only matters for playback
        maxlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        fragsize: SPEC.usec_to_bytes(MicroSeconds(30000)) as u32,
    };
    stream.connect_record(None, Some(&buffer_attr), ADJUST_LATENCY).unwrap();

    //    let mut opus_input_buffer = [0 as i16; 120];
    //    let mut opus_input_buffer_index = 0;

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect("127.0.0.1:18847").unwrap();
    stream.set_read_callback(Some(Box::new(move |_| {
        let mut stream = stream_rc.borrow_mut();
        match stream.peek() {
            Ok(data) => {
                match data {
                    PeekResult::Empty => println!("empty"),
                    PeekResult::Hole(_) => println!("hole detected"),
                    PeekResult::Data(data) => {
//                           let mut packet = Vec::<i16>::with_capacity(data.len() / 2);
//                           for i in 0..(data.len() / 2) {
//                               let sample = i16::from_le_bytes([
//                                   data[i * 2],
//                                   data[i * 2 + 1]
//                               ]);
//                               packet.push(sample);
//                           }

                        // let opus_packet = opus_encoder.encode_vec(&opus_input_buffer, OPUS_PACKET_MAX_LENGTH).unwrap();
                        socket.send(&data).unwrap();
                    }
                }
            },
            Err(_) => {
                panic!("stream.peek() error");
            }
        }
        stream.discard().unwrap();
    })));
}

fn main() {
    let mut mainloop = Mainloop::new().unwrap();
    let mut context = Context::new(&mainloop, "voip-client").unwrap();
    context.connect(Some("/run/user/1000/pulse/native"), NOAUTOSPAWN, None).unwrap();
    context.wait(&mut mainloop).unwrap();
    listen(&mut context);
    mainloop.run().unwrap();
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
