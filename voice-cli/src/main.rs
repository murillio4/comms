#[cfg(target_os = "windows")]
#[macro_use]
extern crate lazy_static;

use futures::channel::oneshot;
use std::{process, io};
use clap::{App, Arg};
use std::sync::mpsc;
use voice_lib::{packet, deserialize};
#[cfg(windows)]
use voice_lib::winlib::bump_priority;
use std::net::ToSocketAddrs;
use std::io::BufRead;
use crate::network::{Handle, SendToNetworkThreadError};
use voice_lib::packet::{Packet, ConnectAccepted};
use std::thread::sleep;
use std::time::Duration;

mod audio;
mod network;

// const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() -> Result<(), CommsError> {
    let matches = App::new("Comms")
        .version("0.1")
        .arg(Arg::with_name("server_address").takes_value(true))
        .arg(Arg::with_name("username").takes_value(true))
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

    let username = match matches.value_of("username") {
        Some(username) => username.to_string(),
        None => {
            loop {
                println!("Enter username:");
                let stdin = io::stdin();
                let username = stdin.lock().lines().next()
                    .unwrap()
                    .unwrap();
                if username != "" {
                    break username;
                }
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

    let (audio_sender, audio_receiver) = mpsc::channel();
    let (network_handle, network_worker) = Handle::new_handle_and_worker(
        server_address,
        audio_sender,
    );

    std::thread::spawn(move || {
        #[cfg(windows)]
        bump_priority();
        if let Err(e) = network_worker.run() {
            eprintln!("Encountered an error in network operations, stopping");
            eprintln!("Error details: {:?}", e);
            process::exit(1);
        }
    });

    let connect = packet::Connect {
        username
    };
    let (sender, mut receiver) = oneshot::channel();
    network_handle.request(connect, packet::ConnectAccepted::HEADER_TYPE, sender)?;
    loop {
        sleep(Duration::from_millis(100));
        match receiver.try_recv() {
            Ok(Some(response)) => {
                let connect_accepted: ConnectAccepted = deserialize(&response).unwrap();
                dbg!(connect_accepted.users);
                break;
            },
            _ => println!("."),
        }
    }

    #[cfg(windows)]
    bump_priority();
    audio::run(pulseaudio_server, network_handle, audio_receiver)?;
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

impl From<SendToNetworkThreadError> for CommsError {
    fn from(e: SendToNetworkThreadError) -> Self {
        CommsError::Audio(e.into())
    }
}