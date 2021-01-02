use crate::buffer::{create_loopback_pipeline, MicForLoopback, PlaybackForLoopback};
use voice_device::audio_connection::EventLoop;
use voice_device::buffer::UnsharedPlaybackSource;

mod buffer;

#[cfg(unix)]
fn create_event_loop(
    mic: MicForLoopback,
    playback: UnsharedPlaybackSource<PlaybackForLoopback>,
) -> impl EventLoop {
    use voice_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
    let parameters = PulseParameters {
        pulseaudio_server: Some("/run/user/1000/pulse/native".to_string()),
    };
    PulseAudioConnection::create(parameters, mic, playback).create_event_loop()
}

#[cfg(windows)]
fn create_event_loop(
    mic: MicForLoopback,
    playback: UnsharedPlaybackSource<PlaybackForLoopback>,
) -> impl EventLoop {
    use voice_device::wasapi::audio_connection::WasapiAudioConnection;
    WasapiAudioConnection::create::<voice_device::buffer::NoopInterruptReceiver>(mic, playback)
        .create_event_loop()
}

fn main() {
    let (mic, playback) = create_loopback_pipeline();
    let event_loop = create_event_loop(mic, playback);
    event_loop.run();
}
