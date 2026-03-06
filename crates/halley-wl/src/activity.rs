use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    Inactive,
    Fading,
    Active,
}

#[derive(Debug)]
pub struct CommitActivity {
    last_commit: Instant,
    window_start: Instant,
    commits: u32,

    state: VisualState,
    state_since: Instant,
}

impl CommitActivity {
    pub fn new(now: Instant) -> Self {
        Self {
            last_commit: now,
            window_start: now,
            commits: 0,
            state: VisualState::Inactive,
            state_since: now,
        }
    }

    pub fn on_commit(&mut self, now: Instant) {
        self.last_commit = now;

        // reset window roughly every 1s
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.commits = 0;
        }
        self.commits = self.commits.saturating_add(1);
    }

    pub fn tick(&mut self, now: Instant, visible: bool) -> Option<(VisualState, f32)> {
        let age = now
            .duration_since(self.window_start)
            .as_secs_f32()
            .max(0.001);
        let cps = (self.commits as f32) / age;

        // v0 thresholds (tweak later)
        let want_active = visible && cps >= 10.0;

        let next = match (self.state, want_active) {
            (_, true) => VisualState::Active,
            (VisualState::Active, false) => VisualState::Fading,
            (VisualState::Fading, false) => {
                if now.duration_since(self.state_since) >= Duration::from_millis(1500) {
                    VisualState::Inactive
                } else {
                    VisualState::Fading
                }
            }
            (VisualState::Inactive, false) => VisualState::Inactive,
        };

        if next != self.state {
            self.state = next;
            self.state_since = now;
            Some((next, cps))
        } else {
            None
        }
    }

    pub fn state(&self) -> VisualState {
        self.state
    }

    pub fn last_commit_at(&self) -> Instant {
        self.last_commit
    }
}
