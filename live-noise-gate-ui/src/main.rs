use std::sync::mpsc::channel;

mod audio;
mod ui;

fn main() {
    let (audio_tx, audio_rx) = channel();
    std::thread::spawn(|| {
        audio::run(audio_tx);
    });
    ui::run(audio_rx);
}
