#[derive(Clone)]
pub struct DeviceInfo {
    pub label: String,
    pub id: String,
}

pub struct FoundDevices {
    pub input_devices: Vec<DeviceInfo>,
    pub output_devices: Vec<DeviceInfo>,
}

pub trait AudioConnection {
    fn list_devices(&self, callback: impl FnOnce(FoundDevices) + 'static);
    fn change_input_device(device_id: String);
    fn change_output_device(device_id: String);
}

pub trait EventLoop {
    type Registration;
    type Notify: EventLoopNotify;
    type AudioConnection: AudioConnection;

    fn run(self);
}

pub trait EventLoopNotify {
    fn set_ready(&self);
}
