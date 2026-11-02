//! Persistent, output-owned hints with one keyboard deletion target. Only a
//! released, freshly pressed shortcut can confirm; changing or blocking the
//! keyboard target always forgets the pending confirmation.
use std::collections::BTreeMap;
use std::time::Duration;

use halley_core::cluster::ClusterId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub id: ClusterId,
    pub output: String,
    pub name: String,
    pub shortcut: String,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub name: String,
    pub shortcut: String,
    pub armed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    Ignored,
    Armed,
    Delete(ClusterId),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Prompt {
    hints: BTreeMap<String, Target>,
    target: Option<Target>,
    input_ready: bool,
    armed: bool,
    held: Option<u32>,
    quiet_until: BTreeMap<String, Duration>,
}

impl Prompt {
    /// Reconcile all displayed hints independently of the selected monitor's
    /// keyboard target. Ordinary focus changes must not erase other outputs.
    pub fn sync_outputs(
        &mut self,
        hints: Vec<Target>,
        target: Option<Target>,
        input_blocked: bool,
        now: Duration,
    ) -> bool {
        self.quiet_until.retain(|_, deadline| now < *deadline);
        let hints = hints
            .into_iter()
            .filter(|target| !self.quiet_until.contains_key(&target.output))
            .map(|target| (target.output.clone(), target))
            .collect();
        let changed = self.hints != hints;
        self.hints = hints;
        self.sync(target, input_blocked, now) | changed
    }

    fn sync(&mut self, target: Option<Target>, blocked: bool, now: Duration) -> bool {
        let changed_target = self.target != target;
        if changed_target {
            self.target = target;
            self.armed = false;
        }
        let input_ready = !blocked
            && self.target.as_ref().is_some_and(|target| {
                self.quiet_until
                    .get(&target.output)
                    .is_none_or(|deadline| now >= *deadline)
            });
        let changed =
            changed_target || self.input_ready != input_ready || (!input_ready && self.armed);
        self.input_ready = input_ready;
        if !input_ready {
            self.armed = false;
        }
        changed
    }

    pub fn snapshot(&self, output: &str) -> Option<Snapshot> {
        self.hints
            .get(output)
            .or_else(|| {
                self.target
                    .as_ref()
                    .filter(|target| self.input_ready && target.output == output)
            })
            .map(|target| Snapshot {
                name: target.name.clone(),
                shortcut: target.shortcut.clone(),
                armed: self.input_ready && self.armed && self.target.as_ref() == Some(target),
            })
    }

    pub fn accepts_input(&self) -> bool {
        self.input_ready
    }

    pub fn owns_output(&self, output: &str) -> bool {
        self.hints.contains_key(output)
            || self
                .target
                .as_ref()
                .is_some_and(|target| target.output == output)
    }

    pub fn cancel(&mut self) -> bool {
        std::mem::take(&mut self.armed)
    }

    pub fn suspend(&mut self, now: Duration) {
        self.armed = false;
        self.input_ready = false;
        // Cover the asynchronous gap before a spawned launcher maps or takes
        // focus. Failed launches recover automatically without arming deletion.
        // Launching on another monitor must leave persistent hints alone.
        if let Some(target) = &self.target {
            self.hints.remove(&target.output);
            self.quiet_until
                .insert(target.output.clone(), now + Duration::from_millis(750));
        }
    }

    pub fn release(&mut self, keycode: u32) {
        if self.held == Some(keycode) {
            self.held = None;
        }
    }

    pub fn press(&mut self, keycode: u32) -> Press {
        if !self.input_ready || self.held.is_some() {
            return Press::Ignored;
        }
        self.held = Some(keycode);
        if self.armed {
            self.armed = false;
            self.input_ready = false;
            let target = self.target.as_ref().expect("keyboard target");
            self.hints.remove(&target.output);
            Press::Delete(target.id)
        } else {
            self.armed = true;
            Press::Armed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(id: u64, output: &str) -> Target {
        Target {
            id: ClusterId::new(id),
            output: output.into(),
            name: "Work".into(),
            shortcut: "Super+Q".into(),
        }
    }
    #[test]
    fn requires_release_and_two_presses_and_is_output_owned() {
        let mut prompt = Prompt::default();
        prompt.sync(Some(target(1, "DP-1")), false, Duration::ZERO);
        assert!(prompt.snapshot("DP-2").is_none());
        assert_eq!(prompt.press(24), Press::Armed);
        assert_eq!(prompt.press(24), Press::Ignored);
        prompt.release(25);
        assert_eq!(prompt.press(24), Press::Ignored);
        prompt.release(24);
        assert_eq!(prompt.press(24), Press::Delete(ClusterId::new(1)));
    }
    #[test]
    fn overlay_and_launcher_cancel_confirmation_then_restore_unarmed_hint() {
        let mut prompt = Prompt::default();
        let t = Some(target(1, "DP-1"));
        prompt.sync(t.clone(), false, Duration::ZERO);
        prompt.press(24);
        prompt.release(24);
        prompt.sync(t.clone(), true, Duration::ZERO);
        assert!(!prompt.accepts_input());
        prompt.sync(t.clone(), false, Duration::ZERO);
        assert!(!prompt.snapshot("DP-1").unwrap().armed);
        prompt.press(24);
        prompt.release(24);
        prompt.suspend(Duration::ZERO);
        prompt.sync(t.clone(), false, Duration::from_millis(500));
        assert!(!prompt.accepts_input());
        prompt.sync(t, false, Duration::from_secs(1));
        assert!(!prompt.snapshot("DP-1").unwrap().armed);
    }
    #[test]
    fn new_window_leaving_switching_and_escape_forget_confirmation() {
        let mut prompt = Prompt::default();
        prompt.sync(Some(target(1, "DP-1")), false, Duration::ZERO);
        prompt.press(24);
        prompt.release(24);
        assert!(prompt.cancel());
        assert_eq!(prompt.press(24), Press::Armed);
        prompt.release(24);
        prompt.sync(None, false, Duration::ZERO);
        assert!(!prompt.accepts_input());
        prompt.sync(Some(target(2, "DP-2")), false, Duration::ZERO);
        assert_eq!(prompt.press(24), Press::Armed);
    }
}
