use crate::network::ipc::NetworkHandle;
use audio_device::buffer::{MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use audio_device::noise_gate::GateNoiseFilter;
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels};
use comms_lib::packet::ClientId;
use comms_lib::OpusAudioFrame;
use dasp::Sample;
use glib::bitflags::_core::cell::RefCell;
use slice_deque::SliceDeque;
use std::collections::HashMap;
use std::rc::Rc;

struct ClientQueue {
    decoder: Decoder,
    sample_buffer: SliceDeque<f32>,
    current_volume: f32,
}

impl ClientQueue {
    fn new() -> Result<Self, audiopus::Error> {
        Ok(Self {
            decoder: Decoder::new(audiopus::SampleRate::Hz48000, Channels::Mono)?,
            // The capacity chosen here is a spit-ball number. The host usually asks for 400 or
            // 800 samples at a time, so it's unlikely to back up this much unless a burst of
            // audio arrives.
            sample_buffer: SliceDeque::with_capacity(4096),
            current_volume: 1.0,
        })
    }

    fn next_sample(&mut self) -> Result<f32, audiopus::Error> {
        let sample = match self.sample_buffer.pop_front() {
            Some(sample) => sample,
            None => {
                let mut decode_buffer = [0f32; 480];
                let size = self.decoder.decode_float(
                    None as Option<&[u8]>,
                    &mut decode_buffer as &mut [f32],
                    false,
                )?;
                self.sample_buffer
                    .extend_from_slice(&decode_buffer[1..size]);
                decode_buffer[0]
            }
        };
        Ok(sample * self.current_volume)
    }
}

pub struct Multiplexer {
    client_queues: HashMap<ClientId, ClientQueue>,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            client_queues: HashMap::new(),
        }
    }

    pub fn new_client(&mut self, id: ClientId) -> Result<(), audiopus::Error> {
        let client_queue = ClientQueue::new()?;
        self.client_queues.insert(id, client_queue);
        Ok(())
    }

    pub fn drop_client(&mut self, id: ClientId) {
        self.client_queues.remove(&id);
    }

    pub fn append_chunk_from_client(
        &mut self,
        id: ClientId,
        data: OpusAudioFrame,
    ) -> Result<(), audiopus::Error> {
        let queue = match self.client_queues.get_mut(&id) {
            Some(queue) => queue,
            None => return Ok(()),
        };

        let mut decode_buffer = [0f32; 480];
        let size =
            queue
                .decoder
                .decode_float(Some(&data), &mut decode_buffer as &mut [f32], false)?;
        queue
            .sample_buffer
            .extend_from_slice(&decode_buffer[..size]);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.client_queues.clear();
    }

    pub fn change_volume(&mut self, id: ClientId, volume: f32) {
        if let Some(client) = self.client_queues.get_mut(&id) {
            client.current_volume = volume;
        }
    }
}

impl PlaybackSource for Multiplexer {
    fn next_sample(&mut self) -> f32 {
        let init: f32 = Sample::EQUILIBRIUM;
        let init = init.to_signed_sample();

        self.client_queues.values_mut().fold(init, |acc, queue| {
            let next_sample = queue
                .next_sample()
                .expect("Unsure how to handle opus decode error yet");
            let acc = acc.add_amp(next_sample);
            if acc == f32::INFINITY {
                f32::MAX
            } else {
                acc
            }
        })
    }
}

#[derive(Clone)]
pub struct RcMultiplexer(Rc<RefCell<Multiplexer>>);

impl RcMultiplexer {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(Multiplexer::new())))
    }
}

impl MaybeSharedPlaybackSource<Multiplexer> for RcMultiplexer {
    fn with(&mut self, cb: impl FnOnce(&mut Multiplexer)) {
        let mut multiplexer = self.0.borrow_mut();
        cb(&mut multiplexer);
    }
}

pub struct MicBuffer {
    network_handle: NetworkHandle,
    sample_buffer: SliceDeque<f32>,
    encoder: Encoder,
    noise_gate: GateNoiseFilter,
}

impl MicBuffer {
    pub fn new(network_handle: NetworkHandle) -> Result<Self, audiopus::Error> {
        Ok(Self {
            network_handle,
            sample_buffer: SliceDeque::with_capacity(4096),
            encoder: Encoder::new(
                audiopus::SampleRate::Hz48000,
                Channels::Mono,
                Application::LowDelay,
            )?,
            noise_gate: GateNoiseFilter::new(2048),
        })
    }
}

impl MicSink for MicBuffer {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.sample_buffer.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.sample_buffer.len() >= 480 {
            let next_samples = &mut self.sample_buffer[..480];
            self.noise_gate.update_window(next_samples);

            for sample in next_samples.iter_mut() {
                *sample = self.noise_gate.process(*sample);
            }

            if next_samples.iter().any(|s| *s != f32::EQUILIBRIUM) {
                let mut encode_buffer = [0u8; 480];
                let size = self
                    .encoder
                    .encode_float(next_samples, &mut encode_buffer)
                    .expect("Unsure how to handle opus encoding failure yet");
                self.network_handle
                    .audio(encode_buffer[..size].to_vec())
                    .unwrap();
            }
            unsafe { self.sample_buffer.move_head(480) };
        }
    }
}
