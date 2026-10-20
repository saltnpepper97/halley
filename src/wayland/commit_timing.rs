//! Absolute commit eligibility in the presentation clock's monotonic domain.
//! Use Smithay's unmanaged timestamp dispatcher to retain the protocol's full
//! unsigned seconds range; its managed heap orders seconds as signed values.
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource, Weak, backend::ObjectId};
use smithay::utils::{Monotonic, Time};
use smithay::wayland::commit_timing::CommitTimerStateUserData;
use smithay::wayland::compositor::{Barrier, add_blocker, with_states};

struct Pending {
    client: Client,
    surface: Weak<WlSurface>,
    deadlines: BTreeMap<Duration, Vec<Barrier>>,
}

#[derive(Default)]
pub struct State {
    pending: HashMap<ObjectId, Pending>,
}

impl State {
    /// Run before the surface transaction is committed, independently of its
    /// FIFO and acquire blockers. A past timestamp needs no additional blocker.
    pub fn prepare(&mut self, surface: &WlSurface, now: Duration) -> bool {
        let timestamp = with_states(surface, |states| {
            states
                .data_map
                .get::<CommitTimerStateUserData>()
                .and_then(|data| data.borrow_mut().timestamp.take())
        });
        let Some(timestamp) = timestamp else {
            return false;
        };
        // The pinned dispatcher stores unsigned seconds in a signed Timespec.
        // Recover the original bits without the nonnegative Time -> Duration
        // conversion, which would assert or misorder very distant timestamps.
        let time: Time<Monotonic> = timestamp.into();
        let raw: smithay::reexports::rustix::time::Timespec = time.into();
        let deadline = Duration::new(raw.tv_sec as u64, raw.tv_nsec as u32);
        if deadline <= now {
            return false;
        }
        let Some(client) = surface.client() else {
            return false;
        };
        let barrier = Barrier::new(false);
        add_blocker(surface, barrier.clone());
        self.pending
            .entry(surface.id())
            .or_insert_with(|| Pending {
                client,
                surface: surface.downgrade(),
                deadlines: BTreeMap::new(),
            })
            .deadlines
            .entry(deadline)
            .or_default()
            .push(barrier);
        true
    }

    /// A destroyed synchronized child can share a transaction with a live
    /// parent. Release only the dead child's constraints, then let the caller
    /// notify its client after Smithay finishes cleaning up the surface tree.
    pub fn destroyed(&mut self, surface: &WlSurface) -> Option<Client> {
        let pending = self.pending.remove(&surface.id())?;
        for barriers in pending.deadlines.into_values() {
            for barrier in barriers {
                barrier.signal();
            }
        }
        Some(pending.client)
    }

    pub fn next_deadline(&mut self) -> Option<Duration> {
        self.pending
            .retain(|_, pending| pending.surface.upgrade().is_ok());
        self.pending
            .values()
            .filter_map(|pending| pending.deadlines.first_key_value().map(|(time, _)| *time))
            .min()
    }

    /// Signal every elapsed timing barrier before clients apply transactions.
    /// A frame target prediction never makes future content eligible early.
    pub fn release_due(&mut self, now: Duration) -> Vec<Client> {
        let mut clients = Vec::new();
        self.pending.retain(|_, pending| {
            let Ok(surface) = pending.surface.upgrade() else {
                return false;
            };
            let mut released = false;
            while pending
                .deadlines
                .first_key_value()
                .is_some_and(|(time, _)| *time <= now)
            {
                let (_, barriers) = pending.deadlines.pop_first().unwrap();
                for barrier in barriers {
                    barrier.signal();
                }
                released = true;
            }
            if released
                && let Some(client) = surface.client()
                && !clients.contains(&client)
            {
                clients.push(client);
            }
            !pending.deadlines.is_empty()
        });
        clients
    }

    /// An unavailable event source must fail affected clients instead of
    /// releasing future content early or leaving live clients blocked forever.
    pub fn drain_surfaces(&mut self) -> Vec<WlSurface> {
        self.pending
            .drain()
            .filter_map(|(_, pending)| pending.surface.upgrade().ok())
            .collect()
    }
}
