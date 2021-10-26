use audio_device::audio_connection::{create_minimal_event_loop, EventLoop};
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

fn main() {
    let (mic, playback) = create_pipeline();
    let event_loop = create_minimal_event_loop(mic, playback);
    event_loop.run();
}
