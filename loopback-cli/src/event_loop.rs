use audio_device::audio_connection::EventLoop;
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};

#[cfg(unix)]
pub fn create_event_loop(
    mic: impl MicSink + 'static,
    playback: UnsharedPlaybackSource<impl PlaybackSource + 'static>,
) -> impl EventLoop {
    use audio_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
    let parameters = PulseParameters {
        pulseaudio_server: Some("/run/user/1000/pulse/native".to_string()),
    };
    PulseAudioConnection::create(parameters, mic, playback).create_event_loop()
}

#[cfg(windows)]
pub fn create_event_loop(
    mic: impl MicSink,
    playback: UnsharedPlaybackSource<impl PlaybackSource>,
) -> impl EventLoop {
    comms_lib::winlib::bump_priority();
    use audio_device::wasapi::audio_connection::WasapiAudioConnection;
    WasapiAudioConnection::create::<audio_device::buffer::NoopInterruptReceiver>(mic, playback)
        .create_event_loop()
}
