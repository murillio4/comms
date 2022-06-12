use audio_device::audio_connection::{create_minimal_event_loop, EventLoop};
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use audio_device::noise_gate::GateNoiseFilter;
use dasp::Sample;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

const BUFFER_SIZE: usize = 4800;

pub struct MicForLoopback {
    sender: Sender<Vec<f32>>,
    pcm_samples: [f32; BUFFER_SIZE],
    pcm_samples_start: usize,
    pcm_samples_end: usize,
}

impl MicSink for MicForLoopback {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples[self.pcm_samples_end] = sample;
        self.pcm_samples_end = (self.pcm_samples_end + 1) % BUFFER_SIZE;
    }

    fn publish_chunks(&mut self) {
        while self.pcm_samples_end < self.pcm_samples_start
            || self.pcm_samples_end - self.pcm_samples_start >= 480
        {
            let next_samples =
                &self.pcm_samples[self.pcm_samples_start..self.pcm_samples_start + 480];
            self.sender.send(next_samples.to_vec()).unwrap();
            self.pcm_samples_start = (self.pcm_samples_start + 480) % BUFFER_SIZE;
        }
    }
}

pub struct PlaybackForLoopback {
    receiver: Receiver<Vec<f32>>,
    gate: GateNoiseFilter,
    pcm_samples: [f32; BUFFER_SIZE],
    pcm_samples_start: usize,
    pcm_samples_end: usize,
}

impl PlaybackSource for PlaybackForLoopback {
    fn next_sample(&mut self) -> f32 {
        let sample = if self.pcm_samples_end != self.pcm_samples_start {
            let sample = self.pcm_samples[self.pcm_samples_start];
            self.pcm_samples_start = (self.pcm_samples_start + 1) % BUFFER_SIZE;
            sample
        } else {
            let samples = match self.receiver.try_recv() {
                Ok(pcm_samples) => pcm_samples,
                Err(TryRecvError::Empty) => return Sample::EQUILIBRIUM,
                Err(TryRecvError::Disconnected) => unimplemented!("channel failed"),
            };
            self.gate.update_window(&samples);

            for sample in &samples[1..] {
                self.pcm_samples[self.pcm_samples_end] = *sample;
                self.pcm_samples_end = (self.pcm_samples_end + 1) % BUFFER_SIZE;
            }

            samples[0]
        };

        self.gate.process(sample)
    }
}

pub fn create_pipeline() -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback {
            sender: tx,
            pcm_samples: [0f32; BUFFER_SIZE],
            pcm_samples_start: 0,
            pcm_samples_end: 0,
        },
        UnsharedPlaybackSource(PlaybackForLoopback {
            receiver: rx,
            gate: GateNoiseFilter::new(128),
            pcm_samples: [0f32; BUFFER_SIZE],
            pcm_samples_start: 0,
            pcm_samples_end: 0,
        }),
    )
}

fn main() {
    let (mic, playback) = create_pipeline();
    let event_loop = create_minimal_event_loop(mic, playback);
    event_loop.run();
}
