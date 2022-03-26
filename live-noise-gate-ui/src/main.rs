use std::sync::mpsc::channel;

mod audio;
mod ui;

fn main() {
    let (audio_tx, audio_rx) = channel();
    let (command_tx, command_rx) = channel();
    std::thread::spawn(|| {
        audio::run(command_rx, audio_tx);
    });
    ui::run(command_tx, audio_rx);
}
