use crate::ui::IncomingAudioPlotData;
use audio_device::audio_connection::{create_minimal_event_loop, EventLoop};
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use audio_device::noise_gate::{GateNoiseFilter, State};
use dasp::Sample;
use ringbuffer::RingBufferExt;
use slice_deque::SliceDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

pub const SAMPLES_IN_CHUNK: usize = 480;
pub const NOISE_GATE_CHUNK_LEN: usize = 128;

pub struct MicForLoopback {
    playback_sender: Sender<Vec<f32>>,
    audio_plot_sender: Sender<IncomingAudioPlotData>,
    pcm_samples: SliceDeque<f32>,
}

impl MicSink for MicForLoopback {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.pcm_samples.len() > SAMPLES_IN_CHUNK {
            let buffer = self.pcm_samples.as_slice()[..SAMPLES_IN_CHUNK].to_vec();
            self.playback_sender.send(buffer.clone()).unwrap();
            self.audio_plot_sender
                .send(IncomingAudioPlotData::SampleChunk(buffer))
                .unwrap();
            unsafe { self.pcm_samples.move_head(SAMPLES_IN_CHUNK as isize) };
        }
    }
}

pub struct PlaybackForLoopback {
    receiver: Receiver<Vec<f32>>,
    audio_plot_sender: Sender<IncomingAudioPlotData>,
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
                let samples_f32: Vec<f32> = samples.iter().map(|s| s.to_sample::<f32>()).collect();
                self.gate.update_window(&samples_f32);
                self.audio_plot_sender
                    .send(IncomingAudioPlotData::ChunkRms(
                        *self.gate.recent_rms.back().unwrap(),
                    ))
                    .unwrap();
                self.audio_plot_sender
                    .send(IncomingAudioPlotData::StateChange(match self.gate.state {
                        State::Off => 0,
                        State::Attack(_) => 1,
                        State::Active => 2,
                        State::Hold(_) => 3,
                        State::Release(_) => 4,
                    }))
                    .unwrap();
                self.audio_plot_sender
                    .send(IncomingAudioPlotData::NoiseGateThreshold(
                        self.gate.threshold as f32,
                    ))
                    .unwrap();
                self.pcm_samples.extend_from_slice(&samples[1..]);
                samples[0]
            }
        };

        self.gate.process(sample.to_sample()).to_sample()
    }
}

pub fn create_pipeline(
    audio_plot_sender: Sender<IncomingAudioPlotData>,
) -> (MicForLoopback, UnsharedPlaybackSource<PlaybackForLoopback>) {
    let (tx, rx) = channel();
    (
        MicForLoopback {
            playback_sender: tx,
            audio_plot_sender: audio_plot_sender.clone(),
            pcm_samples: SliceDeque::new(),
        },
        UnsharedPlaybackSource(PlaybackForLoopback {
            receiver: rx,
            audio_plot_sender,
            pcm_samples: SliceDeque::new(),
            gate: GateNoiseFilter::new(NOISE_GATE_CHUNK_LEN),
        }),
    )
}

pub fn run(audio_plot_sender: Sender<IncomingAudioPlotData>) {
    let (mic, playback) = create_pipeline(audio_plot_sender);
    let event_loop = create_minimal_event_loop(mic, playback);
    event_loop.run();
}
