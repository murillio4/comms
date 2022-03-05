use audio_device::audio_connection::{create_minimal_event_loop, EventLoop};
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels, SampleRate};
use slice_deque::SliceDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::Instant;

pub struct Mic {
    sender: Sender<Vec<u8>>,
    pcm_samples: SliceDeque<f32>,
    encoder: Encoder,
    time_since_press: Option<Instant>,
    reciever_for_time_since_user_input: Receiver<Instant>,
}

impl Mic {
    fn user_has_pressed(&self) -> bool {
        if let Some(instant_since_press) = &self.time_since_press {
            if instant_since_press.elapsed().as_secs() < 3 {
                return true;
            }
        }
        false
    }
}

impl MicSink for Mic {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.pcm_samples.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        for enter_press in self.reciever_for_time_since_user_input.try_iter() {
            self.time_since_press = Some(enter_press);
        }

        while self.pcm_samples.len() > 480 {
            let pcm_buffer = &self.pcm_samples.as_slice()[0..480];
            let mut opus_buffer = [0u8; 2048];
            let size = self
                .encoder
                .encode_float(pcm_buffer, &mut opus_buffer)
                .unwrap();
            let opus_bytes = opus_buffer[..size].to_vec();
            if !self.user_has_pressed() {
                self.sender.send(opus_bytes).unwrap();
            }
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
                Err(TryRecvError::Empty) => {
                    let mut pcm_buffer = [0f32; 480];

                    let size = self
                        .decoder
                        .decode_float(None as Option<&[u8]>, &mut pcm_buffer as &mut [f32], false)
                        .unwrap();
                    self.pcm_samples.extend_from_slice(&pcm_buffer[..size]);
                    self.pcm_samples.pop_front().unwrap()
                }
                Err(TryRecvError::Disconnected) => unimplemented!("channel failed"),
            },
        }
    }
}

pub fn create_pipeline(recv: Receiver<Instant>) -> (Mic, UnsharedPlaybackSource<Playback>) {
    let (tx, rx) = channel();
    (
        Mic {
            sender: tx,
            pcm_samples: SliceDeque::with_capacity(4096),
            encoder: Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::LowDelay)
                .unwrap(),
            time_since_press: None,
            reciever_for_time_since_user_input: recv,
        },
        UnsharedPlaybackSource(Playback {
            receiver: rx,
            pcm_samples: SliceDeque::with_capacity(4096),
            decoder: Decoder::new(SampleRate::Hz48000, Channels::Mono).unwrap(),
        }),
    )
}

fn main() {
    let (send, recv) = channel();

    let (mic, playback) = create_pipeline(recv);
    let event_loop = create_minimal_event_loop(mic, playback);

    std::thread::spawn(move || loop {
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            send.send(Instant::now()).unwrap();
        }
    });

    event_loop.run();
}
