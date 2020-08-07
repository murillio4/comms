use sample::interpolate::{Linear, Interpolator};
use sample::Sample;
use crate::buffer::{MicSink, PlaybackSource};

pub trait MicSampleRateConverter<M: MicSink> {
    fn feed_next(&mut self, sample: f64, mic_sink: &mut M);
}

pub trait PlaybackSampleRateConverter<P: PlaybackSource> {
    fn get_next(&mut self, playback_source: &mut P) -> f64;
}

// TODO f32 instead of f64?
pub struct LinearSampleRateConverter {
    interpolator: Linear<[f64; 1]>,
    interpolation_value: f64,
    source_to_target_ratio: f64,
}

impl<M: MicSink> MicSampleRateConverter<M> for LinearSampleRateConverter {
    fn feed_next(&mut self, sample: f64, mic_sink: &mut M) {
        if self.interpolation_value >= 1.0 {
            self.interpolator.next_source_frame([sample]);
            self.interpolation_value -= 1.0;
        }

        while self.interpolation_value < 1.0 {
            let sample = self.interpolator.interpolate(self.interpolation_value);
            self.interpolation_value += self.source_to_target_ratio;
            mic_sink.append_sample_from_mic(sample[0].to_sample());
        }
    }
}

impl<P: PlaybackSource> PlaybackSampleRateConverter<P> for LinearSampleRateConverter {
    fn get_next(&mut self, playback_source: &mut P) -> f64 {
        while self.interpolation_value >= 1.0 {
            self.interpolator.next_source_frame([playback_source.next_sample().to_sample()]);
            self.interpolation_value -= 1.0;
        }

        let out = self.interpolator.interpolate(self.interpolation_value);
        self.interpolation_value += self.source_to_target_ratio;
        out[0]
    }
}

struct PassthroughSampleRateConverter;
impl<M: MicSink> MicSampleRateConverter<M> for PassthroughSampleRateConverter {
    fn feed_next(&mut self, sample: f64, mic_sink: &mut M) {
        mic_sink.append_sample_from_mic(sample.to_sample())
    }
}

impl<P: PlaybackSource> PlaybackSampleRateConverter<P> for PassthroughSampleRateConverter {
    fn get_next(&mut self, playback_source: &mut P) -> f64 {
        playback_source.next_sample().to_sample()
    }
}

pub fn create_mic_sample_rate_converter<M: MicSink>(source_hz: u32, target_hz: u32) -> Box<dyn MicSampleRateConverter<M>> {
    if source_hz == target_hz {
        return Box::new(PassthroughSampleRateConverter);
    }

    Box::new(LinearSampleRateConverter {
        interpolator: Linear::new([0f64; 1], [0f64; 1]),
        interpolation_value: 0.0,
        source_to_target_ratio: source_hz as f64 / target_hz as f64,
    })
}

pub fn create_playback_sample_rate_converter<P: PlaybackSource>(source_hz: u32, target_hz: u32) -> Box<dyn PlaybackSampleRateConverter<P>> {
    if source_hz == target_hz {
        return Box::new(PassthroughSampleRateConverter);
    }

    Box::new(LinearSampleRateConverter {
        interpolator: Linear::new([0f64; 1], [0f64; 1]),
        interpolation_value: 0.0,
        source_to_target_ratio: source_hz as f64 / target_hz as f64,
    })
}