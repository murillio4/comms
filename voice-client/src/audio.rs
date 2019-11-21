use libpulse_binding::sample::{Spec, Format};
use libpulse_binding::time::MicroSeconds;
use libpulse_binding::context::Context;
use crate::network::{AppendToQueue, AudioCommand};
use std::cell::RefCell;
use std::rc::Rc;
use libpulse_binding::stream::{Stream, PeekResult, SeekMode};
use libpulse_binding::def::BufferAttr;
use libpulse_binding::stream::flags::ADJUST_LATENCY;
use voice_lib::packet::AudioFromClient;
use std::sync::mpsc::Receiver;
use ringbuf::Consumer;
use std::collections::HashMap;
use sample::Sample;

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};
const PULSE_LATENCY: MicroSeconds = MicroSeconds(30000);

pub fn record(pulse_context: &mut Context, append_to_queue: AppendToQueue) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "record", &SPEC, None).unwrap()
    ));
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    stream.set_overflow_callback(Some(Box::new(move || {
        println!("[recording] overflow");
    })));
    stream.set_underflow_callback(Some(Box::new(move || {
        println!("[recording] underflow");
    })));
    stream.set_started_callback(Some(Box::new(move || {
        println!("[recording] started");
    })));
    stream.set_suspended_callback(Some(Box::new(move || {
        println!("[recording] suspended");
    })));
    stream.set_event_callback(Some(Box::new(move |_, _| {
        println!("[recording] event???");
    })));
    stream.set_moved_callback(Some(Box::new(move || {
        println!("[recording] it moved how neat is that");
    })));

    let buffer_attr = BufferAttr {
        tlength: std::u32::MAX, // only matters for playback
        maxlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        fragsize: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };
    stream.connect_record(None, Some(&buffer_attr), ADJUST_LATENCY).unwrap();

    stream.set_read_callback(Some(Box::new(move |_| {
        let mut stream = stream_rc.borrow_mut();
        match stream.peek() {
            Ok(data) => {
                match data {
                    PeekResult::Empty => println!("empty"),
                    PeekResult::Hole(_) => println!("hole detected"),
                    PeekResult::Data(data) => {
//                           let mut packet = Vec::<i16>::with_capacity(data.len() / 2);
//                           for i in 0..(data.len() / 2) {
//                               let sample = i16::from_le_bytes([
//                                   data[i * 2],
//                                   data[i * 2 + 1]
//                               ]);
//                               packet.push(sample);
//                           }

                        // let opus_packet = opus_encoder.encode_vec(&opus_input_buffer, OPUS_PACKET_MAX_LENGTH).unwrap();
                        append_to_queue.push(AudioFromClient(data)).unwrap();
                    }
                }
            },
            Err(_) => {
                panic!("stream.peek() error");
            }
        }
        stream.discard().unwrap();
    })));
}


pub fn playback(pulse_context: &mut Context, command_rx: Receiver<AudioCommand>) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(pulse_context, "playback", &SPEC, None).unwrap()
    ));
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    let receive_from_network_thread = Rc::new(RefCell::new(
        ReceiveFromNetworkThread::new(command_rx)
    ));

    stream.set_overflow_callback(Some(Box::new(move || {
        println!("[playback] overflow");
    })));
    stream.set_underflow_callback(Some(Box::new(move || {
        println!("[playback] underflow");
    })));
    {
        let stream_rc = stream_rc.clone();
        stream.set_started_callback(Some(Box::new(move || {
            let mut stream = stream_rc.borrow_mut();
            println!("[playback] started -> {:?}", stream.get_buffer_attr().unwrap());
        })));
    }
    stream.set_suspended_callback(Some(Box::new(move || {
        println!("[playback] suspended");
    })));
    stream.set_event_callback(Some(Box::new(move |_, _| {
        println!("[playback] event???");
    })));
    stream.set_moved_callback(Some(Box::new(move || {
        println!("[playback] it moved how neat is that");
    })));

    let buffer_attr = BufferAttr {
        fragsize: std::u32::MAX, // only matters for recording
        maxlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        tlength: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };

    stream.connect_playback(None, Some(&buffer_attr), ADJUST_LATENCY, None, None).unwrap();
    stream.set_write_callback(Some(Box::new(move |bytes| {
        let mut stream = stream_rc.borrow_mut();
        let buffer = stream.begin_write(Some(bytes)).unwrap().unwrap();

        let mut receive_from_network_thread = receive_from_network_thread.borrow_mut();
        receive_from_network_thread.handle_commands();

        for i in 0..buffer.len() / 2 {
            let sample = receive_from_network_thread.next_sample();
            let le_bytes = sample.to_le_bytes();
            buffer[i*2] = le_bytes[0];
            buffer[i*2 + 1] = le_bytes[1];
        }

        stream.write_copy(&buffer, 0, SeekMode::Relative).unwrap();
    })));
}

struct ReceiveFromNetworkThread {
    command_rx: Receiver<AudioCommand>,
    client_streams: HashMap<String, Consumer<i16>>,
}

impl ReceiveFromNetworkThread {
    fn new(command_rx: Receiver<AudioCommand>) -> ReceiveFromNetworkThread {
        ReceiveFromNetworkThread {
            command_rx,
            client_streams: HashMap::new(),
        }
    }

    fn handle_commands(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            match command {
                AudioCommand::NewClientStream(id, consumer) => {
                    self.client_streams.insert(id, consumer);
                }
            }
        }
    }

    fn next_sample(&mut self) -> i16 {
        let init: i16 = sample::Sample::equilibrium();
        let init = init.to_signed_sample();
        self.client_streams.values_mut()
            .fold(init, |acc, consumer| {
                let next_sample = consumer.pop().unwrap_or(0);
                acc.checked_add(next_sample).unwrap_or(std::i16::MAX)
            })
    }
}