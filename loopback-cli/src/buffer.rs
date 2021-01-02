use dasp::Sample;
use std::sync::mpsc::{channel, Receiver, Sender};
use voice_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};

pub struct MicForLoopback {
    sender: Sender<f32>,
    number_of_samples: u32,
}

impl MicSink for MicForLoopback {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.sender.send(sample).unwrap();
        self.number_of_samples += 1;
    }

    fn publish_chunks(&mut self) {
        self.number_of_samples = 0;
    }
}

pub struct PlaybackForLoopback {
    receiver: Receiver<f32>,
}

impl PlaybackSource for PlaybackForLoopback {
    fn next_sample(&mut self) -> f32 {
        self.receiver.try_recv().unwrap_or(Sample::EQUILIBRIUM)
    }
}

pub fn create_loopback_pipeline() -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback {
            sender: tx,
            number_of_samples: 0,
        },
        UnsharedPlaybackSource(PlaybackForLoopback { receiver: rx }),
    )
}
