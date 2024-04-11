use nih_plug::{context::gui::ParamSetter, params::Param};

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

