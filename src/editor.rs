use std::sync::Arc;
use nih_plug::prelude::Editor;
use nih_plug_egui::{create_egui_editor, EguiState, egui};
use nih_plug_egui::egui::{Frame, Window};
use crate::FrequencyDisplay;

pub fn default_editor_state() -> Arc<EguiState> {
    EguiState::from_size(800, 600)
}

pub fn create_editor(state: Arc<EguiState>, displays: Arc<FrequencyDisplay>) -> Option<Box<dyn Editor>> {
    create_egui_editor(state, (), |_, _| {}, move |ctx, setter, _| {
        egui::CentralPanel::default().frame(Frame::none()).show(ctx, |ui| {
           Window::new("DEBUG").vscroll(true).show(ctx, |ui| {
               for (idx, display) in displays.iter().enumerate() {
                   ui.group(|ui| {
                       ui.label(format!("VOICE {idx}"));
                       ui.horizontal(|ui| {
                           for filter in display {
                               ui.label(filter.load().map(|v| format!("FREQ: {v}")).unwrap_or("UNUSED".to_string()));
                           }
                       });
                   });
               }
           })
        });
    })
}