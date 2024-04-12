#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::{BiquadDisplay, FrequencyDisplay, ScaleColorizrParams};
use colorgrad::{Color, Gradient};
use cozy_ui::centered;
use cozy_ui::widgets::button::toggle;
use cozy_util::filter::BiquadCoefficients;
use crossbeam::atomic::AtomicCell;
use lyon_path::math::Point;
use lyon_path::Path;
use lyon_tessellation::{BuffersBuilder, StrokeOptions, StrokeTessellator, StrokeVertexConstructor, VertexBuffers};
use nih_plug::context::gui::ParamSetter;
use nih_plug::params::Param;
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::{
    include_image, pos2, Color32, Grid, Mesh, Painter, Pos2, RichText, Ui, WidgetText, Window
};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use noise::{NoiseFn, OpenSimplex, Perlin};
use num_complex::Complex32;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use libsw::Sw;

use self::utils::{end_set, get_set, get_set_normalized, start_set};

mod utils;

fn knob<P, Text>(
    ui: &mut Ui,
    setter: &ParamSetter,
    param: &P,
    diameter: f32,
    description: Option<Text>,
) where
    P: Param,
    Text: Into<WidgetText>,
{
    cozy_ui::widgets::knob(
        ui,
        param.name(),
        Some(param.name().to_uppercase()),
        description,
        diameter,
        get_set_normalized(param, setter),
        start_set(param, setter),
        end_set(param, setter),
        param.default_normalized_value(),
    );
}

#[derive(Default, Serialize, Deserialize)]
struct EditorState {
    show_debug: bool,
    show_about: bool,
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
                    if ui.button("ABOUT").clicked() {
                        if ui.input(|input| input.modifiers.shift) {
                            state.show_debug = !state.show_debug;
                        } else {
                            state.show_about = !state.show_about;
                        }
                    }
                    toggle(
                        ui,
                        "delta",
                        Some("Takes the difference between the dry and wet signal, the \"Delta\""),
                        get_set(&params.delta, setter),
                        false,
                        &params.delta.name().to_ascii_uppercase(),
                        start_set(&params.delta, setter),
                        end_set(&params.delta, setter),
                    );
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
                            Some("The band gain used for the filters"),
                        );
                        knob(
                            ui,
                            setter,
                            &params.attack,
                            50.0,
                            Some("The attack for the filter envelope"),
                        );
                        knob(
                            ui,
                            setter,
                            &params.release,
                            50.0,
                            Some("The release for the filter envelope"),
                        );
                    });
                })
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                let filter_line_stopwatch = Sw::new_started();
                filter_line(ui, &biquads, &gradient);
                let draw_time = filter_line_stopwatch.elapsed();
                ui.memory_mut(|memory| memory.data.insert_temp("filter_elapsed".into(), draw_time));
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
        },
    )
}

struct ColoredVertex {
    position: Pos2,
    color: Color32
}

struct GradientVertex<'a, G: Gradient>(f64, &'a G, f32);

impl<G: Gradient> StrokeVertexConstructor<ColoredVertex> for GradientVertex<'_, G> {
    fn new_vertex(&mut self, vertex: lyon_tessellation::StrokeVertex) -> ColoredVertex {
        static NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(rand::random()));

        let GradientVertex(animation_position, gradient, interpolate) = self;
        let noise_value = norm(
                     NOISE.get([vertex.position_on_path().x as f64 * 0.002, *animation_position]) as f32,
                     -0.5,
                     0.5,
            );
        let gradient = gradient.at(noise_value);

        let color = Color::from_hsva(0.0, 0.0, noise_value, 1.0)
            .interpolate_oklab(&gradient, *interpolate)
            .to_rgba8();

        ColoredVertex { position: pos2(vertex.position().x, vertex.position().y), color: Color32::from_rgb(color[0], color[1], color[2]) }
    }
}

fn filter_line<G: Gradient>(ui: &Ui, biquads: &Arc<BiquadDisplay>, gradient: &G) {
    static ANIMATE_NOISE: Lazy<Perlin> = Lazy::new(|| Perlin::new(rand::random()));

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

    let active_biquads: Vec<BiquadCoefficients<_>> = biquads
        .iter()
        .flatten()
        .filter_map(AtomicCell::load)
        .collect();

    let is_active = !active_biquads.is_empty();

    #[allow(clippy::cast_possible_truncation)]
    for (idx, i) in (left_center.x as i32..=right_center.x as i32).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let freq = 5_000.0f32.mul_add(idx as f32 / len, 20.0);

        let result = active_biquads
            .iter()
            .map(|biquad| biquad.frequency_response(freq))
            .fold(Complex32::new(1.0, 0.0), |acc, resp| acc * resp);

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
    let mut path_builder = Path::builder();
    let first = points.first().unwrap();
    path_builder.begin(Point::new(first.x, first.y));
    for point in points.iter().skip(1) {
        path_builder.line_to(Point::new(point.x, point.y));
    }
    path_builder.end(false);

    let mut buffers: VertexBuffers<ColoredVertex, u32> = VertexBuffers::new();
    let mut vertex_builder = BuffersBuilder::new(&mut buffers, GradientVertex(animation_position + offset, gradient, ui.ctx().animate_bool("active".into(), is_active)));
    let mut tessellator = StrokeTessellator::new();

    tessellator.tessellate_path(&path_builder.build(), &StrokeOptions::default().with_line_width(3.0).with_line_join(lyon_path::LineJoin::Round), &mut vertex_builder).unwrap();

    let mut mesh = Mesh::default();
    for ColoredVertex {position, color} in buffers.vertices {
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
