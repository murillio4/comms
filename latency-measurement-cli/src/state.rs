use std::time::SystemTime;

pub enum MeasureState {
    Idle,
    DetectNoiseProfile,
    ReadyToMeasure {
        ready_time: SystemTime,
    },
    Measuring {
        measure_start_time: SystemTime,
    },
    FinishMeasuring,
}