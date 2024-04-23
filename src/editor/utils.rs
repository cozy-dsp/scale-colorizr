use cozy_ui::util::get_set::Operation;
use nih_plug::{context::gui::ParamSetter, params::Param};

pub fn begin_set<'a, P>(param: &'a P, setter: &'a ParamSetter<'a>) -> impl Fn() + 'a
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
) -> impl FnMut(Operation<f32>) -> f32 + 'a
where
    P: Param,
{
    |value| {
        if let Operation::Set(value) = value {
            setter.set_parameter_normalized(param, value);
            return value;
        }

        param.unmodulated_normalized_value()
    }
}

pub fn get_set<'a, P>(
    param: &'a P,
    setter: &'a ParamSetter<'a>,
) -> impl FnMut(Operation<P::Plain>) -> P::Plain + 'a
where
    P: Param,
    P::Plain: Copy,
{
    |value| {
        if let Operation::Set(value) = value {
            setter.set_parameter(param, value);
            return value;
        }

        param.unmodulated_plain_value()
    }
}
