use crate::{BiquadDisplay, FrequencyDisplay, ScaleColorizrParams};
use nih_plug::params::smoothing::AtomicF32;
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::epaint::PathShape;
use nih_plug_egui::egui::{Color32, Grid, Painter, Pos2, Shape, Stroke, Ui, Window};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use num_complex::Complex32;
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

#[derive(Default, Serialize, Deserialize)]
struct EditorState {
    show_debug: bool
}

pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(800, 600)
}

pub fn create_editor(
    state: Arc<EguiState>,
    sample_rate: Arc<AtomicF32>,
    params: Arc<ScaleColorizrParams>,
    displays: Arc<FrequencyDisplay>,
    biquads: Arc<BiquadDisplay>
) -> Option<Box<dyn Editor>> {
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

            egui::CentralPanel::default()
                .show(ctx, |ui| {
                    ui.add(nih_plug_egui::egui::widgets::Slider::from_get_set(2.0..=40.0, |new_value| {
                        match new_value {
                            Some(v) => {
                                let new_value = nih_plug::util::gain_to_db(v as f32);

                                setter.begin_set_parameter(&params.gain);
                                setter.set_parameter(&params.gain, new_value);
                                setter.end_set_parameter(&params.gain);

                                v
                            }
                            None => nih_plug::util::gain_to_db(params.gain.value()) as f64
                        }
                    }));

                    let debug_complex = filter_line(ui, biquads.clone(), sample_rate.load(std::sync::atomic::Ordering::Relaxed));

                    let debug_window = Window::new("DEBUG").vscroll(true).open(&mut state.show_debug);
                    debug_window.show(ctx, |ui| {
                        ui.collapsing("VOICES", |ui| {
                            for (idx, display) in displays.iter().enumerate() {
                                ui.group(|ui| {
                                    ui.label(format!("VOICE {idx}"));
                                    Grid::new(format!("voice-{idx}")).show(ui, |ui| {
                                        for (i, filter) in display.iter().enumerate() {
                                            ui.label(
                                                filter
                                                    .load()
                                                    .map(|v| format!("FREQ: {v}"))
                                                    .unwrap_or("UNUSED".to_string()),
                                            );
    
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
                                Grid::new(format!("complex")).show(ui, |ui| {
                                    for (i, filter) in debug_complex.iter().enumerate() {
                                        ui.label(
                                            format!("{}", filter)
                                        );

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

fn filter_line(ui: &mut Ui, biquads: Arc<BiquadDisplay>, sample_rate: f32) -> Vec<Complex32> {
    let mut debug = Vec::new();
    let painter = Painter::new(ui.ctx().clone(), ui.layer_id(), ui.available_rect_before_wrap());
    let left_center = painter.clip_rect().left_center();
    let right_center = painter.clip_rect().right_center();

    let len = left_center.x - right_center.x;

    let mut points = Vec::with_capacity(len as usize);

    for (idx, i) in (left_center.x as i32..=right_center.x as i32).enumerate() {
        let freq = 20.0 + 20_000.0 * (idx as f32 / len);
        let mut result = Complex32::new(1.0, 0.0);
        for biquad in biquads.iter().flatten().filter_map(|f| f.load()) {
            result *= biquad.transfer_function((Complex32::i() * TAU * (freq / sample_rate)).exp());
            debug.push(result);
        }
        points.push(Pos2::new(i as f32, left_center.y - result.norm()));
    }

    painter.add(PathShape::line(points, Stroke::new(1.5, Color32::RED)));

    debug
}
