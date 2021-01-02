use crate::audio::buffer::{MicBuffer, RcMultiplexer};
use crate::audio::ipc::AudioReceiver;
use crate::audio::platform::AudioPlatform;
use voice_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
use voice_device::pulseaudio::event_loop::{
    PulseEventLoop, PulseEventLoopNotify, PulseEventLoopRegistration,
};

pub struct PulseAudioPlatform;

impl AudioPlatform for PulseAudioPlatform {
    type Parameters = PulseParameters;
    type Registration = PulseEventLoopRegistration;
    type Notify = PulseEventLoopNotify;
    type EventLoop = PulseEventLoop;

    fn create_parameters() -> Self::Parameters {
        PulseParameters {
            pulseaudio_server: std::env::var_os("PULSEAUDIO_SERVER")
                .map(|s| s.to_string_lossy().to_string()),
        }
    }

    fn create_channel() -> (Self::Registration, Self::Notify) {
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK) };
        (
            PulseEventLoopRegistration { fd },
            PulseEventLoopNotify { fd },
        )
    }

    fn create_event_loop(
        parameters: Self::Parameters,
        receiver: AudioReceiver<Self::Registration>,
        mic_buffer: MicBuffer,
        multiplexer: RcMultiplexer,
    ) -> Self::EventLoop {
        PulseAudioConnection::create(parameters, mic_buffer, multiplexer)
            .with_interrupt(receiver)
            .create_event_loop()
    }
}
