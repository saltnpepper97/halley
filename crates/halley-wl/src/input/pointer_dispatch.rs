use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::SERIAL_COUNTER;

use crate::state::Halley;

use super::pointer_focus::pointer_focus_for_screen;
use super::pointer_frame::{ButtonFrame, now_millis_u32};

pub(super) fn dispatch_pointer_button(
    st: &mut Halley,
    frame: ButtonFrame,
    resize_preview: Option<crate::interaction::types::ResizeCtx>,
    button_code: u32,
    button_state: smithay::backend::input::ButtonState,
) {
    let Some(pointer) = st.platform.seat.get_pointer() else {
        return;
    };
    let focus = pointer_focus_for_screen(
        st,
        frame.ws_w,
        frame.ws_h,
        frame.sx,
        frame.sy,
        std::time::Instant::now(),
        resize_preview,
    );
    let motion_serial = SERIAL_COUNTER.next_serial();
    let button_serial = SERIAL_COUNTER.next_serial();
    let location = if focus
        .as_ref()
        .is_some_and(|(surface, _)| st.is_layer_surface(surface))
    {
        (frame.sx as f64, frame.sy as f64).into()
    } else {
        let cam_scale = st.camera_render_scale() as f64;
        (frame.sx as f64 / cam_scale, frame.sy as f64 / cam_scale).into()
    };
    pointer.motion(
        st,
        focus,
        &MotionEvent {
            location,
            serial: motion_serial,
            time: now_millis_u32(),
        },
    );
    pointer.button(
        st,
        &ButtonEvent {
            serial: button_serial,
            time: now_millis_u32(),
            button: button_code,
            state: button_state,
        },
    );
    pointer.frame(st);
}
