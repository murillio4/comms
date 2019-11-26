use libpulse_binding::sample::{Spec, Format};
use libpulse_binding::time::MicroSeconds;
use libpulse_binding::context::Context;
use std::cell::RefCell;
use std::rc::Rc;
use libpulse_binding::stream::{Stream, PeekResult, SeekMode};
use libpulse_binding::def::{BufferAttr, Retval};
use libpulse_binding::stream::flags::ADJUST_LATENCY;
use voice_lib::packet::{AudioFromClient, Packet};
use std::sync::mpsc::{Receiver, Sender, SendError};
use ringbuf::Consumer;
use std::collections::HashMap;
use sample::Sample;
use libpulse_binding::error::PAErr;
use mio::{SetReadiness, Ready};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use voice_lib::{serialize, SerializeError};
use std::{io, process, fmt};
use crate::network::AudioCommand;
use serde::Serialize;
use libpulse_binding::mainloop::standard::{Mainloop, IterateResult};
use libpulse_binding::context::flags::NOAUTOSPAWN;
use libpulse_binding::context;
use std::fmt::Formatter;
use std::error::Error;
use crate::audio::{AudioError, ReceiveFromNetworkThread, SendToNetworkThread};

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};
const PULSE_LATENCY: MicroSeconds = MicroSeconds(30000);

pub fn run(pulseaudio_server: Option<&str>, send: SendToNetworkThread, receive: ReceiveFromNetworkThread) -> Result<(), AudioError> {
    let mut mainloop = Mainloop::new().ok_or(AudioError::PulseInitializationFailed)?;
    let mut context = Context::new(&mainloop, "voip-client").ok_or(AudioError::PulseInitializationFailed)?;
    context.connect(pulseaudio_server, NOAUTOSPAWN, None)?;
    context.wait(&mut mainloop)?;
    record(&mut context, send)?;
    playback(&mut context, receive)?;
    mainloop.run()?;
    Ok(())
}

fn record(pulse_context: &mut Context, send: SendToNetworkThread) -> Result<(), AudioError> {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "record", &SPEC, None).ok_or(AudioError::StreamCreation)?
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
    stream.connect_record(None, Some(&buffer_attr), ADJUST_LATENCY)?;

    stream.set_read_callback(Some(Box::new(move |_| {
        let mut stream = stream_rc.borrow_mut();
        if let Err(e) = on_record_callback(&mut stream, &send) {
            eprintln!("Encountered an error while handling microphone audio, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    })));
    Ok(())
}

fn on_record_callback(stream: &mut Stream, send_to_network_thread: &SendToNetworkThread) -> Result<(), AudioError> {
    match stream.peek() {
        Ok(data) => {
            match data {
                PeekResult::Empty => println!("empty"),
                PeekResult::Hole(_) => println!("hole detected"),
                PeekResult::Data(data) => {
                    let packet = AudioFromClient(data);
                    send_to_network_thread.send(packet)?;
                }
            }
        },
        Err(_) => {
            panic!("stream.peek() error");
        }
    }
    stream.discard()?;
    Ok(())
}


fn playback(pulse_context: &mut Context, receive: ReceiveFromNetworkThread) -> Result<(), AudioError> {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "playback", &SPEC, None).ok_or(AudioError::StreamCreation)?
    ));
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    let receive = Rc::new(RefCell::new(receive));

    stream.set_overflow_callback(Some(Box::new(move || {
        println!("[playback] overflow");
    })));
    stream.set_underflow_callback(Some(Box::new(move || {
        println!("[playback] underflow");
    })));
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

    stream.connect_playback(None, Some(&buffer_attr), ADJUST_LATENCY, None, None)?;
    stream.set_write_callback(Some(Box::new(move |bytes| {
        let mut stream = stream_rc.borrow_mut();
        if let Err(e) = on_playback_callback(&mut stream, bytes, &mut receive.borrow_mut()) {
            eprintln!("Encountered an error while outputting audio, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    })));
    Ok(())
}

fn on_playback_callback(stream: &mut Stream, bytes: usize, receive_from_network_thread: &mut ReceiveFromNetworkThread) -> Result<(), AudioError> {
    let buffer = stream.begin_write(Some(bytes))?.ok_or(AudioError::NoBufferToWriteTo)?;
    receive_from_network_thread.handle_commands();

    for i in 0..buffer.len() / 2 {
        let sample = receive_from_network_thread.next_sample();
        let le_bytes = sample.to_le_bytes();
        buffer[i*2] = le_bytes[0];
        buffer[i*2 + 1] = le_bytes[1];
    }

    stream.write_copy(&buffer, 0, SeekMode::Relative)?;
    Ok(())
}

impl From<PAErr> for AudioError {
    fn from(err: PAErr) -> Self {
        AudioError::Pulse(err.to_string())
    }
}

impl From<(PAErr, Retval)> for AudioError {
    fn from(err_tuple: (PAErr, Retval)) -> Self {
        AudioError::Pulse(err_tuple.0.to_string())
    }
}

impl From<WaitError> for AudioError {
    fn from(_: WaitError) -> Self {
        AudioError::PulseInitializationFailed
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