use crate::network::{AudioCommand, SendToNetworkThreadError, Handle};
use std::sync::mpsc::{Receiver, Sender};
use ringbuf::Consumer;
use std::collections::HashMap;
use sample::Sample;
use crate::ui::UICommand;

#[cfg(unix)] mod pulseaudio;
#[cfg(windows)] mod wasapi;

#[cfg(unix)]
pub fn run(pulseaudio_server: Option<String>, network_handle: Handle, receiver: Receiver<AudioCommand>) -> Result<(), AudioError> {
    pulseaudio::run(pulseaudio_server, network_handle, receiver)
}

#[cfg(windows)]
pub fn run(_: Option<String>, network_handle: Handle, receiver: Receiver<AudioCommand>, ui: Sender<UICommand>) -> Result<(), AudioError> {
    wasapi::run(network_handle, receiver, ui)
}

pub struct ReceiveFromNetworkThread {
    command_rx: Receiver<AudioCommand>,
    client_streams: HashMap<String, Consumer<i16>>,
}

impl ReceiveFromNetworkThread {
    pub fn new(receiver: Receiver<AudioCommand>) -> ReceiveFromNetworkThread {
        ReceiveFromNetworkThread {
            command_rx: receiver,
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
                let next_sample = consumer.pop().unwrap_or(0);
                acc.checked_add(next_sample).unwrap_or(std::i16::MAX)
            })
    }
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