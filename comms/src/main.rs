#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::sync::mpsc;
use std::sync::Arc;
use std::{panic, process};

use sentry::Hub;

use crate::audio::ipc::AudioHandle;
use crate::audio::{AudioPlatform, AudioPlatformImpl};
use crate::network::ipc::NetworkHandle;
use crate::network::{NetworkError, Worker};
use crate::ui::comms_version;

mod audio;
mod network;
mod ui;

pub static RESOURCES_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/resources/embedded");

fn main() {
    let sentry = option_env!("SENTRY_DSN").map(|sentry_dsn| {
        (
            sentry::init((
                sentry_dsn,
                sentry::ClientOptions {
                    release: Some(comms_version()),
                    send_default_pii: true,
                    environment: Some(Cow::from("production")),
                    ..Default::default()
                },
            )),
            Hub::current(),
        )
    });

    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));

    // Audio <> UI comm channel
    let (ui_handle, ui_receiver) = async_channel::unbounded();
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

    {
        let sentry_hub = sentry.as_ref().map(|s| s.1.clone());
        std::thread::spawn(move || {
            let closure = move || {
                audio_device::priority::bump_priority().unwrap();
                let worker = Worker::new()
                    .map_err(|e| NetworkError::Io(Some(e)))
                    .unwrap();

                let error = worker.run(network_worker_init);
                eprintln!("Encountered an error in network operations, stopping");
                panic!("{error:#?}");
            };

            if let Some(sentry_hub) = sentry_hub {
                let thread_hub = Arc::new(Hub::new_from_top(&sentry_hub));
                Hub::run(thread_hub, closure);
            } else {
                closure();
            }
        });
    }

    {
        AudioPlatformImpl::create_parameters();
        let network_handle = network_handle.clone();
        std::thread::spawn(move || {
            let closure = move || {
                audio_device::priority::bump_priority().unwrap();
                let error = audio::run(
                    (),
                    network_handle,
                    ui_handle,
                    audio_registration,
                    audio_receiver,
                );
                if matches!(error, audio::AudioError::InitializationError) {
                    // Keep the window alive so the frontend can display the device error.
                    return;
                }
                eprintln!("Encountered an error in audio operations, stopping");
                panic!("{error:#?}");
            };

            if let Some((_, sentry_hub)) = sentry {
                let thread_hub = Arc::new(Hub::new_from_top(&sentry_hub));
                Hub::run(thread_hub, closure);
            } else {
                closure();
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
