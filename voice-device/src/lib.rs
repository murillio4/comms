#[cfg(target_os = "windows")]
#[macro_use]
extern crate lazy_static;

pub mod audio_connection;
pub mod buffer;

#[cfg(unix)]
pub mod pulseaudio;
#[cfg(windows)]
pub mod wasapi;