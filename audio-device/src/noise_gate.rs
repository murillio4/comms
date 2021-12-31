use dasp::Sample;
use ringbuffer::{AllocRingBuffer, RingBufferExt, RingBufferWrite};

const ATTACK_SAMPLE_COUNT: usize = 400;
const HOLD_SAMPLE_COUNT: usize = 10000;
const RELEASE_SAMPLE_COUNT: usize = 8000;

pub enum State {
    Off,
    Attack(usize),
    Active,
    Hold(usize),
    Release(usize),
}

impl State {
    fn update_threshold_state(&mut self, is_above: bool) {
        *self = match self {
            State::Off => {
                if is_above {
                    State::Attack(0)
                } else {
                    State::Off
                }
            }
            State::Attack(progress) => State::Attack(*progress),
            State::Active => {
                if is_above {
                    State::Active
                } else {
                    State::Hold(0)
                }
            }
            State::Hold(progress) => {
                if is_above {
                    State::Active
                } else {
                    State::Hold(*progress)
                }
            }
            State::Release(progress) => {
                if is_above {
                    let percentage_from_full =
                        1f64 - (*progress as f64 / RELEASE_SAMPLE_COUNT as f64);
                    let attack_progress =
                        (percentage_from_full * ATTACK_SAMPLE_COUNT as f64) as usize;
                    State::Attack(attack_progress)
                } else {
                    State::Release(*progress)
                }
            }
        }
    }

    fn attenuate_sample(&mut self, sample: f32) -> f32 {
        let (new_state, sample) = match self {
            State::Off => (State::Off, Sample::EQUILIBRIUM),
            State::Attack(mut progress) => {
                progress += 1;
                let state = if progress == ATTACK_SAMPLE_COUNT {
                    State::Active
                } else {
                    State::Attack(progress)
                };

                (
                    state,
                    sample.mul_amp(progress as f32 / ATTACK_SAMPLE_COUNT as f32),
                )
            }
            State::Active => (State::Active, sample),
            State::Hold(mut progress) => {
                progress += 1;
                let state = if progress == HOLD_SAMPLE_COUNT {
                    State::Release(0)
                } else {
                    State::Hold(progress)
                };
                (state, sample)
            }
            State::Release(mut progress) => {
                progress += 1;
                let state = if progress == RELEASE_SAMPLE_COUNT {
                    State::Off
                } else {
                    State::Release(progress)
                };
                (
                    state,
                    sample.mul_amp(1f32 - (progress as f32 / RELEASE_SAMPLE_COUNT as f32)),
                )
            }
        };
        *self = new_state;
        sample
    }
}

pub struct GateNoiseFilter {
    pub recent_rms: AllocRingBuffer<f32>,
    pub threshold: f32,
    pub state: State,
}

impl GateNoiseFilter {
    pub fn new(sliding_window_chunk_count: usize) -> Self {
        GateNoiseFilter {
            recent_rms: AllocRingBuffer::with_capacity(sliding_window_chunk_count),
            threshold: 0f32,
            state: State::Off,
        }
    }

    pub fn update_window(&mut self, next_chunk: &[f32]) {
        let mut accumulator = 0f32;
        for sample in next_chunk {
            accumulator += sample * sample;
        }

        let rms = (accumulator / next_chunk.len() as f32).sqrt() as f32;
        self.recent_rms.push(rms);

        let noise_floor = self.recent_rms.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        self.threshold = noise_floor + 0.007;
        self.state.update_threshold_state(rms > self.threshold);
    }

    pub fn process(&mut self, sample: f32) -> f32 {
        self.state.attenuate_sample(sample)
    }
}
