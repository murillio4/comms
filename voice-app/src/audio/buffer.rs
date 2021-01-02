use crate::network::ipc::NetworkHandle;
use dasp::Sample;
use glib::bitflags::_core::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use voice_device::buffer::{MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use voice_lib::packet::ClientId;
use voice_lib::AudioFrame;

pub struct Multiplexer {
    client_queues: HashMap<ClientId, VecDeque<f32>>,
}

impl Multiplexer {
    pub fn new() -> Self {
        Multiplexer {
            client_queues: HashMap::new(),
        }
    }

    pub fn new_client(&mut self, id: ClientId) {
        self.client_queues.insert(id, VecDeque::new());
    }

    pub fn append_chunk_from_client(&mut self, id: ClientId, data: Box<AudioFrame>) {
        let queue = match self.client_queues.get_mut(&id) {
            Some(queue) => queue,
            None => return,
        };

        for sample in data.iter() {
            queue.push_back(*sample);
        }
    }
}

impl PlaybackSource for Multiplexer {
    fn next_sample(&mut self) -> f32 {
        let init: f32 = Sample::EQUILIBRIUM;
        let init = init.to_signed_sample();
        self.client_queues.values_mut().fold(init, |acc, queue| {
            let next_sample = queue.pop_front().unwrap_or(Sample::EQUILIBRIUM);
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
    queue: VecDeque<f32>,
}

impl MicBuffer {
    pub fn new(network_handle: NetworkHandle) -> Self {
        Self {
            network_handle,
            queue: VecDeque::new(),
        }
    }
}

impl MicSink for MicBuffer {
    fn append_sample_from_mic(&mut self, sample: f32) {
        self.queue.push_back(sample);
    }

    fn publish_chunks(&mut self) {
        while self.queue.len() >= 480 {
            let mut chunk = Box::new([0f32; 480]);
            for i in 0..chunk.len() {
                chunk[i] = self
                    .queue
                    .pop_front()
                    .expect("Queue should have 480 samples");
            }
            self.network_handle.audio(chunk).unwrap();
        }
    }
}
