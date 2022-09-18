use crate::audio::buffer::{MicBuffer, Multiplexer, RcMultiplexer};
use crate::audio::ipc::{AudioReceiver, EventLoopCreationError};
use crate::audio::platform::AudioPlatform;
use audio_device::wasapi::audio_connection::{
    WasapiAudioConnection, WasapiCreationError, WasapiParameters,
};
use audio_device::wasapi::event_loop::{
    WasapiEventLoop, WasapiEventLoopNotify, WasapiEventLoopRegistration,
};
use windows::Win32::System::Threading;

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
        let win_handle = unsafe { Threading::CreateEventA(None, false, false, None) }.unwrap();
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
    ) -> Result<Self::EventLoop, EventLoopCreationError> {
        Ok(
            match WasapiAudioConnection::create(mic_buffer, multiplexer) {
                Ok(c) => c,
                Err(e) => {
                    let e = match e {
                        WasapiCreationError::NoInputDevice => EventLoopCreationError::NoInputDevice,
                        WasapiCreationError::NoOutputDevice => {
                            EventLoopCreationError::NoOutputDevice
                        }
                        WasapiCreationError::NoInputFormat(e) => {
                            EventLoopCreationError::InputDeviceError(format!("{:?}", e))
                        }
                        WasapiCreationError::NoOutputFormat(e) => {
                            EventLoopCreationError::OutputDeviceError(format!("{:?}", e))
                        }
                        WasapiCreationError::InputInitializationFailure(e) => {
                            EventLoopCreationError::InputDeviceError(format!("{:?}", e))
                        }
                        WasapiCreationError::OutputInitializationFailure(e) => {
                            EventLoopCreationError::OutputDeviceError(format!("{:?}", e))
                        }
                    };
                    return Err(e);
                }
            }
            .with_interrupt(receiver)
            .create_event_loop(),
        )
    }
}
