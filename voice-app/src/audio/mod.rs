use std::sync::mpsc::{Receiver, Sender};

use crate::audio::ipc::AudioCommand;
use crate::network::SendToNetworkThreadError;
use crate::network::ipc::NetworkHandle;
use crate::ui::ipc::UICommand;

#[cfg(unix)] mod pulseaudio;
#[cfg(windows)] mod wasapi;
pub mod ipc;

#[cfg(unix)]
pub fn run(pulseaudio_server: Option<String>, network_handle: NetworkHandle, receiver: Receiver<AudioCommand>, ui: Sender<UICommand>) -> Result<(), AudioError> {
    pulseaudio::run(pulseaudio_server, network_handle, receiver, ui)
}

#[cfg(windows)]
pub fn run(_: Option<String>, network_handle: NetworkHandle, receiver: Receiver<AudioCommand>, ui: Sender<UICommand>) -> Result<(), AudioError> {
    wasapi::run(network_handle, receiver, ui)
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