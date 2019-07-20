//! Feeds back the input stream directly into the output stream.
//!
//! Assumes that the input and output devices can use the same stream format and that they support
//! the f32 sample format.
//!
//! Uses a delay of `LATENCY_MS` milliseconds in case the default input and output streams are not
//! precisely synchronised.

extern crate cpal;
extern crate failure;

use std::net::UdpSocket;

use cpal::{Format, SampleFormat, SampleRate};
use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};
use opus::{Application, Channels, Encoder};

const SAMPLE_RATE: u32 = 48000; // Opus requires 48000hz
const OPUS_PACKET_MAX_LENGTH: usize = 1276;

fn main() -> Result<(), failure::Error> {
    let host = cpal::default_host();
    let event_loop = host.event_loop();

    let input_device = host.default_input_device().expect("Failed to get default input device");
    println!("Using default input device: \"{}\"", input_device.name().unwrap());

    let format = Format {
        channels: 1,
        data_type: SampleFormat::I16,
        sample_rate: SampleRate(SAMPLE_RATE),
    };

    println!("Attempting to build input stream with `{:?}`.", format);
    let input_stream_id = event_loop.build_input_stream(&input_device, &format).unwrap();
    event_loop.play_stream(input_stream_id.clone())?;

    let mut opus_encoder = Encoder::new(format.sample_rate.0, Channels::Mono, Application::LowDelay).unwrap();
    let mut opus_input_buffer = [0 as i16; 960];
    let mut opus_input_buffer_index = 0;

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect("127.0.0.1:18847")?;

    event_loop.run(move |id, result| {
        let data = match result {
            Ok(data) => data,
            Err(err) => {
                eprintln!("An error occurred on stream {:?}: {}", id, err);
                return;
            }
        };

        match data {
            cpal::StreamData::Input { buffer: cpal::UnknownTypeInputBuffer::I16(buffer) } => {
                assert_eq!(id, input_stream_id);
                for &sample in buffer.iter() {
                    opus_input_buffer[opus_input_buffer_index] = sample;
                    opus_input_buffer_index = (opus_input_buffer_index + 1) % 960;

                    if opus_input_buffer_index == 0 {
                        let opus_packet = opus_encoder.encode_vec(&opus_input_buffer, OPUS_PACKET_MAX_LENGTH).unwrap();
                        let _ = socket.send(&opus_packet);
                    }
                }
            },
            _ => panic!("we're expecting f32 data"),
        }
    })
}