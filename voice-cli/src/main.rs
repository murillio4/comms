#[cfg(target_os = "windows")]
#[macro_use]
extern crate lazy_static;

use std::{process, io};
use clap::{App, Arg};
use std::sync::{Arc, mpsc};
use mio::Registration;
use voice_lib::packet;
use std::sync::atomic::AtomicUsize;
use crate::audio::{SendToNetworkThread, SendToNetworkThreadError, ReceiveFromNetworkThread};
use std::time::SystemTime;
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use std::net::ToSocketAddrs;
use std::io::BufRead;

mod audio;
mod network;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() -> Result<(), CommsError> {
    let app_start = SystemTime::now();
    let matches = App::new("Comms")
        .version("0.1")
        .arg(Arg::with_name("server_address").takes_value(true))
        .arg(Arg::with_name("pulseaudio").takes_value(true))
        .get_matches();

    let server_address = match matches.value_of("server_address") {
        Some(server_address) => server_address.to_string(),
        None => {
            println!("Enter server address (e.g.: \"kamloops.fuzzlesoft.ca\")");
            let stdin = io::stdin();
            let server_address = stdin.lock().lines().next()
                .unwrap()
                .unwrap();
            if server_address == "" {
                "kamloops.fuzzlesoft.ca".to_string()
            } else {
                server_address
            }
        }
    };

    println!("Connecting to {}...", server_address);
    let pulseaudio_server = matches.value_of("pulseaudio");
    let server_address_with_port = format!("{}:18847", server_address);
    let server_address = match server_address_with_port.to_socket_addrs() {
        Ok(mut addresses) => {
            addresses
                .next()
                .expect(&format!("Unable to resolve domain {}", server_address))
        },
        Err(e) => {
            eprintln!("Could not parse {} as a server address: {}", server_address, e);
            return Err(CommsError::Usage);
        }
    };
    let (sender, receiver) = mpsc::channel();
    let (registration, set_readiness) = Registration::new2();
    let size = Arc::new(AtomicUsize::new(0));
    let send_to_network_thread = SendToNetworkThread::new(sender, set_readiness, size.clone());
    let (command_tx, command_rx) = mpsc::channel();
    let receive_from_network_thread = ReceiveFromNetworkThread::new(app_start, command_rx);

    std::thread::spawn(move || {
        #[cfg(windows)]
        bump_priority();
        if let Err(e) = network::run(server_address, receiver, command_tx, registration, size) {
            eprintln!("Encountered an error in network operations, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    });

    #[cfg(windows)]
    bump_priority();
    let connect = packet::Connect();
    send_to_network_thread.send(connect)?;

    audio::run(pulseaudio_server, send_to_network_thread, receive_from_network_thread)?;
    Ok(())
}

#[derive(Debug)]
enum CommsError {
    Usage,
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

impl From<audio::SendToNetworkThreadError> for CommsError {
    fn from(e: SendToNetworkThreadError) -> Self {
        CommsError::Audio(e.into())
    }
}