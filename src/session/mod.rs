use std::borrow::Cow;
use std::time::Duration;

use smithay::utils::{Logical, Rectangle};

use smithay::wayland::seat::WaylandFocus;

use crate::wayland;

// Source folders group related session work. Explicit paths keep the existing
// session module boundaries and visibility while making those groups browsable.
#[path = "workspace/arrange.rs"]
mod arrange;
#[path = "startup/autostart.rs"]
mod autostart;
#[path = "onboarding/basics.rs"]
mod basics;
#[path = "windows/closing.rs"]
pub(crate) mod closing;
#[path = "workspace/cluster_runtime.rs"]
mod cluster_runtime;
#[path = "protocol/commit_timing.rs"]
mod commit_timing;
#[path = "input/cursor.rs"]
mod cursor;
#[path = "onboarding/decay_notice.rs"]
mod decay_notice;
#[path = "workspace/empty_cluster.rs"]
mod empty_cluster;
#[path = "startup/environment.rs"]
pub mod environment;
#[path = "windows/focus.rs"]
mod focus;
#[path = "protocol/foreign_toplevel.rs"]
mod foreign_toplevel;
#[path = "input/gesture/mod.rs"]
pub(crate) mod gesture;
#[path = "input/grab.rs"]
mod grab;
#[path = "capture/image_copy_capture.rs"]
mod image_copy_capture;
pub(crate) mod input;
#[path = "startup/state.rs"]
mod launch;
#[path = "windows/lifecycle.rs"]
mod lifecycle;
#[path = "workspace/navigation.rs"]
mod navigation;
#[path = "workspace/node_controls.rs"]
mod node_controls;
#[path = "windows/opening.rs"]
pub(crate) mod opening;
#[path = "display/output.rs"]
pub(crate) mod output;
#[path = "display/output_power.rs"]
mod output_power;
#[path = "input/pointer/mod.rs"]
pub(crate) mod pointer;
#[path = "protocol/handlers.rs"]
mod protocol;
#[path = "protocol/setup.rs"]
mod protocol_setup;
#[path = "display/render_completion.rs"]
mod render_completion;
#[path = "capture/screenshot.rs"]
mod screenshot;
#[path = "startup/spawn.rs"]
mod spawn;
#[path = "startup/startup_clusters.rs"]
mod startup_clusters;
#[path = "display/timing.rs"]
mod timing;
#[path = "input/touch.rs"]
pub(crate) mod touch;
#[cfg(feature = "xwayland")]
#[path = "diagnostics/trace.rs"]
pub(crate) mod trace;
#[cfg(not(feature = "xwayland"))]
#[path = "diagnostics/trace_disabled.rs"]
pub(crate) mod trace;
#[path = "drivers/tty/mod.rs"]
pub mod tty;
#[path = "windows/window_presentation.rs"]
mod window_presentation;
#[cfg(feature = "winit")]
#[path = "drivers/winit/mod.rs"]
pub mod winit;
#[path = "workspace/workspace.rs"]
pub(crate) mod workspace;

mod interaction;
mod settings;
mod state;

pub(crate) use cluster_runtime::*;
pub(crate) use focus::{focus_window, focus_window_after_close};
pub(crate) use grab::{
    activate_client_pointer_move, begin_client_pointer_move, begin_pointer_edge_pan,
    begin_pointer_move, begin_pointer_resize, begin_titlebar_pointer_move, begin_window_resize,
    settle_window_movement,
};
pub(crate) use input::{cluster_owns_focus, show_cluster_indicator, sync_cluster_activation_focus};
pub use interaction::InteractionState;
pub(crate) use navigation::{activate_node, center_pointer_on_output};
pub(crate) use node_controls::*;
pub use settings::RuntimeSettings;
pub use state::{OutputDriver, RenderDriver, Session, SessionDriver};
use timing::{
    install_frame_callback_fallback_timer, install_node_decay_timer, install_overlay_timer,
};
pub(crate) use timing::{release_fifo_barriers, schedule_fifo_barriers};
pub(crate) use window_presentation::*;

/// Builds popup unconstrain inputs from disjoint `Session` fields so a later
/// `&mut session.wayland` borrow stays valid.
macro_rules! popup_unconstrain_context {
    ($session:expr) => {
        $crate::wayland::popup::UnconstrainContext {
            cameras: &$session.cameras,
            clusters: &$session.clusters,
            nodes: &$session.nodes,
            window_animations: &$session.window_animations,
            fullscreen: &$session.fullscreen,
            maximize: &$session.maximize,
            decorations: &$session.settings.decorations,
            font: &$session.settings.font,
            now: $crate::frame_clock::monotonic_now(),
        }
    };
}

pub(crate) use popup_unconstrain_context;

pub(crate) fn cancel_grab_for_surface<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    if crate::input::grab::belongs_to_surface(&session.interactions.grab, surface) {
        cancel_compositor_grab(session);
        crate::input::grab::forget_resize_anchor(&mut session.interactions.resize_anchor, surface);
    }
}

/// Cancels compositor-owned pointer state and its cluster-side presentation
/// authority as one transaction. Modal overlays, session locking, and surface
/// destruction all use this path so a held workspace window cannot remain
/// floating or assigned to the wrong output.
pub(crate) fn cancel_compositor_grab<D: SessionDriver>(session: &mut Session<D>) {
    session.interactions.steam_close_pressed = None;
    let provisional_cluster = match &session.interactions.grab {
        crate::input::grab::Grab::MoveWindow {
            id: Some(id),
            cluster_drag: Some(drag),
            ..
        } => Some((*id, drag.output.clone())),
        _ => None,
    };
    if matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::ResizeWindow(_)
    ) {
        crate::input::grab::release_resize_anchor(&mut session.interactions.resize_anchor);
    }
    session.clusters.cancel_join_candidate();
    session.clusters.cancel_window_drag();
    if let Some((id, output_name)) = provisional_cluster {
        let output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == output_name)
            .cloned();
        if let Some(output) = output {
            crate::nodes::set_collapsed_output(session, id, &output);
            reconcile_cluster_surfaces(session, &output_name);
            session.request_redraw();
        }
    }
    session.interactions.grab = crate::input::grab::Grab::None;
    session
        .cursor
        .set_override(crate::cursor::OverrideSource::Grab, None);
}

pub(crate) fn admit_cluster_draft_window<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
) -> bool {
    let Some(app_id) = session
        .nodes
        .record(id)
        .and_then(|record| record.app_id.clone())
    else {
        return false;
    };
    let Some(complete) = session.clusters.match_pending_draft(id, &app_id) else {
        return false;
    };
    stage_draft_window(session, id);
    if !complete {
        return true;
    }

    let Some(output_name) = session.clusters.pending_draft_output().map(str::to_string) else {
        return true;
    };
    let members = session.clusters.ready_draft_members().unwrap_or_default();
    for member in members {
        restore_draft_window(session, member, &output_name);
    }
    match session
        .clusters
        .finish_pending_draft(&mut session.nodes.field)
    {
        Ok((cluster, build)) => {
            crate::nodes::resolve_new_cluster_core(session, cluster);
            if let Some(core) = session.clusters.core_node(cluster)
                && let Some(output) = session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == output_name)
                    .cloned()
                && let (Some(geometry), Some(view)) = (
                    session.wayland.space.output_geometry(&output),
                    session.cameras.view(&output_name),
                )
            {
                let position = halley_core::field::Vec2 {
                    x: geometry.loc.x as f32 + view.center.x,
                    y: geometry.loc.y as f32 + view.center.y,
                };
                if let Some(node) = session.nodes.field.node_mut(core) {
                    node.pos = position;
                }
                session.clusters.set_core_position(cluster, position);
            }
            crate::ipc::publish_cluster_draft(
                session,
                build.id,
                halley_ipc::ClusterDraftState::Completed,
                None,
            );
        }
        Err(message) => {
            eventline::warn!("clusters: failed to complete draft: {message}");
        }
    }
    session.request_redraw();
    true
}

fn stage_draft_window<D: SessionDriver>(session: &mut Session<D>, id: halley_core::field::NodeId) {
    let Some(record) = session.nodes.record(id).cloned() else {
        return;
    };
    session.wayland.space.unmap_elem(&record.window);
    if record.window.toplevel().is_some() {
        session
            .wayland
            .collapsed
            .insert(record.surface.clone(), record.window.clone());
    } else {
        crate::xwayland::set_hidden(&record.window, true);
        session.xwayland.set_window_iconic(&record.window);
    }
    session
        .nodes
        .set_collapsed(id, true, session.start_time.elapsed().as_millis() as u64);
    let _ = session.nodes.field.set_detached(id, true);
}

fn restore_draft_window<D: SessionDriver>(
    session: &mut Session<D>,
    id: halley_core::field::NodeId,
    output_name: &str,
) {
    let Some(record) = session.nodes.record(id).cloned() else {
        return;
    };
    if let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    {
        crate::wayland::set_window_output(&record.window, &output);
    }
    if let Some(window) = session.wayland.collapsed.remove(&record.surface) {
        session
            .wayland
            .space
            .map_element(window, record.geometry.loc, false);
    } else {
        crate::xwayland::set_hidden(&record.window, false);
        session.xwayland.set_window_normal(&record.window);
    }
    if let Some(record) = session.nodes.record_mut(id) {
        record.output = output_name.to_string();
    }
    session
        .nodes
        .set_collapsed(id, false, session.start_time.elapsed().as_millis() as u64);
    let _ = session.nodes.field.set_detached(id, false);
}

fn expire_cluster_draft<D: SessionDriver>(
    session: &mut Session<D>,
    now: std::time::Duration,
) -> bool {
    let Some(build) = session.clusters.take_timed_out_draft(now) else {
        return false;
    };
    for id in build.staged.iter().copied() {
        restore_draft_window(session, id, &build.output);
    }
    crate::ipc::publish_cluster_draft(
        session,
        build.id,
        halley_ipc::ClusterDraftState::Failed,
        Some("timed out waiting 30 seconds for launched windows".into()),
    );
    true
}

pub(crate) use lifecycle::{finish_window_unmap, prepare_window_unmap};

pub(crate) fn reconcile_pointer_constraints<D: SessionDriver>(session: &mut Session<D>) {
    pointer::reconcile_state(session);
}

pub(crate) fn has_active_pointer_confinement<D: SessionDriver>(session: &Session<D>) -> bool {
    pointer::has_active_confinement(session)
}

pub(crate) fn cursor_visible<D: SessionDriver>(session: &Session<D>) -> bool {
    pointer::cursor_visible(session)
}

pub(crate) fn cursor_override<D: SessionDriver>(
    session: &Session<D>,
) -> Option<smithay::input::pointer::CursorIcon> {
    pointer::cursor_override(session)
}

pub(crate) fn note_pointer_activity<D: SessionDriver>(session: &mut Session<D>) {
    session.cursor_policy.pointer_activity();
}

/// Requests that a client close a managed window.
///
/// X11 clients may replace their contents before withdrawing, so retain their
/// visible frame before sending the request. Native XDG close requests are
/// advisory and capture only at the authoritative buffer-removal boundary;
/// this keeps a client live if it presents and then cancels a confirmation.
pub(crate) fn request_window_close<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
) {
    let frozen = closing::capture_before_close_request(session, window);
    if let Some(toplevel) = window.toplevel() {
        toplevel.send_close();
    } else {
        crate::xwayland::close_window(window);
    }
    if frozen {
        session.request_redraw();
    }
}

pub(crate) fn activate_titlebar_control<D: SessionDriver>(
    session: &mut Session<D>,
    target: &crate::titlebar::ButtonTarget,
    serial: smithay::utils::Serial,
) {
    if !crate::titlebar::control_enabled(&target.window, target.control) {
        return;
    }
    match target.control {
        crate::titlebar::Control::Close => {
            let closed = target
                .window
                .wl_surface()
                .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
                .is_some_and(|id| crate::nodes::close(session, id));
            if !closed {
                request_window_close(session, &target.window);
            }
        }
        crate::titlebar::Control::Minimize => {
            if let Some(id) = target
                .window
                .wl_surface()
                .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            {
                let _ = crate::nodes::collapse(session, id, serial);
            }
        }
        crate::titlebar::Control::Maximize => {
            if let Some(surface) = target.window.wl_surface() {
                let maximized = session.maximize.contains(surface.as_ref());
                let _ = set_surface_field_maximized(session, surface.as_ref(), !maximized);
            }
        }
    }
}

pub(crate) fn sync_keyboard_focus<D: SessionDriver>(
    session: &mut Session<D>,
    serial: smithay::utils::Serial,
) {
    if session.session_lock.active() {
        if session.window_rules.set_focus(None) {
            session.refresh_dynamic_window_rules();
        }
        let focused = session
            .session_lock
            .focused_surface()
            .map(crate::xwayland::KeyboardFocusTarget::from);
        let keyboard = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup");
        let has_keyboard_focus = focused.is_some();
        pointer::prepare_keyboard_focus_change(session, None);
        keyboard.set_focus(session, focused, serial);
        session
            .xwayland
            .sync_active_window(None, has_keyboard_focus);
        return;
    }
    wayland::focus::refresh_selected_layer(&mut session.wayland);
    focus::reconcile_modal_focus(session);
    if let Some(surface) = session.wayland.focused_window.clone() {
        session
            .nodes
            .focus_surface(&surface, session.start_time.elapsed().as_millis() as u64);
    } else if session.nodes.focused().is_some_and(|id| {
        session
            .nodes
            .record(id)
            .is_some_and(|record| !record.collapsed)
            && session.clusters.cluster_for_member(id).is_none()
    }) {
        session.nodes.focus(None, 0);
    }
    let focused = wayland::focus::current(
        &session.wayland,
        &session.fullscreen,
        &session.clusters,
        &session.nodes,
        crate::frame_clock::monotonic_now(),
    )
    .and_then(|focus| match focus {
        wayland::focus::KeyboardFocus::Window(surface) => session
            .wayland
            .space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == &surface)
            })
            .and_then(crate::xwayland::KeyboardFocusTarget::for_window)
            .or_else(|| Some(surface.into())),
        wayland::focus::KeyboardFocus::ExclusiveLayer(surface)
        | wayland::focus::KeyboardFocus::OnDemandLayer(surface) => Some(surface.into()),
    });
    let keyboard = session
        .seat
        .get_keyboard()
        .expect("keyboard capability added at seat setup");
    let next_constraint_root = focused
        .as_ref()
        .and_then(|target| target.wl_surface().map(Cow::into_owned))
        .filter(|surface| session.wayland.focused_window.as_ref() == Some(surface));
    if session.window_rules.set_focus(next_constraint_root.clone()) {
        session.refresh_dynamic_window_rules();
    }
    let active_x11_window = focused
        .as_ref()
        .and_then(crate::xwayland::KeyboardFocusTarget::x11_window_id);
    let globally_active_x11_window = focused
        .as_ref()
        .and_then(crate::xwayland::KeyboardFocusTarget::globally_active_x11_window_id);
    let has_keyboard_focus = focused.is_some();
    if let Some(focused) = focused.as_ref() {
        focused.acknowledge_attention();
    }
    // Commit keyboard/X focus before retiring the old pointer constraint.
    // Xwayland releases a locked X pointer when it receives `unlocked`; doing
    // that while the old game still owns core X focus lets its final cursor
    // re-anchor affect the camera. The seat update is synchronous from the
    // compositor's perspective, so no input can reach the new target between
    // these two operations.
    keyboard.set_focus(session, focused, serial);
    session
        .xwayland
        .sync_active_window(active_x11_window, has_keyboard_focus);
    session
        .xwayland
        .focus_globally_active_window(globally_active_x11_window);
    pointer::prepare_keyboard_focus_change(session, next_constraint_root.as_ref());
    // Map, unmap, destroy and raise all funnel through here, so this is the one
    // place the X server's stack can drift from the compositor's.
    crate::window::enforce_dialog_stacking(&mut session.wayland);
    session.xwayland.sync_stacking_order(&session.wayland.space);
    // Activation is the point a client is most likely to act on its own idea of
    // where it is: menu placement, XQueryPointer, root-coordinate hit tests.
    // Hyprland resyncs here too (`activateWindow` -> `sendWindowSize(true)`).
    if let Some(xid) = active_x11_window
        && let Some(window) = crate::xwayland::window_for_xid(&session.wayland.space, xid)
    {
        crate::xwayland::sync_position(session, &window);
    }
    pointer::reconcile_state(session);
}
