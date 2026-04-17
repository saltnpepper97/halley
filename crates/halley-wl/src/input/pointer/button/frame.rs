use crate::compositor::interaction::ModState;
use crate::compositor::root::Halley;
use crate::input::keyboard::modkeys::modifier_active;
use crate::input::pointer::context::{
    clamp_screen_to_workspace, pointer_screen_context_for_monitor,
};
use halley_config::{KeyModifiers, PointerBindingAction};

#[derive(Clone, Copy)]
pub(crate) struct ButtonFrame {
    pub(crate) ws_w: i32,
    pub(crate) ws_h: i32,
    pub(crate) global_sx: f32,
    pub(crate) global_sy: f32,
    pub(crate) sx: f32,
    pub(crate) sy: f32,
    pub(crate) world_now: halley_core::field::Vec2,
    pub(crate) workspace_active: bool,
}

#[inline]
pub(crate) fn now_millis_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_millis() & 0xffff_ffff) as u32)
        .unwrap_or(0)
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
pub(crate) fn active_pointer_binding(
    st: &Halley,
    mods: &ModState,
    button_code: u32,
) -> Option<PointerBindingAction> {
    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st).is_some() {
        return None;
    }
    st.runtime
        .tuning
        .pointer_bindings
        .iter()
        .filter(|binding| binding.button == button_code && modifier_active(mods, binding.modifiers))
        .max_by_key(|binding| modifier_specificity(binding.modifiers))
        .map(|binding| binding.action)
}

pub(crate) fn button_frame_for_monitor(
    st: &mut Halley,
    ws_w: i32,
    ws_h: i32,
    screen: (f32, f32),
) -> (ButtonFrame, String, (f32, f32)) {
    let (sx, sy) = clamp_screen_to_workspace(ws_w, ws_h, screen.0, screen.1);
    let grabbed_layer_surface_active = st.input.interaction_state.grabbed_layer_surface.is_some();
    let grabbed_layer_surface_monitor = st
        .input
        .interaction_state
        .grabbed_layer_surface
        .as_ref()
        .map(|surface| {
            crate::compositor::monitor::layer_shell::layer_surface_monitor_name(st, surface)
        });
    let target_monitor =
        crate::compositor::interaction::pointer::active_constrained_pointer_surface(st)
            .map(|(surface, _)| st.monitor_for_surface_or_current(&surface))
            .or(grabbed_layer_surface_monitor)
            .unwrap_or_else(|| st.monitor_for_screen_or_interaction(sx, sy));
    let context = pointer_screen_context_for_monitor(
        st,
        target_monitor,
        (sx, sy),
        !grabbed_layer_surface_active,
        !grabbed_layer_surface_active,
    );
    st.input.interaction_state.last_pointer_screen_global =
        Some((context.global_sx, context.global_sy));
    (
        ButtonFrame {
            ws_w: context.ws_w,
            ws_h: context.ws_h,
            global_sx: context.global_sx,
            global_sy: context.global_sy,
            sx: context.local_sx,
            sy: context.local_sy,
            world_now: context.world,
            workspace_active: st.has_active_cluster_workspace(),
        },
        context.monitor.clone(),
        (context.global_sx, context.global_sy),
    )
}
