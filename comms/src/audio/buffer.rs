use crate::network::ipc::{NetworkCommand, NetworkHandle, Protocol};
use audio_device::buffer::{MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use audio_device::noise_gate::GateNoiseFilter;
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels};
use comms_lib::packet::{AudioIndex, ClientId, P2pEstablished};
use comms_lib::serialization::OutgoingPacketFrame;
use comms_lib::OpusAudioFrame;
use dasp::Sample;
use glib::bitflags::_core::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const BUFFER_SIZE: usize = 4800;

struct ClientQueue {
    decoder: Decoder,
    sample_buffer: [f32; BUFFER_SIZE],
    sample_buffer_start: usize,
    sample_buffer_end: usize,
    current_volume: f32,
    current_index: AudioIndex,
    is_peer_to_peer: bool,
}

impl ClientQueue {
    fn new() -> Result<Self, audiopus::Error> {
        Ok(Self {
            decoder: Decoder::new(audiopus::SampleRate::Hz48000, Channels::Mono)?,
            // The capacity chosen here is a spit-ball number. The host usually asks for 400 or
            // 800 samples at a time, so it's unlikely to back up this much unless a burst of
            // audio arrives.
            sample_buffer: [0f32; BUFFER_SIZE],
            sample_buffer_start: 0,
            sample_buffer_end: 0,
            current_volume: 1.0,
            current_index: AudioIndex::new(1).unwrap(),
            is_peer_to_peer: false,
        })
    }

    fn next_sample(&mut self) -> Result<f32, audiopus::Error> {
        let sample = if self.sample_buffer_end != self.sample_buffer_start {
            let sample = self.sample_buffer[self.sample_buffer_start];
            self.sample_buffer_start = (self.sample_buffer_start + 1) % BUFFER_SIZE;
            sample
        } else {
            let mut decode_buffer = [0f32; 480];
            let size = self.decoder.decode_float(
                None as Option<&[u8]>,
                &mut decode_buffer as &mut [f32],
                false,
            )?;

            for sample in &decode_buffer[1..size] {
                self.sample_buffer[self.sample_buffer_end] = *sample;
                self.sample_buffer_end = (self.sample_buffer_end + 1) % BUFFER_SIZE;
            }

            decode_buffer[0]
        };

        Ok(sample * self.current_volume)
    }
}

pub struct Multiplexer {
    client_queues: HashMap<ClientId, ClientQueue>,
    main_volume: f32,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            client_queues: HashMap::new(),
            main_volume: 1.0,
        }
    }

    pub fn new_client(&mut self, id: ClientId) -> Result<(), audiopus::Error> {
        self.client_queues.insert(id, ClientQueue::new()?);
        Ok(())
    }

    pub fn drop_client(&mut self, id: ClientId) {
        self.client_queues.remove(&id);
    }

    pub fn append_chunk_from_client(
        &mut self,
        id: ClientId,
        data: OpusAudioFrame,
        is_peer_to_peer: bool,
        network_handle: &NetworkHandle,
    ) -> Result<(), audiopus::Error> {
        let queue = match self.client_queues.get_mut(&id) {
            Some(queue) => queue,
            None => return Ok(()),
        };

        if data.index < queue.current_index {
            // This audio data is older than data we've already received, so throw it out
            return Ok(());
        }

        let mut decode_buffer = [0f32; 480];
        let size = queue.decoder.decode_float(
            Some(&data.bytes),
            &mut decode_buffer as &mut [f32],
            false,
        )?;

        for sample in &decode_buffer[..size] {
            queue.sample_buffer[queue.sample_buffer_end] = *sample;
            queue.sample_buffer_end = (queue.sample_buffer_end + 1) % BUFFER_SIZE;
        }

        queue.current_index = data.index;

        if !queue.is_peer_to_peer && is_peer_to_peer {
            queue.is_peer_to_peer = true;

            network_handle.command(NetworkCommand::Packet {
                protocol: Protocol::Udp,
                packet: OutgoingPacketFrame::from_packet(
                    None,
                    &P2pEstablished {
                        source_client_id: id,
                    },
                )
                .unwrap(),
                send_response: None,
            })
        }

        Ok(())
    }

    pub fn clear(&mut self) {
        self.client_queues.clear();
    }

    pub fn change_main_volume(&mut self, volume: f32) {
        self.main_volume = volume;
    }

    pub fn change_client_volume(&mut self, id: ClientId, volume: f32) {
        if let Some(client) = self.client_queues.get_mut(&id) {
            client.current_volume = volume;
        }
    }
}

impl PlaybackSource for Multiplexer {
    fn next_sample(&mut self) -> f32 {
        let init: f32 = Sample::EQUILIBRIUM;
        let init = init.to_signed_sample();

        let sample = self.client_queues.values_mut().fold(init, |acc, queue| {
            let next_sample = queue
                .next_sample()
                .expect("Unsure how to handle opus decode error yet");
            let acc = acc.add_amp(next_sample);
            if acc == f32::INFINITY {
                f32::MAX
            } else {
                acc
            }
        });
        sample * self.main_volume
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
    sample_buffer: [f32; BUFFER_SIZE],
    sample_buffer_start: usize,
    sample_buffer_end: usize,
    encoder: Encoder,
    noise_gate: GateNoiseFilter,
    index: u64,
}

impl MicBuffer {
    pub fn new(network_handle: NetworkHandle) -> Result<Self, audiopus::Error> {
        Ok(Self {
            network_handle,
            sample_buffer: [0f32; BUFFER_SIZE],
            sample_buffer_start: 0,
            sample_buffer_end: 0,
            encoder: Encoder::new(
                audiopus::SampleRate::Hz48000,
                Channels::Mono,
                Application::LowDelay,
            )?,
            noise_gate: GateNoiseFilter::new(2048),
            index: 1,
        })
    }
}

impl MicSink for MicBuffer {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.sample_buffer[self.sample_buffer_end] = sample;
        self.sample_buffer_end = (self.sample_buffer_end + 1) % BUFFER_SIZE;
    }

    fn publish_chunks(&mut self) {
        while self.sample_buffer_end < self.sample_buffer_start
            || self.sample_buffer_end - self.sample_buffer_start >= 480
        {
            let next_samples =
                &mut self.sample_buffer[self.sample_buffer_start..self.sample_buffer_start + 480];
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

                let index = AudioIndex::new(self.index).unwrap();
                self.network_handle
                    .audio(OpusAudioFrame {
                        index,
                        bytes: encode_buffer[..size].to_vec(),
                    })
                    .unwrap();
                self.index += 1;
            }

            self.sample_buffer_start = (self.sample_buffer_start + 480) % BUFFER_SIZE;
        }
    }
}
