use crate::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};

#[derive(Clone)]
pub struct DeviceInfo {
    pub label: String,
    pub id: String,
}

pub struct FoundDevices {
    pub input_devices: Vec<DeviceInfo>,
    pub output_devices: Vec<DeviceInfo>,
}

pub trait AudioConnection {
    fn list_devices(&self, callback: impl FnOnce(FoundDevices) + 'static);
}

pub trait EventLoop {
    type Registration;
    type Notify: EventLoopNotify;
    type AudioConnection: AudioConnection;

    fn run(self) -> !;
}

pub trait EventLoopNotify {
    fn set_ready(&self);
}

pub fn create_minimal_event_loop(
    mic: impl MicSink + 'static,
    playback: UnsharedPlaybackSource<impl PlaybackSource + 'static>,
) -> impl EventLoop {
    crate::cpal::CpalEventLoop::create(mic, playback, crate::buffer::NoopInterruptReceiver)
        .expect("Failed to initialize audio")
}
