use audio_device::audio_connection::{create_minimal_event_loop, EventLoop};
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use audio_device::noise_gate::GateNoiseFilter;
use dasp::Sample;
use slice_deque::SliceDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

pub struct MicForLoopback {
    sender: Sender<Vec<f32>>,
    pcm_samples: SliceDeque<f32>,
}

impl MicSink for MicForLoopback {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.pcm_samples.len() > 480 {
            let buffer = self.pcm_samples.as_slice()[..480].to_vec();
            self.sender.send(buffer).unwrap();
            unsafe { self.pcm_samples.move_head(480) };
        }
    }
}

pub struct PlaybackForLoopback {
    receiver: Receiver<Vec<f32>>,
    gate: GateNoiseFilter,
    pcm_samples: SliceDeque<f32>,
}

impl PlaybackSource for PlaybackForLoopback {
    fn next_sample(&mut self) -> f32 {
        let sample = match self.pcm_samples.pop_front() {
            Some(sample) => sample,
            None => {
                let samples = match self.receiver.try_recv() {
                    Ok(pcm_samples) => pcm_samples,
                    Err(TryRecvError::Empty) => return Sample::EQUILIBRIUM,
                    Err(TryRecvError::Disconnected) => unimplemented!("channel failed"),
                };
                self.gate.update_window(&samples);
                self.pcm_samples.extend_from_slice(&samples[1..]);
                samples[0]
            }
        };

        self.gate.process(sample)
    }
}

pub fn create_pipeline() -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback {
            sender: tx,
            pcm_samples: SliceDeque::new(),
        },
        UnsharedPlaybackSource(PlaybackForLoopback {
            receiver: rx,
            gate: GateNoiseFilter::new(128),
            pcm_samples: SliceDeque::new(),
        }),
    )
}

fn main() {
    let (mic, playback) = create_pipeline();
    let event_loop = create_minimal_event_loop(mic, playback);
    event_loop.run();
}
