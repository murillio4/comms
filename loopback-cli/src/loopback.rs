use audio_device::audio_connection::EventLoop;
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use dasp::Sample;
use std::sync::mpsc::{channel, Receiver, Sender};

pub struct MicForLoopback {
    sender: Sender<f32>,
}

impl MicSink for MicForLoopback {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.sender.send(sample).unwrap();
    }

    fn publish_chunks(&mut self) {}
}

pub struct PlaybackForLoopback {
    receiver: Receiver<f32>,
}

impl PlaybackSource for PlaybackForLoopback {
    fn next_sample(&mut self) -> f32 {
        self.receiver.try_recv().unwrap_or(Sample::EQUILIBRIUM)
    }
}

pub fn create_pipeline() -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback { sender: tx },
        UnsharedPlaybackSource(PlaybackForLoopback { receiver: rx }),
    )
}

#[cfg(unix)]
fn create_event_loop(
    mic: MicForLoopback,
    playback: UnsharedPlaybackSource<PlaybackForLoopback>,
) -> impl EventLoop {
    use audio_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
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
    comms_lib::winlib::bump_priority();
    use audio_device::wasapi::audio_connection::WasapiAudioConnection;
    WasapiAudioConnection::create::<audio_device::buffer::NoopInterruptReceiver>(mic, playback)
        .create_event_loop()
}

fn main() {
    let (mic, playback) = create_pipeline();
    let event_loop = create_event_loop(mic, playback);
    event_loop.run();
}
