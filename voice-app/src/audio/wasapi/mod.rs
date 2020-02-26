use sample::{interpolate, signal, Signal, Sample};
use crate::audio::wasapi::device::{InputDevice, OutputDevice};
use crate::audio::wasapi::stream::{UnknownTypeInputBuffer, StreamData, UnknownTypeOutputBuffer};
use ringbuf::{RingBuffer, Consumer};
use voice_lib::packet::AudioFromClient;
use crate::audio::{AudioError, ReceiveFromNetworkThread};
use crate::network::{Handle, AudioCommand};
use std::sync::mpsc::Receiver;

mod com;
mod device;
mod event_loop;
mod ms_multimedia;
mod stream;

pub fn run(network_handle: Handle, receiver: Receiver<AudioCommand>) -> Result<(), AudioError> {
    let input_device = InputDevice::default().unwrap();
    let output_device = OutputDevice::default().unwrap();

    let input_format = input_device.default_format().unwrap();
    let output_format = output_device.default_format().unwrap();

    let playback_interpolator = interpolate::Linear::new([0f64; 1], [0f64; 1]);
    let server_signal = signal::from_iter(ReceiveFromNetworkThread::new(receiver));
    let mut playback_signal = server_signal.from_hz_to_hz(playback_interpolator, 48000f64, output_format.sample_rate.0 as f64);

    let mic_buffer = RingBuffer::new(4800);
    let (mut mic_producer, mic_consumer) = mic_buffer.split();
    let capture_interpolator = interpolate::Linear::new([0f64; 1], [0f64; 1]);
    let raw_mic_signal = signal::from_iter(MicIterator::new(mic_consumer));
    let mut mic_signal = raw_mic_signal.from_hz_to_hz(capture_interpolator, input_format.sample_rate.0 as f64, 48000f64);

    let mut event_loop = event_loop::EventLoop::new();
    event_loop.input_stream(&input_device, &input_format).unwrap();
    event_loop.output_stream(&output_device, &output_format).unwrap();

    let mut sample_count_accumulator = 0f64;

    let mic_sample_rate = input_format.sample_rate.0;
    let mut convert_and_send = |sample_count| {
        let adjusted_samples_count: f64 = (sample_count as f64 * (48000f64 / mic_sample_rate as f64)) + sample_count_accumulator;
        let samples_to_send = adjusted_samples_count.floor();
        sample_count_accumulator = adjusted_samples_count - samples_to_send;

        let mut buffer = Vec::<u8>::with_capacity(samples_to_send as usize);
        for _ in 0..(samples_to_send as usize) {
            let sample = mic_signal.next()[0].to_sample::<i16>();
            buffer.extend(&sample.to_le_bytes());
        }
        network_handle.audio(AudioFromClient(buffer)).unwrap();
    };

    event_loop.run(move |id, result| {
        let data = match result {
            Ok(data) => data,
            Err(err) => {
                eprintln!("An error occurred on stream {:?}: {:?}", id, err);
                return;
            }
        };

        match data {
            StreamData::Input(UnknownTypeInputBuffer::F32(buffer)) => {
                let sample_count = buffer.len() / input_format.channels as usize;
                for i in 0..sample_count {
                    let item = buffer[i*input_format.channels as usize];
                    mic_producer.push(item.to_sample::<f64>()).unwrap();
                }
                convert_and_send(sample_count);
            }
            StreamData::Input(UnknownTypeInputBuffer::I16(buffer)) => {
                let sample_count = buffer.len() / input_format.channels as usize;
                for i in 0..sample_count {
                    let item = buffer[i*input_format.channels as usize];
                    mic_producer.push(item.to_sample::<f64>()).unwrap();
                }
                convert_and_send(sample_count);
            },
            StreamData::Output(UnknownTypeOutputBuffer::F32(mut buffer)) => {
                let channel_count = output_format.channels as usize;
                for window in 0..buffer.len() / channel_count {
                    let sample = playback_signal.next()[0].to_sample::<f32>();
                    for channel in 0..channel_count {
                        buffer[window * channel_count + channel] = sample
                    }
                }
            },
            StreamData::Output(UnknownTypeOutputBuffer::I16(mut buffer)) => {
                let channel_count = output_format.channels as usize;
                for window in 0..buffer.len() / channel_count {
                    let sample = playback_signal.next()[0].to_sample::<i16>();
                    for channel in 0..channel_count {
                        buffer[window * channel_count + channel] = sample
                    }
                }
            },
        }
    });
    Ok(())
}


struct MicIterator {
    consumer: Consumer<f64>,
}

impl MicIterator {
    fn new(consumer: Consumer<f64>) -> MicIterator {
        MicIterator {
            consumer
        }
    }
}

impl Iterator for MicIterator {
    type Item = [f64; 1];

    fn next(&mut self) -> Option<Self::Item> {
        Some([self.consumer.pop().unwrap_or(0f64 )])
    }
}



impl Iterator for ReceiveFromNetworkThread {
    type Item = [f64; 1];

    fn next(&mut self) -> Option<Self::Item> {
        self.handle_commands();
        Some([self.next_sample().to_sample::<f64>()])
    }
}