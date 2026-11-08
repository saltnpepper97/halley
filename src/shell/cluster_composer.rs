use std::cell::Cell;
use std::time::Duration;

use halley_core::field::NodeId;
use smithay::desktop::{Space, Window};
use smithay::utils::{Logical, Point, Rectangle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Opening,
    Interactive,
    Canceling,
    Committing,
    CommitEndpointHeld,
    Revealing,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub id: NodeId,
    pub target: Rectangle<i32, Logical>,
    pub source_stack_index: usize,
    pub source_stack_order: u64,
}

#[derive(Clone, Copy, Debug)]
struct Settle {
    from: f32,
    to: f32,
    started_at: Duration,
    duration: Duration,
}

impl Settle {
    fn progress(self, now: Duration) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let t = (now.saturating_sub(self.started_at).as_secs_f32() / self.duration.as_secs_f32())
            .clamp(0.0, 1.0);
        self.from + (self.to - self.from) * ease_in_out_cubic(t)
    }

    fn animating(self, now: Duration) -> bool {
        now.saturating_sub(self.started_at) < self.duration
    }
}

#[derive(Clone, Debug)]
struct CommitTransition {
    prepared: crate::clusters::PreparedCreation,
    opening_progress: f32,
    started_at: Duration,
    duration: Duration,
}

impl CommitTransition {
    fn progress(&self, now: Duration) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let progress = (now.saturating_sub(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        ease_in_out_cubic(progress)
    }

    fn animating(&self, now: Duration) -> bool {
        now.saturating_sub(self.started_at) < self.duration
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub output: String,
    pub tiles: Vec<Tile>,
    pub hovered: Option<NodeId>,
    pub focused: Option<NodeId>,
    phase: Phase,
    settle: Option<Settle>,
    commit: Option<CommitTransition>,
    endpoint_rendered: Cell<bool>,
}

impl Session {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Progress for the ordinary Field-to-mosaic geometry. Commit rendering
    /// uses `commit_progress` and explicit body rectangles instead.
    pub fn progress(&self, now: Duration) -> f32 {
        match self.phase {
            Phase::Opening | Phase::Canceling | Phase::Revealing => {
                self.settle.map_or(1.0, |settle| settle.progress(now))
            }
            Phase::Interactive | Phase::Committing | Phase::CommitEndpointHeld => 1.0,
        }
    }

    pub fn commit_progress(&self, now: Duration) -> f32 {
        match self.phase {
            Phase::Committing => self
                .commit
                .as_ref()
                .map_or(0.0, |commit| commit.progress(now)),
            Phase::CommitEndpointHeld => 1.0,
            _ => 0.0,
        }
    }

    pub fn commit_opening_progress(&self) -> f32 {
        self.commit
            .as_ref()
            .map_or(1.0, |commit| commit.opening_progress)
    }

    pub fn prepared(&self) -> Option<&crate::clusters::PreparedCreation> {
        self.commit.as_ref().map(|commit| &commit.prepared)
    }

    pub fn naming_alpha(&self, now: Duration) -> f32 {
        match self.phase {
            Phase::Committing => (1.0 - self.commit_progress(now) / 0.28).clamp(0.0, 1.0),
            Phase::CommitEndpointHeld | Phase::Revealing => 0.0,
            _ => 1.0,
        }
    }

    pub fn reveal_alpha(&self, now: Duration) -> f32 {
        if self.phase == Phase::Revealing {
            self.settle.map_or(0.0, |settle| settle.progress(now))
        } else {
            0.0
        }
    }

    pub fn replaces_scene(&self) -> bool {
        self.phase != Phase::Revealing
    }

    pub fn mark_endpoint_rendered(&self) {
        if self.phase == Phase::CommitEndpointHeld {
            self.endpoint_rendered.set(true);
        }
    }

    pub fn tile(&self, id: NodeId) -> Option<&Tile> {
        self.tiles.iter().find(|tile| tile.id == id)
    }

    pub fn hit_test(&self, position: Point<f64, Logical>) -> Option<NodeId> {
        self.tiles
            .iter()
            .find(|tile| tile.target.to_f64().contains(position))
            .map(|tile| tile.id)
    }
}

#[derive(Default)]
pub struct ClusterComposerState {
    session: Option<Session>,
}

impl ClusterComposerState {
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn accepts_input(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| matches!(session.phase, Phase::Opening | Phase::Interactive))
    }

    pub fn target_output(&self) -> Option<&str> {
        self.session.as_ref().map(|session| session.output.as_str())
    }

    pub fn replacement_output(&self) -> Option<&str> {
        self.session
            .as_ref()
            .filter(|session| session.replaces_scene())
            .map(|session| session.output.as_str())
    }

    pub fn reveal_on(&self, output: &str) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.output == output && session.phase == Phase::Revealing)
    }

    pub fn open(
        &mut self,
        space: &Space<Window>,
        nodes: &crate::nodes::NodesState,
        clusters: &crate::clusters::ClusterSystem,
        output: &str,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        if self.session.is_some() {
            return false;
        }
        let Some(bounds) = space
            .outputs()
            .find(|candidate| candidate.name() == output)
            .and_then(|output| space.output_geometry(output))
        else {
            return false;
        };
        let tiles = build_layout(space, nodes, clusters, output, bounds, config);
        if tiles.is_empty() {
            return false;
        }
        let focused = nodes
            .focused_on_output(output)
            .filter(|id| tiles.iter().any(|tile| tile.id == *id))
            .or_else(|| {
                tiles
                    .iter()
                    .min_by_key(|tile| (tile.target.loc.y, tile.target.loc.x, tile.id.as_u64()))
                    .map(|tile| tile.id)
            });
        self.session = Some(Session {
            output: output.to_string(),
            tiles,
            hovered: None,
            focused,
            phase: Phase::Opening,
            settle: Some(Settle {
                from: 0.0,
                to: 1.0,
                started_at: now,
                duration: Duration::from_millis(config.transition_ms),
            }),
            commit: None,
            endpoint_rendered: Cell::new(false),
        });
        true
    }

    pub fn hover(&mut self, position: Point<f64, Logical>) -> bool {
        let Some(session) = self
            .session
            .as_mut()
            .filter(|session| matches!(session.phase, Phase::Opening | Phase::Interactive))
        else {
            return false;
        };
        let hovered = session.hit_test(position);
        let changed = session.hovered != hovered;
        session.hovered = hovered;
        if let Some(id) = hovered {
            session.focused = Some(id);
        }
        changed
    }

    pub fn focused(&self) -> Option<NodeId> {
        self.session.as_ref().and_then(|session| session.focused)
    }

    pub fn move_focus(&mut self, direction: Direction) -> bool {
        let Some(session) = self
            .session
            .as_mut()
            .filter(|session| matches!(session.phase, Phase::Opening | Phase::Interactive))
        else {
            return false;
        };
        let Some(current) = session.focused.and_then(|id| session.tile(id)).cloned() else {
            let next = session
                .tiles
                .iter()
                .min_by_key(|tile| (tile.target.loc.y, tile.target.loc.x, tile.id.as_u64()))
                .map(|tile| tile.id);
            let changed = session.focused != next;
            session.focused = next;
            session.hovered = None;
            return changed;
        };
        let center = rect_center(current.target);
        let next = session
            .tiles
            .iter()
            .filter(|tile| tile.id != current.id)
            .filter_map(|tile| {
                let candidate = rect_center(tile.target);
                let dx = candidate.x - center.x;
                let dy = candidate.y - center.y;
                let eligible = match direction {
                    Direction::Left => dx < 0.0,
                    Direction::Right => dx > 0.0,
                    Direction::Up => dy < 0.0,
                    Direction::Down => dy > 0.0,
                };
                if !eligible {
                    return None;
                }
                let (primary, secondary) = match direction {
                    Direction::Left | Direction::Right => (dx.abs(), dy.abs()),
                    Direction::Up | Direction::Down => (dy.abs(), dx.abs()),
                };
                Some((primary + secondary * 2.0, tile.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        let Some(next) = next else {
            return false;
        };
        session.focused = Some(next);
        session.hovered = None;
        true
    }

    pub fn close(&mut self, config: halley_config::Apogee, now: Duration) -> bool {
        let Some(session) = self
            .session
            .as_mut()
            .filter(|session| matches!(session.phase, Phase::Opening | Phase::Interactive))
        else {
            return false;
        };
        let from = session.progress(now);
        session.phase = Phase::Canceling;
        session.settle = Some(Settle {
            from,
            to: 0.0,
            started_at: now,
            duration: proportional_duration(config.transition_ms, from),
        });
        true
    }

    pub fn begin_commit(
        &mut self,
        prepared: crate::clusters::PreparedCreation,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        let Some(session) = self.session.as_mut().filter(|session| {
            matches!(session.phase, Phase::Opening | Phase::Interactive)
                && session.output == prepared.output
        }) else {
            return false;
        };
        let opening_progress = session.progress(now);
        session.phase = Phase::Committing;
        session.settle = None;
        session.commit = Some(CommitTransition {
            prepared,
            opening_progress,
            started_at: now,
            duration: Duration::from_millis(config.transition_ms),
        });
        session.hovered = None;
        session.endpoint_rendered.set(false);
        true
    }

    pub fn abort_commit(&mut self) -> bool {
        let Some(session) = self.session.as_mut().filter(|session| {
            matches!(session.phase, Phase::Committing | Phase::CommitEndpointHeld)
        }) else {
            return false;
        };
        session.phase = Phase::Interactive;
        session.commit = None;
        session.settle = None;
        session.endpoint_rendered.set(false);
        true
    }

    pub fn begin_reveal(&mut self, config: halley_config::Apogee, now: Duration) -> bool {
        let Some(session) = self
            .session
            .as_mut()
            .filter(|session| session.phase == Phase::CommitEndpointHeld)
        else {
            return false;
        };
        session.phase = Phase::Revealing;
        // Keep the immutable prepared core through the reveal. The ordinary
        // Field already owns the committed cluster underneath, while the
        // retained endpoint lets the renderer keep that same core above the
        // fading veil instead of briefly dimming it out of recognition.
        session.settle = Some(Settle {
            from: 1.0,
            to: 0.0,
            started_at: now,
            duration: reveal_duration(config.transition_ms),
        });
        true
    }

    pub fn prune(
        &mut self,
        nodes: &crate::nodes::NodesState,
        clusters: &crate::clusters::ClusterSystem,
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let before = session.tiles.len();
        session.tiles.retain(|tile| {
            nodes.record(tile.id).is_some_and(|record| {
                is_candidate(
                    record.attached,
                    &record.output,
                    &session.output,
                    clusters.is_member(tile.id),
                )
            })
        });
        if session.focused.is_some_and(|id| session.tile(id).is_none()) {
            session.focused = session
                .tiles
                .iter()
                .min_by_key(|tile| (tile.target.loc.y, tile.target.loc.x, tile.id.as_u64()))
                .map(|tile| tile.id);
        }
        if session.hovered.is_some_and(|id| session.tile(id).is_none()) {
            session.hovered = None;
        }
        before != session.tiles.len()
    }

    pub fn tick(&mut self, now: Duration) -> Tick {
        let Some(session) = self.session.as_mut() else {
            return Tick::Idle;
        };
        match session.phase {
            Phase::Opening => {
                let animating = session.settle.is_some_and(|settle| settle.animating(now));
                if !animating {
                    session.phase = Phase::Interactive;
                    session.settle = None;
                }
                Tick::Active { animating }
            }
            Phase::Interactive => Tick::Active { animating: false },
            Phase::Canceling => {
                let animating = session.settle.is_some_and(|settle| settle.animating(now));
                if animating {
                    Tick::Active { animating: true }
                } else {
                    self.session = None;
                    Tick::Cancelled
                }
            }
            Phase::Committing => {
                let animating = session
                    .commit
                    .as_ref()
                    .is_some_and(|commit| commit.animating(now));
                if animating {
                    Tick::Active { animating: true }
                } else {
                    session.phase = Phase::CommitEndpointHeld;
                    session.endpoint_rendered.set(false);
                    Tick::Active { animating: true }
                }
            }
            Phase::CommitEndpointHeld => {
                if session.endpoint_rendered.get() {
                    Tick::CommitReady
                } else {
                    Tick::Active { animating: true }
                }
            }
            Phase::Revealing => {
                let animating = session.settle.is_some_and(|settle| settle.animating(now));
                if animating {
                    Tick::Active { animating: true }
                } else {
                    self.session = None;
                    Tick::Finished
                }
            }
        }
    }
}

pub enum Tick {
    Idle,
    Active { animating: bool },
    CommitReady,
    Cancelled,
    Finished,
}

pub fn tick_session<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    now: Duration,
) -> bool {
    let changed = session
        .shell
        .cluster_composer
        .prune(&session.nodes, &session.clusters);
    let committing = session
        .shell
        .cluster_composer
        .session()
        .is_some_and(|composer| {
            matches!(
                composer.phase(),
                Phase::Committing | Phase::CommitEndpointHeld
            )
        });
    if committing && session.clusters.prepared_creation().is_none() {
        let output = session
            .shell
            .cluster_composer
            .target_output()
            .map(str::to_string);
        session.shell.cluster_composer.abort_commit();
        if let Some(output) = output {
            session.shell.overlays.show_error(
                output,
                "Cluster changed\\nReview the remaining selection",
                3_000,
                now,
            );
        }
        session.request_redraw();
        return false;
    }
    let empty = session
        .shell
        .cluster_composer
        .session()
        .is_some_and(|composer| composer.tiles.is_empty());
    if empty && session.shell.cluster_composer.accepts_input() {
        session
            .shell
            .cluster_composer
            .close(session.settings.apogee, now);
    }
    match session.shell.cluster_composer.tick(now) {
        Tick::Idle => {
            if changed {
                session.request_redraw();
            }
            false
        }
        Tick::Active { animating } => {
            if changed || animating {
                session.request_redraw();
            }
            animating
        }
        Tick::CommitReady => {
            if crate::session::input::finish_cluster_creation(session) {
                session
                    .shell
                    .cluster_composer
                    .begin_reveal(session.settings.apogee, now);
            } else {
                session.shell.cluster_composer.abort_commit();
                session.clusters.abort_prepared_creation();
            }
            session.request_redraw();
            true
        }
        Tick::Cancelled => {
            session.clusters.cancel_creation();
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Modal, None);
            crate::session::note_pointer_activity(session);
            crate::session::reconcile_pointer_constraints(session);
            session.request_redraw();
            false
        }
        Tick::Finished => {
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Modal, None);
            crate::session::note_pointer_activity(session);
            crate::session::reconcile_pointer_constraints(session);
            session.request_redraw();
            false
        }
    }
}

fn build_layout(
    space: &Space<Window>,
    nodes: &crate::nodes::NodesState,
    clusters: &crate::clusters::ClusterSystem,
    output: &str,
    bounds: Rectangle<i32, Logical>,
    config: halley_config::Apogee,
) -> Vec<Tile> {
    let mut entries = nodes
        .records()
        .filter(|record| {
            is_candidate(
                record.attached,
                &record.output,
                output,
                clusters.is_member(record.id),
            )
        })
        .filter_map(|record| {
            let node = nodes.field().node(record.id)?;
            let source_stack_index = record.collapsed_stack_index.or_else(|| {
                space
                    .elements()
                    .position(|candidate| candidate == &record.window)
            });
            let width = record.geometry.size.w.max(1) as f32;
            let height = record.geometry.size.h.max(1) as f32;
            Some((
                record.id,
                source_stack_index.unwrap_or(usize::MAX),
                if record.collapsed { 0 } else { u64::MAX },
                super::mosaic::Item {
                    x: node.pos.x,
                    y: node.pos.y,
                    aspect: (width / height).clamp(0.25, 4.5),
                    stable_key: record.id.as_u64(),
                    weight: width * height,
                },
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|(id, _, _, _)| id.as_u64());

    let upper_band = (bounds.size.h.max(1) as f32 * 0.215).clamp(140.0, 236.0);
    let mosaic_height = (bounds.size.h as f32 - upper_band).round().max(64.0) as i32;
    let slots = super::mosaic::mosaic(
        &entries
            .iter()
            .map(|(_, _, _, item)| *item)
            .collect::<Vec<_>>(),
        bounds.size.w,
        mosaic_height,
        config.gap.max(0.0),
        config.max_rows.clamp(1, 5) as usize,
    );
    entries
        .into_iter()
        .zip(slots)
        .map(
            |((id, source_stack_index, source_stack_order, _), slot)| Tile {
                id,
                target: Rectangle::new(
                    (
                        bounds.loc.x + (slot.cx - slot.w * 0.5).round() as i32,
                        bounds.loc.y
                            + upper_band.round() as i32
                            + (slot.cy - slot.h * 0.5).round() as i32,
                    )
                        .into(),
                    (
                        slot.w.round().max(1.0) as i32,
                        slot.h.round().max(1.0) as i32,
                    )
                        .into(),
                ),
                source_stack_index,
                source_stack_order,
            },
        )
        .collect()
}

fn is_candidate(attached: bool, record_output: &str, target_output: &str, is_member: bool) -> bool {
    attached && record_output == target_output && !is_member
}

fn proportional_duration(base_ms: u64, distance: f32) -> Duration {
    Duration::from_millis((base_ms as f32 * distance.clamp(0.0, 1.0)).round() as u64)
}

fn reveal_duration(base_ms: u64) -> Duration {
    Duration::from_millis((base_ms / 3).clamp(1, 120))
}

fn rect_center(rect: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    (
        rect.loc.x as f64 + rect.size.w as f64 * 0.5,
        rect.loc.y as f64 + rect.size.h as f64 * 0.5,
    )
        .into()
}

fn ease_in_out_cubic(value: f32) -> f32 {
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) * 0.5
    }
}

pub fn send_preview_frames(
    state: &ClusterComposerState,
    nodes: &crate::nodes::NodesState,
    output: &smithay::output::Output,
    elapsed: Duration,
    sequence: u32,
) {
    let Some(session) = state
        .session()
        .filter(|session| session.output == output.name())
    else {
        return;
    };
    for tile in &session.tiles {
        let Some(record) = nodes
            .record(tile.id)
            .filter(|record| record.attached && !record.collapsed)
        else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(id: u64, x: i32, y: i32) -> Tile {
        Tile {
            id: NodeId::new(id),
            target: Rectangle::new((x, y).into(), (100, 100).into()),
            source_stack_index: id as usize,
            source_stack_order: u64::MAX,
        }
    }

    #[test]
    fn candidate_filter_is_output_local_and_keeps_unclustered_collapsed_records() {
        assert!(super::is_candidate(true, "DP-1", "DP-1", false));
        assert!(!super::is_candidate(false, "DP-1", "DP-1", false));
        assert!(!super::is_candidate(true, "DP-2", "DP-1", false));
        assert!(!super::is_candidate(true, "DP-1", "DP-1", true));
    }

    fn interactive_session(tiles: Vec<Tile>) -> Session {
        Session {
            output: "DP-1".into(),
            tiles,
            hovered: None,
            focused: Some(NodeId::new(1)),
            phase: Phase::Interactive,
            settle: None,
            commit: None,
            endpoint_rendered: Cell::new(false),
        }
    }

    fn prepared(members: Vec<NodeId>) -> crate::clusters::PreparedCreation {
        crate::clusters::PreparedCreation {
            output: "DP-1".into(),
            members,
            name: "Work".into(),
            core_position: halley_core::field::Vec2 { x: 40.0, y: 60.0 },
            focus_core: true,
        }
    }

    #[test]
    fn directional_navigation_uses_the_stable_mosaic_geometry() {
        let mut state = ClusterComposerState {
            session: Some(interactive_session(vec![
                tile(1, 0, 0),
                tile(2, 200, 0),
                tile(3, 0, 200),
            ])),
        };
        assert!(state.move_focus(Direction::Right));
        assert_eq!(state.focused(), Some(NodeId::new(2)));
        assert!(state.move_focus(Direction::Down));
        assert_eq!(state.focused(), Some(NodeId::new(3)));
    }

    #[test]
    fn cancel_still_defers_until_the_reverse_endpoint() {
        let mut state = ClusterComposerState {
            session: Some(interactive_session(vec![tile(1, 0, 0)])),
        };
        let config = halley_config::Apogee {
            transition_ms: 300,
            ..halley_config::Apogee::default()
        };
        assert!(state.close(config, Duration::ZERO));
        assert!(!state.accepts_input());
        assert!(matches!(
            state.tick(Duration::from_millis(299)),
            Tick::Active { animating: true }
        ));
        assert!(matches!(
            state.tick(Duration::from_millis(300)),
            Tick::Cancelled
        ));
    }

    #[test]
    fn early_confirmation_starts_from_the_current_opening_geometry() {
        let id = NodeId::new(1);
        let mut opening = interactive_session(vec![tile(1, 0, 0)]);
        opening.phase = Phase::Opening;
        opening.settle = Some(Settle {
            from: 0.0,
            to: 1.0,
            started_at: Duration::ZERO,
            duration: Duration::from_millis(300),
        });
        let mut state = ClusterComposerState {
            session: Some(opening),
        };
        let config = halley_config::Apogee {
            transition_ms: 300,
            ..halley_config::Apogee::default()
        };
        assert!(state.begin_commit(prepared(vec![id]), config, Duration::from_millis(150)));
        assert_eq!(state.session().unwrap().commit_opening_progress(), 0.5);
    }

    #[test]
    fn commit_endpoint_must_be_rendered_before_commit_is_emitted() {
        let id = NodeId::new(1);
        let mut state = ClusterComposerState {
            session: Some(interactive_session(vec![tile(1, 0, 0)])),
        };
        let config = halley_config::Apogee {
            transition_ms: 300,
            ..halley_config::Apogee::default()
        };
        assert!(state.begin_commit(prepared(vec![id]), config, Duration::ZERO));
        assert!(matches!(
            state.tick(Duration::from_millis(300)),
            Tick::Active { animating: true }
        ));
        assert_eq!(
            state.session().expect("held session").phase(),
            Phase::CommitEndpointHeld
        );
        assert!(matches!(
            state.tick(Duration::from_millis(301)),
            Tick::Active { animating: true }
        ));
        state.session().unwrap().mark_endpoint_rendered();
        assert!(matches!(
            state.tick(Duration::from_millis(302)),
            Tick::CommitReady
        ));
    }

    #[test]
    fn zero_duration_commit_still_holds_a_rendered_endpoint() {
        let id = NodeId::new(1);
        let mut state = ClusterComposerState {
            session: Some(interactive_session(vec![tile(1, 0, 0)])),
        };
        let config = halley_config::Apogee {
            transition_ms: 0,
            ..halley_config::Apogee::default()
        };
        assert!(state.begin_commit(prepared(vec![id]), config, Duration::ZERO));
        assert!(matches!(
            state.tick(Duration::ZERO),
            Tick::Active { animating: true }
        ));
        assert!(!matches!(state.tick(Duration::ZERO), Tick::CommitReady));
        state.session().unwrap().mark_endpoint_rendered();
        assert!(matches!(state.tick(Duration::ZERO), Tick::CommitReady));
    }

    #[test]
    fn reveal_retains_the_prepared_core_until_the_handoff_finishes() {
        let id = NodeId::new(1);
        let expected = prepared(vec![id]);
        let mut state = ClusterComposerState {
            session: Some(interactive_session(vec![tile(1, 0, 0)])),
        };
        let config = halley_config::Apogee {
            transition_ms: 300,
            ..halley_config::Apogee::default()
        };
        assert!(state.begin_commit(expected.clone(), config, Duration::ZERO));
        assert!(matches!(
            state.tick(Duration::from_millis(300)),
            Tick::Active { animating: true }
        ));
        state.session().unwrap().mark_endpoint_rendered();
        assert!(matches!(
            state.tick(Duration::from_millis(301)),
            Tick::CommitReady
        ));
        assert!(state.begin_reveal(config, Duration::from_millis(301)));
        assert_eq!(state.session().and_then(Session::prepared), Some(&expected));
        assert!(matches!(
            state.tick(Duration::from_millis(400)),
            Tick::Active { animating: true }
        ));
        assert!(matches!(
            state.tick(Duration::from_millis(401)),
            Tick::Finished
        ));
        assert!(state.session().is_none());
    }
}
