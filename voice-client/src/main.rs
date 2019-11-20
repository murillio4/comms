//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

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
use libpulse_binding::stream::{Stream, PeekResult, SeekMode};
use libpulse_binding::def::BufferAttr;
use std::rc::Rc;
use std::cell::RefCell;
use libpulse_binding::stream::flags::ADJUST_LATENCY;
use clap::{App, Arg};
use std::sync::{Arc, mpsc};
use mio::Registration;
use voice_lib::packet;
use crate::network::AppendToQueue;
use std::sync::atomic::AtomicUsize;
use voice_lib::packet::AudioFromClient;
use ringbuf::{Consumer, RingBuffer};

mod network;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};
const PULSE_LATENCY: MicroSeconds = MicroSeconds(30000);

fn record(pulse_context: &mut Context, append_to_queue: AppendToQueue) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "record", &SPEC, None).unwrap()
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
        fragsize: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };
    stream.connect_record(None, Some(&buffer_attr), ADJUST_LATENCY).unwrap();

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
                        append_to_queue.push(AudioFromClient(data)).unwrap();
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


fn playback(pulse_context: &mut Context, mut rx: Consumer<u8>) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "playback", &SPEC, None).unwrap()
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
        tlength: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };

    stream.connect_playback(None, Some(&buffer_attr), ADJUST_LATENCY, None, None).unwrap();
    stream.set_write_callback(Some(Box::new(move |bytes| {
        let mut stream = stream_rc.borrow_mut();
        let buffer = stream.begin_write(Some(bytes)).unwrap().unwrap();
        let mut input_fell_behind = false;

        for sample in buffer.iter_mut() {
            *sample = match rx.pop() {
                Some(s) => s,
                None => {
                    input_fell_behind = true;
                    0
                },
            }
        }

        if input_fell_behind {
            println!("[!] input fell behind");
        }

        stream.write_copy(&buffer, 0, SeekMode::Relative).unwrap();
    })));
}


fn main() {
    let matches = App::new("Comms")
        .version("0.1")
        .arg(Arg::with_name("server_address").takes_value(true).required(true))
        .arg(Arg::with_name("pulseaudio").takes_value(true))
        .get_matches();

    let server_address = matches.value_of("server_address").unwrap();
    let pulseaudio = matches.value_of("pulseaudio");

    let server_address = format!("{}:18847", server_address);
    let server_address = server_address.parse().unwrap();
    let (sender, receiver) = mpsc::channel();
    let (registration, set_readiness) = Registration::new2();
    let size = Arc::new(AtomicUsize::new(0));
    let append_to_queue = AppendToQueue::new(sender, set_readiness, size.clone());

    let ring = RingBuffer::new(4800);
    let (producer, consumer) = ring.split();

    std::thread::spawn(move || {
        network::run(server_address, receiver, producer, registration, size).unwrap();
    });

    let connect = packet::Connect();
    append_to_queue.push(connect).unwrap();

    let mut mainloop = Mainloop::new().unwrap();
    let mut context = Context::new(&mainloop, "voip-client").unwrap();
    context.connect(pulseaudio, NOAUTOSPAWN, None).unwrap();
    context.wait(&mut mainloop).unwrap();
    record(&mut context, append_to_queue);
    playback(&mut context, consumer);
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
