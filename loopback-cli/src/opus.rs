use audio_device::audio_connection::EventLoop;
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels, SampleRate};
use dasp::Sample;
use slice_deque::SliceDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

pub struct Mic {
    sender: Sender<Vec<u8>>,
    pcm_samples: SliceDeque<f32>,
    encoder: Encoder,
}

impl MicSink for Mic {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.pcm_samples.len() > 480 {
            let pcm_buffer = &self.pcm_samples.as_slice()[0..480];
            let mut opus_buffer = [0u8; 2048];
            let size = self
                .encoder
                .encode_float(pcm_buffer, &mut opus_buffer)
                .unwrap();
            let opus_bytes = opus_buffer[..size].to_vec();
            self.sender.send(opus_bytes).unwrap();
            unsafe { self.pcm_samples.move_head(480) };
        }
    }
}

pub struct Playback {
    receiver: Receiver<Vec<u8>>,
    pcm_samples: SliceDeque<f32>,
    decoder: Decoder,
}

impl PlaybackSource for Playback {
    fn next_sample(&mut self) -> f32 {
        match self.pcm_samples.pop_front() {
            Some(sample) => sample,
            None => match self.receiver.try_recv() {
                Ok(opus_bytes) => {
                    let mut pcm_buffer = [0f32; 2048];
                    let size = self
                        .decoder
                        .decode_float(Some(&opus_bytes), &mut pcm_buffer as &mut [f32], false)
                        .unwrap();
                    self.pcm_samples.extend_from_slice(&pcm_buffer[..size]);
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
            encoder: Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::LowDelay)
                .unwrap(),
        },
        UnsharedPlaybackSource(Playback {
            receiver: rx,
            pcm_samples: SliceDeque::with_capacity(4096),
            decoder: Decoder::new(SampleRate::Hz48000, Channels::Mono).unwrap(),
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
