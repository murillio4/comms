use crate::state::MeasureState;
use audio_device::buffer::{MicSink, PlaybackSource, UnsharedPlaybackSource};
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub struct CalculateDelayMic {
    tx: Sender<Duration>,
    state: Arc<Mutex<MeasureState>>,
    start_time: SystemTime,
    beep_latency: Option<Duration>,
    max_noise_volume: f32,
    recent_samples: VecDeque<f32>,
    just_finished_detecting_noise: bool,
}

impl CalculateDelayMic {
    fn new(state: Arc<Mutex<MeasureState>>, tx: Sender<Duration>, start_time: SystemTime) -> Self {
        Self {
            beep_latency: None,
            max_noise_volume: 0f32,
            recent_samples: VecDeque::with_capacity(100),
            tx,
            state,
            start_time,
            just_finished_detecting_noise: false,
        }
    }
}

impl MicSink for CalculateDelayMic {
    fn append_sample_from_mic(&mut self, sample: f32) {
        {
            let mut state = self.state.lock().unwrap();

            let next_state = match *state {
                MeasureState::ReadyToMeasure { .. } => None,
                MeasureState::DetectNoiseProfile => {
                    let sample_abs = sample.abs();
                    if sample_abs > self.max_noise_volume {
                        self.max_noise_volume = sample_abs;
                    }
                    None
                }
                MeasureState::Measuring { measure_start_time } => {
                    if !self.just_finished_detecting_noise {
                        self.just_finished_detecting_noise = true;
                        println!(
                            "[{}ms] \tMax noise volume is: {}",
                            SystemTime::now()
                                .duration_since(self.start_time)
                                .unwrap()
                                .as_millis(),
                            self.max_noise_volume
                        );
                    }

                    let threshold = self.max_noise_volume + 0.3;
                    if sample.abs() > threshold && self.beep_latency.is_none() {
                        println!(
                            "[{}ms] \tFound sample above max noise volume + buffer",
                            SystemTime::now()
                                .duration_since(self.start_time)
                                .unwrap()
                                .as_millis()
                        );
                        self.beep_latency = Some(
                            SystemTime::now()
                                .duration_since(measure_start_time)
                                .unwrap(),
                        );
                        Some(MeasureState::FinishMeasuring)
                    } else {
                        None
                    }
                }
                MeasureState::FinishMeasuring => {
                    if self.beep_latency.is_some() {
                        self.recent_samples.push_back(sample.abs());
                        if self.recent_samples.len() > 100 {
                            self.recent_samples.pop_front();
                            let avg = self
                                .recent_samples
                                .iter()
                                .fold(0f32, |acc, next| acc + next)
                                / self.recent_samples.len() as f32;
                            if avg < self.max_noise_volume {
                                println!(
                                    "[{}ms] \tBack under noise threshold",
                                    SystemTime::now()
                                        .duration_since(self.start_time)
                                        .unwrap()
                                        .as_millis()
                                );
                                self.tx.send(self.beep_latency.take().unwrap()).unwrap();
                                Some(MeasureState::Idle)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                MeasureState::Idle => {
                    self.beep_latency.take();
                    self.max_noise_volume = 0f32;
                    self.just_finished_detecting_noise = false;
                    self.recent_samples.clear();
                    None
                }
            };

            if let Some(next_state) = next_state {
                *state = next_state;
            }
        }
    }

    fn publish_chunks(&mut self) {}
}

pub struct CalculateDelayPlayback {
    sample_clock: f32,
    state: Arc<Mutex<MeasureState>>,
    start_time: SystemTime,
}

impl CalculateDelayPlayback {
    fn new(state: Arc<Mutex<MeasureState>>, start_time: SystemTime) -> Self {
        Self {
            sample_clock: 0f32,
            state,
            start_time,
        }
    }
}

impl PlaybackSource for CalculateDelayPlayback {
    fn next_sample(&mut self) -> f32 {
        let mut state = self.state.lock().unwrap();

        let sample_clock = &mut self.sample_clock;
        let mut calc_sample = || {
            let sample_rate = 48000f32;

            *sample_clock = (*sample_clock + 1.0) % sample_rate;
            ((*sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin()) as f32
        };

        let (sample, next_state) = match *state {
            MeasureState::Idle => (0f32, None),
            MeasureState::DetectNoiseProfile => (0f32, None),
            MeasureState::ReadyToMeasure { ready_time } => {
                let now = SystemTime::now();
                println!(
                    "[{}ms] \tTook {}ms from prompt-to-beep",
                    now.duration_since(self.start_time).unwrap().as_millis(),
                    now.duration_since(ready_time).unwrap().as_millis()
                );
                (
                    calc_sample(),
                    Some(MeasureState::Measuring {
                        measure_start_time: now,
                    }),
                )
            }
            MeasureState::Measuring { .. } => (calc_sample(), None),
            MeasureState::FinishMeasuring => (0f32, None),
        };

        if let Some(next_state) = next_state {
            *state = next_state;
        }

        sample
    }
}

pub fn create_measurement_pipeline(
    state: Arc<Mutex<MeasureState>>,
    tx: Sender<Duration>,
    start_time: SystemTime,
) -> (
    CalculateDelayMic,
    UnsharedPlaybackSource<CalculateDelayPlayback>,
) {
    (
        CalculateDelayMic::new(state.clone(), tx, start_time),
        UnsharedPlaybackSource(CalculateDelayPlayback::new(state, start_time)),
    )
}
