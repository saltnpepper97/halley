//! Session timers and fallback frame delivery.

use super::{Session, SessionDriver, empty_cluster, input};
use crate::wayland;
use calloop::LoopHandle;
use calloop::timer::{TimeoutAction, Timer};
use smithay::wayland::seat::WaylandFocus;
use std::time::Duration;

pub(super) fn install_node_decay_timer<D: SessionDriver>(
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

pub(super) fn install_overlay_timer<D: SessionDriver>(
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

pub(super) fn install_frame_callback_fallback_timer<D: SessionDriver>(
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
