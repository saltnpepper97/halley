use crate::backend::interface::BackendView;
use crate::compositor::root::Halley;

use super::ctx::InputCtx;
use super::keyboard::handle_keyboard_input;
use super::pointer::axis::handle_pointer_axis_input;
use smithay::backend::input::{AxisRelativeDirection, AxisSource, ButtonState};

pub(crate) enum BackendInputEventData {
    Keyboard {
        code: u32,
        pressed: bool,
    },
    PointerMotionAbsolute {
        ws_w: i32,
        ws_h: i32,
        sx: f32,
        sy: f32,
        delta_x: f64,
        delta_y: f64,
        delta_x_unaccel: f64,
        delta_y_unaccel: f64,
        time_usec: u64,
    },
    PointerButton {
        button_code: u32,
        state: ButtonState,
    },
    PointerAxis {
        source: AxisSource,
        amount_v120_horizontal: Option<f64>,
        amount_v120_vertical: Option<f64>,
        amount_horizontal: Option<f64>,
        amount_vertical: Option<f64>,
        relative_direction_horizontal: AxisRelativeDirection,
        relative_direction_vertical: AxisRelativeDirection,
    },
}

pub(crate) fn handle_backend_input_event<B: BackendView>(
    st: &mut Halley,
    ctx: &InputCtx<'_, B>,
    event: BackendInputEventData,
) {
    st.note_input_activity();

    match event {
        BackendInputEventData::Keyboard { code, pressed } => {
            handle_keyboard_input(st, ctx, code, pressed);
        }
        BackendInputEventData::PointerMotionAbsolute {
            ws_w,
            ws_h,
            sx,
            sy,
            delta_x,
            delta_y,
            delta_x_unaccel,
            delta_y_unaccel,
            time_usec,
        } => {
            super::pointer::motion::handle_pointer_motion_absolute(
                st,
                ctx,
                ws_w,
                ws_h,
                sx,
                sy,
                (delta_x, delta_y),
                (delta_x_unaccel, delta_y_unaccel),
                time_usec,
            );
        }
        BackendInputEventData::PointerButton { button_code, state } => {
            super::pointer::button::handle_pointer_button_input(st, ctx, button_code, state);
        }
        BackendInputEventData::PointerAxis {
            source,
            amount_v120_horizontal,
            amount_v120_vertical,
            amount_horizontal,
            amount_vertical,
            relative_direction_horizontal,
            relative_direction_vertical,
        } => {
            handle_pointer_axis_input(
                st,
                ctx,
                source,
                amount_v120_horizontal,
                amount_v120_vertical,
                amount_horizontal,
                amount_vertical,
                relative_direction_horizontal,
                relative_direction_vertical,
            );
        }
    }
    st.run_maintenance_if_needed(std::time::Instant::now());
}
