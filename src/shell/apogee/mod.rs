use std::collections::HashMap;
use std::time::Duration;

use halley_core::cluster::ClusterId;
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

#[derive(Clone, Debug)]
pub struct Tile {
    pub id: NodeId,
    pub kind: TileKind,
    pub output: String,
    pub target: Rectangle<i32, Logical>,
    pub source_stack_index: usize,
    pub source_stack_order: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Window,
    ClusterCore(ClusterId),
}

#[derive(Clone, Copy, Debug)]
struct Settle {
    from: f32,
    to: f32,
    started_at: Duration,
    duration: Duration,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub tiles: Vec<Tile>,
    pub hovered: Option<NodeId>,
    pub selected: Option<NodeId>,
    settle: Option<Settle>,
    manual_progress: Option<f32>,
    pending_activation: Option<NodeId>,
}

impl Session {
    pub fn progress(&self, now: Duration) -> f32 {
        if let Some(progress) = self.manual_progress {
            return progress.clamp(0.0, 1.0);
        }
        let Some(settle) = self.settle else {
            return 1.0;
        };
        if settle.duration.is_zero() {
            return settle.to;
        }
        let t = (now.saturating_sub(settle.started_at).as_secs_f32()
            / settle.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        let eased = ease_in_out_cubic(t);
        settle.from + (settle.to - settle.from) * eased
    }

    pub fn is_closing(&self) -> bool {
        self.settle.is_some_and(|settle| settle.to == 0.0)
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
pub struct ApogeeState {
    session: Option<Session>,
}

impl ApogeeState {
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// The overlay remains visible while it flies closed, but it must stop
    /// behaving like a modal input target as soon as that close begins.
    pub fn accepts_input(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| !session.is_closing())
    }

    pub fn accepts_live_previews(&self) -> bool {
        self.accepts_input()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.tiles.iter().any(|tile| tile.id == id))
    }

    pub fn hovered(&self) -> Option<NodeId> {
        self.session.as_ref().and_then(|session| session.hovered)
    }

    pub fn open(
        &mut self,
        space: &Space<Window>,
        nodes: &crate::nodes::NodesState,
        clusters: &crate::clusters::ClusterSystem,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        if !config.enabled || self.session.is_some() {
            return false;
        }
        let tiles = build_layout(space, nodes, clusters, config);
        if tiles.is_empty() {
            return false;
        }
        self.session = Some(Session {
            tiles,
            hovered: None,
            selected: None,
            settle: Some(Settle {
                from: 0.0,
                to: 1.0,
                started_at: now,
                duration: Duration::from_millis(config.transition_ms),
            }),
            manual_progress: None,
            pending_activation: None,
        });
        true
    }

    pub fn begin_interactive(
        &mut self,
        space: &Space<Window>,
        nodes: &crate::nodes::NodesState,
        clusters: &crate::clusters::ClusterSystem,
        config: halley_config::Apogee,
    ) -> bool {
        if !config.enabled {
            return false;
        }
        if self.session.is_none() {
            let tiles = build_layout(space, nodes, clusters, config);
            if tiles.is_empty() {
                return false;
            }
            self.session = Some(Session {
                tiles,
                hovered: None,
                selected: None,
                settle: None,
                manual_progress: Some(0.0),
                pending_activation: None,
            });
        }
        self.session.as_mut().is_some_and(|session| {
            session.settle = None;
            session.manual_progress = Some(session.manual_progress.unwrap_or(0.0));
            true
        })
    }

    pub fn set_interactive_progress(&mut self, progress: f32) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let next = progress.clamp(0.0, 1.0);
        let changed = session.manual_progress != Some(next);
        session.manual_progress = Some(next);
        changed
    }

    pub fn finish_interactive(
        &mut self,
        commit: bool,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let from = session.manual_progress.take().unwrap_or(0.0);
        let to = if commit { 1.0 } else { 0.0 };
        session.settle = Some(Settle {
            from,
            to,
            started_at: now,
            duration: proportional_duration(config.transition_ms, (to - from).abs()),
        });
        true
    }

    pub fn close(
        &mut self,
        activation: Option<NodeId>,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        if session.is_closing() {
            return false;
        }
        let from = session.progress(now);
        session.manual_progress = None;
        session.pending_activation = activation;
        session.settle = Some(Settle {
            from,
            to: 0.0,
            started_at: now,
            duration: proportional_duration(config.transition_ms, from),
        });
        true
    }

    pub fn hover(&mut self, position: Point<f64, Logical>) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let hovered = session.hit_test(position);
        let changed = session.hovered != hovered;
        session.hovered = hovered;
        if let Some(id) = hovered {
            session.selected = Some(id);
        }
        changed
    }

    pub fn activate_at(
        &mut self,
        position: Point<f64, Logical>,
        config: halley_config::Apogee,
        now: Duration,
    ) -> bool {
        let target = self
            .session
            .as_ref()
            .and_then(|session| session.hit_test(position));
        self.close(target, config, now)
    }

    pub fn move_selection(&mut self, direction: Direction) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let Some(current) = session.selected.and_then(|id| session.tile(id)).cloned() else {
            let next = session
                .tiles
                .iter()
                .min_by_key(|tile| (tile.target.loc.y, tile.target.loc.x, tile.id.as_u64()))
                .map(|tile| tile.id);
            let changed = session.selected != next;
            session.selected = next;
            session.hovered = None;
            return changed;
        };
        if session.tiles.is_empty() {
            return false;
        }
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
        session.selected = Some(next);
        session.hovered = None;
        true
    }

    pub fn selected(&self) -> Option<NodeId> {
        self.session.as_ref().and_then(|session| session.selected)
    }

    pub fn tick(&mut self, now: Duration) -> Tick {
        let Some(session) = self.session.as_ref() else {
            return Tick::Idle;
        };
        let progress = session.progress(now);
        let settling = session
            .settle
            .is_some_and(|settle| now.saturating_sub(settle.started_at) < settle.duration);
        if session.is_closing() && !settling && progress <= f32::EPSILON {
            let activation = session.pending_activation;
            self.session = None;
            return Tick::Closed(activation);
        }
        Tick::Active {
            animating: settling,
        }
    }
}

pub enum Tick {
    Idle,
    Active { animating: bool },
    Closed(Option<NodeId>),
}

fn build_layout(
    space: &Space<Window>,
    nodes: &crate::nodes::NodesState,
    clusters: &crate::clusters::ClusterSystem,
    config: halley_config::Apogee,
) -> Vec<Tile> {
    let mut by_output = HashMap::<String, Vec<(NodeId, usize, u64)>>::new();
    for record in nodes.records().filter(|record| {
        participates_in_window_mosaic(record.attached, clusters.is_member(record.id))
    }) {
        let source_stack_index = record.collapsed_stack_index.or_else(|| {
            space
                .elements()
                .position(|candidate| candidate == &record.window)
        });
        by_output.entry(record.output.clone()).or_default().push((
            record.id,
            source_stack_index.unwrap_or(usize::MAX),
            if record.collapsed { 0 } else { u64::MAX },
        ));
    }
    let mut tiles = Vec::new();
    let mut outputs = space
        .outputs()
        .filter_map(|output| {
            space
                .output_geometry(output)
                .map(|geometry| (output, geometry))
        })
        .collect::<Vec<_>>();
    outputs.sort_by_key(|(output, geometry)| (geometry.loc.x, geometry.loc.y, output.name()));
    for (output, output_rect) in outputs {
        let output_name = output.name();
        let ids = by_output
            .get(&output_name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let cores = clusters
            .clusters_for_output(&output_name)
            .filter_map(|(_, cluster, _)| clusters.core_node(cluster).map(|core| (cluster, core)))
            .collect::<Vec<_>>();
        tiles.extend(layout_output(
            ids,
            &cores,
            output_name,
            output_rect,
            nodes,
            config,
        ));
    }
    tiles
}

fn participates_in_window_mosaic(attached: bool, cluster_member: bool) -> bool {
    attached && !cluster_member
}

fn layout_output(
    ids: &[(NodeId, usize, u64)],
    cores: &[(ClusterId, NodeId)],
    output: String,
    bounds: Rectangle<i32, Logical>,
    nodes: &crate::nodes::NodesState,
    config: halley_config::Apogee,
) -> Vec<Tile> {
    if ids.is_empty() && cores.is_empty() {
        return Vec::new();
    }
    let entries = ids
        .iter()
        .filter_map(|(id, source_stack_index, source_stack_order)| {
            let record = nodes.record(*id)?;
            let node = nodes.field.node(*id)?;
            let width = record.geometry.size.w.max(1) as f32;
            let height = record.geometry.size.h.max(1) as f32;
            Some((
                *id,
                *source_stack_index,
                *source_stack_order,
                super::mosaic::Item {
                    x: node.pos.x,
                    y: node.pos.y,
                    aspect: (width / height).clamp(0.25, 4.5),
                    stable_key: id.as_u64(),
                    weight: width * height,
                },
            ))
        })
        .collect::<Vec<_>>();
    // Old Halley reserved its upper core rail even when no cluster cores were
    // present. Keeping the same band is part of Apogee's recognizable layout.
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
    let mut tiles = entries
        .into_iter()
        .zip(slots)
        .map(|((id, source_stack_index, source_stack_order, _), slot)| {
            let width = slot.w.round().max(1.0) as i32;
            let height = slot.h.round().max(1.0) as i32;
            Tile {
                id,
                kind: TileKind::Window,
                output: output.clone(),
                target: Rectangle::new(
                    (
                        bounds.loc.x + (slot.cx - slot.w * 0.5).round() as i32,
                        bounds.loc.y
                            + upper_band.round() as i32
                            + (slot.cy - slot.h * 0.5).round() as i32,
                    )
                        .into(),
                    (width, height).into(),
                ),
                source_stack_index,
                source_stack_order,
            }
        })
        .collect::<Vec<_>>();
    tiles.extend(
        cores
            .iter()
            .zip(layout_core_rail(cores.len(), bounds))
            .enumerate()
            .map(|(index, ((cluster, core), target))| Tile {
                id: *core,
                kind: TileKind::ClusterCore(*cluster),
                output: output.clone(),
                target,
                source_stack_index: usize::MAX,
                source_stack_order: index as u64,
            }),
    );
    tiles
}

/// Cluster cores occupy the upper Apogee rail that the mosaic already leaves
/// free. The gap contracts on narrow outputs while retaining the familiar
/// 68px core size whenever the output has room.
fn layout_core_rail(count: usize, bounds: Rectangle<i32, Logical>) -> Vec<Rectangle<i32, Logical>> {
    if count == 0 {
        return Vec::new();
    }
    let side = crate::clusters::CORE_DIAMETER_PX;
    let available = bounds.size.w.max(1) as f32 * 0.84;
    let gap = if count > 1 {
        ((available - side * count as f32) / count.saturating_sub(1) as f32).clamp(12.0, 44.0)
    } else {
        0.0
    };
    let width = side * count as f32 + gap * count.saturating_sub(1) as f32;
    let start_x = bounds.loc.x as f32 + bounds.size.w as f32 * 0.5 - width * 0.5;
    let center_y = bounds.loc.y as f32 + (bounds.size.h.max(1) as f32 * 0.125).max(54.0);
    (0..count)
        .map(|index| {
            Rectangle::new(
                (
                    (start_x + index as f32 * (side + gap)).round() as i32,
                    (center_y - side * 0.5).round() as i32,
                )
                    .into(),
                (side.round() as i32, side.round() as i32).into(),
            )
        })
        .collect()
}

fn proportional_duration(base_ms: u64, distance: f32) -> Duration {
    Duration::from_millis((base_ms as f32 * distance.clamp(0.0, 1.0)).round() as u64)
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

pub fn toggle<D: crate::session::SessionDriver>(session: &mut crate::session::Session<D>) -> bool {
    let now = crate::frame_clock::monotonic_now();
    let changed = if session.shell.apogee.is_active() {
        session
            .shell
            .apogee
            .close(None, session.settings.apogee, now)
    } else if session.capture.is_active()
        || session.shell.focus_cycle.is_open()
        || !matches!(session.interactions.grab, crate::input::grab::Grab::None)
    {
        false
    } else {
        session.nodes.sync_from_space(&session.wayland.space);
        session.shell.apogee.open(
            &session.wayland.space,
            &session.nodes,
            &session.clusters,
            session.settings.apogee,
            now,
        )
    };
    if changed {
        session.cursor.set_override(
            crate::cursor::OverrideSource::Modal,
            Some(smithay::input::pointer::CursorIcon::Default),
        );
        crate::session::note_pointer_activity(session);
        session.request_redraw();
    }
    changed
}

pub fn cancel<D: crate::session::SessionDriver>(session: &mut crate::session::Session<D>) -> bool {
    let changed = session.shell.apogee.close(
        None,
        session.settings.apogee,
        crate::frame_clock::monotonic_now(),
    );
    if changed {
        session.cursor.set_override(
            crate::cursor::OverrideSource::Modal,
            Some(smithay::input::pointer::CursorIcon::Default),
        );
        session.request_redraw();
    }
    changed
}

pub fn select<D: crate::session::SessionDriver>(session: &mut crate::session::Session<D>) -> bool {
    let target = session.shell.apogee.selected();
    let changed = session.shell.apogee.close(
        target,
        session.settings.apogee,
        crate::frame_clock::monotonic_now(),
    );
    if changed {
        session.cursor.set_override(
            crate::cursor::OverrideSource::Modal,
            Some(smithay::input::pointer::CursorIcon::Default),
        );
        session.request_redraw();
    }
    changed
}

pub fn pointer_motion<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    position: (f64, f64),
) -> bool {
    let changed = session.shell.apogee.hover(Point::from(position));
    let icon = if session.shell.apogee.hovered().is_some() {
        smithay::input::pointer::CursorIcon::Pointer
    } else {
        smithay::input::pointer::CursorIcon::Default
    };
    let cursor_changed = session
        .cursor
        .set_override(crate::cursor::OverrideSource::Modal, Some(icon));
    if changed || cursor_changed {
        session.request_redraw();
    }
    changed || cursor_changed
}

pub fn pointer_press<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    position: (f64, f64),
) -> bool {
    let changed = session.shell.apogee.activate_at(
        Point::from(position),
        session.settings.apogee,
        crate::frame_clock::monotonic_now(),
    );
    if changed {
        session.request_redraw();
    }
    changed
}

pub fn move_selection<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    direction: Direction,
) -> bool {
    let changed = session.shell.apogee.move_selection(direction);
    if changed {
        session.request_redraw();
    }
    changed
}

pub fn tick<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    now: Duration,
) -> bool {
    match session.shell.apogee.tick(now) {
        Tick::Idle => false,
        Tick::Active { animating } => animating,
        Tick::Closed(target) => {
            if let Some(target) = target {
                activate_target(session, target, now);
            }
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

fn activate_target<D: crate::session::SessionDriver>(
    session: &mut crate::session::Session<D>,
    target: NodeId,
    now: Duration,
) {
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    if let Some(cluster) = session.clusters.cluster_for_core(target) {
        let output_name = session
            .clusters
            .metadata(cluster)
            .map(|metadata| metadata.output.clone());
        let Some(output_name) = output_name else {
            return;
        };
        // Selecting the core for an already-open workspace must preserve it;
        // ClusterSystem::activate is a toggle, so call it only when entering a
        // different/collapsed cluster.
        if cluster_needs_activation(session.clusters.active_on(&output_name), cluster) {
            let _ = session.clusters.activate(&output_name, cluster, now);
        }
        let output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == output_name)
            .cloned();
        if let Some(output) = output {
            crate::session::sync_cluster_activation_focus(session, &output, cluster, false, serial);
        }
        let _ = crate::session::center_pointer_on_output(session, &output_name);
        return;
    }
    let Some(record) = session.nodes.record(target).cloned() else {
        return;
    };
    // A field target is intentionally outside the active cluster workspace.
    // Selecting it is the one Apogee path that should collapse that workspace;
    // merely opening or cancelling Apogee never reaches this mutation.
    if let Some(active) = session.clusters.active_on(&record.output) {
        let _ = session.clusters.activate(&record.output, active, now);
    }
    // Explicit focus raises the selected window; camera and pointer placement
    // share the same path as Alt+Tab.
    let _ = crate::nodes::focus_and_center_node(session, target, serial);
}

fn cluster_needs_activation(active: Option<ClusterId>, selected: ClusterId) -> bool {
    active != Some(selected)
}

pub fn send_preview_frames(
    state: &ApogeeState,
    nodes: &crate::nodes::NodesState,
    output: &smithay::output::Output,
    elapsed: Duration,
    sequence: u32,
) {
    let Some(session) = state.session() else {
        return;
    };
    for tile in session
        .tiles
        .iter()
        .filter(|tile| tile.output == output.name() && matches!(tile.kind, TileKind::Window))
    {
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
    use super::{
        ApogeeState, Session, Settle, Tile, TileKind, cluster_needs_activation, layout_output,
        participates_in_window_mosaic,
    };
    use halley_core::cluster::ClusterId;
    use halley_core::field::NodeId;
    use smithay::utils::Rectangle;
    use std::time::Duration;

    #[test]
    fn reversing_manual_progress_has_no_discontinuity() {
        let mut session = Session {
            tiles: vec![Tile {
                id: NodeId::new(1),
                kind: TileKind::Window,
                output: "DP-1".into(),
                target: Rectangle::new((0, 0).into(), (100, 100).into()),
                source_stack_index: 0,
                source_stack_order: u64::MAX,
            }],
            hovered: None,
            selected: Some(NodeId::new(1)),
            settle: None,
            manual_progress: Some(0.7),
            pending_activation: None,
        };
        assert_eq!(session.progress(Duration::ZERO), 0.7);
        session.manual_progress = Some(0.2);
        assert_eq!(session.progress(Duration::from_secs(20)), 0.2);
        session.manual_progress = Some(0.0);
        assert_eq!(session.progress(Duration::from_secs(30)), 0.0);
    }

    #[test]
    fn proportional_close_starts_at_the_live_progress() {
        let session = Session {
            tiles: Vec::new(),
            hovered: None,
            selected: None,
            settle: Some(Settle {
                from: 0.4,
                to: 0.0,
                started_at: Duration::from_secs(2),
                duration: Duration::from_millis(128),
            }),
            manual_progress: None,
            pending_activation: None,
        };
        assert_eq!(session.progress(Duration::from_secs(2)), 0.4);
        assert_eq!(
            session.progress(Duration::from_secs(2) + Duration::from_millis(128)),
            0.0
        );
    }

    #[test]
    fn closing_overlay_remains_visible_but_releases_input_and_live_commits() {
        let state = ApogeeState {
            session: Some(Session {
                tiles: Vec::new(),
                hovered: None,
                selected: None,
                settle: Some(Settle {
                    from: 1.0,
                    to: 0.0,
                    started_at: Duration::ZERO,
                    duration: Duration::from_millis(320),
                }),
                manual_progress: None,
                pending_activation: None,
            }),
        };

        assert!(state.is_active());
        assert!(!state.accepts_input());
        assert!(!state.accepts_live_previews());
    }

    #[test]
    fn core_only_output_still_produces_an_apogee_layout() {
        let nodes = crate::nodes::NodesState::new(&halley_config::RuntimeConfig::default());
        let cores = [
            (ClusterId::new(1), NodeId::new(11)),
            (ClusterId::new(2), NodeId::new(12)),
        ];
        let tiles = layout_output(
            &[],
            &cores,
            "DP-1".into(),
            Rectangle::new((0, 0).into(), (1920, 1080).into()),
            &nodes,
            halley_config::Apogee::default(),
        );

        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].kind, TileKind::ClusterCore(ClusterId::new(1)));
        assert_eq!(tiles[1].kind, TileKind::ClusterCore(ClusterId::new(2)));
        assert!(tiles.iter().all(|tile| tile.target.loc.y < 236));
    }

    #[test]
    fn cluster_members_are_represented_by_their_core_not_window_tiles() {
        assert!(participates_in_window_mosaic(true, false));
        assert!(!participates_in_window_mosaic(true, true));
        assert!(!participates_in_window_mosaic(false, false));
    }

    #[test]
    fn selecting_the_active_cluster_core_preserves_its_workspace() {
        let active = ClusterId::new(4);
        assert!(!cluster_needs_activation(Some(active), active));
        assert!(cluster_needs_activation(None, active));
        assert!(cluster_needs_activation(Some(ClusterId::new(3)), active));
    }
}
