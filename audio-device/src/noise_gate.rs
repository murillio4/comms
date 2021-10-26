use dasp::Sample;
use ringbuffer::{ConstGenericRingBuffer, RingBufferExt, RingBufferWrite};

const ATTACK_SAMPLE_COUNT: usize = 400;
const HOLD_SAMPLE_COUNT: usize = 10000;
const RELEASE_SAMPLE_COUNT: usize = 4000;

pub enum State {
    Off,
    Attack(usize),
    Hold(usize),
    Release(usize),
}

impl State {
    fn update_state(&mut self, is_above: bool) {
        *self = match self {
            State::Off => {
                if is_above {
                    State::Attack(0)
                } else {
                    State::Off
                }
            }
            State::Attack(mut progress) => {
                progress += 1;
                if progress == ATTACK_SAMPLE_COUNT {
                    State::Hold(0)
                } else {
                    State::Attack(progress)
                }
            }
            State::Hold(mut progress) => {
                if !is_above {
                    progress += 1;
                    if progress == HOLD_SAMPLE_COUNT {
                        State::Release(0)
                    } else {
                        State::Hold(progress)
                    }
                } else {
                    State::Hold(0)
                }
            }
            State::Release(mut progress) => {
                progress += 1;
                if is_above {
                    let percentage_from_full =
                        1f64 - (progress as f64 / RELEASE_SAMPLE_COUNT as f64);
                    let attack_progress =
                        (percentage_from_full * ATTACK_SAMPLE_COUNT as f64) as usize;
                    State::Attack(attack_progress)
                } else if progress == RELEASE_SAMPLE_COUNT {
                    State::Off
                } else {
                    State::Release(progress)
                }
            }
        }
    }

    fn attenuate_sample(&self, sample: f64) -> f64 {
        match self {
            State::Off => Sample::EQUILIBRIUM,
            State::Attack(progress) => {
                sample.mul_amp(*progress as f64 / ATTACK_SAMPLE_COUNT as f64)
            }
            State::Hold(_) => sample,
            State::Release(progress) => {
                sample.mul_amp(1f64 - (*progress as f64 / RELEASE_SAMPLE_COUNT as f64))
            }
        }
    }
}

pub struct GateNoiseFilter {
    pub recent_rms: ConstGenericRingBuffer<f64, 128>,
    pub threshold: f64,
    pub state: State,
}

impl GateNoiseFilter {
    pub fn update_window(&mut self, next_chunk: &[f64]) -> ConstGenericRingBuffer<f64, 128> {
        let mut accumulator = 0f64;
        for sample in next_chunk {
            accumulator += sample * sample;
        }

        let rms = (accumulator / next_chunk.len() as f64).sqrt();
        self.recent_rms.push(rms);

        let noise_floor = self.recent_rms.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        self.threshold = noise_floor * 1.01 + 0.01;
        self.recent_rms.clone()
    }

    pub fn process(&mut self, sample: f64) -> f64 {
        let is_above = self.is_above_threshold(sample);
        self.state.update_state(is_above);
        self.state.attenuate_sample(sample)
    }

    fn is_above_threshold(&self, sample: f64) -> bool {
        let signed_sample = sample.to_signed_sample();
        signed_sample > self.threshold || signed_sample < -self.threshold
    }
}

impl Default for GateNoiseFilter {
    fn default() -> Self {
        GateNoiseFilter {
            recent_rms: ConstGenericRingBuffer::new(),
            threshold: 0f64,
            state: State::Off,
        }
    }
}
