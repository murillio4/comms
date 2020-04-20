use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use ringbuf::{Consumer, Producer, RingBuffer};
use sample::Sample;

use voice_lib::packet::ClientId;
use winapi::um::winnt;

pub enum AudioCommand {
    NewClientStream(String, Consumer<i16>),
    SwitchInputDevice(String),
    SwitchOutputDevice(String)
//    RemoveClientStream(String),
}

pub struct AudioHandle {
    command_tx: Sender<AudioCommand>,
    client_streams: HashMap<ClientId, Producer<i16>>,

    #[cfg(windows)]
    winnt_handle: winnt::HANDLE
}

impl AudioHandle {
    #[cfg(windows)]
    pub fn new(command_tx: Sender<AudioCommand>, winnt_handle: winnt::HANDLE) -> AudioHandle {
        AudioHandle {
            command_tx,
            client_streams: HashMap::new(),
            winnt_handle
        }
    }
    #[cfg(unix)]
    pub fn new(command_tx: Sender<AudioCommand>) -> AudioHandle {
        AudioHandle {
            command_tx,
            client_streams: HashMap::new(),
        }
    }

    pub fn send(&mut self, id: ClientId, bytes: &[u8]) {
        let l16le_samples: Vec<i16> = bytes
            .chunks(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        match self.client_streams.get_mut(&id) {
            Some(producer) => {
                producer.push_slice(&l16le_samples);
            }
            None => {
                let ring = RingBuffer::new(4800);
                let (mut producer, consumer) = ring.split();
                self.command_tx.send(AudioCommand::NewClientStream(id.to_string(), consumer)).unwrap();
                producer.push_slice(&l16le_samples);
                self.client_streams.insert(id, producer);
            }
        };
    }

    pub fn change_input_device(&self, device_id: String) {
        self.command_tx.send(AudioCommand::SwitchInputDevice(device_id));
    }

    pub fn change_output_device(&self, device_id: String) {
        self.command_tx.send(AudioCommand::SwitchOutputDevice(device_id));
    }
}

pub struct ReceiveFromNetworkThread {
    command_rx: Receiver<AudioCommand>,
    client_streams: HashMap<String, Consumer<i16>>,
}

impl ReceiveFromNetworkThread {
    pub fn new(receiver: Receiver<AudioCommand>) -> ReceiveFromNetworkThread {
        ReceiveFromNetworkThread {
            command_rx: receiver,
            client_streams: HashMap::new(),
        }
    }

    pub(crate) fn handle_commands(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                AudioCommand::NewClientStream(id, consumer) => {
                    self.client_streams.insert(id, consumer);
                }
            }
        }
    }

    pub(crate) fn next_sample(&mut self) -> i16 {
        let init: i16 = sample::Sample::equilibrium();
        let init = init.to_signed_sample();
        self.client_streams.values_mut()
            .fold(init, |acc, consumer| {
                let next_sample = consumer.pop().unwrap_or(0);
                acc.checked_add(next_sample).unwrap_or(std::i16::MAX)
            })
    }
}