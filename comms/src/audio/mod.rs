mod buffer;
pub mod ipc;
mod platform;

use crate::audio::buffer::{MicBuffer, RcMultiplexer};
use crate::audio::ipc::{AudioCommand, AudioReceiver};
pub use crate::audio::platform::AudioPlatform;
use crate::network::ipc::NetworkHandle;
use crate::network::SendToNetworkThreadError;
use crate::ui::ipc::{AudioIPCInitStatus, UICommand};
use audio_device::audio_connection::EventLoop;
use std::sync::mpsc::{Receiver, Sender};

#[cfg(windows)]
mod wasapi;
#[cfg(windows)]
pub type AudioPlatformImpl = crate::audio::wasapi::WasapiAudioPlatform;

#[cfg(unix)]
mod pulseaudio;
#[cfg(unix)]
pub type AudioPlatformImpl = crate::audio::pulseaudio::PulseAudioPlatform;

pub fn run(
    parameters: <AudioPlatformImpl as AudioPlatform>::Parameters,
    network_handle: NetworkHandle,
    ui_handle: Sender<UICommand>,
    registration: <AudioPlatformImpl as AudioPlatform>::Registration,
    receiver: Receiver<AudioCommand>,
) -> Result<(), AudioError> {
    let multiplexer = RcMultiplexer::new();
    let receiver = AudioReceiver::new(
        multiplexer.clone(),
        registration,
        receiver,
        ui_handle.clone(),
        network_handle.clone(),
    );
    let mic_buffer = MicBuffer::new(network_handle)?;
    let event_loop =
        match AudioPlatformImpl::create_event_loop(parameters, receiver, mic_buffer, multiplexer) {
            Ok(l) => l,
            Err(e) => {
                ui_handle
                    .send(UICommand::AudioIPCInitStatus(AudioIPCInitStatus::Error(e)))
                    .unwrap();
                return Ok(());
            }
        };
    ui_handle
        .send(UICommand::AudioIPCInitStatus(AudioIPCInitStatus::Ready))
        .unwrap();
    event_loop.run();
    Ok(())
}

#[derive(Debug)]
pub enum AudioError {
    SendToNetworkThread(SendToNetworkThreadError),
    Opus(audiopus::Error),
}

impl From<SendToNetworkThreadError> for AudioError {
    fn from(err: SendToNetworkThreadError) -> Self {
        AudioError::SendToNetworkThread(err)
    }
}

impl From<audiopus::Error> for AudioError {
    fn from(err: audiopus::Error) -> Self {
        AudioError::Opus(err)
    }
}
