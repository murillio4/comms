use std::sync::mpsc::{Receiver, SendError, Sender};

use crate::ui::ipc::UICommand;
use std::fmt;
use std::fmt::{Display, Formatter};
use voice_lib::packet::ClientId;

use crate::audio::buffer::RcMultiplexer;
use crate::audio::{AudioPlatform, AudioPlatformImpl};
use voice_device::audio_connection::{AudioConnection, EventLoopNotify};
use voice_device::buffer::{InterruptReceiver, MaybeSharedPlaybackSource};
use voice_lib::AudioFrame;

pub enum AudioCommand {
    NewClient(ClientId),
    AudioFromClient(ClientId, Box<AudioFrame>),
    #[allow(dead_code)]
    SwitchInputDevice(String),
    #[allow(dead_code)]
    SwitchOutputDevice(String),
    ListDevices,
    // TODO remove client
}

impl Display for AudioCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match *self {
            AudioCommand::NewClient(_) => f.write_str("NewClient"),
            AudioCommand::AudioFromClient(..) => f.write_str("AudioFromClient"),
            AudioCommand::SwitchInputDevice(_) => f.write_str("SwitchInputDevice"),
            AudioCommand::SwitchOutputDevice(_) => f.write_str("SwitchOutputDevice"),
            AudioCommand::ListDevices => f.write_str("ListDevices"),
        }
    }
}

#[derive(Clone)]
pub struct AudioHandle {
    pub command_tx: Sender<AudioCommand>,
    pub notify: <AudioPlatformImpl as AudioPlatform>::Notify,
}

impl AudioHandle {
    pub fn send_audio(&mut self, id: ClientId, bytes: &[u8]) {
        let mut samples = Box::new([0f32; 480]);
        for i in 0..samples.len() {
            samples[i] = f32::from_le_bytes([
                bytes[i * 4],
                bytes[i * 4 + 1],
                bytes[i * 4 + 2],
                bytes[i * 4 + 3],
            ]);
        }
        self.send(AudioCommand::AudioFromClient(id, samples))
            .unwrap();
    }

    pub fn send(&self, command: AudioCommand) -> Result<(), SendError<AudioCommand>> {
        let result = self.command_tx.send(command);
        self.notify.set_ready();
        result
    }
}

pub struct AudioReceiver<R> {
    pub(crate) multiplexer: RcMultiplexer,
    pub(crate) registration: R,
    pub(crate) command_rx: Receiver<AudioCommand>,
    pub(crate) ui_handle: Sender<UICommand>,
}

impl<R> AudioReceiver<R> {
    pub fn new(
        multiplexer: RcMultiplexer,
        registration: R,
        command_rx: Receiver<AudioCommand>,
        ui_handle: Sender<UICommand>,
    ) -> Self {
        Self {
            multiplexer,
            registration,
            command_rx,
            ui_handle,
        }
    }
}

impl<R> InterruptReceiver<R> for AudioReceiver<R> {
    fn registration(&self) -> &R {
        &self.registration
    }

    fn invoke<AC: AudioConnection>(&mut self, connection: &AC) {
        for command in self.command_rx.try_iter() {
            let command: AudioCommand = command;
            match command {
                AudioCommand::NewClient(id) => self
                    .multiplexer
                    .with(|multiplexer| multiplexer.new_client(id)),
                AudioCommand::AudioFromClient(client_id, chunk) => self
                    .multiplexer
                    .with(|multiplexer| multiplexer.append_chunk_from_client(client_id, chunk)),
                AudioCommand::SwitchInputDevice(device_id) => AC::change_input_device(device_id),
                AudioCommand::SwitchOutputDevice(device_id) => AC::change_output_device(device_id),
                AudioCommand::ListDevices => {
                    let ui_handle = self.ui_handle.clone();
                    connection.list_devices(move |devices| {
                        ui_handle
                            .send(UICommand::DevicesFound {
                                input_devices: devices.input_devices,
                                output_devices: devices.output_devices,
                            })
                            .unwrap();
                    });
                }
            }
        }
    }
}
