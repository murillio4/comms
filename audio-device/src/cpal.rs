//! CPAL streams exchange 48 kHz mono samples with the application through bounded
//! queues. Callbacks never wait for application processing or allocate buffers.
use crate::audio_connection::{
    AudioConnection, DeviceInfo, EventLoop, EventLoopNotify, FoundDevices,
};
use crate::buffer::{InterruptReceiver, MaybeSharedPlaybackSource, MicSink, PlaybackSource};
use crate::sample_rate_conversion::{
    create_mic_sample_rate_converter, create_playback_sample_rate_converter,
};
use ::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ::cpal::{
    Device, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
};
use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

pub struct Registration(mpsc::Receiver<()>);
#[derive(Clone)]
pub struct Notify(mpsc::SyncSender<()>);
pub fn channel() -> (Registration, Notify) {
    let (tx, rx) = mpsc::sync_channel(1);
    (Registration(rx), Notify(tx))
}
impl EventLoopNotify for Notify {
    fn set_ready(&self) {
        let _ = self.0.try_send(());
    }
}
#[derive(Debug)]
pub enum CreationError {
    Input(String),
    Output(String),
}
impl std::fmt::Display for CreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CreationError {}

pub struct CpalConnection;
impl AudioConnection for CpalConnection {
    fn list_devices(&self, callback: impl FnOnce(FoundDevices) + 'static) {
        fn collect(
            devices: Result<impl Iterator<Item = Device>, ::cpal::DevicesError>,
        ) -> Vec<DeviceInfo> {
            devices
                .into_iter()
                .flatten()
                .filter_map(|device| device.name().ok())
                .map(|name| DeviceInfo {
                    id: name.clone(),
                    label: name,
                })
                .collect()
        }
        let host = ::cpal::default_host();
        callback(FoundDevices {
            input_devices: collect(host.input_devices()),
            output_devices: collect(host.output_devices()),
        });
    }
}
struct InputQueue(HeapProd<f32>);
impl MicSink for InputQueue {
    fn append_sample_from_mic(&mut self, sample: f32) {
        let _ = self.0.try_push(sample);
    }
    fn publish_chunks(&mut self) {}
}
struct OutputQueue(HeapCons<f32>);
impl PlaybackSource for OutputQueue {
    fn next_samples(&mut self, count: usize, callback: impl FnOnce((&[f32], Option<&[f32]>))) {
        // Output is converted in blocks of at most 128 frames (at device rate).
        let mut samples = [0.0; 1024];
        assert!(count <= samples.len());
        for sample in &mut samples[..count] {
            *sample = self.0.try_pop().unwrap_or(0.0);
        }
        callback((&samples[..count], None));
    }
    fn drop_samples(&mut self) {
        while self.0.try_pop().is_some() {}
    }
}
fn input<T>(
    device: &Device,
    config: &SupportedStreamConfig,
    queue: InputQueue,
    failed: Arc<AtomicBool>,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels() as usize;
    let mut queue = queue;
    let mut converter = create_mic_sample_rate_converter(config.sample_rate().0, 48000);
    device
        .build_input_stream(
            &config.config(),
            move |data: &[T], _| {
                for frame in data.chunks_exact(channels) {
                    let mono =
                        frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                    converter.feed_next(mono as f64, &mut queue);
                }
            },
            move |_| {
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| e.to_string())
}
fn output<T>(
    device: &Device,
    config: &SupportedStreamConfig,
    queue: OutputQueue,
    failed: Arc<AtomicBool>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels() as usize;
    let mut queue = queue;
    let mut converter = create_playback_sample_rate_converter(48000, config.sample_rate().0);
    device
        .build_output_stream(
            &config.config(),
            move |data: &mut [T], _| {
                let mut mono = [0.0; 128];
                for block in data.chunks_mut(128 * channels) {
                    let frames = block.len() / channels;
                    converter.convert_f32(&mut mono[..frames], &mut queue, 1);
                    for (frame, sample) in block.chunks_exact_mut(channels).zip(mono.iter()) {
                        frame.fill(T::from_sample(sample.clamp(-1.0, 1.0)));
                    }
                }
            },
            move |_| {
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| e.to_string())
}
macro_rules! build {
    ($function:ident, $device:expr, $config:expr, $queue:expr, $failed:expr) => {
        match $config.sample_format() {
            SampleFormat::I8 => $function::<i8>($device, $config, $queue, $failed),
            SampleFormat::I16 => $function::<i16>($device, $config, $queue, $failed),
            SampleFormat::I32 => $function::<i32>($device, $config, $queue, $failed),
            SampleFormat::I64 => $function::<i64>($device, $config, $queue, $failed),
            SampleFormat::U8 => $function::<u8>($device, $config, $queue, $failed),
            SampleFormat::U16 => $function::<u16>($device, $config, $queue, $failed),
            SampleFormat::U32 => $function::<u32>($device, $config, $queue, $failed),
            SampleFormat::U64 => $function::<u64>($device, $config, $queue, $failed),
            SampleFormat::F32 => $function::<f32>($device, $config, $queue, $failed),
            SampleFormat::F64 => $function::<f64>($device, $config, $queue, $failed),
            format => Err(format!("Unsupported sample format: {format}")),
        }
    };
}

pub struct CpalEventLoop<M, P: PlaybackSource, S, I> {
    mic: M,
    playback: S,
    interrupt: I,
    input: HeapCons<f32>,
    output: HeapProd<f32>,
    streams: (Stream, Stream),
    failed: Arc<AtomicBool>,
    marker: std::marker::PhantomData<P>,
}
impl<
        M: MicSink,
        P: PlaybackSource,
        S: MaybeSharedPlaybackSource<P>,
        I: InterruptReceiver<Registration>,
    > CpalEventLoop<M, P, S, I>
{
    pub fn create(mic: M, playback: S, interrupt: I) -> Result<Self, CreationError> {
        let host = ::cpal::default_host();
        let input_device = host
            .default_input_device()
            .ok_or_else(|| CreationError::Input("No default input device".into()))?;
        let output_device = host
            .default_output_device()
            .ok_or_else(|| CreationError::Output("No default output device".into()))?;
        let input_config = input_device
            .default_input_config()
            .map_err(|e| CreationError::Input(e.to_string()))?;
        let output_config = output_device
            .default_output_config()
            .map_err(|e| CreationError::Output(e.to_string()))?;
        if input_config.sample_rate().0 < 8000 {
            return Err(CreationError::Input(
                "Device sample rate must be at least 8 kHz".into(),
            ));
        }
        if output_config.sample_rate().0 < 8000 {
            return Err(CreationError::Output(
                "Device sample rate must be at least 8 kHz".into(),
            ));
        }
        let (input_tx, input_rx) = HeapRb::<f32>::new(2048).split();
        let (output_tx, output_rx) = HeapRb::<f32>::new(2048).split();
        let failed = Arc::new(AtomicBool::new(false));
        let input = build!(
            input,
            &input_device,
            &input_config,
            InputQueue(input_tx),
            failed.clone()
        )
        .map_err(CreationError::Input)?;
        let output = build!(
            output,
            &output_device,
            &output_config,
            OutputQueue(output_rx),
            failed.clone()
        )
        .map_err(CreationError::Output)?;
        Ok(Self {
            mic,
            playback,
            interrupt,
            input: input_rx,
            output: output_tx,
            streams: (input, output),
            failed,
            marker: std::marker::PhantomData,
        })
    }
}
impl<
        M: MicSink,
        P: PlaybackSource,
        S: MaybeSharedPlaybackSource<P>,
        I: InterruptReceiver<Registration>,
    > EventLoop for CpalEventLoop<M, P, S, I>
{
    type Registration = Registration;
    type Notify = Notify;
    type AudioConnection = CpalConnection;
    fn run(mut self) -> ! {
        self.streams.0.play().expect("Failed to start input stream");
        self.streams
            .1
            .play()
            .expect("Failed to start output stream");
        loop {
            assert!(
                !self.failed.load(Ordering::Relaxed),
                "CPAL stream failed; restart audio to reconnect devices"
            );
            self.interrupt.invoke(&CpalConnection);
            // Publish frequently so even a full queue cannot overrun a caller's mic buffer.
            for _ in 0..2048 {
                let Some(sample) = self.input.try_pop() else {
                    break;
                };
                self.mic.append_sample_from_mic(sample);
                self.mic.publish_chunks();
            }
            for _ in 0..4 {
                if self.output.occupied_len() >= 960 {
                    break;
                }
                self.playback.with(|source| {
                    source.next_samples(240, |(first, second)| {
                        for sample in first.iter().chain(second.unwrap_or_default()) {
                            let _ = self.output.try_push(*sample);
                        }
                    })
                });
            }
            // Minimal utilities have no registration; they still need periodic queue servicing.
            if let Some(registration) = self.interrupt.optional_registration() {
                let _ = registration.0.recv_timeout(Duration::from_millis(2));
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_underflow_is_silence_and_consumes_available_samples() {
        let (mut tx, rx) = HeapRb::<f32>::new(4).split();
        tx.try_push(0.5).unwrap();
        tx.try_push(-0.5).unwrap();
        let mut source = OutputQueue(rx);
        source.next_samples(4, |(samples, _)| {
            assert_eq!(samples, &[0.5, -0.5, 0.0, 0.0])
        });
        source.next_samples(1, |(samples, _)| assert_eq!(samples, &[0.0]));
    }

    #[test]
    fn capture_overflow_drops_samples_without_waiting() {
        let (tx, mut rx) = HeapRb::<f32>::new(2).split();
        let mut sink = InputQueue(tx);
        for sample in [1.0, 2.0, 3.0] {
            sink.append_sample_from_mic(sample);
        }
        assert_eq!(rx.try_pop(), Some(1.0));
        assert_eq!(rx.try_pop(), Some(2.0));
        assert_eq!(rx.try_pop(), None);
    }

    #[test]
    fn notifications_coalesce_and_can_be_reused() {
        let (registration, notify) = channel();
        notify.set_ready();
        notify.set_ready();
        assert!(registration.0.try_recv().is_ok());
        assert!(registration.0.try_recv().is_err());
        notify.set_ready();
        assert!(registration.0.try_recv().is_ok());
    }
}
