use crate::buffer::{MicSink, PlaybackSource};
use crate::wasapi::device::IAudioClientWrapper;
use crate::wasapi::event_loop::WasapiEventLoopRegistration;
use crate::wasapi::sample_rate_conversion::{MicSampleRateConverter, PlaybackSampleRateConverter};
use dasp::Sample;
use std::mem::MaybeUninit;
use std::{mem, ptr, slice};
use windows::Win32::Media::Audio;

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
    audio_client: IAudioClientWrapper,
    max_frames_in_buffer: u32,
    bytes_per_frame: u16,
    sample_format: SampleFormat,
    channel_count: usize,
}

impl RawWasapiStream {
    pub fn new(
        waveformatex: Audio::WAVEFORMATEX,
        audio_client: IAudioClientWrapper,
        format: Format,
    ) -> Self {
        let max_buffer_frames = audio_client.get_buffer_max_frames().unwrap();

        Self {
            audio_client,
            bytes_per_frame: waveformatex.nBlockAlign,
            channel_count: format.channels as usize,
            max_frames_in_buffer: max_buffer_frames,
            sample_format: format.data_type,
        }
    }

    pub fn start(&self) {
        if let Err(err) = unsafe { (*self.audio_client).Start() } {
            eprintln!("An error occurred when playing stream: {:?}", err);
        }
    }
}

pub struct WasapiMicStream<M: MicSink> {
    pub raw_stream: RawWasapiStream,
    capture_client: Audio::IAudioCaptureClient,
    pub notify: WasapiEventLoopRegistration,
    sample_rate_converter: Box<dyn MicSampleRateConverter<M>>,
}

impl<M: MicSink> WasapiMicStream<M> {
    pub fn new(
        sample_rate_converter: Box<dyn MicSampleRateConverter<M>>,
        capture_client: Audio::IAudioCaptureClient,
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
        let mut buffer = MaybeUninit::<*mut u8>::uninit();
        // TODO remove flags? Maybe ptr::null_mut instead?
        let mut flags = MaybeUninit::<_>::uninit();

        loop {
            let mut frames_available = match unsafe { self.capture_client.GetNextPacketSize() } {
                Ok(frames_available) => frames_available,
                Err(e) => return Err(stream_error_from_win_error(e)),
            };

            if frames_available == 0 {
                return Ok(());
            }

            match unsafe {
                self.capture_client.GetBuffer(
                    buffer.as_mut_ptr(),
                    &mut frames_available,
                    flags.as_mut_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            } {
                Ok(()) => {}
                Err(ref e) if e.code() == Audio::AUDCLNT_S_BUFFER_EMPTY => return Ok(()),
                Err(e) => return Err(stream_error_from_win_error(e)),
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

                    if let Err(err) = unsafe { self.capture_client.ReleaseBuffer(frames_available) }
                    {
                        return Err(stream_error_from_win_error(err));
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

                    if let Err(err) = unsafe { self.capture_client.ReleaseBuffer(frames_available) }
                    {
                        return Err(stream_error_from_win_error(err));
                    }
                }
            }
            mic_buffer.publish_chunks();
        }
    }
}

pub struct WasapiPlaybackStream<P: PlaybackSource> {
    pub raw_stream: RawWasapiStream,
    render_client: Audio::IAudioRenderClient,
    pub notify: WasapiEventLoopRegistration,
    sample_rate_converter: Box<dyn PlaybackSampleRateConverter<P>>,
}

impl<P: PlaybackSource> WasapiPlaybackStream<P> {
    pub fn new(
        sample_rate_converter: Box<dyn PlaybackSampleRateConverter<P>>,
        render_client: Audio::IAudioRenderClient,
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
            let padding = unsafe { (*self.raw_stream.audio_client).GetCurrentPadding() }
                .map_err(stream_error_from_win_error)?;
            let frames_available = self.raw_stream.max_frames_in_buffer - padding;

            if frames_available == 0 {
                return Ok(());
            }

            let raw_buffer = unsafe { self.render_client.GetBuffer(frames_available) }
                .map_err(stream_error_from_win_error)?;

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

                    unsafe { self.render_client.ReleaseBuffer(frames_available as u32, 0) }
                        .map_err(stream_error_from_win_error)?;
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

                    unsafe { self.render_client.ReleaseBuffer(frames_available as u32, 0) }
                        .map_err(stream_error_from_win_error)?;
                }
            }
        }
    }
}

// TODO refactor into useful abstraction around unsafe calls with results
fn stream_error_from_win_error(error: windows::core::Error) -> StreamError {
    if error.code() == Audio::AUDCLNT_E_DEVICE_INVALIDATED {
        StreamError::DeviceNotAvailable
    } else {
        let description = format!("{}", error);
        StreamError::BackendSpecific(description)
    }
}
