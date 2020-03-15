#[derive(Clone)]
pub struct UIDevice {
    pub label: String,
    pub id: String
}

pub enum UICommand {
    DevicesFound {
        input_devices: Vec<UIDevice>,
        output_devices: Vec<UIDevice>
    }
}