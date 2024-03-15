use crate::{BiquadDisplay, FrequencyDisplay, ScaleColorizrParams};
use colorgrad::Gradient;
use crossbeam::atomic::AtomicCell;
use lazy_static::lazy_static;
use nih_plug::params::smoothing::AtomicF32;
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::epaint::{Hsva, PathShape};
use nih_plug_egui::egui::{Color32, Grid, Mesh, Painter, Pos2, Rgba, Stroke, Ui, Vec2, Window};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use noise::{NoiseFn, OpenSimplex, Perlin};
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

lazy_static! {
    static ref NOISE: OpenSimplex = OpenSimplex::new(rand::random());
    static ref ANIMATE_NOISE: Perlin = Perlin::new(rand::random());
}

#[derive(Default, Serialize, Deserialize)]
struct EditorState {
    show_debug: bool,
}

pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(800, 600)
}

pub fn create(
    state: Arc<EguiState>,
    sample_rate: Arc<AtomicF32>,
    params: Arc<ScaleColorizrParams>,
    displays: Arc<FrequencyDisplay>,
    biquads: Arc<BiquadDisplay>,
) -> Option<Box<dyn Editor>> {
    let gradient = colorgrad::preset::rainbow();

    create_egui_editor(
        state,
        EditorState::default(),
        |_, _| {},
        move |ctx, setter, state| {
            egui::TopBottomPanel::top("menu").show(ctx, |ui| {
                if ui.button("Debug").clicked() {
                    state.show_debug = !state.show_debug;
                }
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(nih_plug_egui::egui::widgets::Slider::from_get_set(
                    2.0..=40.0,
                    |new_value| {
                        new_value.map_or_else(
                            || f64::from(params.gain.value()),
                            |v| {
                                setter.begin_set_parameter(&params.gain);
                                #[allow(clippy::cast_possible_truncation)]
                                setter.set_parameter(&params.gain, v as f32);
                                setter.end_set_parameter(&params.gain);

                                v
                            },
                        )
                    },
                ));

                let debug_complex = filter_line(
                    ui,
                    &biquads,
                    sample_rate.load(std::sync::atomic::Ordering::Relaxed),
                    &gradient,
                );

                let debug_window = Window::new("DEBUG")
                    .vscroll(true)
                    .open(&mut state.show_debug);
                debug_window.show(ctx, |ui| {
                    ui.collapsing("VOICES", |ui| {
                        for (idx, display) in displays.iter().enumerate() {
                            ui.group(|ui| {
                                ui.label(format!("VOICE {idx}"));
                                Grid::new(format!("voice-{idx}")).show(ui, |ui| {
                                    for (i, filter) in display.iter().enumerate() {
                                        ui.label(filter.load().map_or("UNUSED".to_string(), |v| {
                                            format!("FREQ: {v}")
                                        }));

                                        if (i + 1) % 3 == 0 {
                                            ui.end_row();
                                        }
                                    }
                                });
                            });
                        }
                    });
                    ui.collapsing("FREQ GRAPH", |ui| {
                        ui.group(|ui| {
                            Grid::new("complex").show(ui, |ui| {
                                for (i, filter) in debug_complex.iter().enumerate() {
                                    ui.label(format!("{filter}"));

                                    if (i + 1) % 10 == 0 {
                                        ui.end_row();
                                    }
                                }
                            });
                        })
                    });
                })
            });
        },
    )
}

fn filter_line<G: Gradient>(
    ui: &Ui,
    biquads: &Arc<BiquadDisplay>,
    sample_rate: f32,
    gradient: &G,
) -> Vec<Complex32> {
    let mut debug = Vec::new();
    let painter = Painter::new(
        ui.ctx().clone(),
        ui.layer_id(),
        ui.available_rect_before_wrap(),
    );
    let left_center = painter.clip_rect().left_center();
    let right_center = painter.clip_rect().right_center();

    let len = left_center.x - right_center.x;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut points = Vec::with_capacity(len as usize);

    #[allow(clippy::cast_possible_truncation)]
    for (idx, i) in (left_center.x as i32..=right_center.x as i32).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let freq = 10_000.0f32.mul_add(idx as f32 / len, 20.0);
        let mut result = Complex32::new(1.0, 0.0);

        for biquad in biquads.iter().flatten().filter_map(AtomicCell::load) {
            result *= biquad.frequency_response(freq);
            debug.push(result);
        }

        #[allow(clippy::cast_precision_loss)]
        points.push(Pos2::new(
            i as f32,
            left_center.y - result.norm().log10() * 50.0,
        ));
    }

    // DISGUSTING: i would MUCH rather meshify the line so i can apply shaders
    // but i couldn't get it to work, so i'm doing this terribleness instead.
    let animation_position = ui.ctx().frame_nr() as f64 * 0.005;
    let offset = ANIMATE_NOISE.get([animation_position * 0.01, 0.0]);
    for (idx, p) in points.array_windows().enumerate() {
        let x = idx as f64 * 0.002;
        let noise_value = NOISE.get([x, animation_position + offset]);
        let color = gradient.at(norm(noise_value as f32, -0.5, 0.5)).to_rgba8();
        painter.line_segment(
            *p,
            Stroke::new(1.5, Color32::from_rgb(color[0], color[1], color[2])),
        );
    }

    debug
}

fn norm(t: f32, a: f32, b: f32) -> f32 {
    (t - a) * (1.0 / (b - a))
}
