use crate::network::ipc::{NetworkCommand, NetworkHandle, Protocol};
use crate::ui::ipc::UICommand;
use audio_device::buffer::{MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use audio_device::noise_gate::{ChangedState, GateNoiseFilter};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels};
use comms_lib::packet::{AudioIndex, ClientId, P2pEstablished};
use comms_lib::serialization::OutgoingPacketFrame;
use comms_lib::OpusAudioFrame;
use dasp::Sample;
use glib::bitflags::_core::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

const BUFFER_SIZE: usize = 4800;

struct ClientQueue {
    decoder: Decoder,
    sample_buffer_offset: usize,
    current_volume: f32,
    current_index: AudioIndex,
    is_peer_to_peer: bool,
}

impl ClientQueue {
    fn new() -> Result<Self, audiopus::Error> {
        Ok(Self {
            decoder: Decoder::new(audiopus::SampleRate::Hz48000, Channels::Mono)?,
            sample_buffer_offset: 0,
            current_volume: 1.0,
            current_index: AudioIndex::new(1).unwrap(),
            is_peer_to_peer: false,
        })
    }
}

pub struct Multiplexer {
    client_queues: HashMap<ClientId, ClientQueue>,
    main_volume: f32,
    // The capacity chosen here is a spit-ball number. The host usually asks for 400 or
    // 800 samples at a time, so it's unlikely to back up this much unless a burst of
    // audio arrives.
    sample_buffer: Box<[f32; BUFFER_SIZE]>,
    sample_buffer_start: usize,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            client_queues: HashMap::new(),
            main_volume: 1.0,
            sample_buffer: Box::new([Sample::EQUILIBRIUM; BUFFER_SIZE]),
            sample_buffer_start: 0,
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
        ui_handle: &mut async_channel::Sender<UICommand>,
    ) -> Result<(), audiopus::Error> {
        let queue = match self.client_queues.get_mut(&id) {
            Some(queue) => queue,
            None => return Ok(()),
        };

        if data.index < queue.current_index {
            // This audio data is older than data we've already received, so throw it out
            return Ok(());
        }
        queue.current_index = data.index;

        // If we've received data faster than we can play it (e.g. due to a sudden burst
        // of packets), then throw it away without processing it through opus.
        // (By avoiding processing it through opus, we allow opus to "bridge the wave" for the
        // next packet that _can_ fit).
        if queue.sample_buffer_offset + 480 < BUFFER_SIZE {
            let mut decode_buffer = [0f32; 480];
            let size = queue.decoder.decode_float(
                Some(&data.bytes),
                &mut decode_buffer as &mut [f32],
                false,
            )?;

            ui_handle
                .try_send(UICommand::UserIsSpeaking {
                    client_id: id,
                    duration: Duration::from_millis((size as f64 / 48.0).round() as u64),
                })
                .unwrap();

            for new_sample in &decode_buffer[..size] {
                let sample_buffer_index =
                    (self.sample_buffer_start + queue.sample_buffer_offset) % BUFFER_SIZE;
                let existing_sample = self.sample_buffer[sample_buffer_index];
                let sample =
                    existing_sample.add_amp(*new_sample * self.main_volume * queue.current_volume);
                self.sample_buffer[sample_buffer_index] = if sample == f32::INFINITY {
                    f32::MAX
                } else {
                    sample
                };
                queue.sample_buffer_offset += 1;
            }

            debug_assert!(queue.sample_buffer_offset <= BUFFER_SIZE);
        }

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
    fn next_samples(&mut self, count: usize, callback: impl FnOnce((&[f32], Option<&[f32]>))) {
        let end_index = (self.sample_buffer_start + count) % BUFFER_SIZE;

        for queue in self.client_queues.values_mut() {
            if queue.sample_buffer_offset < count {
                let mut decode_buffer = [0f32; 480];
                let decoded_sample_count = queue
                    .decoder
                    .decode_float(None::<&Vec<u8>>, &mut decode_buffer as &mut [f32], false)
                    .expect("Failed to produce generated opus audio");

                for new_sample in &decode_buffer[..decoded_sample_count] {
                    let sample_buffer_index =
                        (self.sample_buffer_start + queue.sample_buffer_offset) % BUFFER_SIZE;
                    let existing_sample = self.sample_buffer[sample_buffer_index];
                    let sample = existing_sample
                        .add_amp(*new_sample * self.main_volume * queue.current_volume);
                    self.sample_buffer[sample_buffer_index] = if sample == f32::INFINITY {
                        f32::MAX
                    } else {
                        sample
                    };
                    queue.sample_buffer_offset += 1;
                }
                // temporary: identify if this is the cause of intermittent droppage when chatting
                // with Caius. Turn into debug_assert after testing.
                assert!(queue.sample_buffer_offset <= BUFFER_SIZE);
            }
            queue.sample_buffer_offset = queue.sample_buffer_offset.saturating_sub(count);
        }

        if end_index >= self.sample_buffer_start {
            let windows = (
                &self.sample_buffer[self.sample_buffer_start..end_index],
                None,
            );
            callback(windows);
            for i in self.sample_buffer_start..end_index {
                self.sample_buffer[i] = Sample::EQUILIBRIUM;
            }
        } else {
            let windows = (
                &self.sample_buffer[self.sample_buffer_start..],
                Some(&self.sample_buffer[..end_index]),
            );
            callback(windows);
            for i in self.sample_buffer_start..BUFFER_SIZE {
                self.sample_buffer[i] = Sample::EQUILIBRIUM;
            }
            for i in 0..end_index {
                self.sample_buffer[i] = Sample::EQUILIBRIUM;
            }
        }
        self.sample_buffer_start = end_index;
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
    fn with<T>(&mut self, cb: impl FnOnce(&mut Multiplexer) -> T) -> T {
        let mut multiplexer = self.0.borrow_mut();
        cb(&mut multiplexer)
    }
}

pub struct MicBuffer {
    network_handle: NetworkHandle,
    ui_handle: async_channel::Sender<UICommand>,
    sample_buffer: [f32; BUFFER_SIZE],
    sample_buffer_start: usize,
    sample_buffer_end: usize,
    encoder: Encoder,
    noise_gate: GateNoiseFilter,
    index: u64,
}

impl MicBuffer {
    pub fn new(
        network_handle: NetworkHandle,
        ui_handle: async_channel::Sender<UICommand>,
    ) -> Result<Self, audiopus::Error> {
        Ok(Self {
            network_handle,
            ui_handle,
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
            let state_change = self.noise_gate.update_window(next_samples);
            match state_change {
                ChangedState::OPEN => self
                    .ui_handle
                    .try_send(UICommand::CurrentUserStartedSpeaking)
                    .unwrap(),
                ChangedState::CLOSED => self
                    .ui_handle
                    .try_send(UICommand::CurrentUserStoppedSpeaking)
                    .unwrap(),
                _ => (),
            };

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
