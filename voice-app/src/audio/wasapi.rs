use crate::audio::platform::AudioPlatform;
use voice_device::wasapi::event_loop::{WasapiEventLoop, WasapiEventLoopRegistration, WasapiEventLoopNotify};
use crate::audio::buffer::{MicBuffer, RcMultiplexer, Multiplexer};
use crate::audio::ipc::AudioReceiver;
use voice_device::wasapi::audio_connection::WasapiAudioConnection;
use winapi::um::synchapi;
use std::ptr;

pub struct WasapiAudioPlatform;

impl AudioPlatform for WasapiAudioPlatform {
    type Parameters = ();
    type Registration = WasapiEventLoopRegistration;
    type Notify = WasapiEventLoopNotify;
    type EventLoop = WasapiEventLoop<MicBuffer, Multiplexer, RcMultiplexer, AudioReceiver<Self::Registration>>;

    fn create_parameters() -> Self::Parameters {
        ()
    }

    fn create_channel() -> (Self::Registration, Self::Notify) {
        let win_handle = unsafe { synchapi::CreateEventA(ptr::null_mut(), 0, 0, ptr::null()) };
        (WasapiEventLoopRegistration {
            win_handle: win_handle.clone()
        }, WasapiEventLoopNotify {
            win_handle
        })
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