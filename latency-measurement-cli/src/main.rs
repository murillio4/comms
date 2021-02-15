mod cli;
mod measurement;
mod state;

use crate::measurement::{create_measurement_pipeline, CalculateDelayMic, CalculateDelayPlayback};
use crate::state::MeasureState;
use audio_device::audio_connection::EventLoop;
use audio_device::buffer::UnsharedPlaybackSource;
use audio_thread_priority::promote_current_thread_to_real_time;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[cfg(unix)]
fn create_event_loop(
    mic: CalculateDelayMic,
    playback: UnsharedPlaybackSource<CalculateDelayPlayback>,
) -> impl EventLoop {
    use audio_device::pulseaudio::audio_connection::{PulseAudioConnection, PulseParameters};
    let parameters = PulseParameters {
        pulseaudio_server: Some("/run/user/1000/pulse/native".to_string()),
    };
    PulseAudioConnection::create(parameters, mic, playback).create_event_loop()
}

#[cfg(windows)]
fn create_event_loop(
    mic: CalculateDelayMic,
    playback: UnsharedPlaybackSource<CalculateDelayPlayback>,
) -> impl EventLoop {
    use audio_device::wasapi::audio_connection::WasapiAudioConnection;
    WasapiAudioConnection::create::<audio_device::buffer::NoopInterruptReceiver>(mic, playback)
        .create_event_loop()
}

fn main() {
    let start_time = SystemTime::now();
    let state = Arc::new(Mutex::new(MeasureState::Idle));
    let (tx, rx) = channel();

    {
        promote_current_thread_to_real_time(20, 48000).unwrap();
        let state = state.clone();
        std::thread::spawn(move || {
            let (mic, playback) = create_measurement_pipeline(state, tx, start_time);
            let event_loop = create_event_loop(mic, playback);
            event_loop.run();
        });
    }

    cli::cli(state, rx, start_time);
}
