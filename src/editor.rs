#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::{BiquadDisplay, FrequencyDisplay, ScaleColorizrParams};
use colorgrad::{Color, Gradient};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::{Color32, Grid, Painter, Pos2, Stroke, Ui, Window};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use noise::{NoiseFn, OpenSimplex, Perlin};
use num_complex::Complex32;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Default, Serialize, Deserialize)]
struct EditorState {
    show_debug: bool,
}

pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(800, 600)
}

pub fn create(
    state: Arc<EguiState>,
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
    gradient: &G,
) -> Vec<Complex32> {
    static NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(rand::random()));
    static ANIMATE_NOISE: Lazy<Perlin> = Lazy::new(|| Perlin::new(rand::random()));

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

    let is_active = biquads.iter().flatten().any(|b| b.load().is_some());

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
            result.norm().log10().mul_add(-50.0, left_center.y),
        ));
    }

    // DISGUSTING: i would MUCH rather meshify the line so i can apply shaders
    // but i couldn't get it to work, so i'm doing this terribleness instead.
    let animation_position = ui.ctx().frame_nr() as f64 * 0.005;
    let offset = ANIMATE_NOISE.get([animation_position * 0.01, 0.0]);
    for (idx, p) in points.array_windows().enumerate() {
        let x = idx as f64 * 0.002;
        let noise_value = norm(NOISE.get([x, animation_position + offset]) as f32, -0.5, 0.5);
        let gradient = gradient.at(noise_value);
        let color = Color::from_hsva(0.0, 0.0, noise_value, 1.0).interpolate_oklab(&gradient, ui.ctx().animate_bool("active".into(), is_active)).to_rgba8();
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
