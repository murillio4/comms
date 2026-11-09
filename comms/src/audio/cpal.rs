use crate::audio::buffer::{MicBuffer, Multiplexer, RcMultiplexer};
use crate::audio::ipc::{AudioReceiver, EventLoopCreationError};
use crate::audio::platform::AudioPlatform;
use audio_device::cpal::{CpalEventLoop, CreationError, Notify, Registration};

pub struct CpalAudioPlatform;
impl AudioPlatform for CpalAudioPlatform {
    type Parameters = ();
    type Registration = Registration;
    type Notify = Notify;
    type EventLoop =
        CpalEventLoop<MicBuffer, Multiplexer, RcMultiplexer, AudioReceiver<Registration>>;
    fn create_parameters() {}
    fn create_channel() -> (Registration, Notify) {
        audio_device::cpal::channel()
    }
    fn create_event_loop(
        _: (),
        receiver: AudioReceiver<Registration>,
        mic: MicBuffer,
        playback: RcMultiplexer,
    ) -> Result<Self::EventLoop, EventLoopCreationError> {
        CpalEventLoop::create(mic, playback, receiver).map_err(|e| match e {
            CreationError::Input(e) => EventLoopCreationError::InputDeviceError(e),
            CreationError::Output(e) => EventLoopCreationError::OutputDeviceError(e),
        })
    }
}
