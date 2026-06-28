use super::*;
use crate::compositor::{actions, fullscreen, monitor, spawn, surface, workspace};
use crate::compositor::spawn::rules::ResolvedInitialWindowRule;
use smithay::desktop::{
    PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, find_popup_root_surface,
};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgDecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface};
use smithay::utils::{Logical, Rectangle};

fn popup_positioner_geometry(positioner: PositionerState) -> Rectangle<i32, Logical> {
    positioner.get_geometry()
}

fn rectangle_fits_within(target: Rectangle<i32, Logical>, rect: Rectangle<i32, Logical>) -> bool {
    rect.loc.x >= target.loc.x
        && rect.loc.y >= target.loc.y
        && (rect.loc.x as i64 + rect.size.w as i64) <= (target.loc.x as i64 + target.size.w as i64)
        && (rect.loc.y as i64 + rect.size.h as i64) <= (target.loc.y as i64 + target.size.h as i64)
}

/// Unconstrain `positioner` within `target` (output working area expressed relative
/// to the popup's parent), escalating to flip/slide/resize if it still overflows —
/// mirrors `constrain_layer_popup`. This is what slides a corner-anchored popup
/// (e.g. an XWayland override-redirect notification with `BottomRight` gravity) to
/// the screen edge instead of leaving it floating relative to the parent.
fn unconstrain_geometry_for_target(
    positioner: PositionerState,
    target: Rectangle<i32, Logical>,
) -> (PositionerState, Rectangle<i32, Logical>) {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner::ConstraintAdjustment;
    let mut adjusted = positioner;
    let mut geometry = adjusted.get_unconstrained_geometry(target);
    if !rectangle_fits_within(target, geometry) {
        adjusted.constraint_adjustment |= ConstraintAdjustment::FlipX
            | ConstraintAdjustment::FlipY
            | ConstraintAdjustment::SlideX
            | ConstraintAdjustment::SlideY
            | ConstraintAdjustment::ResizeX
            | ConstraintAdjustment::ResizeY;
        geometry = adjusted.get_unconstrained_geometry(target);
    }
    (adjusted, geometry)
}

/// For a popup whose root surface is a window node, the output working area of the
/// parent's monitor expressed relative to the parent geometry's top-left. `None` for
/// non-window popups (layer-shell etc.), which keep their existing handling.
fn window_popup_constraint_target(
    st: &Halley,
    popup: &PopupSurface,
) -> Option<(halley_core::field::NodeId, Rectangle<i32, Logical>)> {
    let kind = PopupKind::from(popup.clone());
    let root = find_popup_root_surface(&kind).ok()?;
    let node_id = *st.model.surface_to_node.get(&root.id())?;
    let monitor = st.model.monitor_state.node_monitor.get(&node_id)?.clone();
    let viewport = st.usable_viewport_for_monitor(monitor.as_str());
    let node = st.model.field.node(node_id)?;

    // Fullscreen is presentation-only: the window keeps its logical `node.pos` but is
    // drawn filling the output. So the popup's working area is the screen-filling
    // window itself (origin at the output top-left), not the `node.pos`-relative
    // viewport rect — using the latter offsets the constraint target by the window's
    // logical distance from the camera center and mis-slides the popup off its anchor.
    if st.fullscreen_monitor_for_node(node_id).is_some() {
        let (_gx, _gy, gw, gh) = crate::compositor::surface::window_geometry_for_node(st, node_id)
            .unwrap_or((0.0, 0.0, node.intrinsic_size.x, node.intrinsic_size.y));
        let target = Rectangle::new(
            (0, 0).into(),
            ((gw.round() as i32).max(1), (gh.round() as i32).max(1)).into(),
        );
        return Some((node_id, target));
    }

    let parent_tl_x = node.pos.x - node.intrinsic_size.x * 0.5;
    let parent_tl_y = node.pos.y - node.intrinsic_size.y * 0.5;
    let vp_tl_x = viewport.center.x - viewport.size.x * 0.5;
    let vp_tl_y = viewport.center.y - viewport.size.y * 0.5;
    let origin_x = (parent_tl_x - vp_tl_x).round() as i32;
    let origin_y = (parent_tl_y - vp_tl_y).round() as i32;

    let target = Rectangle::new(
        (-origin_x, -origin_y).into(),
        (
            (viewport.size.x.round() as i32).max(1),
            (viewport.size.y.round() as i32).max(1),
        )
            .into(),
    );
    Some((node_id, target))
}

/// Whether a window-parented popup should render pinned to the screen (immune to
/// camera zoom/pan) rather than tracking its parent window. Currently scoped to
/// the Steam client's notifications (e.g. install-complete), whose root window
/// app_id is `steam`. Deliberately excludes per-game `steam_app_*` windows and
/// every other app, so ordinary interactive context menus keep tracking their
/// parent.
fn popup_should_pin_to_screen(st: &Halley, root_node: halley_core::field::NodeId) -> bool {
    st.model
        .node_app_ids
        .get(&root_node)
        .is_some_and(|app_id| app_id.eq_ignore_ascii_case("steam"))
}

fn configure_popup_position(st: &mut Halley, popup: &PopupSurface, positioner: PositionerState) {
    // Window-parented popups (incl. XWayland override-redirect overlays via
    // xwayland-satellite) are unconstrained within the parent's monitor so corner
    // overlays slide to the screen edge and off-screen ones are pulled on-screen.
    if let Some((root_node, target)) = window_popup_constraint_target(st, popup) {
        let (adjusted, geometry) = unconstrain_geometry_for_target(positioner, target);
        popup.with_pending_state(|state| {
            state.positioner = adjusted;
            state.geometry = geometry;
        });
        // Freeze the pan-free anchor (`target.loc`) so the render path can pin
        // this popup to the monitor output instead of tracking the parent.
        let key = popup.wl_surface().id();
        if popup_should_pin_to_screen(st, root_node) {
            st.model.pinned_popup_anchor.insert(key, target.loc);
        } else {
            st.model.pinned_popup_anchor.remove(&key);
        }
        return;
    }
    popup.with_pending_state(|state| {
        state.positioner = positioner;
        state.geometry = popup_positioner_geometry(positioner);
    });
    super::handlers::constrain_layer_popup(st, popup, positioner);
}

fn restore_drag_offset_for_maximized_move(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    monitor: &str,
    press_global_sx: f32,
    press_global_sy: f32,
    now: std::time::Instant,
) -> Option<halley_core::field::Vec2> {
    let session = st.model.workspace_state.maximize_sessions.get(monitor)?;
    let snapshot = session.node_snapshots.get(&node_id)?;
    let (visual_center, visual_size) =
        crate::compositor::workspace::state::maximized_visual_for_node_on_monitor_at(
            st, node_id, monitor, now,
        )?;
    let (ws_w, ws_h, local_sx, local_sy) =
        st.local_screen_in_monitor(monitor, press_global_sx, press_global_sy);
    let pointer_world = crate::spatial::screen_to_world(st, ws_w, ws_h, local_sx, local_sy);
    let ratio_x = ((pointer_world.x - (visual_center.x - visual_size.x * 0.5))
        / visual_size.x.max(1.0))
    .clamp(0.0, 1.0);
    let ratio_y = ((pointer_world.y - (visual_center.y - visual_size.y * 0.5))
        / visual_size.y.max(1.0))
    .clamp(0.0, 1.0);

    Some(halley_core::field::Vec2 {
        x: (ratio_x - 0.5) * snapshot.size.x,
        y: (ratio_y - 0.5) * snapshot.size.y,
    })
}

impl XdgDecorationHandler for Halley {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: XdgDecorationMode) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_xdg_decoration_mode();
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
            self.apply_toplevel_tiled_hint(state);
        });
        toplevel.send_configure();
    }
}

delegate_xdg_decoration!(Halley);

impl XdgShellHandler for Halley {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.platform.xdg_shell_state
    }

    fn new_toplevel(&mut self, toplevel: ToplevelSurface) {
        let intent = spawn::rules::resolve_initial_window_intent(self, &toplevel);
        let initial_size = super::handlers::initial_toplevel_size(self, &toplevel, &intent);
        let is_default = intent.rule == ResolvedInitialWindowRule::default()
            && intent.parent_node.is_none()
            && !intent.prefer_app_intent;
        let monitor = if is_default {
            self.model
                .spawn_state
                .pending_spawn_monitor
                .clone()
                .filter(|m| self.model.monitor_state.monitors.contains_key(m))
                .unwrap_or_else(|| self.spawn_target_monitor_for_intent(&intent))
        } else {
            self.spawn_target_monitor_for_intent(&intent)
        };

        let view = self.usable_viewport_for_monitor(&monitor);
        let bounds_w = view.size.x as i32;
        let bounds_h = view.size.y as i32;
        toplevel.with_pending_state(|s| {
            if let Some((w, h)) = initial_size.configure_size {
                s.size = Some((w, h).into());
                s.bounds = Some((bounds_w, bounds_h).into());
            }
            s.decoration_mode = Some(self.preferred_xdg_decoration_mode());
            self.apply_toplevel_tiled_hint(s);
        });
        toplevel.send_configure();

        let is_transient = toplevel.parent().is_some();
        let wl = toplevel.wl_surface().clone();
        let id = workspace::lifecycle::ensure_node_for_surface(
            &mut self.surface_lifecycle_ctx(),
            &wl,
            "toplevel",
            initial_size.node_size,
            &intent,
        );
        let now = Instant::now();
        let _ = crate::protocol::wayland::activation::consume_pending_surface_activation(self, &wl);
        let node_monitor = self.model.monitor_state.node_monitor.get(&id).cloned();
        let node_cluster = self.model.field.cluster_id_for_member_public(id);
        let handled_by_active_cluster =
            node_cluster
                .zip(node_monitor.as_deref())
                .is_some_and(|(cid, monitor)| {
                    self.active_cluster_workspace_for_monitor(monitor) == Some(cid)
                });
        let handled_by_cluster = node_cluster.is_some();
        let handled_by_lift_staging =
            crate::compositor::clusters::system::pending_lift_cluster_node_staged(&*self, id);
        if !is_transient && !handled_by_cluster && !handled_by_lift_staging {
            self.model.spawn_state.pending_initial_reveal.insert(id);
            let _ = self.model.field.set_detached(id, true);
        }
        if !handled_by_cluster && !handled_by_lift_staging {
            let _ = self.model.field.touch(id, self.now_ms(now));
        }
        if !handled_by_lift_staging && (!handled_by_cluster || handled_by_active_cluster) {
            spawn::reveal::reveal_new_toplevel_node_via_ctx(
                &mut crate::compositor::ctx::spawn_ctx(self),
                id,
                is_transient,
                now,
            );
        }
        if !handled_by_cluster && !handled_by_lift_staging {
            if !self.model.spawn_state.pending_initial_reveal.contains(&id) {
                self.resolve_surface_overlap();
            }
            self.request_maintenance();
        } else if handled_by_lift_staging {
            self.request_maintenance();
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::refresh_surface_identity(
            &mut self.surface_lifecycle_ctx(),
            surface.wl_surface(),
            "Window",
        );
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::refresh_surface_identity(
            &mut self.surface_lifecycle_ctx(),
            surface.wl_surface(),
            "Window",
        );
    }

    fn new_popup(&mut self, popup: PopupSurface, positioner: PositionerState) {
        let _ = self
            .platform
            .popup_manager
            .track_popup(PopupKind::from(popup.clone()));
        configure_popup_position(self, &popup, positioner);
        let _ = popup.send_configure();
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            return;
        };
        let now = Instant::now();
        let restore_drag_offset = self
            .input
            .interaction_state
            .last_pointer_screen_global
            .and_then(|(press_global_sx, press_global_sy)| {
                crate::compositor::workspace::state::maximize_session_monitor_for_node(
                    self, node_id,
                )
                .and_then(|monitor| {
                    restore_drag_offset_for_maximized_move(
                        self,
                        node_id,
                        monitor.as_str(),
                        press_global_sx,
                        press_global_sy,
                        now,
                    )
                })
            });
        if let Some(monitor) =
            crate::compositor::workspace::state::maximize_session_monitor_for_node(self, node_id)
        {
            let _ = crate::compositor::workspace::state::abort_maximize_session_for_monitor(
                self,
                monitor.as_str(),
            );
        }
        let focus_target = surface::stack_focus_target_for_node(self, node_id).unwrap_or(node_id);
        self.set_recent_top_node(focus_target, now + std::time::Duration::from_millis(1200));
        self.set_interaction_focus(Some(focus_target), 700, now);
        if let Some((press_global_sx, press_global_sy)) =
            self.input.interaction_state.last_pointer_screen_global
        {
            self.input.interaction_state.pending_move_press =
                Some(crate::compositor::interaction::state::PendingMovePress {
                    node_id,
                    press_global_sx,
                    press_global_sy,
                    workspace_active: self.has_active_cluster_workspace(),
                    restore_drag_offset,
                });
        }
        self.request_maintenance();
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        fullscreen::system::enter_xdg_fullscreen(self, node_id, output, Instant::now());
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        fullscreen::system::exit_xdg_fullscreen(self, node_id, Instant::now());
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        // Maximize is not allowed inside a cluster (it conflicts with the cluster's
        // own tiling/stacking session). Silently ignore a client maximize request
        // (e.g. a GTK title-bar button) on a cluster member rather than mapping it
        // to fullscreen, so the title-bar control reflects that maximize is barred.
        if self
            .model
            .field
            .cluster_id_for_member_public(node_id)
            .is_some()
        {
            surface.send_configure();
            return;
        }
        // Map a client maximize request (e.g. a GTK title-bar maximize button) to
        // fullscreen: edge-to-edge, zoom 1.0, no decorations. If the node is
        // already fullscreen — whether from a prior maximize or a Mod+F keybind —
        // leave its session untouched so an app re-request neither clobbers a user
        // fullscreen nor flips its origin (which would let a later unmaximize
        // dismiss the user's fullscreen). `enter_fullscreen` is otherwise idempotent.
        if self.fullscreen_monitor_for_node(node_id).is_some() {
            surface.send_configure();
            return;
        }
        fullscreen::system::enter_xdg_fullscreen(self, node_id, None, Instant::now());
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        // Only tear down a fullscreen that we started from a client maximize
        // request, so an app unmaximize can never dismiss a user-initiated
        // (Mod+F) fullscreen.
        let is_client_origin = self.model.fullscreen_state.fullscreen_origin.get(&node_id)
            == Some(&fullscreen::state::FullscreenOrigin::ClientRequest);
        if self.fullscreen_monitor_for_node(node_id).is_some() && is_client_origin {
            fullscreen::system::exit_xdg_fullscreen(self, node_id, Instant::now());
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        let key = surface.wl_surface().id();
        let Some(node_id) = self.model.surface_to_node.get(&key).copied() else {
            surface.send_configure();
            return;
        };
        let monitor = self
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| self.focused_monitor().to_string());
        let _ = actions::window::toggle_node_state(self, node_id, Instant::now(), monitor.as_str());
        surface.send_configure();
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::from(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let seat = self.platform.seat.clone();
        // Actually install the popup grab on the seat (standard Smithay flow,
        // mirroring the DnD `set_grab`). Without this the grab is discarded and the
        // menu only stays focused while the pointer is over its hit region — so a
        // popup that overflows the parent window loses pointer focus in the overflow
        // and Qt clients (kdenlive) dismiss it on motion. The installed grab keeps
        // pointer + keyboard focus on the popup chain and dismisses on outside click.
        let Ok(mut grab) = self
            .platform
            .popup_manager
            .grab_popup::<Self>(root, popup, &seat, serial)
        else {
            return;
        };

        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            // Route popup-grab focus through the choke point so it flushes stale forwarded
            // keys like every other focus change (no surface inherits a stuck repeat).
            crate::compositor::focus::system::set_keyboard_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }

        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(
                self,
                PopupPointerGrab::new(&grab),
                serial,
                smithay::input::pointer::Focus::Keep,
            );
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        configure_popup_position(self, &surface, positioner);
        surface.send_repositioned(token);
        self.request_maintenance();
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.model
            .pinned_popup_anchor
            .remove(&surface.wl_surface().id());
        self.platform.popup_manager.cleanup();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        workspace::lifecycle::on_toplevel_destroyed(&mut self.surface_lifecycle_ctx(), surface);
    }
}

delegate_xdg_shell!(Halley);

impl XdgActivationHandler for Halley {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.platform.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        crate::protocol::wayland::activation::request_surface_activation(
            self,
            &surface,
            token.as_str(),
            &token_data,
            Instant::now(),
        );
    }
}

delegate_xdg_activation!(Halley);

#[cfg(test)]
mod tests {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_positioner;
    use smithay::utils::{Logical, Rectangle, Size};

    use super::*;

    #[test]
    fn normal_popup_geometry_uses_client_positioner() {
        let mut positioner = PositionerState {
            rect_size: Size::<i32, Logical>::from((120, 48)),
            anchor_rect: Rectangle::<i32, Logical>::new((24, 30).into(), (80, 20).into()),
            anchor_edges: xdg_positioner::Anchor::BottomLeft,
            gravity: xdg_positioner::Gravity::BottomRight,
            ..Default::default()
        };
        positioner.offset = (7, 11).into();

        let geometry = popup_positioner_geometry(positioner);

        assert_eq!(geometry, positioner.get_geometry());
        assert_ne!(geometry.loc, (0, 0).into());
    }
}

impl WlrLayerShellHandler for Halley {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.platform.wlr_layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        monitor::layer_shell::register_layer_surface(
            &mut self.layer_shell_ctx(),
            surface,
            output,
            layer,
            namespace,
        );
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        monitor::layer_shell::remove_layer_surface(&mut self.layer_shell_ctx(), &surface);
    }
}

delegate_layer_shell!(Halley);

impl OutputHandler for Halley {}

delegate_output!(Halley);
