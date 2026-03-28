use halley_config::{KeyModifiers, PointerBindingAction};
use smithay::reexports::wayland_server::Resource;

use crate::interaction::types::ModState;
use crate::spatial::screen_to_world;
use crate::state::Halley;

use super::utils::modifier_active;

#[derive(Clone, Copy)]
pub(super) struct ButtonFrame {
    pub(super) ws_w: i32,
    pub(super) ws_h: i32,
    pub(super) global_sx: f32,
    pub(super) global_sy: f32,
    pub(super) sx: f32,
    pub(super) sy: f32,
    pub(super) world_now: halley_core::field::Vec2,
    pub(super) workspace_active: bool,
}

#[inline]
pub(super) fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
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

#[inline]
fn modifier_specificity(modifiers: KeyModifiers) -> u32 {
    [
        modifiers.super_key,
        modifiers.left_super,
        modifiers.right_super,
        modifiers.alt,
        modifiers.left_alt,
        modifiers.right_alt,
        modifiers.ctrl,
        modifiers.left_ctrl,
        modifiers.right_ctrl,
        modifiers.shift,
        modifiers.left_shift,
        modifiers.right_shift,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count() as u32
}

#[inline]
pub(super) fn active_pointer_binding(
    st: &Halley,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    st.runtime
        .tuning
        .pointer_bindings
        .iter()
        .filter(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
        .max_by_key(|binding| modifier_specificity(binding.modifiers))
        .map(|binding| binding.action)
}

pub(super) fn button_frame_for_monitor(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    screen: (f32, f32),
) -> (ButtonFrame, String, (f32, f32)) {
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, screen.0, screen.1);
    let target_monitor = st
        .active_locked_pointer_surface()
        .and_then(|surface| {
            let node_id = st.model.surface_to_node.get(&surface.id()).copied()?;
            Some(
                st.model
                    .monitor_state
                    .node_monitor
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone()),
            )
        })
        .unwrap_or_else(|| {
            st.monitor_for_screen(sx, sy)
                .unwrap_or_else(|| st.interaction_monitor().to_string())
        });
    st.set_interaction_monitor(target_monitor.as_str());
    let _ = st.activate_monitor(target_monitor.as_str());
    let (local_w, local_h, local_sx, local_sy) =
        st.local_screen_in_monitor(target_monitor.as_str(), sx, sy);
    let world_now = screen_to_world(st, local_w, local_h, local_sx, local_sy);
    (
        ButtonFrame {
            ws_w: local_w,
            ws_h: local_h,
            global_sx: sx,
            global_sy: sy,
            sx: local_sx,
            sy: local_sy,
            world_now,
            workspace_active: st.has_active_cluster_workspace(),
        },
        target_monitor,
        (sx, sy),
    )
}
