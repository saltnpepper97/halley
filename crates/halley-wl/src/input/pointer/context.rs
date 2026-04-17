use crate::compositor::root::Halley;
use crate::spatial::screen_to_world;

pub(super) struct PointerScreenContext {
    pub(super) monitor: String,
    pub(super) ws_w: i32,
    pub(super) ws_h: i32,
    pub(super) global_sx: f32,
    pub(super) global_sy: f32,
    pub(super) local_sx: f32,
    pub(super) local_sy: f32,
    pub(super) world: halley_core::field::Vec2,
}

#[inline]
pub(super) fn clamp_screen_to_workspace(ws_w: i32, ws_h: i32, sx: f32, sy: f32) -> (f32, f32) {
    let max_x = (ws_w.max(1) - 1) as f32;
    let max_y = (ws_h.max(1) - 1) as f32;
    (sx.clamp(0.0, max_x), sy.clamp(0.0, max_y))
}

#[inline]
pub(super) fn clamp_screen_to_monitor(st: &Halley, name: &str, sx: f32, sy: f32) -> (f32, f32) {
    if let Some(monitor) = st.model.monitor_state.monitors.get(name) {
        let max_x = (monitor.offset_x + monitor.width - 1) as f32;
        let max_y = (monitor.offset_y + monitor.height - 1) as f32;
        (
            sx.clamp(monitor.offset_x as f32, max_x),
            sy.clamp(monitor.offset_y as f32, max_y),
        )
    } else {
        (sx, sy)
    }
}

pub(super) fn pointer_screen_context_for_monitor(
    st: &mut Halley,
    monitor: String,
    screen: (f32, f32),
    activate_monitor: bool,
    clamp_to_monitor: bool,
) -> PointerScreenContext {
    let (global_sx, global_sy) = if clamp_to_monitor {
        clamp_screen_to_monitor(st, monitor.as_str(), screen.0, screen.1)
    } else {
        screen
    };

    if activate_monitor {
        st.set_interaction_monitor(monitor.as_str());
        let _ = st.activate_monitor(monitor.as_str());
    }

    let (ws_w, ws_h, local_sx, local_sy) =
        st.local_screen_in_monitor(monitor.as_str(), global_sx, global_sy);
    let world = screen_to_world(st, ws_w, ws_h, local_sx, local_sy);

    PointerScreenContext {
        monitor,
        ws_w,
        ws_h,
        global_sx,
        global_sy,
        local_sx,
        local_sy,
        world,
    }
}
