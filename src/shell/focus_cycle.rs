use std::time::Duration;

use halley_config::FocusCycleDirection;
use halley_core::field::NodeId;

pub const OPEN_MS: u64 = 140;
pub const STEP_MS: u64 = 130;
pub const CLOSE_MS: u64 = 120;
pub const VISIBLE_RADIUS: i32 = 2;

#[derive(Clone, Debug)]
pub struct Session {
    pub candidates: Vec<NodeId>,
    pub preview_index: usize,
    pub opened_at: Duration,
    pub step_from_visual_index: f32,
    pub step_to_visual_index: f32,
    pub step_started_at: Duration,
    pub closing_started_at: Option<Duration>,
    pub origin_focus: Option<NodeId>,
}

impl Session {
    pub fn preview(&self) -> Option<NodeId> {
        self.candidates.get(self.preview_index).copied()
    }

    pub fn visible_slots(&self, radius: i32) -> Vec<(i32, NodeId)> {
        let len = self.candidates.len() as i32;
        if len == 0 {
            return Vec::new();
        }
        let visible_count = len.min(radius * 2 + 1).max(1);
        let left_count = (visible_count - 1) / 2;
        let right_count = visible_count - 1 - left_count;
        (-left_count..=right_count)
            .map(|offset| {
                let index = (self.preview_index as i32 + offset).rem_euclid(len) as usize;
                (offset, self.candidates[index])
            })
            .collect()
    }

    pub fn open_progress(&self, now: Duration) -> f32 {
        let t = now.saturating_sub(self.opened_at).as_secs_f32() * 1_000.0 / OPEN_MS as f32;
        1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
    }

    pub fn close_progress(&self, now: Duration) -> f32 {
        self.closing_started_at
            .map(|started| {
                ease_in_out_cubic(
                    (now.saturating_sub(started).as_secs_f32() * 1_000.0 / CLOSE_MS as f32)
                        .clamp(0.0, 1.0),
                )
            })
            .unwrap_or(0.0)
    }

    pub fn visual_index(&self, now: Duration) -> f32 {
        let t = ease_in_out_cubic(
            (now.saturating_sub(self.step_started_at).as_secs_f32() * 1_000.0 / STEP_MS as f32)
                .clamp(0.0, 1.0),
        );
        if t >= 1.0 {
            self.preview_index as f32
        } else {
            self.step_from_visual_index
                + (self.step_to_visual_index - self.step_from_visual_index) * t
        }
    }

    pub fn visual_offset(&self, candidate_index: usize, now: Duration) -> f32 {
        let visual_index = self.visual_index(now);
        let len = self.candidates.len() as f32;
        let mut index = candidate_index as f32;
        while index - visual_index > len * 0.5 {
            index -= len;
        }
        while index - visual_index < -len * 0.5 {
            index += len;
        }
        index - visual_index
    }
}

#[derive(Default)]
pub struct FocusCycleState {
    session: Option<Session>,
}

impl FocusCycleState {
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn is_open(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.closing_started_at.is_none())
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn accepts_live_previews(&self) -> bool {
        self.is_open()
    }

    pub fn contains_preview(&self, id: NodeId) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .visible_slots(VISIBLE_RADIUS)
                .iter()
                .any(|(_, preview)| *preview == id)
        })
    }

    pub fn start_or_step(
        &mut self,
        nodes: &crate::nodes::NodesState,
        direction: FocusCycleDirection,
        now: Duration,
    ) -> bool {
        if self.session.is_none() {
            let candidates = halley_core::focus::focus_cycle_candidates(
                &nodes.field,
                nodes.last_focus_ms(),
                nodes.focused(),
            )
            .into_iter()
            .filter(|id| nodes.record(*id).is_some_and(|record| record.attached))
            .collect::<Vec<_>>();
            if candidates.len() < 2 {
                return false;
            }
            self.session = Some(Session {
                candidates,
                preview_index: 0,
                opened_at: now,
                step_from_visual_index: 0.0,
                step_to_visual_index: 0.0,
                step_started_at: now,
                closing_started_at: None,
                origin_focus: nodes.focused(),
            });
        }
        self.refresh(nodes, now);
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if session.closing_started_at.is_some() || session.candidates.len() < 2 {
            return false;
        }
        let len = session.candidates.len();
        let from = session.preview_index;
        let to = match direction {
            FocusCycleDirection::Forward => (from + 1) % len,
            FocusCycleDirection::Backward => (from + len - 1) % len,
        };
        session.preview_index = to;
        session.step_from_visual_index = from as f32;
        session.step_to_visual_index = match direction {
            FocusCycleDirection::Forward if from + 1 == len => len as f32,
            FocusCycleDirection::Backward if from == 0 => -1.0,
            _ => to as f32,
        };
        session.step_started_at = now;
        true
    }

    pub fn cancel(&mut self, now: Duration) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if session.closing_started_at.is_some() {
            return false;
        }
        session.closing_started_at = Some(now);
        true
    }

    pub fn commit(&mut self, now: Duration) -> Option<NodeId> {
        let session = self.session.as_mut()?;
        if session.closing_started_at.is_some() {
            return None;
        }
        session.closing_started_at = Some(now);
        session.preview().or(session.origin_focus)
    }

    pub fn tick(&mut self, now: Duration) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        if session
            .closing_started_at
            .is_some_and(|started| now.saturating_sub(started) >= Duration::from_millis(CLOSE_MS))
        {
            self.session = None;
            return false;
        }
        session.open_progress(now) < 1.0
            || session.close_progress(now) > 0.0
            || now.saturating_sub(session.step_started_at) < Duration::from_millis(STEP_MS)
    }

    fn refresh(&mut self, nodes: &crate::nodes::NodesState, now: Duration) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let preview = session.preview();
        session.candidates.retain(|id| {
            halley_core::focus::is_focus_cycle_candidate(&nodes.field, *id)
                && nodes.record(*id).is_some_and(|record| record.attached)
        });
        if session.candidates.is_empty() {
            self.session = None;
            return;
        }
        session.preview_index = preview
            .and_then(|id| {
                session
                    .candidates
                    .iter()
                    .position(|candidate| *candidate == id)
            })
            .unwrap_or_else(|| session.preview_index.min(session.candidates.len() - 1));
        session.step_from_visual_index = session.preview_index as f32;
        session.step_to_visual_index = session.preview_index as f32;
        session.step_started_at = now;
    }
}

fn ease_in_out_cubic(value: f32) -> f32 {
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) * 0.5
    }
}

pub fn start_or_step<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: FocusCycleDirection,
) -> bool {
    let changed = session.shell.focus_cycle.start_or_step(
        &session.nodes,
        direction,
        crate::frame_clock::monotonic_now(),
    );
    if changed {
        session.request_redraw();
    }
    changed
}

pub fn send_preview_frames(
    state: &FocusCycleState,
    nodes: &crate::nodes::NodesState,
    output: &smithay::output::Output,
    elapsed: Duration,
    sequence: u32,
) {
    let Some(session) = state.session() else {
        return;
    };
    for (_, id) in session.visible_slots(VISIBLE_RADIUS) {
        let Some(record) = nodes.record(id).filter(|record| {
            record.attached && !record.collapsed && record.output == output.name()
        }) else {
            continue;
        };
        record.window.send_frame(
            output,
            elapsed,
            crate::wayland::frame_callbacks::FALLBACK_THROTTLE,
            |surface, states| {
                crate::wayland::frame_callbacks::callback_output(
                    surface, states, output, sequence, false,
                )
            },
        );
    }
}

pub fn cancel<D: crate::session::SessionDriver>(session: &mut crate::session::Session<D>) -> bool {
    let changed = session
        .shell
        .focus_cycle
        .cancel(crate::frame_clock::monotonic_now());
    if changed {
        session.request_redraw();
    }
    changed
}

pub fn commit<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    serial: smithay::utils::Serial,
) -> bool {
    let Some(target) = session
        .shell
        .focus_cycle
        .commit(crate::frame_clock::monotonic_now())
    else {
        return false;
    };
    // The shared focus path commits keyboard/X focus before releasing the old
    // pointer constraint, then centers the pointer on the selected output.
    let _ = crate::nodes::focus_cycle_node(session, target, serial);
    session.request_redraw();
    true
}

#[cfg(test)]
mod tests {
    use super::{FocusCycleState, Session};
    use halley_core::field::NodeId;
    use std::time::Duration;

    #[test]
    fn visible_slots_do_not_duplicate_small_candidate_sets() {
        let session = Session {
            candidates: vec![NodeId::new(1), NodeId::new(2)],
            preview_index: 1,
            opened_at: Duration::ZERO,
            step_from_visual_index: 1.0,
            step_to_visual_index: 1.0,
            step_started_at: Duration::ZERO,
            closing_started_at: None,
            origin_focus: Some(NodeId::new(1)),
        };
        let slots = session.visible_slots(2);
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().any(|(_, id)| *id == NodeId::new(1)));
        assert!(slots.iter().any(|(_, id)| *id == NodeId::new(2)));
    }

    #[test]
    fn all_visible_cards_are_live_preview_members() {
        let state = FocusCycleState {
            session: Some(Session {
                candidates: (1..=7).map(NodeId::new).collect(),
                preview_index: 3,
                opened_at: Duration::ZERO,
                step_from_visual_index: 3.0,
                step_to_visual_index: 3.0,
                step_started_at: Duration::ZERO,
                closing_started_at: None,
                origin_focus: Some(NodeId::new(1)),
            }),
        };

        assert!(state.contains_preview(NodeId::new(4)));
        assert!(state.contains_preview(NodeId::new(2)));
        assert!(state.contains_preview(NodeId::new(6)));
        assert!(!state.contains_preview(NodeId::new(1)));
        assert!(!state.contains_preview(NodeId::new(7)));
    }
}
