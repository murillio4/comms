use audio_device::audio_connection::EventLoop;
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use dasp::Sample;
use nnnoiseless::DenoiseState;
use slice_deque::SliceDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

/// Takeaways:
/// * Noise removal is amazing
/// * Audio quality has a significant decrease
/// * Voices become a little warbly/robot-y
/// * 10ms delay

pub struct Mic {
    sender: Sender<Vec<f32>>,
    pcm_samples: SliceDeque<f32>,
    denoise: Box<DenoiseState<'static>>,
}

impl MicSink for Mic {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.pcm_samples.len() > DenoiseState::FRAME_SIZE {
            let pcm_buffer = &mut self.pcm_samples.as_mut_slice()[..DenoiseState::FRAME_SIZE];
            for sample in pcm_buffer.iter_mut() {
                // actually typing.
                *sample *= 32767.0;
            }
            let mut noiseless_buffer = [0f32; DenoiseState::FRAME_SIZE]; // actually typing now
            self.denoise
                .process_frame(&mut noiseless_buffer, pcm_buffer);
            for sample in noiseless_buffer.iter_mut() {
                *sample /= 32767.0;
            }
            self.sender.send(noiseless_buffer.to_vec()).unwrap();
            unsafe { self.pcm_samples.move_head(480) };
        }
    }
}

pub struct Playback {
    receiver: Receiver<Vec<f32>>,
    pcm_samples: SliceDeque<f32>,
}

impl PlaybackSource for Playback {
    fn next_sample(&mut self) -> f32 {
        match self.pcm_samples.pop_front() {
            Some(sample) => sample,
            None => match self.receiver.try_recv() {
                Ok(pcm_samples) => {
                    self.pcm_samples.extend_from_slice(&pcm_samples);
                    self.pcm_samples.pop_front().unwrap()
                }
                Err(TryRecvError::Empty) => Sample::EQUILIBRIUM,
                Err(TryRecvError::Disconnected) => unimplemented!("channel failed"),
            },
        }
    }
}

pub fn create_pipeline() -> (Mic, UnsharedPlaybackSource<Playback>) {
    let (tx, rx) = channel();
    (
        Mic {
            sender: tx,
            pcm_samples: SliceDeque::with_capacity(4096),
            denoise: DenoiseState::new(),
        },
        UnsharedPlaybackSource(Playback {
            receiver: rx,
            pcm_samples: SliceDeque::with_capacity(4096),
        }),
    )
}

#[cfg(unix)]
fn create_event_loop(mic: Mic, playback: UnsharedPlaybackSource<Playback>) -> impl EventLoop {
    use audio_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
    let parameters = PulseParameters {
        pulseaudio_server: Some("/run/user/1000/pulse/native".to_string()),
    };
    PulseAudioConnection::create(parameters, mic, playback).create_event_loop()
}

#[cfg(windows)]
fn create_event_loop(mic: Mic, playback: UnsharedPlaybackSource<Playback>) -> impl EventLoop {
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
