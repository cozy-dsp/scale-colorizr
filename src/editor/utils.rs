use nih_plug::{context::gui::ParamSetter, params::Param};
use nih_plug_egui::egui::Ui;

pub fn start_set<'a, P>(param: &'a P, setter: &'a ParamSetter<'a>) -> impl Fn() + 'a
where
    P: Param + 'a,
{
    || {
        setter.begin_set_parameter(param);
    }
}

pub fn end_set<'a, P>(param: &'a P, setter: &'a ParamSetter<'a>) -> impl Fn() + 'a
where
    P: Param + 'a,
{
    || {
        setter.end_set_parameter(param);
    }
}

pub fn get_set_normalized<'a, P>(
    param: &'a P,
    setter: &'a ParamSetter<'a>,
) -> impl FnMut(Option<f32>) -> f32 + 'a
where
    P: Param,
{
    |value| {
        if let Some(value) = value {
            setter.set_parameter_normalized(param, value);
            return value;
        }

        param.unmodulated_normalized_value()
    }
}

pub fn get_set<'a, P>(
    param: &'a P,
    setter: &'a ParamSetter<'a>,
) -> impl FnMut(Option<P::Plain>) -> P::Plain + 'a
where
    P: Param,
    P::Plain: Copy,
{
    |value| {
        if let Some(value) = value {
            setter.set_parameter(param, value);
            return value;
        }

        param.unmodulated_plain_value()
    }
}


// special thanks - https://gist.github.com/juancampa/faf3525beefa477babdad237f5e81ffe
pub fn centerer(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
      let id = ui.id().with("_centerer");
      let last_width: Option<f32> = ui.memory_mut(|mem| mem.data.get_temp(id));
      if let Some(last_width) = last_width {
        ui.add_space((ui.available_width() - last_width) / 2.0);
      }
      let res = ui
        .scope(|ui| {
          add_contents(ui);
        })
        .response;
      let width = res.rect.width();
      ui.memory_mut(|mem| mem.data.insert_temp(id, width));

      // Repaint if width changed
      match last_width {
        None => ui.ctx().request_repaint(),
        Some(last_width) if last_width != width => ui.ctx().request_repaint(),
        Some(_) => {}
      }
    });
  }
