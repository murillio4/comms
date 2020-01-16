use crate::network::AudioCommand;
use std::sync::mpsc::{Receiver, Sender, SendError};
use ringbuf::Consumer;
use std::collections::HashMap;
use mio::{SetReadiness, Ready};
use std::sync::Arc;
use voice_lib::packet::Packet;
use serde::Serialize;
use voice_lib::{serialize, SerializeError};
use std::sync::atomic::Ordering;
use std::io;
use std::sync::atomic::AtomicUsize;
use sample::Sample;

#[cfg(unix)]
mod pulseaudio;
#[cfg(windows)]
mod wasapi;

#[cfg(unix)]
pub fn run(pulseaudio_server: Option<&str>, send: SendToNetworkThread, receive: ReceiveFromNetworkThread) -> Result<(), AudioError> {
    pulseaudio::run(pulseaudio_server, send, receive)
}

#[cfg(windows)]
pub fn run(_: Option<&str>, send: SendToNetworkThread, receive: ReceiveFromNetworkThread) -> Result<(), AudioError> {
    wasapi::run(send, receive)
}

#[derive(Debug)]
pub enum AudioError {
    SendToNetworkThread(SendToNetworkThreadError),
    #[cfg(unix)]
    StreamCreation,
    #[cfg(unix)]
    Pulse(Option<String>),
    #[cfg(unix)]
    NoBufferToWriteTo,
    #[cfg(unix)]
    PulseInitializationFailed,
}

impl From<SendToNetworkThreadError> for AudioError {
    fn from(err: SendToNetworkThreadError) -> Self {
        AudioError::SendToNetworkThread(err)
    }
}

pub struct ReceiveFromNetworkThread {
    command_rx: Receiver<AudioCommand>,
    client_streams: HashMap<String, Consumer<i16>>,
}

impl ReceiveFromNetworkThread {
    pub fn new(command_rx: Receiver<AudioCommand>) -> ReceiveFromNetworkThread {
        ReceiveFromNetworkThread {
            command_rx,
            client_streams: HashMap::new(),
        }
    }

    fn handle_commands(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                AudioCommand::NewClientStream(id, consumer) => {
                    self.client_streams.insert(id, consumer);
                }
            }
        }
    }

    fn next_sample(&mut self) -> i16 {
        let init: i16 = sample::Sample::equilibrium();
        let init = init.to_signed_sample();
        self.client_streams.values_mut()
            .fold(init, |acc, consumer| {
                let next_sample = consumer.pop().unwrap_or_else(|| {
                    0
                });
                acc.checked_add(next_sample).unwrap_or(std::i16::MAX)
            })
    }
}

pub struct SendToNetworkThread {
    sender: Sender<Vec<u8>>,
    set_readiness: SetReadiness,
    size: Arc<AtomicUsize>,
}

impl SendToNetworkThread {
    pub fn new(sender: Sender<Vec<u8>>, set_readiness: SetReadiness, size: Arc<AtomicUsize>) -> SendToNetworkThread {
        SendToNetworkThread {
            sender,
            set_readiness,
            size,
        }
    }

    pub fn send<P>(&self, packet: P) -> Result<(), SendToNetworkThreadError>
        where P: Serialize + Packet {
        let raw_packet = serialize(packet)?;
        self.sender.send(raw_packet)?;
        self.set_readiness.set_readiness(Ready::readable())?;
        self.size.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug)]
pub enum SendToNetworkThreadError {
    SerializeFailed(SerializeError),
    SendFailed(SendError<Vec<u8>>),
    SetReadinessFailed(io::Error),
}

impl From<SerializeError> for SendToNetworkThreadError {
    fn from(e: SerializeError) -> Self {
        SendToNetworkThreadError::SerializeFailed(e)
    }
}

impl From<SendError<Vec<u8>>> for SendToNetworkThreadError {
    fn from(e: SendError<Vec<u8>>) -> Self {
        SendToNetworkThreadError::SendFailed(e)
    }
}

impl From<io::Error> for SendToNetworkThreadError {
    fn from(e: io::Error) -> Self {
        SendToNetworkThreadError::SetReadinessFailed(e)
    }
}