//! FIFO barriers belong to the surface commit that was actually rendered.
//! Keep immutable frame snapshots so a delayed GPU submission or another
//! output's presentation cannot release a newer commit's barrier.
use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::renderer::element::RenderElementStates;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource, Weak, backend::ObjectId};
use smithay::wayland::compositor::{Barrier, with_states};
use smithay::wayland::fifo::FifoBarrierCachedState;

/// A slow fallback for surfaces without output progress, including unmapped
/// surfaces and suspended outputs. The existing one-second timer bounds the
/// actual wait to less than two seconds. It never bypasses acquire fences.
const STALLED_AFTER: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct FrameBarrier {
    surface: Weak<WlSurface>,
    barrier: Barrier,
}

impl FrameBarrier {
    fn live(&self) -> bool {
        !self.barrier.is_signaled() && self.surface.upgrade().is_ok()
    }
}

struct Pending {
    frame: FrameBarrier,
    applied_at: Duration,
}

#[derive(Default)]
pub struct State {
    pending: HashMap<ObjectId, Pending>,
}

impl State {
    /// Called only once Smithay has applied the transaction, after all its
    /// FIFO and implicit/explicit synchronization blockers have cleared.
    pub fn committed(&mut self, surface: &WlSurface, now: Duration) {
        let barrier = with_states(surface, |states| {
            states
                .cached_state
                .has::<FifoBarrierCachedState>()
                .then(|| {
                    states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .clone()
                })
                .flatten()
        });
        let Some(barrier) = barrier.filter(|barrier| !barrier.is_signaled()) else {
            self.pending.remove(&surface.id());
            return;
        };
        if self
            .pending
            .get(&surface.id())
            .is_some_and(|pending| pending.frame.barrier == barrier)
        {
            return;
        }
        self.pending.insert(
            surface.id(),
            Pending {
                frame: FrameBarrier {
                    surface: surface.downgrade(),
                    barrier,
                },
                applied_at: now,
            },
        );
    }

    pub fn destroyed(&mut self, surface: &WlSurface) {
        self.pending.remove(&surface.id());
    }

    /// Capture at composition, including for frames retained behind GPU sync.
    /// Render-element visibility includes subsurfaces, popups, layer surfaces,
    /// lock surfaces and live previews without guessing from window focus.
    pub fn snapshot(&self, elements: &RenderElementStates) -> Vec<FrameBarrier> {
        self.pending
            .values()
            .filter_map(|pending| {
                let frame = &pending.frame;
                let surface = frame.surface.upgrade().ok()?;
                (!frame.barrier.is_signaled() && elements.element_was_presented(&surface))
                    .then(|| frame.clone())
            })
            .collect()
    }

    pub fn take_stalled(&mut self, now: Duration) -> Vec<FrameBarrier> {
        let mut ready = Vec::new();
        self.pending.retain(|_, pending| {
            if !pending.frame.live() {
                return false;
            }
            if now.saturating_sub(pending.applied_at) >= STALLED_AFTER {
                ready.push(pending.frame.clone());
                return false;
            }
            true
        });
        ready
    }
}

/// Signal the whole frame before notifying clients. Notifications can apply
/// new commits, so callers must relinquish their FIFO-state borrow first.
pub fn signal(frames: Vec<FrameBarrier>) -> Vec<Client> {
    let mut clients = Vec::new();
    for frame in frames {
        if frame.barrier.is_signaled() {
            continue;
        }
        frame.barrier.signal();
        if let Ok(surface) = frame.surface.upgrade()
            && let Some(client) = surface.client()
            && !clients.contains(&client)
        {
            clients.push(client);
        }
    }
    clients
}
