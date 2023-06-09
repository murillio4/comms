#[cfg(not(debug_assertions))]
use sentry::Hub;
use std::sync::mpsc;
#[cfg(not(debug_assertions))]
use std::sync::Arc;
use std::{panic, process};

#[cfg(windows)]
use windows::w;
#[cfg(windows)]
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

use crate::audio::ipc::AudioHandle;
use crate::audio::{AudioPlatform, AudioPlatformImpl};
use crate::network::ipc::NetworkHandle;
use crate::network::{NetworkError, SendToNetworkThreadError, Worker};

mod audio;
mod network;
#[cfg(windows)]
mod register_protocol;
mod ui;
mod warp_wrapper_path;

pub static RESOURCES_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/resources/embedded");

fn main() {
    #[cfg(not(debug_assertions))]
    let (_guard, sentry_hub) = {
        use crate::ui::comms_version;
        use std::borrow::Cow;
        (
            sentry::init((
                "https://acf304e45177402c94af33eecfc31ded@o1316575.ingest.sentry.io/6569253",
                sentry::ClientOptions {
                    release: Some(comms_version()),
                    send_default_pii: true,
                    environment: Some(Cow::from("production")),
                    ..Default::default()
                },
            )),
            Hub::current(),
        )
    };

    #[cfg(windows)]
    {
        // Set app user model ID (and set the same for the warp bundle) so that pinning
        // Comms will pin the top-level executable, not the nested one.
        unsafe { SetCurrentProcessExplicitAppUserModelID(w!("mitchhentges.comms")) }.unwrap();
        // Memory usage on Windows with the opengl/gl/vulkan renderers are much higher,
        // so explicitly fall back to tried 'n true "cairo".
        std::env::set_var("GSK_RENDERER", "cairo");
    }

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
        #[cfg(not(debug_assertions))]
        let sentry_hub = sentry_hub.clone();
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

            #[cfg(debug_assertions)]
            closure();
            #[cfg(not(debug_assertions))]
            {
                let thread_hub = Arc::new(Hub::new_from_top(&sentry_hub));
                Hub::run(thread_hub, closure);
            }
        });
    }

    #[cfg(windows)]
    {
        std::thread::spawn(register_protocol::run);
    }

    {
        let parameters = AudioPlatformImpl::create_parameters();
        let network_handle = network_handle.clone();
        std::thread::spawn(move || {
            let closure = move || {
                audio_device::priority::bump_priority().unwrap();
                let error = audio::run(
                    parameters,
                    network_handle,
                    ui_handle,
                    audio_registration,
                    audio_receiver,
                );
                eprintln!("Encountered an error in audio operations, stopping");
                panic!("{error:#?}");
            };

            #[cfg(debug_assertions)]
            closure();
            #[cfg(not(debug_assertions))]
            {
                let thread_hub = Arc::new(Hub::new_from_top(&sentry_hub));
                Hub::run(thread_hub, closure);
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
