use smithay::{
    reexports::wayland_server::{
        Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size},
    wayland::{
        compositor::with_states,
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurface,
            LayerSurfaceCachedState, LayerSurfaceData,
        },
    },
};
use std::time::Instant;

use super::Halley;
use crate::compositor::ctx::LayerShellCtx;
use smithay::desktop::{PopupKind, find_popup_root_surface};
use smithay::wayland::shell::xdg::PopupSurface;
use smithay::wayland::shell::xdg::PositionerState;

const APERTURE_LAYER_NAMESPACE: &str = "halley-aperture";
const HALLEY_LIFT_LAYER_NAMESPACE: &str = "halley-lift";

#[derive(Clone)]
pub(crate) struct LayerPlacement {
    pub wl_surface: WlSurface,
    pub layer: Layer,
    pub origin: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
    pub keyboard_interactivity: KeyboardInteractivity,
}

impl LayerShellCtx<'_> {
    pub(crate) fn register_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        register_layer_surface_impl(self.st, surface, output, layer, namespace);
    }

    pub(crate) fn remove_layer_surface(&mut self, surface: &LayerSurface) {
        remove_layer_surface_impl(self.st, surface);
    }

    pub(crate) fn maybe_grant_layer_surface_focus_on_commit(&mut self, surface: &WlSurface) {
        maybe_grant_layer_surface_focus_on_commit_impl(self.st, surface);
    }

    pub(crate) fn layer_popup_constraint_target(
        &self,
        popup: &PopupSurface,
    ) -> Option<Rectangle<i32, Logical>> {
        layer_popup_constraint_target(self.st, popup)
    }
}

pub(crate) fn register_layer_surface(
    ctx: &mut LayerShellCtx<'_>,
    surface: LayerSurface,
    output: Option<WlOutput>,
    layer: Layer,
    namespace: String,
) {
    ctx.register_layer_surface(surface, output, layer, namespace);
}

pub(crate) fn remove_layer_surface(ctx: &mut LayerShellCtx<'_>, surface: &LayerSurface) {
    ctx.remove_layer_surface(surface);
}

#[allow(dead_code)]
pub(crate) fn maybe_grant_layer_surface_focus_on_commit(
    ctx: &mut LayerShellCtx<'_>,
    surface: &WlSurface,
) {
    ctx.maybe_grant_layer_surface_focus_on_commit(surface);
}

pub(crate) fn constrain_layer_popup(
    ctx: &mut LayerShellCtx<'_>,
    popup: &PopupSurface,
    positioner: PositionerState,
) {
    let Some(target) = ctx.layer_popup_constraint_target(popup) else {
        return;
    };
    let (constrained_positioner, geometry) = constrained_layer_popup_position(positioner, target);

    popup.with_pending_state(|state| {
        state.positioner = constrained_positioner;
        state.geometry = geometry;
    });
}

fn constrained_layer_popup_position(
    positioner: PositionerState,
    target: Rectangle<i32, Logical>,
) -> (PositionerState, Rectangle<i32, Logical>) {
    let mut constrained_positioner = positioner;
    let mut geometry = constrained_positioner.get_unconstrained_geometry(target);
    if !rectangle_fits_within(target, geometry) {
        constrained_positioner.constraint_adjustment |= smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::FlipX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::FlipY
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::SlideX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::SlideY
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::ResizeX
            | smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment::ResizeY;
        geometry = constrained_positioner.get_unconstrained_geometry(target);
    }

    (constrained_positioner, geometry)
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
    let rect_right = i64::from(rect.loc.x) + i64::from(rect.size.w);
    let rect_bottom = i64::from(rect.loc.y) + i64::from(rect.size.h);
    let target_right = i64::from(target.loc.x) + i64::from(target.size.w);
    let target_bottom = i64::from(target.loc.y) + i64::from(target.size.h);

    rect.loc.x >= target.loc.x
        && rect.loc.y >= target.loc.y
        && rect_right <= target_right
        && rect_bottom <= target_bottom
}

/// True when `monitor` is in a mode that needs a frozen work area. The
/// aperture-driven reservation is established once through a forced refresh, and
/// later aperture commits are deferred so in-flight layout animations keep a
/// stable `usable_viewport` target.
fn monitor_workarea_locked(st: &Halley, monitor: &str) -> bool {
    let cluster_locked = st.active_cluster_workspace_for_monitor(monitor).is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        );
    let maximize_locked =
        crate::compositor::workspace::state::maximize_session_present_on_monitor(st, monitor);

    cluster_locked || maximize_locked
}

/// True when at least one monitor with a deferred work-area refresh is currently
/// unlocked, i.e. a [`refresh_monitor_usable_viewports`] pass would actually
/// apply something. While a session stays locked the pending entry is re-inserted
/// every pass, so the maintenance tick uses this to avoid re-running the full
/// refresh (and invalidating the aperture mode cache) every frame for the whole
/// session.
pub(crate) fn any_pending_workarea_unlocked(st: &Halley) -> bool {
    st.model
        .monitor_state
        .pending_workarea_refresh
        .iter()
        .any(|monitor| !monitor_workarea_locked(st, monitor))
}

pub(crate) fn refresh_monitor_usable_viewports(st: &mut Halley) {
    refresh_monitor_usable_viewports_inner(st, None);
}

/// Like [`refresh_monitor_usable_viewports`] but force-applies the reservation for
/// `force_monitor`, bypassing the active-cluster work-area lock for that one
/// monitor. Called at cluster enter to write the session baseline (the cluster is
/// already marked active at that point, so the lock would otherwise defer it).
pub(crate) fn refresh_monitor_usable_viewport_forced(st: &mut Halley, force_monitor: &str) {
    refresh_monitor_usable_viewports_inner(st, Some(force_monitor));
}

fn refresh_monitor_usable_viewports_inner(st: &mut Halley, force_monitor: Option<&str>) {
    let monitor_names: Vec<String> = st.model.monitor_state.monitors.keys().cloned().collect();
    for monitor_name in monitor_names {
        let Some(space) = st.model.monitor_state.monitors.get(&monitor_name).cloned() else {
            continue;
        };
        let full: Rectangle<i32, Logical> =
            Rectangle::from_size((space.width, space.height).into());
        let mut usable = full;
        let aperture_reserve = crate::aperture::small_reservation_px_for_monitor(st, &monitor_name);
        if aperture_reserve > 0 {
            usable.loc.y = aperture_reserve.clamp(0, full.size.h.saturating_sub(1));
            usable.size.h = (full.size.h - usable.loc.y).max(1);
        }
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
        // Freeze the work area for active layout sessions: applying the aperture's
        // animated reservation mid-session rewrites `usable_viewport`, which
        // re-bases in-flight tile/maximize easing. The reservation is established
        // once at entry via forced refresh; later aperture-height changes are
        // deferred until the session unlocks. The entering monitor is exempted so
        // its baseline still gets written.
        if usable_viewport != space.usable_viewport
            && force_monitor != Some(monitor_name.as_str())
            && monitor_workarea_locked(st, &monitor_name)
        {
            st.model
                .monitor_state
                .pending_workarea_refresh
                .insert(monitor_name.clone());
            if crate::perf::enabled() {
                eventline::info!(
                    "perf workarea_defer monitor={} old_top={:.1} new_top={:.1} (session_locked)",
                    monitor_name,
                    space.usable_viewport.center.y - space.usable_viewport.size.y * 0.5,
                    usable_viewport.center.y - usable_viewport.size.y * 0.5,
                );
            }
            continue;
        }
        st.model
            .monitor_state
            .pending_workarea_refresh
            .remove(&monitor_name);
        if let Some(space_mut) = st.model.monitor_state.monitors.get_mut(&monitor_name) {
            space_mut.usable_viewport = usable_viewport;
        }
    }
    // This runs on exactly the discrete transitions that can flip the aperture
    // mode (cluster enter/exit, fullscreen, maximize, output reconfigure), and not
    // in the per-frame path — so drop the cached modes here for an immediate
    // re-derive on the next status poll.
    st.aperture.invalidate_mode_cache();
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
    if crate::compositor::interaction::pointer::active_constrained_pointer_surface(st)
        .is_some_and(|(constrained_surface, _)| constrained_surface.id() != surface.id())
    {
        crate::compositor::interaction::pointer::release_active_pointer_constraint(st);
    }

    crate::compositor::focus::system::set_keyboard_focus(
        st,
        Some(surface.clone()),
        SERIAL_COUNTER.next_serial(),
    );
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

#[allow(dead_code)]
pub(crate) fn aperture_layer_present_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .monitor_state
        .aperture_layer_monitors
        .contains(monitor)
}

/// Monitor for a layer surface that requested no specific output. Lift
/// (`NAMESPACE` in `crates/halley-lift/src/main.rs`) follows the cursor; every
/// other namespace falls back to the current (focused) monitor.
fn assigned_monitor_for_no_output(st: &Halley, namespace: &str) -> String {
    if namespace == "halley-lift"
        && let Some((sx, sy)) = st.input.interaction_state.cursor.last_screen_global
    {
        return st.monitor_for_screen_or_current(sx, sy);
    }
    st.model.monitor_state.current_monitor.clone()
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
        assigned_monitor_for_no_output(st, &namespace)
    };

    crate::compositor::monitor::state::assign_layer_surface_to_monitor(
        st,
        surface.wl_surface(),
        assigned_monitor.clone(),
    );
    st.model
        .monitor_state
        .layer_surface_namespace
        .insert(surface.wl_surface().id(), namespace.clone());
    if !st
        .model
        .monitor_state
        .layer_surface_order
        .contains(&surface.wl_surface().id())
    {
        st.model
            .monitor_state
            .layer_surface_order
            .push(surface.wl_surface().id());
    }

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
    st.model
        .monitor_state
        .layer_surface_committed
        .insert(surface.id());
    if layer_surface_namespace(st, surface).as_deref() == Some(APERTURE_LAYER_NAMESPACE) {
        let monitor = layer_surface_monitor_name(st, surface);
        let height = aperture_layer_height_from_committed_surface(st, surface);
        let newly_present = st
            .model
            .monitor_state
            .aperture_layer_monitors
            .insert(monitor.clone());
        // Only learn the minimal-tab height while minimal is the intended mode for
        // this monitor. Otherwise the Minimal→Normal close ramp — whose climbing
        // heights stay within the accepted band for a while — overwrites the stored
        // value last-wins with a too-large height, which FINAL-5 then freezes as a
        // persistently oversized cluster gap. See `monitor_minimal_aperture_intended`.
        let height_changed = crate::aperture::monitor_minimal_aperture_intended(st, &monitor)
            && height
                .and_then(|height| {
                    crate::aperture::accepted_minimal_aperture_tab_height_px(st, height)
                })
                .is_some_and(|height| {
                    st.model
                        .monitor_state
                        .aperture_layer_heights
                        .insert(monitor.clone(), height)
                        != Some(height)
                });
        if newly_present || height_changed {
            refresh_monitor_usable_viewports(st);
        }
    }

    if !layer_surface_initial_configure_sent(surface) {
        let monitor = layer_surface_monitor_name(st, surface);
        configure_layer_shell_surfaces_for_monitor(st, monitor.as_str());
        refresh_monitor_usable_viewports(st);
    }

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
    let removed_namespace = st
        .model
        .monitor_state
        .layer_surface_namespace
        .remove(&surface.wl_surface().id());
    st.model
        .monitor_state
        .layer_surface_committed
        .remove(&surface.wl_surface().id());
    st.model
        .monitor_state
        .layer_surface_last_configured_size
        .remove(&surface.wl_surface().id());
    st.model
        .monitor_state
        .layer_surface_order
        .retain(|id| id != &surface.wl_surface().id());
    if removed_focused_layer {
        st.model.monitor_state.layer_keyboard_focus = None;
    }
    for output in st.model.monitor_state.outputs.values() {
        output.leave(surface.wl_surface());
    }
    if removed_namespace.as_deref() == Some(APERTURE_LAYER_NAMESPACE) {
        if !aperture_layer_attached_to_monitor(st, removed_monitor.as_str()) {
            st.model
                .monitor_state
                .aperture_layer_monitors
                .remove(removed_monitor.as_str());
            st.model
                .monitor_state
                .aperture_layer_heights
                .remove(removed_monitor.as_str());
        }
        refresh_monitor_usable_viewports(st);
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
        .map(|monitor| (monitor.width, monitor.height).into())
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

fn layer_surface_namespace(st: &Halley, surface: &WlSurface) -> Option<String> {
    st.model
        .monitor_state
        .layer_surface_namespace
        .get(&surface.id())
        .cloned()
}

/// True when the layer surface is Halley's own aperture-peek client.
pub(crate) fn surface_is_aperture(st: &Halley, surface: &WlSurface) -> bool {
    layer_surface_namespace(st, surface).as_deref() == Some(APERTURE_LAYER_NAMESPACE)
}

fn aperture_layer_height_from_committed_surface(st: &Halley, surface: &WlSurface) -> Option<i32> {
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .find_map(|layer| {
            (layer.wl_surface().id() == surface.id()).then(|| layer_cached_state(&layer).size.h)
        })
}

fn aperture_layer_attached_to_monitor(st: &Halley, monitor: &str) -> bool {
    st.model
        .monitor_state
        .layer_surface_namespace
        .iter()
        .any(|(id, namespace)| {
            namespace == APERTURE_LAYER_NAMESPACE
                && st.model.monitor_state.layer_surface_committed.contains(id)
                && st
                    .model
                    .monitor_state
                    .layer_surface_monitor
                    .get(id)
                    .is_some_and(|surface_monitor| surface_monitor == monitor)
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
        configure_layer_shell_surfaces_for_monitor(st, monitor_name.as_str());
    }
    refresh_monitor_usable_viewports(st);
}

fn configure_layer_shell_surfaces_for_monitor(st: &mut Halley, monitor_name: &str) {
    let output_size = layer_output_size_for_monitor(st, monitor_name);
    let output_rect = Rectangle::from_size(output_size);
    let mut zone = output_rect;

    for surface in layer_shell_surfaces_sorted(st) {
        if layer_surface_monitor_name(st, surface.wl_surface()) != monitor_name {
            continue;
        }
        if !st
            .model
            .monitor_state
            .layer_surface_committed
            .contains(&surface.wl_surface().id())
        {
            continue;
        }
        let data = layer_cached_state(&surface);
        let (_, size) = compute_layer_placement(output_rect, &mut zone, data);
        if last_configured_layer_surface_size(st, surface.wl_surface()) == Some(size) {
            continue;
        }
        surface.with_pending_state(|state| {
            state.size = Some(size);
        });
        surface.send_configure();
        record_layer_surface_configured_size(st, surface.wl_surface(), size);
    }
}

fn last_configured_layer_surface_size(
    st: &Halley,
    surface: &WlSurface,
) -> Option<Size<i32, Logical>> {
    st.model
        .monitor_state
        .layer_surface_last_configured_size
        .get(&surface.id())
        .copied()
}

fn record_layer_surface_configured_size(
    st: &mut Halley,
    surface: &WlSurface,
    size: Size<i32, Logical>,
) {
    st.model
        .monitor_state
        .layer_surface_last_configured_size
        .insert(surface.id(), size);
}

fn layer_surface_initial_configure_sent(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<LayerSurfaceData>()
            .map(|data| data.lock().unwrap().initial_configure_sent)
            .unwrap_or(false)
    })
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
        let (origin, size) = compute_layer_placement(output_rect, &mut zone, data);
        placements.push(LayerPlacement {
            wl_surface: surface.wl_surface().clone(),
            layer: data.layer,
            origin,
            size,
            keyboard_interactivity: data.keyboard_interactivity,
        });
    }

    placements
}

fn layer_shell_surfaces_sorted(st: &Halley) -> Vec<LayerSurface> {
    let mut surfaces: Vec<_> = st
        .platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .filter(|surface| surface.alive())
        .collect();
    surfaces.sort_by_key(|surface| {
        let id = surface.wl_surface().id();
        let order = st
            .model
            .monitor_state
            .layer_surface_order
            .iter()
            .position(|surface_id| surface_id == &id)
            .unwrap_or(usize::MAX);
        let data = layer_cached_state(surface);
        (
            layer_depth(data.layer),
            layer_reservation_priority(data),
            order,
        )
    });
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

/// Returns true when the surface holding keyboard focus is the Lift overlay
/// (`halley-lift`). Used to dismiss the launcher when a compositor keybind fires,
/// fuzzel-style.
pub(crate) fn keyboard_focus_is_lift_layer_surface(st: &Halley) -> bool {
    if let Some(focus_id) = st.model.monitor_state.layer_keyboard_focus.as_ref() {
        return st
            .model
            .monitor_state
            .layer_surface_namespace
            .get(focus_id)
            .is_some_and(|namespace| namespace == HALLEY_LIFT_LAYER_NAMESPACE);
    }
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return false;
    };
    keyboard
        .current_focus()
        .is_some_and(|focus| is_lift_layer_surface(st, &focus))
}

/// Returns true when the layer-shell surface that currently holds keyboard
/// focus is **modal** — i.e. it should block hover-focus and compositor
/// shortcuts the way a launcher or lock screen does.
///
/// Returns true for:
/// - The Lift launcher (`halley-lift` namespace)
/// - Layer surfaces with `KeyboardInteractivity::Exclusive`
/// - Session lock surfaces
///
/// Returns false for persistent shells that use `OnDemand` interactivity
/// (e.g. Quickshell/Noctalia panels) so that hover-focus and compositor
/// shortcuts continue to work while the shell is present.
pub(crate) fn layer_keyboard_focus_is_modal(st: &Halley) -> bool {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        return true;
    }
    let Some(focus_id) = st.model.monitor_state.layer_keyboard_focus.as_ref() else {
        // No tracked layer keyboard focus — but also check the seat's actual
        // current focus in case state drifted. If the seat focus is not on a
        // layer surface at all, there is nothing modal.
        return false;
    };
    if st
        .model
        .monitor_state
        .layer_surface_namespace
        .get(focus_id)
        .is_some_and(|ns| ns == HALLEY_LIFT_LAYER_NAMESPACE)
    {
        return true;
    }

    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .find_map(|layer| {
            (layer.wl_surface().id() == *focus_id)
                .then_some(layer_cached_state(&layer).keyboard_interactivity)
        })
        .is_some_and(|i| i == KeyboardInteractivity::Exclusive)
}

/// Returns true when a layer-shell pointer hit should suppress desktop hover
/// focus. Persistent shells (Quickshell/Noctalia panels) often keep an OnDemand
/// layer surface alive for the whole session; treating every such hit as modal
/// makes `input.focus-mode "hover"` unusable. Only modal layer roles should
/// block hover focus behind them.
pub(crate) fn layer_surface_blocks_desktop_hover(st: &Halley, surface: &WlSurface) -> bool {
    let Some(root) = layer_surface_root_for_surface(st, surface) else {
        return false;
    };
    if st
        .model
        .monitor_state
        .layer_surface_namespace
        .get(&root.id())
        .is_some_and(|ns| ns == HALLEY_LIFT_LAYER_NAMESPACE)
    {
        return true;
    }
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .find_map(|layer| {
            (layer.wl_surface().id() == root.id())
                .then_some(layer_cached_state(&layer).keyboard_interactivity)
        })
        .is_some_and(|i| i == KeyboardInteractivity::Exclusive)
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

/// Returns true if `surface` (or its layer-surface root, for subsurfaces/popups)
/// is the Lift overlay.
pub(crate) fn is_lift_layer_surface(st: &Halley, surface: &WlSurface) -> bool {
    let Some(root) = layer_surface_root_for_surface(st, surface) else {
        return false;
    };
    st.model
        .monitor_state
        .layer_surface_namespace
        .get(&root.id())
        .is_some_and(|namespace| namespace == HALLEY_LIFT_LAYER_NAMESPACE)
}

/// Closes any open Lift overlay, regardless of which monitor it lives on. Used
/// for click-away dismissal: clicking anywhere outside Lift closes it. Returns
/// true if a Lift layer was closed.
pub(crate) fn close_any_lift_layer(st: &mut Halley) -> bool {
    if let Some(focus_id) = st.model.monitor_state.layer_keyboard_focus.clone() {
        let focused_lift = st
            .model
            .monitor_state
            .layer_surface_namespace
            .get(&focus_id)
            .is_some_and(|namespace| namespace == HALLEY_LIFT_LAYER_NAMESPACE);
        if focused_lift {
            if let Some(layer) = st
                .platform
                .wlr_layer_shell_state
                .layer_surfaces()
                .find(|layer| layer.wl_surface().id() == focus_id)
            {
                layer.send_close();
                return true;
            }
            st.model.monitor_state.layer_keyboard_focus = None;
        }
    }

    let Some(layer) = layer_shell_surfaces_sorted(st)
        .into_iter()
        .rev()
        .find(|layer| {
            st.model
                .monitor_state
                .layer_surface_namespace
                .get(&layer.wl_surface().id())
                .is_some_and(|namespace| namespace == HALLEY_LIFT_LAYER_NAMESPACE)
        })
    else {
        return false;
    };
    layer.send_close();
    true
}

pub(crate) fn is_layer_surface(st: &Halley, surface: &WlSurface) -> bool {
    st.platform
        .wlr_layer_shell_state
        .layer_surfaces()
        .any(|layer| layer.wl_surface().id() == surface.id())
}

fn popup_parent_surface(popup: &PopupKind) -> Option<WlSurface> {
    match popup {
        PopupKind::Xdg(popup) => popup.get_parent_surface(),
        PopupKind::InputMethod(popup) => popup.get_parent().map(|parent| parent.surface.clone()),
    }
}

pub(crate) fn layer_surface_root_for_surface(
    st: &Halley,
    surface: &WlSurface,
) -> Option<WlSurface> {
    let mut current = surface.clone();
    loop {
        if is_layer_surface(st, &current) {
            return Some(current);
        }
        if let Some(parent) = smithay::wayland::compositor::get_parent(&current) {
            current = parent;
            continue;
        }
        let popup = st.platform.popup_manager.find_popup(&current)?;
        let parent = popup_parent_surface(&popup)?;
        current = parent;
    }
}

pub(crate) fn is_layer_surface_tree(st: &Halley, surface: &WlSurface) -> bool {
    layer_surface_root_for_surface(st, surface).is_some()
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
        crate::compositor::focus::system::set_keyboard_focus(
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
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner;
    use smithay::utils::{Logical, Rectangle, Size};
    use smithay::wayland::shell::wlr_layer::{
        Anchor, ExclusiveZone, KeyboardInteractivity, Layer, LayerSurfaceCachedState, Margins,
    };

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

    #[test]
    fn lift_opens_on_monitor_under_cursor_not_focused_monitor() {
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

        // Focus on the left monitor, but the cursor is over the right one.
        state.model.monitor_state.current_monitor = "left".to_string();
        state.input.interaction_state.cursor.last_screen_global = Some((3200.0, 300.0));

        assert_eq!(
            super::assigned_monitor_for_no_output(&state, "halley-lift"),
            "right",
            "Lift should open on the monitor under the cursor"
        );

        // A non-Lift namespace still follows the focused/current monitor.
        assert_eq!(
            super::assigned_monitor_for_no_output(&state, "halley-panel"),
            "left",
            "other layer surfaces keep current-monitor placement"
        );
    }

    #[test]
    fn lift_falls_back_to_current_monitor_without_cursor() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        state.input.interaction_state.cursor.last_screen_global = None;

        let current = state.model.monitor_state.current_monitor.clone();
        assert_eq!(
            super::assigned_monitor_for_no_output(&state, "halley-lift"),
            current
        );
    }

    #[test]
    fn constrained_layer_popup_position_preserves_adjusted_positioner() {
        let positioner = super::PositionerState {
            rect_size: Size::<i32, Logical>::from((200, 80)),
            anchor_rect: Rectangle::<i32, Logical>::new((760, 560).into(), (20, 20).into()),
            anchor_edges: xdg_positioner::Anchor::BottomRight,
            gravity: xdg_positioner::Gravity::BottomRight,
            ..Default::default()
        };
        let target = Rectangle::<i32, Logical>::new((0, 0).into(), (800, 600).into());

        let (constrained_positioner, geometry) =
            super::constrained_layer_popup_position(positioner, target);

        assert!(constrained_positioner.constraint_adjustment.contains(
            xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY
                | xdg_positioner::ConstraintAdjustment::ResizeX
                | xdg_positioner::ConstraintAdjustment::ResizeY
        ));
        assert!(super::rectangle_fits_within(target, geometry));
    }

    #[test]
    fn dont_care_all_anchored_layer_ignores_reserved_zone() {
        let output_rect = Rectangle::<i32, Logical>::from_size((2560, 1440).into());
        let mut zone = output_rect;
        let top_bar = LayerSurfaceCachedState {
            size: (0, 30).into(),
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: ExclusiveZone::Exclusive(30),
            exclusive_edge: None,
            margin: Margins::default(),
            keyboard_interactivity: KeyboardInteractivity::None,
            layer: Layer::Top,
            last_acked: None,
        };
        let background = LayerSurfaceCachedState {
            size: (0, 0).into(),
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: ExclusiveZone::DontCare,
            exclusive_edge: None,
            margin: Margins::default(),
            keyboard_interactivity: KeyboardInteractivity::None,
            layer: Layer::Top,
            last_acked: None,
        };

        let _ = super::compute_layer_placement(output_rect, &mut zone, top_bar);
        assert_eq!(zone.loc.y, 30);
        assert_eq!(zone.size.h, 1410);

        let (origin, size) = super::compute_layer_placement(output_rect, &mut zone, background);

        assert_eq!(origin, (0, 0).into());
        assert_eq!(size, (2560, 1440).into());
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

fn layer_reservation_priority(data: LayerSurfaceCachedState) -> i32 {
    if exclusive_zone_amount(data.exclusive_zone) > 0 {
        0
    } else {
        1
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
    let fill_full_output =
        data.layer == Layer::Overlay || matches!(data.exclusive_zone, ExclusiveZone::DontCare);
    let placement_zone = if fill_full_output { output_rect } else { *zone };

    if width == 0 && anchored_left && anchored_right {
        width = placement_zone.size.w.max(1);
    }
    if height == 0 && anchored_top && anchored_bottom {
        height = placement_zone.size.h.max(1);
    }
    if width == 0 {
        width = output_rect.size.w.max(1);
    }
    if height == 0 {
        height = output_rect.size.h.max(1);
    }

    let mut x = if anchored_left {
        placement_zone.loc.x + data.margin.left
    } else if anchored_right {
        placement_zone.loc.x + placement_zone.size.w - width - data.margin.right
    } else {
        placement_zone.loc.x + (placement_zone.size.w - width) / 2
    };

    let mut y = if anchored_top {
        placement_zone.loc.y + data.margin.top
    } else if anchored_bottom {
        placement_zone.loc.y + placement_zone.size.h - height - data.margin.bottom
    } else {
        placement_zone.loc.y + (placement_zone.size.h - height) / 2
    };

    if anchored_left && anchored_right {
        x = placement_zone.loc.x + data.margin.left;
        width = (placement_zone.size.w - data.margin.left - data.margin.right).max(1);
    }
    if anchored_top && anchored_bottom {
        y = placement_zone.loc.y + data.margin.top;
        height = (placement_zone.size.h - data.margin.top - data.margin.bottom).max(1);
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
