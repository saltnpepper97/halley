use smithay::{
    reexports::wayland_server::{
        protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface, Resource,
    },
    utils::{Logical, Point, Rectangle, Size, SERIAL_COUNTER},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurface,
            LayerSurfaceCachedState,
        },
    },
};
use std::time::Instant;

use super::Halley;
use crate::compositor::ctx::LayerShellCtx;
use smithay::desktop::{find_popup_root_surface, PopupKind};
use smithay::wayland::shell::xdg::PopupSurface;
use smithay::wayland::shell::xdg::PositionerState;

#[derive(Clone)]
pub(crate) struct LayerPlacement {
    pub wl_surface: WlSurface,
    pub layer: Layer,
    pub origin: Point<i32, Logical>,
    pub keyboard_interactivity: KeyboardInteractivity,
}

pub(crate) fn register_layer_surface(
    ctx: &mut LayerShellCtx<'_>,
    surface: LayerSurface,
    output: Option<WlOutput>,
    layer: Layer,
    namespace: String,
) {
    register_layer_surface_impl(ctx.st, surface, output, layer, namespace);
}

pub(crate) fn remove_layer_surface(ctx: &mut LayerShellCtx<'_>, surface: &LayerSurface) {
    remove_layer_surface_impl(ctx.st, surface);
}

#[allow(dead_code)]
pub(crate) fn maybe_grant_layer_surface_focus_on_commit(
    ctx: &mut LayerShellCtx<'_>,
    surface: &WlSurface,
) {
    maybe_grant_layer_surface_focus_on_commit_impl(ctx.st, surface);
}

pub(crate) fn constrain_layer_popup(
    ctx: &mut LayerShellCtx<'_>,
    popup: &PopupSurface,
    positioner: PositionerState,
) {
    let Some(target) = layer_popup_constraint_target(ctx.st, popup) else {
        return;
    };
    let mut geometry = positioner.get_unconstrained_geometry(target);
    if !rectangle_fits_within(target, geometry) {
        let mut fallback_positioner = positioner;
        fallback_positioner.constraint_adjustment |= smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::FlipX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::FlipY
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::SlideX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::SlideY
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::ResizeX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::ResizeY;
        geometry = fallback_positioner.get_unconstrained_geometry(target);
    }

    popup.with_pending_state(|state| {
        state.geometry = geometry;
    });
}

fn layer_popup_constraint_target(
    st: &Halley,
    popup: &PopupSurface,
) -> Option<Rectangle<i32, Logical>> {
    let popup = PopupKind::from(popup.clone());
    let root = find_popup_root_surface(&popup).ok()?;
    if !is_layer_surface(st, &root) {
        return None;
    }

    let monitor = layer_surface_monitor_name(st, &root);
    let placement = layer_shell_placements_for_monitor(st, monitor.as_str())
        .into_iter()
        .find(|placement| placement.wl_surface.id() == root.id())?;
    let output_size = layer_output_size_for_monitor(st, monitor.as_str());

    Some(Rectangle::new(
        (-placement.origin.x, -placement.origin.y).into(),
        output_size,
    ))
}

fn rectangle_fits_within(target: Rectangle<i32, Logical>, rect: Rectangle<i32, Logical>) -> bool {
    rect.loc.x >= target.loc.x
        && rect.loc.y >= target.loc.y
        && rect.loc.x + rect.size.w <= target.loc.x + target.size.w
        && rect.loc.y + rect.size.h <= target.loc.y + target.size.h
}

pub(crate) fn refresh_monitor_usable_viewports(st: &mut Halley) {
    let monitor_names: Vec<String> = st.model.monitor_state.monitors.keys().cloned().collect();
    for monitor_name in monitor_names {
        let Some(space) = st.model.monitor_state.monitors.get(&monitor_name).cloned() else {
            continue;
        };
        let usable = layer_shell_usable_rect_for_monitor(st, &monitor_name);
        let full = Rectangle::from_size((space.width, space.height).into());
        let usable_viewport = if usable == full {
            space.viewport
        } else {
            let scale_x = space.viewport.size.x / space.width.max(1) as f32;
            let scale_y = space.viewport.size.y / space.height.max(1) as f32;
            let left = space.viewport.center.x - space.viewport.size.x * 0.5
                + usable.loc.x as f32 * scale_x;
            let top = space.viewport.center.y - space.viewport.size.y * 0.5
                + usable.loc.y as f32 * scale_y;
            let size = halley_core::field::Vec2 {
                x: (usable.size.w.max(1) as f32 * scale_x).max(1.0),
                y: (usable.size.h.max(1) as f32 * scale_y).max(1.0),
            };
            halley_core::viewport::Viewport::new(
                halley_core::field::Vec2 {
                    x: left + size.x * 0.5,
                    y: top + size.y * 0.5,
                },
                size,
            )
        };
        if let Some(space_mut) = st.model.monitor_state.monitors.get_mut(&monitor_name) {
            space_mut.usable_viewport = usable_viewport;
        }
    }
}

fn restore_focus_after_layer_surface_close_for_monitor(
    st: &mut Halley,
    monitor: &str,
    now: Instant,
) {
    // Preserve any pending spawn target so launchers like fuzzel can close
    // before their chosen toplevel maps without losing monitor affinity.
    let pending_spawn_monitor = st.model.spawn_state.pending_spawn_monitor.clone();
    if let Some(id) = st.last_focused_surface_node_for_monitor(monitor) {
        st.set_interaction_focus(Some(id), 30_000, now);
    } else {
        st.set_interaction_focus(None, 0, now);
    }
    st.model.spawn_state.pending_spawn_monitor = pending_spawn_monitor;
}

fn apply_layer_surface_focus(
    st: &mut Halley,
    surface: &WlSurface,
    interactivity: KeyboardInteractivity,
) -> bool {
    if interactivity == KeyboardInteractivity::None {
        return false;
    }

    let monitor = layer_surface_monitor_name(st, surface);
    st.set_interaction_monitor(monitor.as_str());
    st.set_focused_monitor(monitor.as_str());
    st.model.spawn_state.pending_spawn_monitor = Some(monitor.clone());
    let _ = st.activate_monitor(monitor.as_str());

    st.model.focus_state.primary_interaction_focus = None;
    st.model.focus_state.interaction_focus_until_ms = 0;
    st.model.monitor_state.layer_keyboard_focus = Some(surface.id());
    if crate::compositor::interaction::pointer::active_locked_pointer_surface(st)
        .is_some_and(|locked_surface| locked_surface.id() != surface.id())
    {
        crate::compositor::interaction::pointer::release_active_pointer_constraint(st);
    }

    if let Some(keyboard) = st.platform.seat.get_keyboard() {
        keyboard.set_focus(st, Some(surface.clone()), SERIAL_COUNTER.next_serial());
    }
    st.update_selection_focus_from_surface(Some(surface));

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let changed = top.with_pending_state(|state| {
                let was_active = state.states.contains(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                );
                state.states.unset(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated,
                );
                was_active
            });
        if changed {
            top.send_configure();
        }
    }

    true
}

fn layer_focus_candidate_surface(st: &Halley) -> Option<WlSurface> {
    let mut placements = layer_shell_placements(st, layer_output_size(st));
    placements.sort_by_key(|placement| std::cmp::Reverse(layer_depth(placement.layer)));
    placements.into_iter().find_map(|placement| {
        layer_surface_can_take_keyboard_focus(placement.keyboard_interactivity)
            .then_some(placement.wl_surface)
    })
}

pub(crate) fn layer_surface_monitor_name(st: &Halley, surface: &WlSurface) -> String {
    st.model
        .monitor_state
        .layer_surface_monitor
        .get(&surface.id())
        .cloned()
        .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
}

fn register_layer_surface_impl(
    st: &mut Halley,
    surface: LayerSurface,
    output: Option<WlOutput>,
    layer: Layer,
    namespace: String,
) {
    let assigned_monitor = if let Some(requested_output) = output.as_ref() {
        st.model
            .monitor_state
            .outputs
            .iter()
            .find_map(|(name, output)| output.owns(requested_output).then_some(name.clone()))
            .unwrap_or_else(|| st.model.monitor_state.current_monitor.clone())
    } else {
        st.model.monitor_state.current_monitor.clone()
    };

    let size = layer_output_size_for_monitor(st, &assigned_monitor);
    surface.with_pending_state(|state| {
        state.size = Some(size);
    });
    surface.send_configure();

    st.assign_layer_surface_to_monitor(surface.wl_surface(), assigned_monitor.clone());

    if let Some(requested_output) = output.as_ref() {
        for output in st.model.monitor_state.outputs.values() {
            if output.owns(requested_output) {
                output.enter(surface.wl_surface());
            }
        }
    } else if let Some(output) = st.model.monitor_state.outputs.get(&assigned_monitor) {
        output.enter(surface.wl_surface());
    }

    let interactivity = layer_cached_state(&surface).keyboard_interactivity;
    if layer_surface_can_take_keyboard_focus(interactivity) {
        let _ = apply_layer_surface_focus(st, surface.wl_surface(), interactivity);
    }

    let _ = (layer, namespace);
}

/// Called on every surface commit. If this surface is a layer surface that
/// requests keyboard focus and doesn't already have it, grant it now.
/// This is the correct time to do it — `register_layer_surface` fires before
/// the client has committed its desired `keyboard_interactivity`, so the
/// cached state is still the default `None` at that point.
fn maybe_grant_layer_surface_focus_on_commit_impl(st: &mut Halley, surface: &WlSurface) {
    if st.model.monitor_state.layer_keyboard_focus == Some(surface.id()) {
        return;
    }

    let Some(interactivity) =
        st.platform
            .wlr_layer_shell_state
            .layer_surfaces()
            .find_map(|layer| {
                (layer.wl_surface().id() == surface.id())
                    .then_some(layer_cached_state(&layer).keyboard_interactivity)
            })
    else {
        return;
    };

    if layer_surface_can_take_keyboard_focus(interactivity) {
        let _ = apply_layer_surface_focus(st, surface, interactivity);
    }
}

fn remove_layer_surface_impl(st: &mut Halley, surface: &LayerSurface) {
    let removed_monitor = layer_surface_monitor_name(st, surface.wl_surface());
    let removed_focused_layer =
        st.model.monitor_state.layer_keyboard_focus == Some(surface.wl_surface().id());
    st.model
        .monitor_state
        .layer_surface_monitor
        .remove(&surface.wl_surface().id());
    if removed_focused_layer {
        st.model.monitor_state.layer_keyboard_focus = None;
    }
    for output in st.model.monitor_state.outputs.values() {
        output.leave(surface.wl_surface());
    }
    if !removed_focused_layer {
        return;
    }

    if let Some(next_layer) = layer_focus_candidate_surface(st) {
        let _ = focus_layer_surface(st, &next_layer);
        return;
    }

    restore_focus_after_layer_surface_close_for_monitor(
        st,
        removed_monitor.as_str(),
        Instant::now(),
    );
}

pub(crate) fn layer_output_size(st: &Halley) -> Size<i32, Logical> {
    layer_output_size_for_monitor(st, &st.model.monitor_state.current_monitor)
}

pub(crate) fn layer_output_size_for_monitor(st: &Halley, monitor_name: &str) -> Size<i32, Logical> {
    st.model
        .monitor_state
        .monitors
        .get(monitor_name)
        .map(|monitor| (monitor.width as i32, monitor.height as i32).into())
        .unwrap_or_else(|| {
            (
                st.model.zoom_ref_size.x.round().max(1.0) as i32,
                st.model.zoom_ref_size.y.round().max(1.0) as i32,
            )
                .into()
        })
}

fn layer_cached_state(surface: &LayerSurface) -> LayerSurfaceCachedState {
    with_states(surface.wl_surface(), |states| {
        *states
            .cached_state
            .get::<LayerSurfaceCachedState>()
            .current()
    })
}

pub(crate) fn configure_layer_shell_surfaces(st: &mut Halley, _output_size: Size<i32, Logical>) {
    for monitor_name in st
        .model
        .monitor_state
        .monitors
        .keys()
        .cloned()
        .collect::<Vec<_>>()
    {
        let output_size = layer_output_size_for_monitor(st, &monitor_name);
        let output_rect = Rectangle::from_size(output_size);
        let mut zone = output_rect;

        for surface in layer_shell_surfaces_sorted(st) {
            if layer_surface_monitor_name(st, surface.wl_surface()) != monitor_name {
                continue;
            }
            let data = layer_cached_state(&surface);
            let (_, size) = compute_layer_placement(output_rect, &mut zone, data);
            if data.size == size {
                continue;
            }
            surface.with_pending_state(|state| {
                state.size = Some(size);
            });
            let _ = surface.send_pending_configure();
        }
    }
    refresh_monitor_usable_viewports(st);
}

pub(crate) fn layer_shell_placements(
    st: &Halley,
    _output_size: Size<i32, Logical>,
) -> Vec<LayerPlacement> {
    let monitor_name = st.model.monitor_state.current_monitor.clone();
    layer_shell_placements_for_monitor(st, &monitor_name)
}

pub(crate) fn layer_shell_placements_for_monitor(
    st: &Halley,
    monitor_name: &str,
) -> Vec<LayerPlacement> {
    let output_rect = Rectangle::from_size(layer_output_size_for_monitor(st, monitor_name));
    let mut zone = output_rect;
    let mut placements = Vec::new();

    for surface in layer_shell_surfaces_sorted(st) {
        if layer_surface_monitor_name(st, surface.wl_surface()) != monitor_name {
            continue;
        }
        let data = layer_cached_state(&surface);
        let (origin, _) = compute_layer_placement(output_rect, &mut zone, data);
        placements.push(LayerPlacement {
            wl_surface: surface.wl_surface().clone(),
            layer: data.layer,
            origin,
            keyboard_interactivity: data.keyboard_interactivity,
        });
    }

    placements
}

pub(crate) fn layer_shell_usable_rect_for_monitor(
    st: &Halley,
    monitor_name: &str,
) -> Rectangle<i32, Logical> {
    let output_rect = Rectangle::from_size(layer_output_size_for_monitor(st, monitor_name));
    let mut zone = output_rect;

    for surface in layer_shell_surfaces_sorted(st) {
        if layer_surface_monitor_name(st, surface.wl_surface()) != monitor_name {
            continue;
        }
        let data = layer_cached_state(&surface);
        let _ = compute_layer_placement(output_rect, &mut zone, data);
    }

    zone
}

fn layer_shell_surfaces_sorted(st: &Halley) -> Vec<LayerSurface> {
    let mut surfaces: Vec<_> = st
        .platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .filter(|surface| surface.alive())
        .collect();
    surfaces.sort_by_key(|surface| layer_depth(layer_cached_state(surface).layer));
    surfaces
}

pub(crate) fn keyboard_focus_is_layer_surface(st: &Halley) -> bool {
    if st.model.monitor_state.layer_keyboard_focus.is_some() {
        return true;
    }
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return false;
    };
    keyboard
        .current_focus()
        .is_some_and(|focus| is_layer_surface(st, &focus))
}

fn layer_focus_surface(st: &Halley) -> Option<WlSurface> {
    let focus_id = st.model.monitor_state.layer_keyboard_focus.clone()?;
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .find_map(|layer| (layer.wl_surface().id() == focus_id).then(|| layer.wl_surface().clone()))
}

pub(crate) fn focus_layer_surface(st: &mut Halley, surface: &WlSurface) -> bool {
    let Some(interactivity) =
        st.platform
            .wlr_layer_shell_state
            .layer_surfaces()
            .find_map(|layer| {
                (layer.wl_surface().id() == surface.id())
                    .then_some(layer_cached_state(&layer).keyboard_interactivity)
            })
    else {
        return false;
    };
    apply_layer_surface_focus(st, surface, interactivity)
}

pub(crate) fn is_layer_surface(st: &Halley, surface: &WlSurface) -> bool {
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .any(|layer| layer.wl_surface().id() == surface.id())
}

pub(crate) fn reassert_layer_surface_keyboard_focus_if_drifted(st: &mut Halley) {
    let Some(desired_focus) = layer_focus_surface(st) else {
        st.model.monitor_state.layer_keyboard_focus = None;
        return;
    };

    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return;
    };

    let current_focus = keyboard.current_focus();
    let matches = current_focus
        .as_ref()
        .is_some_and(|focus| focus.id() == desired_focus.id());

    if !matches {
        keyboard.set_focus(
            st,
            Some(desired_focus.clone()),
            SERIAL_COUNTER.next_serial(),
        );
        st.update_selection_focus_from_surface(Some(&desired_focus));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use halley_core::field::Vec2;

    use super::Halley;

    #[test]
    fn closing_layer_surface_restores_surface_focus_without_clearing_pending_spawn_monitor() {
        let tuning = halley_config::RuntimeTuning::default();
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let id = state.model.field.spawn_surface(
            "focused",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_current_monitor(id);
        state.set_interaction_focus(Some(id), 30_000, Instant::now());

        state.model.focus_state.primary_interaction_focus = None;
        state.model.focus_state.interaction_focus_until_ms = 0;
        state.model.spawn_state.pending_spawn_monitor = Some("default".to_string());

        super::restore_focus_after_layer_surface_close_for_monitor(
            &mut state,
            "default",
            Instant::now(),
        );

        assert_eq!(state.model.focus_state.primary_interaction_focus, Some(id));
        assert_eq!(
            state.model.spawn_state.pending_spawn_monitor.as_deref(),
            Some("default")
        );
    }

    #[test]
    fn closing_layer_surface_does_not_refocus_surface_from_another_monitor() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 2560,
                offset_y: 0,
                width: 1920,
                height: 1200,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, tuning);

        let right_id = state.model.field.spawn_surface(
            "right",
            Vec2 {
                x: 3200.0,
                y: 300.0,
            },
            Vec2 { x: 120.0, y: 90.0 },
        );
        state.assign_node_to_monitor(right_id, "right");
        state.set_interaction_focus(Some(right_id), 30_000, Instant::now());

        state.model.focus_state.primary_interaction_focus = None;
        state.model.focus_state.interaction_focus_until_ms = 0;
        state.model.spawn_state.pending_spawn_monitor = Some("left".to_string());

        super::restore_focus_after_layer_surface_close_for_monitor(
            &mut state,
            "left",
            Instant::now(),
        );

        assert_eq!(state.model.focus_state.primary_interaction_focus, None);
        assert_eq!(
            state.model.spawn_state.pending_spawn_monitor.as_deref(),
            Some("left")
        );
    }
}

fn layer_surface_can_take_keyboard_focus(interactivity: KeyboardInteractivity) -> bool {
    interactivity != KeyboardInteractivity::None
}

fn layer_depth(layer: Layer) -> i32 {
    match layer {
        Layer::Background => 0,
        Layer::Bottom => 1,
        Layer::Top => 2,
        Layer::Overlay => 3,
    }
}

fn exclusive_zone_amount(zone: ExclusiveZone) -> i32 {
    match zone {
        ExclusiveZone::Exclusive(v) => v as i32,
        _ => 0,
    }
}

fn compute_layer_placement(
    output_rect: Rectangle<i32, Logical>,
    zone: &mut Rectangle<i32, Logical>,
    data: LayerSurfaceCachedState,
) -> (Point<i32, Logical>, Size<i32, Logical>) {
    let mut width = data.size.w;
    let mut height = data.size.h;

    let anchored_left = data.anchor.contains(Anchor::LEFT);
    let anchored_right = data.anchor.contains(Anchor::RIGHT);
    let anchored_top = data.anchor.contains(Anchor::TOP);
    let anchored_bottom = data.anchor.contains(Anchor::BOTTOM);

    if width == 0 && anchored_left && anchored_right {
        width = zone.size.w.max(1);
    }
    if height == 0 && anchored_top && anchored_bottom {
        height = zone.size.h.max(1);
    }
    if width == 0 {
        width = output_rect.size.w.max(1);
    }
    if height == 0 {
        height = output_rect.size.h.max(1);
    }

    let mut x = if anchored_left {
        zone.loc.x + data.margin.left
    } else if anchored_right {
        zone.loc.x + zone.size.w - width - data.margin.right
    } else {
        zone.loc.x + (zone.size.w - width) / 2
    };

    let mut y = if anchored_top {
        zone.loc.y + data.margin.top
    } else if anchored_bottom {
        zone.loc.y + zone.size.h - height - data.margin.bottom
    } else {
        zone.loc.y + (zone.size.h - height) / 2
    };

    if anchored_left && anchored_right {
        x = zone.loc.x + data.margin.left;
        width = (zone.size.w - data.margin.left - data.margin.right).max(1);
    }
    if anchored_top && anchored_bottom {
        y = zone.loc.y + data.margin.top;
        height = (zone.size.h - data.margin.top - data.margin.bottom).max(1);
    }

    let size: Size<i32, Logical> = (width, height).into();
    let origin: Point<i32, Logical> = (x, y).into();

    let exclusive = exclusive_zone_amount(data.exclusive_zone);
    if exclusive > 0 {
        if anchored_top && !anchored_bottom {
            let consumed = (exclusive + data.margin.top).clamp(0, zone.size.h);
            zone.loc.y += consumed;
            zone.size.h -= consumed;
        } else if anchored_bottom && !anchored_top {
            let consumed = (exclusive + data.margin.bottom).clamp(0, zone.size.h);
            zone.size.h -= consumed;
        } else if anchored_left && !anchored_right {
            let consumed = (exclusive + data.margin.left).clamp(0, zone.size.w);
            zone.loc.x += consumed;
            zone.size.w -= consumed;
        } else if anchored_right && !anchored_left {
            let consumed = (exclusive + data.margin.right).clamp(0, zone.size.w);
            zone.size.w -= consumed;
        }
    }

    (origin, size)
}
