#[cfg(target_os = "windows")]
#[macro_use]
extern crate lazy_static;

pub mod audio_connection;
pub mod buffer;
pub mod noise_gate;
pub mod priority;

#[cfg(unix)]
pub mod pulseaudio;
#[cfg(windows)]
pub mod wasapi;
