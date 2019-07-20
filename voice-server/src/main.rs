extern crate failure;

use std::net::UdpSocket;
use opus::{Decoder, Channels};
use cpal::{Format, SampleFormat, SampleRate};
use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};

const SAMPLE_RATE: u32 = 48000; // Opus requires 48000hz
const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() -> Result<(), failure::Error> {
    let host = cpal::default_host();
    let event_loop = host.event_loop();

    let output_device = host.default_output_device().expect("Failed to get default output device");
    println!("Using default output device: \"{}\"", output_device.name().unwrap());
    let format = Format {
        channels: 1,
        data_type: SampleFormat::I16,
        sample_rate: SampleRate(SAMPLE_RATE),
    };

    println!("Attempting to build stream with `{:?}`.", format);
    let output_stream_id = event_loop.build_output_stream(&output_device, &format).unwrap();
    event_loop.play_stream(output_stream_id.clone())?;

    let mut opus_decoder = Decoder::new(format.sample_rate.0, Channels::Mono).unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(48000);

    std::thread::spawn(move || {
        event_loop.run(move |id, result| {
            let data = match result {
                Ok(data) => data,
                Err(err) => {
                    eprintln!("An error occurred on stream {:?}: {}", id, err);
                    return;
                }
            };

            match data {
                cpal::StreamData::Output { buffer: cpal::UnknownTypeOutputBuffer::I16(mut buffer) } => {
                    assert_eq!(id, output_stream_id);
                    for sample in buffer.iter_mut() {
                        *sample = match rx.try_recv() {
                            Ok(s) => s,
                            Err(_) => 0,
                        };
                    }
                },
                _ => panic!("we're expecting f32 data"),
            }
        });
    });

    let socket = UdpSocket::bind("127.0.0.1:18847")?;
    let mut buf = [0; OPUS_PACKET_MAX_LENGTH];
    loop {
        let opus_packet_length = socket.recv_from(&mut buf)?.0;
        let mut opus_output = [0 as i16; 960];
        opus_decoder.decode(&buf[..opus_packet_length], &mut opus_output, false).unwrap();
        for &sample in opus_output.iter() {
            let _ = tx.try_send(sample);
        }

    }
}