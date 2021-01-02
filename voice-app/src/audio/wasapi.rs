use crate::audio::buffer::{MicBuffer, Multiplexer, RcMultiplexer};
use crate::audio::ipc::AudioReceiver;
use crate::audio::platform::AudioPlatform;
use std::ptr;
use voice_device::wasapi::audio_connection::{WasapiAudioConnection, WasapiParameters};
use voice_device::wasapi::event_loop::{
    WasapiEventLoop, WasapiEventLoopNotify, WasapiEventLoopRegistration,
};
use winapi::um::synchapi;

pub struct WasapiAudioPlatform;

impl AudioPlatform for WasapiAudioPlatform {
    type Parameters = WasapiParameters;
    type Registration = WasapiEventLoopRegistration;
    type Notify = WasapiEventLoopNotify;
    type EventLoop =
        WasapiEventLoop<MicBuffer, Multiplexer, RcMultiplexer, AudioReceiver<Self::Registration>>;

    fn create_parameters() -> Self::Parameters {
        WasapiParameters {}
    }

    fn create_channel() -> (Self::Registration, Self::Notify) {
        let win_handle = unsafe { synchapi::CreateEventA(ptr::null_mut(), 0, 0, ptr::null()) };
        (
            WasapiEventLoopRegistration { win_handle },
            WasapiEventLoopNotify { win_handle },
        )
    }

    fn create_event_loop(
        _parameters: Self::Parameters,
        receiver: AudioReceiver<Self::Registration>,
        mic_buffer: MicBuffer,
        multiplexer: RcMultiplexer,
    ) -> Self::EventLoop {
        WasapiAudioConnection::create(mic_buffer, multiplexer)
            .with_interrupt(receiver)
            .create_event_loop()
    }
}
