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
use std::error::Error;
use std::fmt;
use std::fmt::Formatter;
use libpulse_binding::context;
use clap::{App, Arg};
use std::sync::{Arc, mpsc};
use mio::Registration;
use voice_lib::packet;
use crate::network::AppendToQueue;
use std::sync::atomic::AtomicUsize;

mod audio;
mod network;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

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

    let (command_tx, command_rx) = mpsc::channel();

    std::thread::spawn(move || {
        network::run(server_address, receiver, command_tx, registration, size).unwrap();
    });

    let connect = packet::Connect();
    append_to_queue.push(connect).unwrap();

    let mut mainloop = Mainloop::new().unwrap();
    let mut context = Context::new(&mainloop, "voip-client").unwrap();
    context.connect(pulseaudio, NOAUTOSPAWN, None).unwrap();
    context.wait(&mut mainloop).unwrap();
    audio::record(&mut context, append_to_queue);
    audio::playback(&mut context, command_rx);
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
