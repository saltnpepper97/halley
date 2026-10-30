use std::borrow::Cow;
use std::time::Duration;

use calloop::LoopHandle;
use calloop::timer::{TimeoutAction, Timer};
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
#[path = "display/render_completion.rs"]
mod render_completion;
#[path = "capture/screenshot.rs"]
mod screenshot;
#[path = "startup/spawn.rs"]
mod spawn;
#[path = "startup/startup_clusters.rs"]
mod startup_clusters;
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
pub(crate) use input::{cluster_owns_focus, show_cluster_indicator, sync_cluster_activation_focus};
pub use interaction::InteractionState;
pub(crate) use navigation::{activate_node, center_pointer_on_output};
pub(crate) use node_controls::*;
pub use settings::RuntimeSettings;
pub use state::{OutputDriver, RenderDriver, Session, SessionDriver};
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

fn install_node_decay_timer<D: SessionDriver>(
    handle: &LoopHandle<'_, Session<D>>,
) -> Result<(), Box<dyn std::error::Error>> {
    handle
        .insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            |_, _, session| {
                crate::nodes::tick_decay(session);
                TimeoutAction::ToDuration(Duration::from_secs(1))
            },
        )
        .map(|_| ())
        .map_err(Into::into)
}

fn install_overlay_timer<D: SessionDriver>(
    handle: &LoopHandle<'_, Session<D>>,
) -> Result<(), Box<dyn std::error::Error>> {
    handle
        .insert_source(
            Timer::from_duration(Duration::from_millis(8)),
            |_, _, session| {
                let now = crate::frame_clock::monotonic_now();
                let empty_cluster_changed = empty_cluster::sync(session, now);
                let overlays_changed = session.shell.overlays.wakeup(now) | empty_cluster_changed;
                let bloom_changed = session.clusters.bloom_wakeup(now);
                if bloom_changed {
                    for core in session.clusters.bloom_pinned_core_nodes() {
                        session.nodes.clear_direct_motion(core);
                    }
                }
                let interactions_changed = input::wakeup_cluster_interactions(session, now);
                let resize_changed = input::wakeup_smooth_resize(session, now);
                if overlays_changed || bloom_changed || interactions_changed || resize_changed {
                    session.request_redraw();
                }
                TimeoutAction::ToDuration(Duration::from_millis(8))
            },
        )
        .map(|_| ())
        .map_err(Into::into)
}

fn install_frame_callback_fallback_timer<D: SessionDriver>(
    handle: &LoopHandle<'_, Session<D>>,
) -> Result<(), Box<dyn std::error::Error>> {
    handle
        .insert_source(
            Timer::from_duration(Duration::from_secs(1)),
            |_, _, session| {
                let barriers = session
                    .wayland
                    .fifo
                    .take_stalled(crate::frame_clock::monotonic_now());
                release_fifo_barriers(session, barriers);
                send_fallback_frame_callbacks(session);
                TimeoutAction::ToDuration(Duration::from_secs(1))
            },
        )
        .map(|_| ())
        .map_err(Into::into)
}

pub(crate) fn release_fifo_barriers<D: SessionDriver>(
    session: &mut Session<D>,
    barriers: Vec<wayland::fifo::FrameBarrier>,
) {
    use smithay::wayland::compositor::CompositorHandler;
    let clients = wayland::fifo::signal(barriers);
    let display = session.wayland.display_handle.clone();
    for client in clients {
        session
            .client_compositor_state(&client)
            .blocker_cleared(session, &display);
    }
}

/// Nested EGL has no per-frame page-flip callback. Unchanged native frames
/// likewise have no DRM submission. Give their captured barriers one output
/// refresh opportunity, independently of client frame-callback requests.
pub(crate) fn schedule_fifo_barriers<D: SessionDriver>(
    session: &mut Session<D>,
    handle: &LoopHandle<'_, Session<D>>,
    output: &smithay::output::Output,
    barriers: Vec<wayland::fifo::FrameBarrier>,
) {
    if barriers.is_empty() {
        return;
    }
    let refresh = output
        .current_mode()
        .filter(|mode| mode.refresh > 0)
        .map(|mode| Duration::from_secs_f64(1000.0 / f64::from(mode.refresh)))
        .unwrap_or(Duration::from_secs_f64(1.0 / 60.0));
    // Retain a copy only until registration succeeds, so an unavailable
    // timer cannot leave the client stuck. Early release is permitted to
    // preserve forward progress; acquire-fence blockers remain independent.
    let fallback = barriers.clone();
    if handle
        .insert_source(Timer::from_duration(refresh), move |_, _, session| {
            release_fifo_barriers(session, barriers.clone());
            TimeoutAction::Drop
        })
        .is_err()
    {
        release_fifo_barriers(session, fallback);
    }
}

fn send_fallback_frame_callbacks<D: SessionDriver>(session: &mut Session<D>) {
    let outputs = session.wayland.space.outputs().cloned().collect::<Vec<_>>();
    let elapsed = session.start_time.elapsed();
    let callback_now = crate::frame_clock::monotonic_now();
    for output in outputs {
        let sequence = session.driver.frame_callback_sequence(&output);
        if session.session_lock.active() {
            crate::wayland::session_lock::send_frames(
                &session.session_lock,
                &output,
                elapsed,
                sequence,
            );
        } else {
            let cluster_exclusive_member =
                crate::wayland::frame_callbacks::cluster_exclusive_callback_member(
                    &session.wayland.space,
                    &session.clusters,
                    &session.nodes,
                    &session.fullscreen,
                    &session.maximize,
                    &output,
                    callback_now,
                );
            for window in session.wayland.space.elements() {
                let window_member = window
                    .wl_surface()
                    .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()));
                let compositor_snapshot = window.wl_surface().is_some_and(|surface| {
                    session
                        .render
                        .fullscreen_textures
                        .awaiting_target(surface.as_ref())
                });
                let require_visible = crate::wayland::frame_callbacks::requires_render_visibility(
                    window_member,
                    cluster_exclusive_member,
                    compositor_snapshot,
                );
                window.send_frame(
                    &output,
                    elapsed,
                    crate::wayland::frame_callbacks::FALLBACK_THROTTLE,
                    |surface, states| {
                        crate::wayland::frame_callbacks::callback_output(
                            surface,
                            states,
                            &output,
                            sequence,
                            require_visible,
                        )
                    },
                );
            }
        }
        crate::wayland::layer_shell::send_frames(&output, elapsed, sequence);
        crate::cursor::surface::send_frame(
            &session.cursor,
            &session.wayland.space,
            &output,
            session.pointer.position(),
            elapsed,
            sequence,
        );
        crate::wayland::dnd::send_frame(
            session.wayland.dnd_icon.as_ref(),
            &output,
            elapsed,
            sequence,
        );
    }
}

pub(crate) fn reconcile_pointer_constraints<D: SessionDriver>(session: &mut Session<D>) {
    pointer::reconcile_state(session);
}

/// Applies cluster workspace geometry at the client protocol boundary.
///
/// ClusterSystem owns the layout and deduplication; Session owns Smithay
/// surfaces and X11 configure calls. Field remains unaware of workspace sizes.
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

/// Hand an interaction the position actually on screen, instead of the already
/// committed movement endpoint. The interaction now owns the window's geometry.
pub(crate) fn settle_window_movement<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
) {
    let Some(surface) = window.wl_surface() else {
        return;
    };
    let Some(current) = session.wayland.space.element_geometry(window) else {
        return;
    };
    let now = crate::frame_clock::monotonic_now();
    let visible = session
        .window_animations
        .movement_visual(surface.as_ref(), now, current);
    session.window_animations.cancel_move(surface.as_ref());
    if let Some(visible) = visible {
        session.wayland.space.relocate_element(window, visible.loc);
        if crate::xwayland::is_x11(window) {
            crate::xwayland::configure_window(session, window, visible);
        }
    }
}

pub(crate) fn begin_window_resize<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    handle: crate::input::grab::ResizeHandle,
    button: u32,
    cursor: halley_core::field::Vec2,
    visual_geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    serial: smithay::utils::Serial,
) -> bool {
    let surface = window.wl_surface();
    if surface
        .as_ref()
        .is_some_and(|surface| session.maximize.contains(surface.as_ref()))
        || surface
            .as_ref()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| session.clusters.active_layout_for_member(id).is_some())
        || surface
            .as_ref()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
    {
        return false;
    }
    settle_window_movement(session, window);
    let Some(start_rect) = session.wayland.space.element_geometry(window) else {
        return false;
    };
    focus::focus_window_from_pointer(session, window, serial);
    session.interactions.grab =
        crate::input::grab::Grab::ResizeWindow(crate::input::grab::ResizeState {
            window: window.clone(),
            handle,
            button,
            start_rect,
            start_cursor: cursor,
            start_screen: session.pointer.position(),
            screen_to_source_scale: crate::input::grab::resize_screen_to_source_scale(
                start_rect,
                visual_geometry,
            ),
            target_size: start_rect.size,
            preview_size: halley_core::field::Vec2 {
                x: start_rect.size.w as f32,
                y: start_rect.size.h as f32,
            },
            last_smooth_tick: crate::frame_clock::monotonic_now(),
        });
    session.interactions.resize_anchor =
        window.toplevel().map(|_| crate::input::grab::ResizeAnchor {
            window: window.clone(),
            handle,
            phase: crate::input::grab::ResizePhase::Ongoing,
            last_configure: None,
            last_size: start_rect.size,
        });
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(handle.cursor_icon()),
    );
    true
}

pub(crate) fn begin_pointer_resize<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    handle: crate::input::grab::ResizeHandle,
    button: u32,
) -> bool {
    let Some(route) = pointer::route_client(session) else {
        return false;
    };
    let routed_window = match &route.target {
        crate::input::pointer::PointerTarget::Window(routed)
        | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
        _ => return false,
    };
    if routed_window != window {
        return false;
    }
    let cursor = halley_core::field::Vec2 {
        x: route.location.x as f32,
        y: route.location.y as f32,
    };
    let visual_geometry = route.visual_geometry.unwrap_or_else(|| {
        session
            .wayland
            .space
            .element_geometry(window)
            .unwrap_or_else(|| Rectangle::from_size((1, 1).into()))
    });
    begin_window_resize(
        session,
        window,
        handle,
        button,
        cursor,
        visual_geometry,
        smithay::utils::SERIAL_COUNTER.next_serial(),
    )
}

/// Arms a client-requested move without committing any move side effects.
/// Toolkits commonly send this request from their title-bar press handler,
/// before they know whether the gesture will become a click or a drag.
pub(crate) fn begin_client_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pending_pointer_move(session, window, serial, button, true)
}

pub(crate) fn begin_titlebar_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pending_pointer_move(session, window, serial, button, false)
}

fn begin_pending_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
    client_owned: bool,
) -> bool {
    if !matches!(session.interactions.grab, crate::input::grab::Grab::None)
        || !crate::window::accepts_compositor_grab(window)
        || window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
        || window.wl_surface().is_some_and(|surface| {
            session
                .fullscreen
                .is_fullscreen_or_pending(surface.as_ref())
        })
    {
        return false;
    }
    let Some(route) = pointer::route_client(session) else {
        return false;
    };
    let routed_window = match &route.target {
        crate::input::pointer::PointerTarget::Window(routed)
        | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
        _ => return false,
    };
    if routed_window != window {
        return false;
    }
    let visual_geometry = route.visual_geometry.unwrap_or_else(|| {
        session
            .wayland
            .space
            .element_geometry(window)
            .unwrap_or_else(|| window.geometry())
    });
    let maximized = window
        .wl_surface()
        .is_some_and(|surface| session.maximize.contains(surface.as_ref()));
    session.interactions.grab =
        crate::input::grab::Grab::PendingWindowMove(crate::input::grab::PendingWindowMove {
            window: window.clone(),
            serial,
            button,
            press_screen: smithay::utils::Point::from(session.pointer.position()),
            output: route.output.name(),
            visual_geometry,
            maximized,
            client_owned,
        });
    true
}

/// Starts a compositor-owned move immediately. Client title bars use
/// `begin_client_pointer_move` and promote only after real pointer motion.
pub(crate) fn begin_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pointer_move_active(session, window, serial, button, false, None, false)
}

pub(crate) fn begin_pointer_edge_pan<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
) -> bool {
    begin_pointer_move_active(session, window, serial, button, false, None, true)
}

pub(crate) fn activate_client_pointer_move<D: SessionDriver>(
    session: &mut Session<D>,
    pending: crate::input::grab::PendingWindowMove,
) -> bool {
    let window = pending.window.clone();
    begin_pointer_move_active(
        session,
        &window,
        pending.serial,
        pending.button,
        pending.client_owned,
        Some(pending),
        false,
    )
}

fn begin_pointer_move_active<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    serial: smithay::utils::Serial,
    button: u32,
    client_owned: bool,
    pending: Option<crate::input::grab::PendingWindowMove>,
    edge_pan_requested: bool,
) -> bool {
    if !crate::window::accepts_compositor_grab(window)
        || window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| node_user_pinned(session, id))
        || window.wl_surface().is_some_and(|surface| {
            session
                .fullscreen
                .is_fullscreen_or_pending(surface.as_ref())
        })
    {
        return false;
    }
    let (output_name, visual_geometry, press_screen, was_maximized) =
        if let Some(pending) = pending.as_ref() {
            (
                pending.output.clone(),
                pending.visual_geometry,
                pending.press_screen,
                pending.maximized,
            )
        } else {
            let Some(route) = pointer::route_client(session) else {
                return false;
            };
            let routed_window = match &route.target {
                crate::input::pointer::PointerTarget::Window(routed)
                | crate::input::pointer::PointerTarget::Decoration { window: routed, .. } => routed,
                _ => return false,
            };
            if routed_window != window {
                return false;
            }
            let visual = route.visual_geometry.unwrap_or_else(|| {
                session
                    .wayland
                    .space
                    .element_geometry(window)
                    .unwrap_or_else(|| window.geometry())
            });
            let maximized = window
                .wl_surface()
                .is_some_and(|surface| session.maximize.contains(surface.as_ref()));
            (
                route.output.name(),
                visual,
                smithay::utils::Point::from(session.pointer.position()),
                maximized,
            )
        };
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return false;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    let Some(camera) = session.cameras.get(&output_name) else {
        return false;
    };
    if edge_pan_requested && was_maximized {
        return false;
    }
    let pointer_position = session.pointer.position();
    let pointer_world =
        crate::input::grab::screen_to_world_on_output(pointer_position, camera, output_geometry);
    let press_world = crate::input::grab::screen_to_world_on_output(
        (press_screen.x, press_screen.y),
        camera,
        output_geometry,
    );
    let world = halley_core::field::Vec2 {
        x: pointer_world.x,
        y: pointer_world.y,
    };
    settle_window_movement(session, window);
    let Some(window_location) = session.wayland.space.element_location(window) else {
        return false;
    };
    let restore = was_maximized
        .then(|| {
            window
                .wl_surface()
                .and_then(|surface| session.maximize.restore(surface.as_ref()))
        })
        .flatten();

    let id = window
        .wl_surface()
        .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()));
    let mut cluster_drag = id.and_then(|id| {
        let cluster_id = session.clusters.cluster_for_member(id)?;
        let metadata = session.clusters.metadata(cluster_id)?;
        let floating = session.clusters.is_member_floating(id);
        (session.clusters.active_on(&metadata.output) == Some(cluster_id)
            && (floating || metadata.output == output_name))
            .then(|| {
                let kind = if floating {
                    crate::input::grab::ClusterWindowDragKind::Floating
                } else {
                    crate::input::grab::ClusterWindowDragKind::Layout(metadata.layout)
                };
                crate::input::grab::ClusterWindowDrag {
                    cluster_id,
                    output: metadata.output.clone(),
                    kind,
                    on_origin_output: metadata.output == output_name,
                }
            })
    });
    if cluster_drag.as_ref().is_some_and(|drag| {
        matches!(
            drag.kind,
            crate::input::grab::ClusterWindowDragKind::Layout(
                halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking
            )
        ) && id != session.clusters.first_member(drag.cluster_id)
    }) {
        return false;
    }
    if edge_pan_requested {
        let output_right = output_geometry.loc.x + output_geometry.size.w;
        let output_bottom = output_geometry.loc.y + output_geometry.size.h;
        let visual_right = visual_geometry.loc.x + visual_geometry.size.w;
        let visual_bottom = visual_geometry.loc.y + visual_geometry.size.h;
        let fully_visible = visual_geometry.loc.x >= output_geometry.loc.x
            && visual_geometry.loc.y >= output_geometry.loc.y
            && visual_right <= output_right
            && visual_bottom <= output_bottom;
        if id.is_none()
            || cluster_drag.is_some()
            || !fully_visible
            || session.clusters.active_on(&output_name).is_some()
        {
            return false;
        }
    }
    focus::focus_window_from_pointer(session, window, serial);

    let mut cluster_grab_location = None;
    let mut cluster_drag_rect = None;
    let anchor = if cluster_drag.is_some() {
        let offset = crate::input::grab::screen_grip_offset(
            (press_screen.x, press_screen.y),
            visual_geometry.loc,
        );
        cluster_drag_rect = Some(Rectangle::new(
            visual_geometry.loc - output_geometry.loc,
            visual_geometry.size,
        ));
        let camera = session
            .cameras
            .get(&output_name)
            .expect("the pointer output camera was validated above");
        cluster_grab_location = Some(crate::input::grab::world_location_from_screen_grip(
            pointer_position,
            offset,
            camera,
            output_geometry,
        ));
        crate::input::grab::WindowGrabAnchor::Screen(offset)
    } else if was_maximized {
        let source = restore
            .as_ref()
            .map(|restore| restore.geometry)
            .or_else(|| session.wayland.space.element_geometry(window))
            .unwrap_or_else(|| window.geometry());
        let ratio_x = ((press_screen.x - f64::from(visual_geometry.loc.x))
            / f64::from(visual_geometry.size.w.max(1)))
        .clamp(0.0, 1.0);
        let ratio_y = ((press_screen.y - f64::from(visual_geometry.loc.y))
            / f64::from(visual_geometry.size.h.max(1)))
        .clamp(0.0, 1.0);
        crate::input::grab::WindowGrabAnchor::Source(halley_core::field::Vec2 {
            x: -(source.size.w as f32 * ratio_x as f32),
            y: -(source.size.h as f32 * ratio_y as f32),
        })
    } else {
        crate::input::grab::WindowGrabAnchor::Source(halley_core::field::Vec2 {
            x: window_location.x as f32 - press_world.x,
            y: window_location.y as f32 - press_world.y,
        })
    };

    if let Some(expected) = restore.as_ref() {
        let maximize_output = session
            .maximize
            .output_for_surface(&expected.surface)
            .map(str::to_owned);
        if let Some(restore) = session.maximize.take_restore(&expected.surface) {
            session.render.fullscreen_textures.remove(&restore.surface);
            configure_field_geometry(session, &restore);
            if let Some(output) = maximize_output {
                let _ = session.cameras.apply_field_maximize(&output, None);
            }
        }
    }

    if let (Some(id), Some(location), Some(drag)) =
        (id, cluster_grab_location, cluster_drag.as_mut())
    {
        let Some(rect) = cluster_drag_rect else {
            return false;
        };
        let began = match drag.kind {
            crate::input::grab::ClusterWindowDragKind::Layout(_) => session
                .clusters
                .begin_workspace_drag(&drag.output, id, rect),
            crate::input::grab::ClusterWindowDragKind::Floating => session
                .clusters
                .begin_floating_member_drag(&drag.output, &output_name, id, rect),
        };
        if !began {
            return false;
        }
        reconcile_cluster_surfaces(session, &output_name);
        session.wayland.space.relocate_element(window, location);
        crate::window::raise_managed(&mut session.wayland, window);
        session.xwayland.raise_window(window);
    } else if let Some(id) = id {
        let _ = session.clusters.begin_field_drag(id);
        crate::window::raise_managed(&mut session.wayland, window);
        session.xwayland.raise_window(window);
    }

    if let Some(id) = id {
        session.nodes.clear_direct_motion(id);
    }
    let drag_size = restore.as_ref().map(|restore| restore.geometry.size);
    let center = drag_size
        .and_then(|size| {
            let camera = session.cameras.get(&output_name)?;
            let location = anchor.world_location(pointer_position, camera, output_geometry);
            Some(halley_core::field::Vec2 {
                x: location.x as f32 + size.w as f32 * 0.5,
                y: location.y as f32 + size.h as f32 * 0.5,
            })
        })
        .or_else(|| {
            session
                .wayland
                .space
                .element_geometry(window)
                .map(|geometry| halley_core::field::Vec2 {
                    x: geometry.loc.x as f32 + geometry.size.w as f32 * 0.5,
                    y: geometry.loc.y as f32 + geometry.size.h as f32 * 0.5,
                })
        })
        .unwrap_or(world);
    session.interactions.grab = crate::input::grab::Grab::MoveWindow {
        id,
        window: window.clone(),
        cluster_drag,
        drag_size,
        button,
        client_owned,
        anchor,
        edge_pan: edge_pan_requested.then(|| {
            crate::input::grab::WindowEdgePan::new(
                output_name.clone(),
                crate::frame_clock::monotonic_now(),
            )
        }),
        last_world: center,
        last_update: crate::frame_clock::monotonic_now(),
        velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
    };
    session.cursor.set_override(
        crate::cursor::OverrideSource::Grab,
        Some(smithay::input::pointer::CursorIcon::Grabbing),
    );
    true
}
