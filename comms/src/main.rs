use std::sync::mpsc;
use std::{panic, process};

use crate::audio::ipc::AudioHandle;
use crate::audio::{AudioPlatform, AudioPlatformImpl};
use crate::network::ipc::NetworkHandle;
use crate::network::{NetworkError, SendToNetworkThreadError, Worker};

mod audio;
mod network;
mod ui;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() {
    #[cfg(not(debug_assertions))]
    let _guard = {
        use std::borrow::Cow;
        sentry::init((
            "https://acf304e45177402c94af33eecfc31ded@o1316575.ingest.sentry.io/6569253",
            sentry::ClientOptions {
                release: std::option_env!("CI_COMMIT_SHA")
                    .map(|sha| Cow::from(format!("commit-{}", sha))),
                send_default_pii: true,
                environment: Some(Cow::from("production")),
                ..Default::default()
            },
        ))
    };

    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));

    // Audio <> UI comm channel
    let (ui_handle, ui_receiver) = mpsc::channel();
    // Audio <> Network comm channel
    let (audio_registration, audio_notify) = AudioPlatformImpl::create_channel();
    let (audio_sender, audio_receiver) = mpsc::channel();
    let audio_handle = AudioHandle {
        notify: audio_notify,
        command_tx: audio_sender,
    };
    // UI <> Network comm channel
    let (network_handle, network_event_receiver, network_event_sender, network_worker_init) =
        { NetworkHandle::new_handle_and_worker(audio_handle.clone()) };

    std::thread::spawn(move || {
        audio_device::priority::bump_priority().unwrap();
        if let Err(e) = Worker::new()
            .map_err(|e| NetworkError::Io(Some(e)))
            .and_then(|w| w.run(network_worker_init))
        {
            eprintln!("Encountered an error in network operations, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    });

    {
        let parameters = AudioPlatformImpl::create_parameters();
        let network_handle = network_handle.clone();
        std::thread::spawn(move || {
            audio_device::priority::bump_priority().unwrap();
            if let Err(e) = audio::run(
                parameters,
                network_handle,
                ui_handle,
                audio_registration,
                audio_receiver,
            ) {
                eprintln!("Encountered an error in audio operations, stopping");
                eprintln!("Error details: {:?}", e);
                process::exit(1);
            }
        });
    }

    ui::run(
        network_handle,
        network_event_receiver,
        network_event_sender,
        ui_receiver,
        audio_handle,
    );
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
