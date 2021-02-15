use audio_device::audio_connection::DeviceInfo;

pub enum UICommand {
    AudioIPCReady,
    DevicesFound {
        input_devices: Vec<DeviceInfo>,
        output_devices: Vec<DeviceInfo>,
    },
}
