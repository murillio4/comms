mod platform;
mod buffer;
pub mod ipc;

use crate::network::SendToNetworkThreadError;
use crate::network::ipc::NetworkHandle;
use crate::audio::ipc::{AudioReceiver, AudioCommand};
use std::sync::mpsc::{Sender, Receiver};
use crate::ui::ipc::UICommand;
use crate::audio::buffer::{MicBuffer, RcMultiplexer};
use voice_device::audio_connection::EventLoop;
pub use crate::audio::platform::AudioPlatform;

#[cfg(windows)] mod wasapi;
#[cfg(windows)] pub type AudioPlatformImpl = crate::audio::wasapi::WasapiAudioPlatform;

#[cfg(unix)] mod pulseaudio;
#[cfg(unix)] pub type AudioPlatformImpl = crate::audio::pulseaudio::PulseAudioPlatform;

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
    );
    let mic_buffer = MicBuffer::new(network_handle);
    let event_loop = AudioPlatformImpl::create_event_loop(parameters, receiver, mic_buffer, multiplexer);
    ui_handle.send(UICommand::AudioIPCReady).unwrap();
    event_loop.run();
    Ok(())
}

#[derive(Debug)]
pub enum AudioError {
    SendToNetworkThread(SendToNetworkThreadError),
}

impl From<SendToNetworkThreadError> for AudioError {
    fn from(err: SendToNetworkThreadError) -> Self {
        AudioError::SendToNetworkThread(err)
    }
}