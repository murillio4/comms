use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui::plot::{Legend, Values};
use eframe::egui::Context;
use eframe::epi::Frame;
use eframe::{egui, epi};
use egui::plot::{HLine, Line, Plot};
use egui::*;
use ringbuffer::{AllocRingBuffer, RingBuffer, RingBufferExt};

const SAMPLE_BUCKETS: usize = 32;
const PLOT_NUMBER_OF_CHUNKS: u64 = 128;
// Each chunk is roughly 10ms, depending on OS.
const GRAPH_DURATION: Duration = Duration::from_millis(PLOT_NUMBER_OF_CHUNKS * 10);

pub enum IncomingAudioPlotData {
    Sample(f32),
    NoiseGateLevel(f32),
}

pub struct AudioPlot {
    receiver: Receiver<IncomingAudioPlotData>,
    aggregate: Vec<f32>,
    threshold: f32,
    samples: VecDeque<(Instant, f32)>,
}

impl AudioPlot {
    fn new(receiver: Receiver<IncomingAudioPlotData>) -> Self {
        Self {
            receiver,
            aggregate: Vec::new(),
            threshold: 0f32,
            samples: VecDeque::new(),
        }
    }
}

impl AudioPlot {
    fn update(&mut self) {
        let Self {
            receiver,
            aggregate,
            samples,
            ..
        } = self;

        for incoming_data in receiver.try_iter() {
            match incoming_data {
                IncomingAudioPlotData::Sample(sample) => {
                    aggregate.push(sample);
                    if aggregate.len() == SAMPLE_BUCKETS {
                        let next_sample =
                            aggregate.drain(..).map(|sample| sample.abs()).sum::<f32>()
                                / SAMPLE_BUCKETS as f32;
                        samples.push_back((Instant::now(), next_sample));
                    }
                }
                IncomingAudioPlotData::NoiseGateLevel(threshold) => {
                    self.threshold = threshold;
                }
            }
        }

        samples.retain(|(instant, _)| instant.elapsed() < GRAPH_DURATION);
    }

    fn samples(&mut self) -> Line {
        let samples = &self.samples;

        if samples.is_empty() {
            return Line::new(Values::default());
        }

        Line::new(Values::from_parametric_callback(
            move |t| {
                let (instant, sample) = samples.get(t as usize).copied().unwrap();
                if instant.elapsed().as_millis() > GRAPH_DURATION.as_millis() {}
                (
                    (GRAPH_DURATION.as_millis()
                        - instant
                            .elapsed()
                            .as_millis()
                            .min(GRAPH_DURATION.as_millis())) as f64,
                    sample as f64,
                )
            },
            0f64..=samples.len() as f64 - 1f64,
            samples.len(),
        ))
        .color(Color32::from_rgb(100, 220, 100))
        .name("Samples")
    }
}

impl Widget for &mut AudioPlot {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.ctx().request_repaint();
        self.update();

        Plot::new("Demo Plot")
            .include_x(0f64)
            .include_x(GRAPH_DURATION.as_millis() as f64)
            .include_y(0f64)
            .include_y(0.05f64)
            .legend(Legend::default())
            .show(ui, |plot| {
                plot.line(self.samples());
                plot.hline(
                    HLine::new(self.threshold)
                        .color(Color32::from_rgb(50, 50, 255))
                        .name("Threshold"),
                )
            })
            .response
    }
}

pub enum IncomingNoiseGatePlotData {
    RecentRms(AllocRingBuffer<f32>),
    Threshold(f32),
    StateChange(u8),
}

pub struct NoiseGatePlot {
    receiver: Receiver<IncomingNoiseGatePlotData>,
    recent_rms: Option<AllocRingBuffer<f32>>,
    threshold: f32,
    state: u8,
}

impl NoiseGatePlot {
    fn new(receiver: Receiver<IncomingNoiseGatePlotData>) -> Self {
        Self {
            receiver,
            recent_rms: None,
            threshold: 0f32,
            state: 0u8,
        }
    }
}

impl NoiseGatePlot {
    fn update(&mut self) {
        for incoming_data in self.receiver.try_iter() {
            match incoming_data {
                IncomingNoiseGatePlotData::RecentRms(recent_rms) => {
                    self.recent_rms = Some(recent_rms)
                }
                IncomingNoiseGatePlotData::Threshold(threshold) => self.threshold = threshold,
                IncomingNoiseGatePlotData::StateChange(state) => self.state = state,
            }
        }
    }

    fn recent_rms(&mut self) -> Line {
        if let Some(recent_rms) = &self.recent_rms {
            if recent_rms.is_empty() {
                return Line::new(Values::default());
            }
            let offset = (PLOT_NUMBER_OF_CHUNKS - recent_rms.len() as u64) as f64;
            Line::new(Values::from_parametric_callback(
                move |t| {
                    (
                        offset + t,
                        recent_rms.get(t as isize).copied().unwrap_or(0f32) as f64,
                    )
                },
                0f64..=recent_rms.len() as f64 - 1f64,
                recent_rms.len(),
            ))
        } else {
            Line::new(Values::default())
        }
    }
}

impl Widget for &mut NoiseGatePlot {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.ctx().request_repaint();
        self.update();

        let threshold_color = match self.state {
            0 => Color32::from_rgb(255, 50, 255),
            1 => Color32::from_rgb(255, 50, 50),
            2 => Color32::from_rgb(50, 50, 255),
            3 => Color32::from_rgb(50, 50, 255),
            4 => Color32::from_rgb(50, 255, 50),
            _ => Color32::from_rgb(50, 50, 50),
        };

        Plot::new("Noise Gate Plot")
            .include_x(0f64)
            .include_x(PLOT_NUMBER_OF_CHUNKS as f64)
            .include_y(0f64)
            .include_y(0.05f64)
            .legend(Legend::default())
            .show(ui, |plot| {
                plot.line(self.recent_rms());
                plot.hline(
                    HLine::new(self.threshold)
                        .color(threshold_color)
                        .name("Threshold"),
                )
            })
            .response
    }
}

struct App {
    time: AudioPlot,
    noise_gate: NoiseGatePlot,
}

impl epi::App for App {
    fn update(&mut self, ctx: &Context, frame: &Frame) {
        let Self { time, noise_gate } = self;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            egui::menu::bar(ui, |ui| {
                egui::menu::menu_button(ui, "File", |ui| {
                    if ui.button("Quit").clicked() {
                        frame.quit();
                    }
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let plot_height = ui.available_height() / 2.0;
            ui.add_sized([ui.available_width(), plot_height], time);
            ui.add_sized([ui.available_width(), plot_height], noise_gate);
        });
    }

    fn name(&self) -> &str {
        "live noise gate ui"
    }
}

pub fn run(
    time_plot_receiver: Receiver<IncomingAudioPlotData>,
    noise_gate_receiver: Receiver<IncomingNoiseGatePlotData>,
) {
    let app = App {
        time: AudioPlot::new(time_plot_receiver),
        noise_gate: NoiseGatePlot::new(noise_gate_receiver),
    };
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(Box::new(app), native_options);
}
