use std::mem;
use std::ops::{Deref, DerefMut};
use winapi::um::audioclient;
use sample::Sample;

#[derive(Debug, Clone, Copy)]
pub enum SampleFormat {
    I16,
    U16,
    F32,
}

impl SampleFormat {
    pub fn sample_size(&self) -> usize {
        match self {
            &SampleFormat::I16 => mem::size_of::<i16>(),
            &SampleFormat::U16 => mem::size_of::<u16>(),
            &SampleFormat::F32 => mem::size_of::<f32>(),
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

pub struct InputBuffer<'a, T: 'a>
    where T: Sample {
    pub buffer: &'a [T],
}

impl<'a, T> Deref for InputBuffer<'a, T> where T: Sample {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.buffer
    }
}

pub struct OutputBuffer<'a, T: 'a>
    where T: Sample {
    pub buffer: &'a mut [T],
}

impl<'a, T> Deref for OutputBuffer<'a, T> where T: Sample {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        self.buffer
    }
}

impl<'a, T> DerefMut for OutputBuffer<'a, T> where T: Sample {
    fn deref_mut(&mut self) -> &mut [T] {
        self.buffer
    }
}

pub enum UnknownTypeInputBuffer<'a> {
    U16(InputBuffer<'a, u16>),
    I16(InputBuffer<'a, i16>),
    F32(InputBuffer<'a, f32>),
}

pub enum UnknownTypeOutputBuffer<'a> {
    U16(OutputBuffer<'a, u16>),
    I16(OutputBuffer<'a, i16>),
    F32(OutputBuffer<'a, f32>),
}

pub enum StreamData<'a> {
    Input(UnknownTypeInputBuffer<'a>),
    Output(UnknownTypeOutputBuffer<'a>),
}

#[derive(Debug)]
pub enum StreamError {
    DeviceNotAvailable,
    BackendSpecific(String),
}

pub type StreamDataResult<'a> = Result<StreamData<'a>, StreamError>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StreamId(pub usize);

pub enum AudioClientFlow {
    Render {
        render_client: *mut audioclient::IAudioRenderClient,
    },
    Capture {
        capture_client: *mut audioclient::IAudioCaptureClient,
    },
}