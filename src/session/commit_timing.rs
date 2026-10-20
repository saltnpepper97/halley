use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::CompositorHandler;

use super::{Session, SessionDriver};

/// One event source per session, regardless of pending commits or outputs.
/// Far-future deadlines are rearmed in bounded steps without overflowing an
/// Instant or releasing content before its absolute presentation-clock time.
pub(super) struct Policy<D: SessionDriver> {
    handle: LoopHandle<'static, Session<D>>,
    queue: crate::wayland::commit_timing::State,
    alarm: Option<(Duration, RegistrationToken)>,
}

impl<D: SessionDriver> Policy<D> {
    pub(super) fn new(handle: LoopHandle<'static, Session<D>>) -> Self {
        Self {
            handle,
            queue: Default::default(),
            alarm: None,
        }
    }
}

pub(super) fn prepare<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    if session
        .commit_timing
        .queue
        .prepare(surface, crate::frame_clock::monotonic_now())
    {
        rearm(session);
    }
}

pub(super) fn destroyed<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    let client = session.commit_timing.queue.destroyed(surface);
    rearm(session);
    if let Some(client) = client {
        session.commit_timing.handle.insert_idle(move |session| {
            let display = session.wayland.display_handle.clone();
            session
                .client_compositor_state(&client)
                .blocker_cleared(session, &display);
        });
    }
}

fn rearm<D: SessionDriver>(session: &mut Session<D>) {
    let next = session.commit_timing.queue.next_deadline();
    if session.commit_timing.alarm.as_ref().map(|(time, _)| *time) == next {
        return;
    }
    let handle = session.commit_timing.handle.clone();
    if let Some((_, token)) = session.commit_timing.alarm.take() {
        handle.remove(token);
    }
    let Some(deadline) = next else {
        return;
    };
    let delay = deadline
        .saturating_sub(crate::frame_clock::monotonic_now())
        .min(Duration::from_secs(60));
    match handle.insert_source(Timer::from_duration(delay), |_, _, session| {
        session.commit_timing.alarm = None;
        let clients = session
            .commit_timing
            .queue
            .release_due(crate::frame_clock::monotonic_now());
        let display = session.wayland.display_handle.clone();
        for client in clients {
            session
                .client_compositor_state(&client)
                .blocker_cleared(session, &display);
        }
        rearm(session);
        TimeoutAction::Drop
    }) {
        Ok(token) => session.commit_timing.alarm = Some((deadline, token)),
        Err(_) => {
            let display = session.wayland.display_handle.clone();
            for surface in session.commit_timing.queue.drain_surfaces() {
                crate::wayland::buffer_readiness::reject_commit(
                    &surface,
                    "commit timing setup failed: deadline event source was not registered".into(),
                    |client, error| {
                        session
                            .driver
                            .defer_client_error(client, display.clone(), error)
                    },
                );
            }
        }
    }
}
