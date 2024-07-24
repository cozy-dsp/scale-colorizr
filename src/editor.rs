#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::editor::utils::PowersOfTen;
use crate::spectrum::SpectrumOutput;
use crate::{FilterDisplay, FrequencyDisplay, ScaleColorizrParams, VERSION};
use colorgrad::{CatmullRomGradient, Color, Gradient};
use cozy_ui::centered;
use cozy_ui::colors::HIGHLIGHT_COL32;
use cozy_ui::widgets::button::toggle;
use cozy_ui::widgets::Knob;
use cozy_util::svf::SVF;
use crossbeam::atomic::AtomicCell;
use directories::ProjectDirs;
use libsw::Sw;
use nih_plug::context::gui::ParamSetter;
use nih_plug::midi::NoteEvent;
use nih_plug::params::enums::Enum;
use nih_plug::params::smoothing::AtomicF32;
use nih_plug::params::{EnumParam, Param};
use nih_plug::prelude::Editor;
use nih_plug_egui::egui::epaint::{PathShape, PathStroke};
use nih_plug_egui::egui::mutex::Mutex;
use nih_plug_egui::egui::{
    include_image, pos2, remap, remap_clamp, vec2, Align2, Color32, DragValue, FontData,
    FontDefinitions, FontId, Frame, Grid, Layout, Margin, Mesh, Pos2, Rect, RichText, Rounding,
    Sense, Shadow, Stroke, Ui, WidgetText, Window,
};
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use noise::{NoiseFn, OpenSimplex, Perlin};
use num_complex::Complex32;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::f32::consts::E;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strum_macros::Display;

use self::utils::{begin_set, end_set, get_set, get_set_normalized};

mod utils;

const FREQ_RANGE_START_HZ: f32 = 20.0;
const FREQ_RANGE_END_HZ: f32 = 15_000.0;

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

static CONFIG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    ProjectDirs::from("space", "cozy dsp", "Scale Colorizr")
        .map(|d| d.config_dir().to_path_buf())
        .expect("no home directory is set")
});
static CONFIG_FILE: Lazy<PathBuf> = Lazy::new(|| CONFIG_DIR.join("config.toml"));

#[derive(Default)]
struct EditorState {
    show_debug: bool,
    show_about: bool,
    show_settings: bool,
    config_io_error: Option<String>,
    options: EditorOptions,
}

#[derive(Default, Deserialize, Serialize, Display, PartialEq)]
enum GradientType {
    #[default]
    Rainbow,
    Lesbian,
    Bi,
    Trans,
    Ace,
    Custom,
}

#[derive(Default, Deserialize, Serialize)]
struct EditorOptions {
    gradient_type: GradientType,
    gradient_colors: Vec<[u8; 3]>,
}

pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(800, 600)
}

pub fn create(
    params: Arc<ScaleColorizrParams>,
    displays: Arc<FrequencyDisplay>,
    pre_spectrum: Arc<Mutex<SpectrumOutput>>,
    post_spectrum: Arc<Mutex<SpectrumOutput>>,
    sample_rate: Arc<AtomicF32>,
    midi_debug: Arc<AtomicCell<Option<NoteEvent<()>>>>,
    biquads: Arc<FilterDisplay>,
) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        EditorState::default(),
        |ctx, state| {
            cozy_ui::setup(ctx);
            ctx.style_mut(|style| {
                style.visuals.widgets.open.weak_bg_fill = cozy_ui::colors::WIDGET_BACKGROUND_COL32;
                style.visuals.selection.bg_fill =
                    cozy_ui::colors::HIGHLIGHT_COL32.gamma_multiply(0.5);
                style.visuals.window_highlight_topmost = false;
            });

            egui_extras::install_image_loaders(ctx);

            let mut fonts = FontDefinitions::default();

            fonts.font_data.insert(
                "0x".to_string(),
                FontData::from_static(include_bytes!("../assets/0xProto-Regular.ttf")),
            );

            fonts
                .families
                .entry(nih_plug_egui::egui::FontFamily::Name("0x".into()))
                .or_default()
                .insert(0, "0x".to_string());

            ctx.set_fonts(fonts);

            if let Err(e) = std::fs::create_dir_all(CONFIG_DIR.as_path()) {
                state.config_io_error = Some(format!("{e:?}"));
            } else {
                match CONFIG_FILE.as_path().try_exists() {
                    Ok(true) => match std::fs::read_to_string(CONFIG_FILE.as_path()) {
                        Ok(file) => match toml::from_str(&file) {
                            Ok(options) => state.options = options,
                            Err(e) => {
                                state.config_io_error = Some(format!("Invalid config - {e:?}"))
                            }
                        },
                        Err(e) => {
                            state.config_io_error = Some(format!("Can't read config - {e:?}"));
                        }
                    },
                    Ok(false) => {
                        if let Err(e) = fs::write(
                            CONFIG_FILE.as_path(),
                            toml::to_string_pretty(&EditorOptions::default()).unwrap(),
                        ) {
                            state.config_io_error =
                                Some(format!("Couldn't write default config - {e:?}"));
                        }
                    }
                    Err(e) => state.config_io_error = Some(format!("Can't read config - {e:?}")),
                }
            }
        },
        move |ctx, setter, state| {
            egui::TopBottomPanel::top("menu")
                .frame(Frame::side_top_panel(&ctx.style()).shadow(Shadow {
                    offset: vec2(0.0, 4.0),
                    blur: 8.0,
                    spread: 4.0,
                    color: Color32::BLACK,
                }))
                .show(ctx, |ui| {
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
                            params.delta.name().to_ascii_uppercase(),
                            get_set(&params.delta, setter),
                            begin_set(&params.delta, setter),
                            end_set(&params.delta, setter),
                        )
                        .description(
                            "Takes the difference between the dry and wet signal, the \"Delta\"",
                        ),
                    );
                        state.show_settings |= ui.button("SETTINGS").clicked();

                        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                            switch(ui, &params.filter_mode, setter);
                            if let Some(error) = &state.config_io_error {
                                ui.label(RichText::new("⚠").color(Color32::GOLD))
                                    .on_hover_text(error);
                            }
                        })
                    });
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
                        knob(
                            ui,
                            setter,
                            &params.band_width,
                            50.0,
                            "Changes how narrow the filters are",
                        );
                    });
                })
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                egui::Frame::canvas(ui.style())
                    .stroke(Stroke::new(2.0, Color32::DARK_GRAY))
                    .show(ui, |ui| {
                        let (_, rect) = ui.allocate_space(ui.available_size_before_wrap());

                        draw_log_grid(ui, rect);

                        draw_spectrum(
                            ui,
                            rect,
                            &pre_spectrum,
                            sample_rate.clone(),
                            Color32::GRAY.gamma_multiply(remap(
                                ui.ctx().animate_bool(
                                    "delta_active".into(),
                                    !params.delta.modulated_plain_value(),
                                ),
                                0.0..=1.0,
                                0.25..=1.0,
                            )),
                        );
                        draw_spectrum(
                            ui,
                            rect,
                            &post_spectrum,
                            sample_rate.clone(),
                            cozy_ui::colors::HIGHLIGHT_COL32.gamma_multiply(
                                ui.memory(|m| m.data.get_temp("active_amt".into()).unwrap_or(0.0)),
                            ),
                        );

                        let filter_line_stopwatch = Sw::new_started();
                        match state.options.gradient_type {
                            GradientType::Rainbow => {
                                draw_filter_line(ui, rect, &biquads, colorgrad::preset::rainbow())
                            }
                            GradientType::Lesbian => draw_filter_line(
                                ui,
                                rect,
                                &biquads,
                                colorgrad::GradientBuilder::new()
                                    .colors(&[
                                        Color::from_rgba8(213, 45, 0, 255),
                                        Color::from_rgba8(238, 118, 39, 255),
                                        Color::from_rgba8(255, 154, 86, 255),
                                        Color::from_rgba8(255, 255, 255, 255),
                                        Color::from_rgba8(209, 98, 164, 255),
                                        Color::from_rgba8(181, 86, 144, 255),
                                        Color::from_rgba8(163, 2, 98, 255),
                                    ])
                                    .mode(colorgrad::BlendMode::Oklab)
                                    .build::<CatmullRomGradient>()
                                    .unwrap(),
                            ),
                            GradientType::Bi => draw_filter_line(
                                ui,
                                rect,
                                &biquads,
                                colorgrad::GradientBuilder::new()
                                    .colors(&[
                                        Color::from_rgba8(214, 2, 12, 255),
                                        Color::from_rgba8(155, 79, 150, 255),
                                        Color::from_rgba8(0, 56, 168, 255),
                                    ])
                                    .mode(colorgrad::BlendMode::Oklab)
                                    .build::<CatmullRomGradient>()
                                    .unwrap(),
                            ),
                            GradientType::Trans => draw_filter_line(
                                ui,
                                rect,
                                &biquads,
                                colorgrad::GradientBuilder::new()
                                    .colors(&[
                                        Color::from_rgba8(91, 206, 250, 255),
                                        Color::from_rgba8(245, 169, 184, 255),
                                        Color::from_rgba8(255, 255, 255, 255),
                                    ])
                                    .mode(colorgrad::BlendMode::Oklab)
                                    .build::<CatmullRomGradient>()
                                    .unwrap(),
                            ),
                            GradientType::Ace => draw_filter_line(
                                ui,
                                rect,
                                &biquads,
                                colorgrad::GradientBuilder::new()
                                    .colors(&[
                                        Color::from_rgba8(0, 0, 0, 255),
                                        Color::from_rgba8(163, 163, 163, 255),
                                        Color::from_rgba8(255, 255, 255, 255),
                                        Color::from_rgba8(128, 0, 128, 255),
                                    ])
                                    .mode(colorgrad::BlendMode::Oklab)
                                    .build::<CatmullRomGradient>()
                                    .unwrap(),
                            ),
                            GradientType::Custom => draw_filter_line(
                                ui,
                                rect,
                                &biquads,
                                colorgrad::GradientBuilder::new()
                                    .colors(
                                        &state
                                            .options
                                            .gradient_colors
                                            .iter()
                                            .map(|[r, g, b]| Color::from_rgba8(*r, *g, *b, 255))
                                            .collect::<Vec<Color>>(),
                                    )
                                    .mode(colorgrad::BlendMode::Oklab)
                                    .build::<CatmullRomGradient>()
                                    .unwrap(),
                            ),
                        };
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
                    ui.collapsing("MIDI", |ui| ui.label(format!("{:?}", midi_debug.load())))
                });

            Window::new("ABOUT")
                .vscroll(true)
                .open(&mut state.show_about)
                .show(ctx, |ui| {
                    ui.image(include_image!("../assets/Cozy_logo.png"));
                    ui.vertical_centered(|ui| {
                        ui.heading(RichText::new("SCALE COLORIZR").strong());
                        ui.label(RichText::new(format!("Version {}", VERSION)).italics());
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
                        }).custom_parser(|s| params.voice_count.string_to_normalized_value(s).map(|v| v as f64)).speed(0.01).range(0.0..=1.0).custom_formatter(|v, _| {
                            params.voice_count.normalized_value_to_string(v as f32, false)
                        }))
                    });
                    ui.separator();
                    ui.label(RichText::new("This allows the filters to go above the nyquist frequency."));
                    ui.label(RichText::new("⚠ DO NOT TURN THIS OFF UNLESS YOU KNOW WHAT YOU ARE DOING. THIS WILL BLOW YOUR HEAD OFF ⚠").color(Color32::RED).strong());
                    ui.add(toggle("safety_switch", "SAFETY SWITCH", get_set(&params.safety_switch, setter), begin_set(&params.safety_switch, setter), end_set(&params.safety_switch, setter)));
                    ui.separator();
                    ui.heading("Gradient Editor");
                    let mut options_edited = egui::ComboBox::from_label("Gradient Type").selected_text(state.options.gradient_type.to_string()).show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Rainbow, GradientType::Rainbow.to_string()).changed() ||
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Lesbian, GradientType::Lesbian.to_string()).changed() ||
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Bi, GradientType::Bi.to_string()).changed() ||
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Trans, GradientType::Trans.to_string()).changed() ||
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Ace, GradientType::Ace.to_string()).changed() ||
                        ui.selectable_value(&mut state.options.gradient_type, GradientType::Custom, GradientType::Custom.to_string()).changed()
                    }).inner.is_some_and(|i| i);

                    if let GradientType::Custom = state.options.gradient_type {
                        let to_remove: Vec<_> = state.options.gradient_colors.iter_mut().enumerate().filter_map(|(i, color)| ui.horizontal(|ui| {
                            let changed = ui.color_edit_button_srgb(color).changed();
                            if ui.button("Delete").clicked() {
                                options_edited = true;
                                Some(i)
                            } else {
                                options_edited |= changed;
                                None
                            }
                        }).inner).collect();

                        for i in to_remove {
                            state.options.gradient_colors.remove(i);
                        }

                        if ui.button("Add Color").clicked() {
                            options_edited = true;
                            state.options.gradient_colors.push([100, 0, 0]);
                        }
                    }

                    if options_edited {
                        if let Err(e) = fs::write(CONFIG_FILE.as_path(), toml::to_string_pretty(&state.options).unwrap()) {
                            state.config_io_error = Some(format!("Couldn't write config: {e:?}"));
                        }
                    }
                });
        },
    )
}

fn draw_log_grid(ui: &Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    let log_min = FREQ_RANGE_START_HZ.log10();
    let log_max = FREQ_RANGE_END_HZ.log10();

    let mut previous = 10.0;
    for max in PowersOfTen::new(10.0, 20_000.0) {
        for freq in (previous as i32..=max as i32).step_by(max as usize / 10) {
            let freq = freq.max(20) as f32;
            let x = ((freq.log10() - log_min) * (rect.width() - 1.0)) / (log_max - log_min)
                + rect.left();
            let x2 = (((freq - (max / 20.0)).log10() - log_min) * (rect.width() - 1.0))
                / (log_max - log_min)
                + rect.left();
            painter.vline(
                x,
                rect.y_range(),
                Stroke::new(1.0, Color32::DARK_GRAY.gamma_multiply(0.5)),
            );

            if freq == max {
                painter.text(
                    pos2(x + 5.0, rect.bottom() - 10.0),
                    Align2::LEFT_CENTER,
                    if freq >= 1000.0 {
                        format!("{:.0}k", max / 1000.0)
                    } else {
                        format!("{freq:.0}")
                    },
                    FontId::new(10.0, egui::FontFamily::Name("0x".into())),
                    Color32::DARK_GRAY,
                );
            }

            painter.vline(
                x2,
                rect.y_range(),
                Stroke::new(1.0, Color32::DARK_GRAY.gamma_multiply(0.25)),
            );
        }
        previous = max;
    }
}

fn draw_spectrum(
    ui: &Ui,
    rect: Rect,
    spectrum: &Mutex<SpectrumOutput>,
    sample_rate: Arc<AtomicF32>,
    color: Color32,
) {
    let painter = ui.painter_at(rect);
    let mut lock = spectrum.lock();
    let spectrum_data = lock.read();
    let nyquist = sample_rate.load(std::sync::atomic::Ordering::Relaxed) / 2.0;

    let bin_freq = |bin_idx: f32| (bin_idx / spectrum_data.len() as f32) * nyquist;
    let magnitude_height = |magnitude: f32| {
        let magnitude_db = nih_plug::util::gain_to_db(magnitude);
        (magnitude_db + 80.0) / 100.0
    };
    let bin_t = |bin_idx: f32| {
        (bin_freq(bin_idx).log10() - FREQ_RANGE_START_HZ.log10())
            / (FREQ_RANGE_END_HZ.log10() - FREQ_RANGE_START_HZ.log10())
    };

    let points: Vec<Pos2> = spectrum_data
        .iter()
        .enumerate()
        .filter_map(|(idx, magnitude)| {
            let t = bin_t(idx as f32).max(0.0);

            if t > 1.0 {
                return None;
            }

            let x_coord = rect.lerp_inside(vec2(t, 0.0)).x;

            let height = magnitude_height(*magnitude);

            Some(pos2(x_coord, rect.top() + (rect.height() * (1.0 - height))))
        })
        .collect();

    let color_bg = color.gamma_multiply(0.25);

    for [left, right] in points.array_windows() {
        let mut mesh = Mesh::default();
        mesh.colored_vertex(*left, color_bg);
        mesh.colored_vertex(*right, color_bg);

        let bottom_left = pos2(left.x, rect.bottom());
        let bottom_right = pos2(right.x, rect.bottom());

        mesh.colored_vertex(bottom_right, color_bg);
        mesh.colored_vertex(bottom_left, color_bg);

        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(3, 2, 0);

        painter.add(mesh);
    }

    painter.add(PathShape::line(points, Stroke::new(1.5, color)));
}

fn draw_filter_line<G: Gradient + Sync + Send + 'static>(
    ui: &mut Ui,
    rect: Rect,
    biquads: &Arc<FilterDisplay>,
    gradient: G,
) {
    static ANIMATE_NOISE: Lazy<Perlin> = Lazy::new(|| Perlin::new(rand::random()));

    let painter = ui.painter_at(rect);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut points = Vec::with_capacity(rect.width().round() as usize);
    let mut sampled_frequencies = Vec::with_capacity(rect.width().round() as usize);

    let active_biquads: Vec<SVF<_>> = biquads
        .iter()
        .flatten()
        .filter_map(AtomicCell::load)
        .collect();

    let is_active = !active_biquads.is_empty();

    let log_min = FREQ_RANGE_START_HZ.log10();
    let log_max = FREQ_RANGE_END_HZ.log10();

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
            remap(
                (result.norm().log10() * 0.05 + 0.5).max(0.0),
                0.0..=1.0,
                rect.bottom_up_range(),
            ),
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
    let interpolate = ui.ctx().animate_bool("active".into(), is_active);
    ui.memory_mut(|m| m.data.insert_temp("active_amt".into(), interpolate));

    painter.add(PathShape::line(
        points,
        PathStroke::new_uv(3.0, move |bounds, pos| {
            static NOISE: Lazy<OpenSimplex> = Lazy::new(|| OpenSimplex::new(rand::random()));

            let noise_value = remap(
                NOISE.get([
                    remap_clamp(pos.x, bounds.x_range(), 0.0..=1.5) as f64,
                    animation_position + offset,
                ]) as f32,
                -0.5..=0.5,
                0.0..=1.0,
            );
            let gradient = gradient.at(noise_value);

            let color = Color::from_hsva(0.0, 0.0, noise_value, 1.0)
                .interpolate_oklab(&gradient, interpolate)
                .to_rgba8();

            Color32::from_rgba_premultiplied(color[0], color[1], color[2], color[3])
        }),
    ));
}

fn switch<T: Enum + PartialEq>(ui: &mut Ui, param: &EnumParam<T>, setter: &ParamSetter) {
    ui.horizontal(|ui| {
        Frame::default()
            .rounding(Rounding::same(5.0))
            .fill(Color32::DARK_GRAY)
            .inner_margin(Margin::same(4.0))
            .show(ui, |ui| {
                for variant in T::variants().iter().rev() {
                    let galley = WidgetText::from(variant.to_uppercase()).into_galley(
                        ui,
                        None,
                        50.0,
                        FontId::new(10.0, egui::FontFamily::Name("0x".into())),
                    );

                    let (rect, response) =
                        ui.allocate_exact_size(galley.rect.size(), Sense::click());
                    let response = response.on_hover_cursor(egui::CursorIcon::Grab);
                    ui.painter_at(rect).galley(
                        pos2(
                            rect.center().x - galley.size().x / 2.0,
                            0.5f32.mul_add(-galley.size().y, rect.center().y),
                        ),
                        galley,
                        if param.modulated_normalized_value()
                            == param.string_to_normalized_value(variant).unwrap()
                        {
                            HIGHLIGHT_COL32
                        } else {
                            Color32::WHITE
                        },
                    );

                    if response.clicked() {
                        setter.begin_set_parameter(param);
                        setter.set_parameter_normalized(
                            param,
                            param.string_to_normalized_value(variant).unwrap(),
                        );
                        setter.end_set_parameter(param);
                    }
                }
            });
    });
}
