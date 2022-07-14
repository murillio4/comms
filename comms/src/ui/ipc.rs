use crate::audio::ipc::EventLoopCreationError;
use audio_device::audio_connection::DeviceInfo;

pub enum AudioIPCInitStatus {
    Ready,
    Error(EventLoopCreationError),
}

pub enum UICommand {
    AudioIPCInitStatus(AudioIPCInitStatus),
    DevicesFound {
        input_devices: Vec<DeviceInfo>,
        output_devices: Vec<DeviceInfo>,
    },
}
