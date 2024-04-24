#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::editor::utils::PowersOfTen;
use crate::{BiquadDisplay, FrequencyDisplay, ScaleColorizrParams};
use colorgrad::{Color, Gradient};
use cozy_ui::centered;
use cozy_ui::widgets::button::toggle;
use cozy_ui::widgets::Knob;
use cozy_util::filter::BiquadCoefficients;
use crossbeam::atomic::AtomicCell;
use libsw::Sw;
use lyon_path::math::Point;
use lyon_path::Path;
use lyon_tessellation::{
    BuffersBuilder, StrokeOptions, StrokeTessellator, StrokeVertexConstructor, VertexBuffers,
};
use nih_plug::context::gui::ParamSetter;
use nih_plug::params::Param;
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::{
    include_image, pos2, Align2, Color32, DragValue, FontId, Grid, Mesh, Pos2, RichText, Stroke, Ui, WidgetText, Window
};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use noise::{NoiseFn, OpenSimplex, Perlin};
use num_complex::Complex32;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::f32::consts::E;
use std::sync::Arc;
use std::time::Duration;

use self::utils::{begin_set, end_set, get_set, get_set_normalized};

mod utils;

fn knob<P, Text>(ui: &mut Ui, setter: &ParamSetter, param: &P, diameter: f32, description: Text)
where
    P: Param,
    Text: Into<WidgetText>,
{
    ui.add(
        Knob::new(
            param.name(),
            diameter,
            get_set_normalized(param, setter),
            begin_set(param, setter),
            end_set(param, setter),
        )
        .label(param.name().to_ascii_uppercase())
        .description(description)
        .modulated_value(param.modulated_normalized_value())
        .default_value(param.default_normalized_value()),
    );
}

#[derive(Default, Serialize, Deserialize)]
struct EditorState {
    show_debug: bool,
    show_about: bool,
    show_settings: bool,
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
        |ctx, _| {
            cozy_ui::setup(ctx);
            egui_extras::install_image_loaders(ctx);
        },
        move |ctx, setter, state| {
            egui::TopBottomPanel::top("menu").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let about_debug = if ui.input(|input| input.modifiers.shift) {
                        &mut state.show_debug
                    } else {
                        &mut state.show_about
                    };
                    *about_debug |= ui.button("ABOUT").clicked();
                    ui.add(
                        toggle(
                            "delta",
                            &params.delta.name().to_ascii_uppercase(),
                            get_set(&params.delta, setter),
                            begin_set(&params.delta, setter),
                            end_set(&params.delta, setter),
                        )
                        .description(
                            "Takes the difference between the dry and wet signal, the \"Delta\"",
                        ),
                    );
                    state.show_settings |= ui.button("SETTINGS").clicked();
                })
            });

            egui::TopBottomPanel::bottom("controls").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    centered(ctx, ui, |ui| {
                        knob(
                            ui,
                            setter,
                            &params.gain,
                            50.0,
                            "The band gain used for the filters",
                        );
                        knob(
                            ui,
                            setter,
                            &params.attack,
                            50.0,
                            "The attack for the filter envelope",
                        );
                        knob(
                            ui,
                            setter,
                            &params.release,
                            50.0,
                            "The release for the filter envelope",
                        );
                    });
                })
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                egui::Frame::canvas(ui.style())
                    .stroke(Stroke::new(2.0, Color32::DARK_GRAY))
                    .show(ui, |ui| {
                        let filter_line_stopwatch = Sw::new_started();
                        filter_line(ui, &biquads, &gradient);
                        let draw_time = filter_line_stopwatch.elapsed();
                        ui.memory_mut(|memory| {
                            memory.data.insert_temp("filter_elapsed".into(), draw_time)
                        });
                    });
            });

            Window::new("DEBUG")
                .vscroll(true)
                .open(&mut state.show_debug)
                .show(ctx, |ui| {
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
                            ui.label(format!(
                                "drawing filter line took: {:.2?}",
                                ui.memory(|memory| memory
                                    .data
                                    .get_temp::<Duration>("filter_elapsed".into())
                                    .unwrap_or_default())
                            ));
                        });
                        ui.group(|ui| {
                            ui.label(format!(
                                "{:?}",
                                ui.memory(|m| m
                                    .data
                                    .get_temp::<Vec<f32>>("sampled_frequencies".into())
                                    .unwrap_or_default())
                            ))
                        })
                    });
                });

            Window::new("ABOUT")
                .vscroll(true)
                .open(&mut state.show_about)
                .show(ctx, |ui| {
                    ui.image(include_image!("../assets/Cozy_logo.png"));
                    ui.vertical_centered(|ui| {
                        ui.heading(RichText::new("SCALE COLORIZR").strong());
                        ui.label(
                            RichText::new(format!("Version {}", env!("VERGEN_GIT_DESCRIBE")))
                                .italics(),
                        );
                        ui.hyperlink_to("Homepage", env!("CARGO_PKG_HOMEPAGE"));
                        ui.separator();
                        ui.heading(RichText::new("Credits"));
                        ui.label("Original concept by Virtual Riot");
                        ui.label("Plugin by joe sorensen");
                        ui.label("cozy dsp branding and design by gordo");
                    });
                });

            Window::new("SETTINGS")
                .open(&mut state.show_settings)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Voice Count");
                        ui.add(DragValue::from_get_set(|value| {
                            match value {
                                Some(v) => {
                                    setter.begin_set_parameter(&params.voice_count);
                                    setter.set_parameter_normalized(&params.voice_count, v as f32);
                                    setter.end_set_parameter(&params.voice_count);
                                    v
                                },
                                None => {
                                    params.voice_count.modulated_normalized_value() as f64
                                }
                            }
                        }).custom_parser(|s| params.voice_count.string_to_normalized_value(s).map(|v| v as f64)).speed(0.01).clamp_range(0.0..=1.0).custom_formatter(|v, _| {
                            params.voice_count.normalized_value_to_string(v as f32, false)
                        }))
                    });
                    ui.separator();
                    ui.label(RichText::new("This allows the filters to go above the nyquist frequency."));
                    ui.label(RichText::new("⚠ DO NOT TURN THIS OFF UNLESS YOU KNOW WHAT YOU ARE DOING. THIS WILL BLOW YOUR HEAD OFF ⚠").color(Color32::RED).strong());
                    ui.add(toggle("safety_switch", "SAFETY SWITCH", get_set(&params.safety_switch, setter), begin_set(&params.safety_switch, setter), end_set(&params.safety_switch, setter)));
                });
        },
    )
}

struct ColoredVertex {
    position: Pos2,
    color: Color32,
}

struct GradientVertex<'a, G: Gradient>(f64, &'a G, f32);

impl<G: Gradient> StrokeVertexConstructor<ColoredVertex> for GradientVertex<'_, G> {
    fn new_vertex(&mut self, vertex: lyon_tessellation::StrokeVertex) -> ColoredVertex {
        static NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(rand::random()));

        let GradientVertex(animation_position, gradient, interpolate) = self;
        let noise_value = norm(
            NOISE.get([
                vertex.position_on_path().x as f64 * 0.002,
                *animation_position,
            ]) as f32,
            -0.5,
            0.5,
        );
        let gradient = gradient.at(noise_value);

        let color = Color::from_hsva(0.0, 0.0, noise_value, 1.0)
            .interpolate_oklab(&gradient, *interpolate)
            .to_rgba8();

        ColoredVertex {
            position: pos2(vertex.position().x, vertex.position().y),
            color: Color32::from_rgb(color[0], color[1], color[2]),
        }
    }
}

fn filter_line<G: Gradient>(ui: &mut Ui, biquads: &Arc<BiquadDisplay>, gradient: &G) {
    static ANIMATE_NOISE: Lazy<Perlin> = Lazy::new(|| Perlin::new(rand::random()));
    let (_, rect) = ui.allocate_space(ui.available_size_before_wrap());

    let painter = ui.painter_at(rect);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut points = Vec::with_capacity(rect.width().round() as usize);
    let mut sampled_frequencies = Vec::with_capacity(rect.width().round() as usize);

    let active_biquads: Vec<BiquadCoefficients<_>> = biquads
        .iter()
        .flatten()
        .filter_map(AtomicCell::load)
        .collect();

    let is_active = !active_biquads.is_empty();

    let log_min = 20.0_f32.log10();
    let log_max = 15_000_f32.log10();

    let mut previous = 10.0;
    for max in PowersOfTen::new(10.0, 20_000.0) {
        for freq in (previous as i32..=max as i32).step_by(max as usize / 10) {
            let freq = freq.max(20) as f32;
            let x = ((freq.log10() - log_min) * (rect.width() - 1.0)) / (log_max - log_min)
                + rect.left();
            let x2 = (((freq - (max as f32 / 20.0)).log10() - log_min) * (rect.width() - 1.0))
                / (log_max - log_min)
                + rect.left();
            painter.vline(
                x,
                rect.y_range(),
                Stroke::new(1.0, Color32::DARK_GRAY.gamma_multiply(0.5)),
            );

            if freq == max {
                painter.text(pos2(x + 5.0, rect.bottom() - 10.0), Align2::LEFT_CENTER, if freq >= 1000.0 {
                    format!("{:.0}k", max / 1000.0)
                } else {
                    format!("{freq:.0}")
                }, FontId::monospace(10.0), Color32::DARK_GRAY);
            }

            painter.vline(
                x2,
                rect.y_range(),
                Stroke::new(1.0, Color32::DARK_GRAY.gamma_multiply(0.25)),
            );
        }
        previous = max;
    }

    #[allow(clippy::cast_possible_truncation)]
    for i in rect.left() as i32..=rect.right() as i32 {
        let x = i as f32;
        let freq = ((log_min * (rect.left() + rect.width() - x - 1.0)
            + log_max * (x - rect.left()))
            / ((rect.width() - 1.0) * E.log10()))
        .exp();

        sampled_frequencies.push(freq);

        let result = active_biquads
            .iter()
            .map(|biquad| biquad.frequency_response(freq))
            .fold(Complex32::new(1.0, 0.0), |acc, resp| acc * resp);

        points.push(Pos2::new(
            x,
            result.norm().log10().mul_add(-50.0, rect.center().y),
        ));
    }

    ui.memory_mut(|m| {
        m.data
            .insert_temp("sampled_frequencies".into(), sampled_frequencies)
    });

    // DISGUSTING: i would MUCH rather meshify the line so i can apply shaders
    // but i couldn't get it to work, so i'm doing this terribleness instead.
    let animation_position = ui.ctx().frame_nr() as f64 * 0.005;
    let offset = ANIMATE_NOISE.get([animation_position * 0.01, 0.0]);
    let mut path_builder = Path::builder();
    let first = points.first().unwrap();
    path_builder.begin(Point::new(first.x, first.y));
    for point in points.iter().skip(1) {
        path_builder.line_to(Point::new(point.x, point.y));
    }
    path_builder.end(false);

    let mut buffers: VertexBuffers<ColoredVertex, u32> = VertexBuffers::new();
    let mut vertex_builder = BuffersBuilder::new(
        &mut buffers,
        GradientVertex(
            animation_position + offset,
            gradient,
            ui.ctx().animate_bool("active".into(), is_active),
        ),
    );
    let mut tessellator = StrokeTessellator::new();

    tessellator
        .tessellate_path(
            &path_builder.build(),
            &StrokeOptions::default()
                .with_line_width(3.0)
                .with_line_join(lyon_path::LineJoin::Round),
            &mut vertex_builder,
        )
        .unwrap();

    let mut mesh = Mesh::default();
    for ColoredVertex { position, color } in buffers.vertices {
        mesh.colored_vertex(position, color)
    }

    mesh.indices = buffers.indices;

    painter.add(mesh);

    // for (idx, p) in points.array_windows().enumerate() {
    //     let x = idx as f64 * 0.002;
    //     let noise_value = norm(
    //         NOISE.get([x, animation_position + offset]) as f32,
    //         -0.5,
    //         0.5,
    //     );
    //     let gradient = gradient.at(noise_value);
    //     let color = Color::from_hsva(0.0, 0.0, noise_value, 1.0)
    //         .interpolate_oklab(&gradient, ui.ctx().animate_bool("active".into(), is_active))
    //         .to_rgba8();
    //     painter.line_segment(
    //         *p,
    //         Stroke::new(1.5, Color32::from_rgb(color[0], color[1], color[2])),
    //     );
    // }
}

fn norm(t: f32, a: f32, b: f32) -> f32 {
    (t - a) * (1.0 / (b - a))
}
