use std::sync::mpsc::{Sender, Receiver, channel};
use sample::Sample;
use voice_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};

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
        self.receiver.try_recv().unwrap_or(Sample::equilibrium())
    }
}

pub fn create_loopback_pipeline() -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback { sender: tx },
        UnsharedPlaybackSource(PlaybackForLoopback { receiver: rx }),
    )
}