#[cfg(target_os = "windows")]
#[macro_use]
extern crate lazy_static;

use std::{process, env};
use std::sync::mpsc;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use crate::network::{Handle, SendToNetworkThreadError, Worker, NetworkError};

mod audio;
mod network;
mod ui;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() -> Result<(), CommsError> {
    let (audio_sender, audio_receiver) = mpsc::channel();
    let (network_handle, network_event_receiver, network_worker_init) = Handle::new_handle_and_worker(audio_sender);

    std::thread::spawn(move || {
        #[cfg(windows)]
        bump_priority();
        if let Err(e) = Worker::new()
            .map_err(|e| NetworkError::Io(Some(e)))
            .and_then(|w| w.run(network_worker_init)) {
            eprintln!("Encountered an error in network operations, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    });

    {
        let pulseaudio_server = env::var_os("PULSEAUDIO_SERVER").map(|s| s.to_string_lossy().to_string());
        let network_handle = network_handle.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            bump_priority();
            if let Err(e) = audio::run(pulseaudio_server, network_handle, audio_receiver) {
                eprintln!("Encountered an error in audio operations, stopping");
                eprintln!("Error details: {:?}", e);
                process::exit(1);
            }
        });
    }

    ui::run(network_handle, network_event_receiver);
    Ok(())
}

#[derive(Debug)]
enum CommsError {
    Audio(audio::AudioError),
    Network(network::NetworkError),
}

impl From<audio::AudioError> for CommsError {
    fn from(e: audio::AudioError) -> Self {
        CommsError::Audio(e)
    }
}

impl From<network::NetworkError> for CommsError {
    fn from(e: network::NetworkError) -> Self {
        CommsError::Network(e)
    }
}

impl From<SendToNetworkThreadError> for CommsError {
    fn from(e: SendToNetworkThreadError) -> Self {
        CommsError::Audio(e.into())
    }
}