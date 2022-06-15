use crate::buffer::{MicSink, PlaybackSource};
use dasp::interpolate::linear::Linear;
use dasp::interpolate::Interpolator;
use dasp::sample::FromSample;
use dasp::Sample;

pub trait MicSampleRateConverter<M: MicSink> {
    fn feed_next(&mut self, sample: f64, mic_sink: &mut M);
}

pub trait PlaybackSampleRateConverter<P: PlaybackSource> {
    fn convert_f32(
        &mut self,
        destination: &mut [f32],
        playback_source: &mut P,
        channel_count: usize,
    );
    fn convert_i16(
        &mut self,
        destination: &mut [i16],
        playback_source: &mut P,
        channel_count: usize,
    );
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
    fn convert_f32(
        &mut self,
        destination: &mut [f32],
        playback_source: &mut P,
        channel_count: usize,
    ) {
        convert(
            &mut self.interpolator,
            &mut self.interpolation_value,
            self.source_to_target_ratio,
            destination,
            playback_source,
            channel_count,
        );
    }

    fn convert_i16(
        &mut self,
        destination: &mut [i16],
        playback_source: &mut P,
        channel_count: usize,
    ) {
        convert(
            &mut self.interpolator,
            &mut self.interpolation_value,
            self.source_to_target_ratio,
            destination,
            playback_source,
            channel_count,
        );
    }
}

fn convert<T: FromSample<f64>, P: PlaybackSource>(
    interpolator: &mut Linear<[f64; 1]>,
    interpolation_value: &mut f64,
    source_to_target_ratio: f64,
    destination: &mut [T],
    playback_source: &mut P,
    channel_count: usize,
) {
    let destination_sample_count = destination.len() / channel_count;
    let samples_to_fetch_count = (destination_sample_count as f64 * source_to_target_ratio
        - *interpolation_value)
        .ceil()
        .max(0f64) as usize;

    playback_source.next_samples(samples_to_fetch_count, |(window_1, window_2)| {
        let mut source_samples = window_1.iter().chain(window_2.unwrap_or_default().iter());
        let mut sample_index: usize = 0;

        while sample_index <= destination_sample_count {
            while *interpolation_value >= 1.0 {
                interpolator.next_source_frame([source_samples
                    .next()
                    .expect("Should always have loaded enough samples")
                    .to_sample()]);
                *interpolation_value -= 1.0;
            }

            for channel_index in 0..channel_count {
                destination[sample_index * channel_count + channel_index] =
                    interpolator.interpolate(*interpolation_value)[0].to_sample();
            }
            sample_index += 1;
            *interpolation_value += source_to_target_ratio;
        }
    });
}

struct PassthroughSampleRateConverter;
impl<M: MicSink> MicSampleRateConverter<M> for PassthroughSampleRateConverter {
    fn feed_next(&mut self, sample: f64, mic_sink: &mut M) {
        mic_sink.append_sample_from_mic(sample.to_sample())
    }
}

impl<P: PlaybackSource> PlaybackSampleRateConverter<P> for PassthroughSampleRateConverter {
    fn convert_f32(
        &mut self,
        destination: &mut [f32],
        playback_source: &mut P,
        channel_count: usize,
    ) {
        playback_source.next_samples(destination.len() / channel_count, |(window_1, window_2)| {
            for (i, sample) in window_1
                .iter()
                .chain(window_2.unwrap_or_default().iter())
                .enumerate()
            {
                for channel_index in 0..channel_count {
                    destination[(i * channel_count) + channel_index] = *sample;
                }
            }
        });
    }

    fn convert_i16(
        &mut self,
        destination: &mut [i16],
        playback_source: &mut P,
        channel_count: usize,
    ) {
        playback_source.next_samples(destination.len() / channel_count, |(window_1, window_2)| {
            for (i, sample) in window_1
                .iter()
                .chain(window_2.unwrap_or_default().iter())
                .enumerate()
            {
                for channel_index in 0..channel_count {
                    destination[(i * channel_count) + channel_index] = sample.to_sample();
                }
            }
        });
    }
}

pub fn create_mic_sample_rate_converter<M: MicSink>(
    source_hz: u32,
    target_hz: u32,
) -> Box<dyn MicSampleRateConverter<M>> {
    if source_hz == target_hz {
        return Box::new(PassthroughSampleRateConverter);
    }

    Box::new(LinearSampleRateConverter {
        interpolator: Linear::new([0f64; 1], [0f64; 1]),
        interpolation_value: 0.0,
        source_to_target_ratio: source_hz as f64 / target_hz as f64,
    })
}

pub fn create_playback_sample_rate_converter<P: PlaybackSource>(
    source_hz: u32,
    target_hz: u32,
) -> Box<dyn PlaybackSampleRateConverter<P>> {
    if source_hz == target_hz {
        return Box::new(PassthroughSampleRateConverter);
    }

    Box::new(LinearSampleRateConverter {
        interpolator: Linear::new([0f64; 1], [0f64; 1]),
        interpolation_value: 0.0,
        source_to_target_ratio: source_hz as f64 / target_hz as f64,
    })
}
