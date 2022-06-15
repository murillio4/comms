use crate::buffer::{MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use dasp::Sample;
use libpulse_binding::context::Context;
use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream;
use libpulse_binding::stream::{PeekResult, SeekMode, Stream};
use libpulse_binding::time::MicroSeconds;
use std::cell::RefCell;
use std::rc::Rc;

const SPEC: Spec = Spec {
    channels: 1,
    rate: 48000, // Opus requires 48000hz
    format: Format::S16le,
};
const PULSE_LATENCY: MicroSeconds = MicroSeconds(30000);

pub fn record(context: &mut Context, mut mic: impl MicSink + 'static) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(context, "record", &SPEC, None).unwrap(),
    ));

    let buffer_attr = BufferAttr {
        tlength: u32::MAX, // only matters for playback
        maxlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };
    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();
    stream
        .connect_record(None, Some(&buffer_attr), stream::FlagSet::ADJUST_LATENCY)
        .unwrap();
    stream.set_read_callback(Some(Box::new(move |_| {
        let mut stream = stream_rc.borrow_mut();
        match stream.peek() {
            Ok(PeekResult::Data(data)) => {
                for i in 0..data.len() / 2 {
                    let i16_sample = i16::from_le_bytes([data[i * 2], data[i * 2 + 1]]);
                    mic.append_sample_from_mic(i16_sample.to_sample())
                }
                mic.publish_chunks();
            }
            Err(_) => {
                panic!("stream.peek() error");
            }
            _ => {}
        }
        stream.discard().unwrap();
    })));
}

pub fn play<P: PlaybackSource>(
    context: &mut Context,
    mut playback: impl MaybeSharedPlaybackSource<P> + 'static,
) {
    let stream_rc = Rc::new(RefCell::new(
        Stream::new(context, "playback", &SPEC, None).unwrap(),
    ));

    let buffer_attr = BufferAttr {
        fragsize: u32::MAX, // only matters for recording
        maxlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        tlength: SPEC.usec_to_bytes(PULSE_LATENCY) as u32,
    };

    let stream = stream_rc.clone();
    let mut stream = stream.borrow_mut();

    stream
        .connect_playback(
            None,
            Some(&buffer_attr),
            stream::FlagSet::ADJUST_LATENCY,
            None,
            None,
        )
        .unwrap();
    stream.set_write_callback(Some(Box::new(move |bytes| {
        let mut stream = stream_rc.borrow_mut();
        playback.with(|playback: &mut P| {
            let buffer = stream.begin_write(Some(bytes)).unwrap().unwrap();
            playback.next_samples(buffer.len() / 2, |(window_1, window_2)| {
                for (i, sample) in window_1.iter().enumerate() {
                    let le_bytes = sample.to_sample::<i16>().to_le_bytes();
                    buffer[i * 2] = le_bytes[0];
                    buffer[i * 2 + 1] = le_bytes[1];
                }
                if let Some(window_2) = window_2 {
                    for (i, sample) in window_2.iter().enumerate() {
                        let le_bytes = sample.to_sample::<i16>().to_le_bytes();
                        buffer[(window_1.len() * 2) + i * 2] = le_bytes[0];
                        buffer[(window_1.len() * 2) + i * 2 + 1] = le_bytes[1];
                    }
                }
            });

            stream.write_copy(buffer, 0, SeekMode::Relative).unwrap();
        });
    })));
}
