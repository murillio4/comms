use crate::audio::buffer::{MicBuffer, RcMultiplexer};
use crate::audio::ipc::AudioReceiver;
use audio_device::audio_connection::{EventLoop, EventLoopNotify};

pub trait AudioPlatform {
    type Parameters;
    type Registration;
    type Notify: EventLoopNotify;
    type EventLoop: EventLoop;

    fn create_parameters() -> Self::Parameters;
    fn create_channel() -> (Self::Registration, Self::Notify);
    fn create_event_loop(
        parameters: Self::Parameters,
        receiver: AudioReceiver<Self::Registration>,
        mic_buffer: MicBuffer,
        multiplexer: RcMultiplexer,
    ) -> Self::EventLoop;
}
