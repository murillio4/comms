use crate::buffer::{MicSink, PlaybackSource};
use crate::wasapi::com::check_result;
use crate::wasapi::device::IAudioClientWrapper;
use crate::wasapi::event_loop::WasapiEventLoopRegistration;
use crate::wasapi::sample_rate_conversion::{MicSampleRateConverter, PlaybackSampleRateConverter};
use dasp::Sample;
use std::mem::MaybeUninit;
use std::{mem, ptr, slice};
use winapi::shared::{basetsd, minwindef, mmreg};
use winapi::um::{audioclient, winnt};

#[derive(Debug, Clone, Copy)]
pub enum SampleFormat {
    I16,
    F32,
}

impl SampleFormat {
    pub fn sample_size(&self) -> usize {
        match self {
            SampleFormat::I16 => mem::size_of::<i16>(),
            SampleFormat::F32 => mem::size_of::<f32>(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SampleRate(pub u32);

#[derive(Debug, Clone)]
pub struct Format {
    pub channels: u16,
    pub sample_rate: SampleRate,
    pub data_type: SampleFormat,
}

#[derive(Debug)]
pub enum StreamError {
    DeviceNotAvailable,
    BackendSpecific(String),
}

pub struct RawWasapiStream {
    audio_client: *mut audioclient::IAudioClient,
    max_frames_in_buffer: basetsd::UINT32,
    bytes_per_frame: minwindef::WORD,
    sample_format: SampleFormat,
    channel_count: usize,
}

impl RawWasapiStream {
    pub fn new(
        waveformatex: mmreg::WAVEFORMATEX,
        audio_client: IAudioClientWrapper,
        format: Format,
    ) -> Self {
        let max_buffer_frames = audio_client.get_buffer_max_frames().unwrap();

        Self {
            audio_client: *audio_client,
            bytes_per_frame: waveformatex.nBlockAlign,
            channel_count: format.channels as usize,
            max_frames_in_buffer: max_buffer_frames,
            sample_format: format.data_type,
        }
    }

    pub fn start(&self) {
        let hresult = unsafe { (*self.audio_client).Start() };
        if let Err(err) = stream_error_from_hresult(hresult) {
            eprintln!("An error occurred when playing stream: {:?}", err);
        }
    }
}

pub struct WasapiMicStream<M: MicSink> {
    pub raw_stream: RawWasapiStream,
    capture_client: *mut audioclient::IAudioCaptureClient,
    pub notify: WasapiEventLoopRegistration,
    sample_rate_converter: Box<dyn MicSampleRateConverter<M>>,
}

impl<M: MicSink> WasapiMicStream<M> {
    pub fn new(
        sample_rate_converter: Box<dyn MicSampleRateConverter<M>>,
        capture_client: *mut audioclient::IAudioCaptureClient,
        notify: WasapiEventLoopRegistration,
        raw_stream: RawWasapiStream,
    ) -> Self {
        Self {
            raw_stream,
            capture_client,
            notify,
            sample_rate_converter,
        }
    }

    pub fn invoke(&mut self, mic_buffer: &mut M) -> Result<(), StreamError> {
        let mut frames_available = 0;
        let mut buffer = MaybeUninit::<*mut minwindef::BYTE>::uninit();
        // TODO remove flags? Maybe ptr::null_mut instead?
        let mut flags = MaybeUninit::<_>::uninit();

        loop {
            let hresult =
                unsafe { (*self.capture_client).GetNextPacketSize(&mut frames_available) };
            if let Err(err) = stream_error_from_hresult(hresult) {
                return Err(err);
            }

            if frames_available == 0 {
                return Ok(());
            }

            let hresult = unsafe {
                (*self.capture_client).GetBuffer(
                    buffer.as_mut_ptr(),
                    &mut frames_available,
                    flags.as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };

            if hresult == audioclient::AUDCLNT_S_BUFFER_EMPTY {
                return Ok(());
            } else if let Err(err) = stream_error_from_hresult(hresult) {
                return Err(err);
            }

            let raw_buffer = unsafe { buffer.assume_init() };
            let sample_size = self.raw_stream.sample_format.sample_size();
            let buffer_len =
                frames_available as usize * self.raw_stream.bytes_per_frame as usize / sample_size;

            match self.raw_stream.sample_format {
                SampleFormat::F32 => {
                    // When the AudioClient is initialized, it is configured to align its buffer
                    // according to the audio format in use.
                    #[allow(clippy::cast_ptr_alignment)]
                    let buffer_data = raw_buffer as *mut _ as *const f32;
                    let sample_buffer = unsafe { slice::from_raw_parts(buffer_data, buffer_len) };
                    let sample_count = sample_buffer.len() / self.raw_stream.channel_count;
                    let mut sample_buffer_iter = sample_buffer.iter();

                    for _ in 0..sample_count {
                        let mut next_sample: f64 = Sample::EQUILIBRIUM;
                        for _ in 0..self.raw_stream.channel_count {
                            next_sample = {
                                let next = next_sample.add_amp(
                                    sample_buffer_iter
                                        .next()
                                        .expect("Buffer had less samples than expected")
                                        .to_sample(),
                                );
                                if next == f64::INFINITY {
                                    f64::MAX
                                } else {
                                    next
                                }
                            }
                        }

                        self.sample_rate_converter
                            .feed_next(next_sample, mic_buffer)
                    }

                    let hresult = unsafe { (*self.capture_client).ReleaseBuffer(frames_available) };
                    if let Err(err) = stream_error_from_hresult(hresult) {
                        return Err(err);
                    }
                }
                SampleFormat::I16 => {
                    // When the AudioClient is initialized, it is configured to align its buffer
                    // according to the audio format in use.
                    #[allow(clippy::cast_ptr_alignment)]
                    let buffer_data = raw_buffer as *mut _ as *const i16;
                    let sample_buffer = unsafe { slice::from_raw_parts(buffer_data, buffer_len) };
                    let sample_count = sample_buffer.len() / self.raw_stream.channel_count;
                    let mut sample_buffer_iter = sample_buffer.iter();

                    for _ in 0..sample_count {
                        let mut next_sample: f64 = Sample::EQUILIBRIUM;
                        for _ in 0..self.raw_stream.channel_count {
                            next_sample = {
                                let next = next_sample.add_amp(
                                    sample_buffer_iter
                                        .next()
                                        .expect("Buffer had less samples than expected")
                                        .to_sample(),
                                );
                                if next == f64::INFINITY {
                                    f64::MAX
                                } else {
                                    next
                                }
                            }
                        }

                        self.sample_rate_converter
                            .feed_next(next_sample, mic_buffer)
                    }

                    let hresult = unsafe { (*self.capture_client).ReleaseBuffer(frames_available) };
                    if let Err(err) = stream_error_from_hresult(hresult) {
                        return Err(err);
                    }
                }
            }
            mic_buffer.publish_chunks();
        }
    }
}

pub struct WasapiPlaybackStream<P: PlaybackSource> {
    pub raw_stream: RawWasapiStream,
    render_client: *mut audioclient::IAudioRenderClient,
    pub notify: WasapiEventLoopRegistration,
    sample_rate_converter: Box<dyn PlaybackSampleRateConverter<P>>,
}

impl<P: PlaybackSource> WasapiPlaybackStream<P> {
    pub fn new(
        sample_rate_converter: Box<dyn PlaybackSampleRateConverter<P>>,
        render_client: *mut audioclient::IAudioRenderClient,
        notify: WasapiEventLoopRegistration,
        raw_stream: RawWasapiStream,
    ) -> Self {
        Self {
            raw_stream,
            render_client,
            notify,
            sample_rate_converter,
        }
    }

    pub fn invoke(&mut self, playback_source: &mut P) -> Result<(), StreamError> {
        loop {
            let mut padding = MaybeUninit::<u32>::uninit();
            let hresult =
                unsafe { (*self.raw_stream.audio_client).GetCurrentPadding(padding.as_mut_ptr()) };
            stream_error_from_hresult(hresult)?;
            let frames_available =
                self.raw_stream.max_frames_in_buffer - unsafe { padding.assume_init() };

            if frames_available == 0 {
                return Ok(());
            }

            let mut buffer = MaybeUninit::<*mut minwindef::BYTE>::uninit();
            let hresult =
                unsafe { (*self.render_client).GetBuffer(frames_available, buffer.as_mut_ptr()) };

            if let Err(err) = stream_error_from_hresult(hresult) {
                return Err(err);
            }

            let raw_buffer = unsafe { buffer.assume_init() };
            let sample_size = self.raw_stream.sample_format.sample_size();
            let buffer_len =
                frames_available as usize * self.raw_stream.bytes_per_frame as usize / sample_size;

            match self.raw_stream.sample_format {
                SampleFormat::F32 => {
                    // When the AudioClient is initialized, it is configured to align its buffer
                    // according to the audio format in use.
                    #[allow(clippy::cast_ptr_alignment)]
                    let buffer_data = raw_buffer as *mut f32;
                    let sample_buffer =
                        unsafe { slice::from_raw_parts_mut(buffer_data, buffer_len) };
                    let sample_count = sample_buffer.len() / self.raw_stream.channel_count;

                    for i in 0..sample_count {
                        let sample = self
                            .sample_rate_converter
                            .get_next(playback_source)
                            .to_sample();
                        for channel_index in 0..self.raw_stream.channel_count {
                            sample_buffer[(i * self.raw_stream.channel_count) + channel_index] =
                                sample;
                        }
                    }

                    let hresult =
                        unsafe { (*self.render_client).ReleaseBuffer(frames_available as u32, 0) };
                    if let Err(err) = stream_error_from_hresult(hresult) {
                        return Err(err);
                    }
                }
                SampleFormat::I16 => {
                    // When the AudioClient is initialized, it is configured to align its buffer
                    // according to the audio format in use.
                    #[allow(clippy::cast_ptr_alignment)]
                    let buffer_data = raw_buffer as *mut i16;
                    let sample_buffer =
                        unsafe { slice::from_raw_parts_mut(buffer_data, buffer_len) };
                    let sample_count = sample_buffer.len() / self.raw_stream.channel_count;

                    for i in 0..sample_count {
                        let sample = self
                            .sample_rate_converter
                            .get_next(playback_source)
                            .to_sample();
                        for channel_index in 0..self.raw_stream.channel_count {
                            sample_buffer[(i * self.raw_stream.channel_count) + channel_index] =
                                sample;
                        }
                    }

                    let hresult =
                        unsafe { (*self.render_client).ReleaseBuffer(frames_available as u32, 0) };
                    if let Err(err) = stream_error_from_hresult(hresult) {
                        return Err(err);
                    }
                }
            }
        }
    }
}

// TODO refactor into useful abstraction around unsafe calls with results
fn stream_error_from_hresult(hresult: winnt::HRESULT) -> Result<(), StreamError> {
    if hresult == audioclient::AUDCLNT_E_DEVICE_INVALIDATED {
        Err(StreamError::DeviceNotAvailable)
    } else if let Err(err) = check_result(hresult) {
        let description = format!("{}", err);
        Err(StreamError::BackendSpecific(description))
    } else {
        Ok(())
    }
}
