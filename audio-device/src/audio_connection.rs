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
    fn change_input_device(device_id: String);
    fn change_output_device(device_id: String);
}

pub trait EventLoop {
    type Registration;
    type Notify: EventLoopNotify;
    type AudioConnection: AudioConnection;

    fn run(self);
}

pub trait EventLoopNotify {
    fn set_ready(&self);
}

#[cfg(unix)]
pub fn create_minimal_event_loop(
    mic: impl MicSink + 'static,
    playback: UnsharedPlaybackSource<impl PlaybackSource + 'static>,
) -> impl EventLoop {
    use crate::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
    let parameters = PulseParameters {
        pulseaudio_server: Some("/run/user/1000/pulse/native".to_string()),
    };
    PulseAudioConnection::create(parameters, mic, playback).create_event_loop()
}

#[cfg(windows)]
pub fn create_minimal_event_loop(
    mic: impl MicSink,
    playback: UnsharedPlaybackSource<impl PlaybackSource>,
) -> impl EventLoop {
    comms_lib::winlib::bump_priority();
    use crate::wasapi::audio_connection::WasapiAudioConnection;
    WasapiAudioConnection::create::<crate::buffer::NoopInterruptReceiver>(mic, playback)
        .create_event_loop()
}
