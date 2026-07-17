//! Apogee — the Observatory overview.
//!
//! A keybind-triggered overlay that packs every window on the current monitor's
//! Field into a row-budgeted live-preview mosaic. Collapsed windows keep their
//! previews with a node-state badge; cluster cores stay compact. Tiles fly from
//! their real on-screen positions into the mosaic on open, and back on close.
//!
//! This module owns the session state, the spatial layout, and the open/close/tick
//! controller. Rendering lives in `overlay/observatory.rs`; both share the types
//! defined here.

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use halley_core::cluster::ClusterId;
use halley_core::field::{NodeId, NodeKind, NodeState, Vec2, Visibility};

use crate::compositor::root::Halley;
use crate::overlay::OverlayView;

/// A rectangle expressed as a center point plus size, in physical screen pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileRect {
    pub(crate) cx: f32,
    pub(crate) cy: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl TileRect {
    pub(crate) fn lerp(self, other: TileRect, t: f32) -> TileRect {
        TileRect {
            cx: self.cx + (other.cx - self.cx) * t,
            cy: self.cy + (other.cy - self.cy) * t,
            w: self.w + (other.w - self.w) * t,
            h: self.h + (other.h - self.h) * t,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApogeeTileKind {
    /// A live window preview drawn with its border.
    Window,
    /// A cluster core, drawn as its core marker.
    Core,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ApogeeTile {
    pub(crate) node_id: NodeId,
    pub(crate) kind: ApogeeTileKind,
    /// Whether this preview represents a collapsed surface node.
    pub(crate) collapsed: bool,
    /// Source rect (the node's real on-screen rect at open time).
    pub(crate) from: TileRect,
    /// Destination rect (the node's mosaic slot).
    pub(crate) to: TileRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApogeePhase {
    Opening,
    Open,
    Closing,
}

pub(crate) struct ApogeeMonitorSession {
    pub(crate) monitor: String,
    pub(crate) core_scroll_offset: f32,
    pub(crate) core_atlas_width: f32,
    pub(crate) tiles: Vec<ApogeeTile>,
    pub(crate) core_tiles: Vec<ApogeeTile>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApogeeCollapsedCluster {
    pub(crate) monitor: String,
    pub(crate) cluster_id: ClusterId,
    pub(crate) core_id: NodeId,
    pub(crate) members: Vec<NodeId>,
}

pub(crate) struct ApogeeSession {
    pub(crate) phase: ApogeePhase,
    pub(crate) started_at: Instant,
    pub(crate) duration: Duration,
    pub(crate) monitors: Vec<ApogeeMonitorSession>,
    /// When set, the open transition follows a gesture instead of the clock:
    /// `progress()` returns this value directly so the overview tracks the finger.
    /// Cleared on commit/cancel, handing back to the timed `started_at` animation.
    pub(crate) manual_progress: Option<f32>,
    /// When set, the close transition will activate this target after finishing,
    /// so the desktop doesn't mutate underneath the fading overlay.
    pub(crate) pending_selection: Option<NodeId>,
    pub(crate) collapsed_clusters: Vec<ApogeeCollapsedCluster>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApogeeInteractionRegion {
    CoreBar,
    WindowRing,
}

impl ApogeeSession {
    /// Linear transition progress in `0.0..=1.0`. While a gesture is driving the
    /// open (`manual_progress`), that value is returned verbatim so the overview
    /// follows the finger; otherwise progress is time-based off `started_at`.
    pub(crate) fn progress(&self, now: Instant) -> f32 {
        if let Some(manual) = self.manual_progress {
            return manual.clamp(0.0, 1.0);
        }
        let elapsed = now.saturating_duration_since(self.started_at).as_secs_f32();
        let duration = self.duration.as_secs_f32().max(0.001);
        (elapsed / duration).clamp(0.0, 1.0)
    }

    pub(crate) fn monitor_session(&self, monitor: &str) -> Option<&ApogeeMonitorSession> {
        self.monitors
            .iter()
            .find(|session| session.monitor == monitor)
    }

    pub(crate) fn monitor_session_mut(
        &mut self,
        monitor: &str,
    ) -> Option<&mut ApogeeMonitorSession> {
        self.monitors
            .iter_mut()
            .find(|session| session.monitor == monitor)
    }
}

/// One item to place in the mosaic. `marker` items (cores / non-window markers) get
/// a small square slot; window items keep their aspect ratio.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApogeeLayoutItem {
    pub(crate) field_pos: Vec2,
    pub(crate) aspect: f32,
    pub(crate) marker: bool,
    pub(crate) stable_key: u64,
    pub(crate) weight: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ApogeeProjection {
    pub(crate) rect: TileRect,
    pub(crate) depth: f32,
}

impl Halley {
    /// Toggle the Observatory for the active monitor. Re-pressing while open starts
    /// the fly-back/close transition.
    pub(crate) fn toggle_apogee(&mut self, now: Instant) -> bool {
        if self.input.interaction_state.apogee_session.is_some() {
            self.close_apogee(now);
        } else {
            self.open_apogee(now);
        }
        true
    }

    pub(crate) fn open_apogee(&mut self, now: Instant) {
        if !self.runtime.tuning.apogee.enabled {
            return;
        }
        self.input.interaction_state.apogee_live_preview_node = None;
        self.input.interaction_state.apogee_live_preview_last_at = None;
        self.input.interaction_state.apogee_hover_node = None;
        self.ui.render_state.view.apogee_core_hover_mix.clear();
        let monitor_names = apogee_monitor_names(self);
        let collapsed_clusters =
            collapse_active_cluster_workspaces_for_apogee(self, monitor_names.as_slice(), now);
        let mut monitors = Vec::new();
        let mut has_content = false;
        for monitor in monitor_names {
            let (screen_w, screen_h) = match self.model.monitor_state.monitors.get(&monitor) {
                Some(space) => (space.width, space.height),
                None => continue,
            };
            if screen_w <= 0 || screen_h <= 0 {
                continue;
            }

            let previous_monitor = self.begin_temporary_render_monitor(monitor.as_str());
            let (tiles, core_tiles, core_atlas_width) =
                build_apogee_tiles(self, monitor.as_str(), screen_w, screen_h, now);
            self.end_temporary_render_monitor(previous_monitor);
            has_content |= !tiles.is_empty() || !core_tiles.is_empty();
            monitors.push(ApogeeMonitorSession {
                monitor,
                core_scroll_offset: 0.0,
                core_atlas_width,
                tiles,
                core_tiles,
            });
        }
        if !has_content || monitors.is_empty() {
            return;
        }
        retarget_apogee_collapsed_cluster_ghosts(
            self,
            monitors.as_slice(),
            collapsed_clusters.as_slice(),
        );

        let duration = Duration::from_millis(self.runtime.tuning.apogee.transition_ms.max(1));
        self.input.interaction_state.apogee_session = Some(ApogeeSession {
            phase: ApogeePhase::Opening,
            started_at: now,
            duration,
            monitors,
            manual_progress: None,
            pending_selection: None,
            collapsed_clusters,
        });
    }

    /// Begin a gesture-driven open: build the overview but hold it at progress 0
    /// under `manual_progress` so the swipe can scrub it open frame by frame.
    pub(crate) fn begin_apogee_open_gesture(&mut self, now: Instant) {
        if self.input.interaction_state.apogee_session.is_some() {
            return;
        }
        self.open_apogee(now);
        if let Some(session) = self.input.interaction_state.apogee_session.as_mut() {
            session.phase = ApogeePhase::Opening;
            session.manual_progress = Some(0.0);
        }
    }

    /// Scrub the gesture-driven open with a per-update cap, so fast flicks still
    /// commit but cannot visually teleport the overview to fully open.
    pub(crate) fn set_apogee_open_gesture_progress_capped(
        &mut self,
        progress: f32,
        max_delta: f32,
    ) {
        let drives = self
            .input
            .interaction_state
            .apogee_session
            .as_mut()
            .filter(|session| {
                session.manual_progress.is_some() && session.phase == ApogeePhase::Opening
            });
        if let Some(session) = drives {
            let target = progress.clamp(0.0, 1.0);
            let next = match session.manual_progress {
                Some(current) => {
                    let current = current.clamp(0.0, 1.0);
                    let max_delta = max_delta.max(0.0);
                    current + (target - current).clamp(-max_delta, max_delta)
                }
                None => target,
            };
            session.manual_progress = Some(next.clamp(0.0, 1.0));
            self.request_maintenance();
        }
    }

    /// Release a gesture-driven open as committed: hand back to the timed Opening
    /// animation, continuing from the current progress through to fully open.
    pub(crate) fn commit_apogee_open_gesture(&mut self, now: Instant) {
        if let Some(session) = self.input.interaction_state.apogee_session.as_mut()
            && let Some(progress) = session.manual_progress.take()
        {
            let dur = session.duration.as_secs_f32().max(0.001);
            let elapsed = Duration::from_secs_f32(dur * progress.clamp(0.0, 1.0));
            session.started_at = now.checked_sub(elapsed).unwrap_or(now);
            session.phase = ApogeePhase::Opening;
            self.request_maintenance();
        }
    }

    /// Release a gesture-driven open as cancelled: fly the partly-opened overview
    /// back to the desktop and close it.
    pub(crate) fn cancel_apogee_open_gesture(&mut self, now: Instant) {
        let driving = self
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .is_some_and(|session| session.manual_progress.is_some());
        if !driving {
            return;
        }
        self.close_apogee(now);
        if let Some(session) = self.input.interaction_state.apogee_session.as_mut() {
            session.manual_progress = None;
        }
        self.request_maintenance();
    }

    pub(crate) fn close_apogee(&mut self, now: Instant) {
        let screen_sizes = self
            .model
            .monitor_state
            .monitors
            .iter()
            .map(|(monitor, space)| (monitor.clone(), (space.width, space.height)))
            .collect::<std::collections::HashMap<_, _>>();
        // Desktop draw rank per window tile, computed under an immutable borrow so
        // the close fly-back can overlap windows in the same z-order the live
        // desktop uses (otherwise they snap into order when the overlay clears).
        let draw_ranks: std::collections::HashMap<NodeId, (u64, u64)> =
            match self.input.interaction_state.apogee_session.as_ref() {
                Some(session) => session
                    .monitors
                    .iter()
                    .flat_map(|m| m.tiles.iter())
                    .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
                    .map(|tile| {
                        (
                            tile.node_id,
                            crate::window::active_surface_draw_rank(self, tile.node_id),
                        )
                    })
                    .collect(),
                None => std::collections::HashMap::new(),
            };
        if let Some(session) = self.input.interaction_state.apogee_session.as_mut() {
            if session.phase == ApogeePhase::Closing {
                return;
            }
            self.input.interaction_state.apogee_live_preview_node = None;
            self.input.interaction_state.apogee_live_preview_last_at = None;
            let eased = ease_in_out_cubic(session.progress(now));
            let pending_sel = session.pending_selection;
            for monitor_session in &mut session.monitors {
                let (screen_w, screen_h) = screen_sizes
                    .get(&monitor_session.monitor)
                    .copied()
                    .unwrap_or((1, 1));
                // Reorder window tiles into the live desktop's z-order so the
                // fly-back overlaps correctly, then keep any picked tile on top.
                monitor_session
                    .tiles
                    .sort_by_key(|tile| draw_ranks.get(&tile.node_id).copied().unwrap_or((0, 0)));
                if let Some(sel) = pending_sel
                    && let Some(idx) = monitor_session
                        .tiles
                        .iter()
                        .position(|tile| tile.node_id == sel)
                {
                    let tile = monitor_session.tiles.remove(idx);
                    monitor_session.tiles.push(tile);
                }
                // Fly back from the currently displayed atlas rect, not from the
                // raw atlas slot, so scroll position does not snap on close.
                for tile in &mut monitor_session.tiles {
                    let projected = apogee_project_window_rect(tile.to).rect;
                    let desktop = tile.from;
                    tile.from = tile.from.lerp(projected, eased);
                    tile.to = desktop;
                }
                for tile in &mut monitor_session.core_tiles {
                    let projected = apogee_project_core_rect(
                        tile.to,
                        monitor_session.core_scroll_offset,
                        monitor_session.core_atlas_width,
                        screen_w,
                        screen_h,
                    )
                    .rect;
                    let desktop = tile.from;
                    tile.from = tile.from.lerp(projected, eased);
                    tile.to = desktop;
                }
            }
            session.phase = ApogeePhase::Closing;
            session.started_at = now;
        }
    }

    pub(crate) fn adjust_apogee_orbit(
        &mut self,
        monitor: &str,
        delta_px: f32,
        region: ApogeeInteractionRegion,
    ) -> bool {
        let Some(session) = self.input.interaction_state.apogee_session.as_mut() else {
            return false;
        };
        if session.phase == ApogeePhase::Closing {
            return false;
        }
        let Some(monitor_session) = session.monitor_session_mut(monitor) else {
            return false;
        };
        match region {
            ApogeeInteractionRegion::CoreBar => {
                if monitor_session.core_tiles.len() <= 1 {
                    return false;
                }
                let screen_w = self
                    .model
                    .monitor_state
                    .monitors
                    .get(monitor)
                    .map(|space| space.width.max(1) as f32)
                    .unwrap_or(1.0);
                let max_offset = (monitor_session.core_atlas_width - screen_w).max(0.0);
                if max_offset <= 0.5 {
                    return false;
                }
                let next = (monitor_session.core_scroll_offset + delta_px).clamp(0.0, max_offset);
                if (next - monitor_session.core_scroll_offset).abs() < 0.5 {
                    return false;
                }
                monitor_session.core_scroll_offset = next;
            }
            ApogeeInteractionRegion::WindowRing => {
                return false;
            }
        }
        true
    }

    /// Advance the session phase; clears it once the close transition finishes.
    pub(crate) fn tick_apogee(&mut self, now: Instant) {
        let Some(session) = self.input.interaction_state.apogee_session.as_mut() else {
            return;
        };
        let progress = session.progress(now);
        match session.phase {
            ApogeePhase::Opening => {
                if progress >= 1.0 {
                    session.phase = ApogeePhase::Open;
                }
            }
            ApogeePhase::Closing => {
                if progress >= 1.0 {
                    let pending = session.pending_selection.take();
                    let collapsed_clusters = std::mem::take(&mut session.collapsed_clusters);
                    self.input.interaction_state.apogee_session = None;
                    self.input.interaction_state.apogee_live_preview_node = None;
                    self.input.interaction_state.apogee_live_preview_last_at = None;
                    self.input.interaction_state.apogee_hover_node = None;
                    self.ui.render_state.view.apogee_core_hover_mix.clear();
                    if let Some(node_id) = pending {
                        activate_apogee_target_with_collapsed_clusters(
                            self,
                            node_id,
                            now,
                            collapsed_clusters,
                        );
                        self.request_maintenance();
                    }
                }
            }
            ApogeePhase::Open => {}
        }
    }
}

/// Whether an open Apogee session still needs frames drawn. Previews are frozen snapshots
/// captured once on open, so a fully-open overview is static: it only needs frames while the
/// open/close transition is animating, or until every window preview has been captured. When
/// this is false an idle open overview stops repainting entirely (cursor motion alone must
/// not re-render the whole Observatory).
pub(crate) fn apogee_render_pending(st: &Halley) -> bool {
    let Some(session) = st.input.interaction_state.apogee_session.as_ref() else {
        return false;
    };
    if session.phase != ApogeePhase::Open {
        return true;
    }
    // A cluster core is mid expand/collapse: keep drawing so the in-place
    // viewport cross-fade animates instead of snapping.
    if st
        .ui
        .render_state
        .view
        .apogee_core_hover_mix
        .values()
        .any(|mix| *mix > 0.005 && *mix < 0.995)
    {
        return true;
    }
    // Still settling captures? Keep drawing until each window preview exists.
    session.monitors.iter().any(|monitor_session| {
        monitor_session
            .tiles
            .iter()
            .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
            .any(|tile| {
                st.ui
                    .render_state
                    .cache
                    .window_offscreen_cache
                    .get(&tile.node_id)
                    .is_none_or(|cache| {
                        cache.texture.is_none() || cache.bbox.is_none() || !cache.has_content
                    })
            })
    })
}

fn apogee_monitor_names(st: &Halley) -> Vec<String> {
    let mut monitors = st
        .model
        .monitor_state
        .monitors
        .iter()
        .map(|(name, space)| (name.clone(), space.offset_x, space.offset_y))
        .collect::<Vec<_>>();
    monitors.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    monitors.into_iter().map(|(name, _, _)| name).collect()
}

fn collapse_active_cluster_workspaces_for_apogee(
    st: &mut Halley,
    monitors: &[String],
    now: Instant,
) -> Vec<ApogeeCollapsedCluster> {
    let mut collapsed = Vec::new();
    for monitor in monitors {
        let Some(cluster_id) = st.active_cluster_workspace_for_monitor(monitor.as_str()) else {
            continue;
        };
        let Some((core_id, members)) = st.model.field.cluster(cluster_id).and_then(|cluster| {
            cluster
                .core
                .map(|core_id| (core_id, cluster.members().to_vec()))
        }) else {
            continue;
        };
        if crate::compositor::clusters::system::exit_cluster_workspace_for_monitor(
            st,
            monitor.as_str(),
            now,
        ) {
            collapsed.push(ApogeeCollapsedCluster {
                monitor: monitor.clone(),
                cluster_id,
                core_id,
                members,
            });
        }
    }
    collapsed
}

fn retarget_apogee_collapsed_cluster_ghosts(
    st: &mut Halley,
    monitors: &[ApogeeMonitorSession],
    collapsed_clusters: &[ApogeeCollapsedCluster],
) {
    for collapsed in collapsed_clusters {
        let Some(monitor_session) = monitors
            .iter()
            .find(|session| session.monitor == collapsed.monitor)
        else {
            continue;
        };
        let Some(core_tile) = monitor_session
            .core_tiles
            .iter()
            .find(|tile| tile.node_id == collapsed.core_id)
        else {
            continue;
        };
        let (screen_w, screen_h) = st
            .model
            .monitor_state
            .monitors
            .get(collapsed.monitor.as_str())
            .map(|space| (space.width, space.height))
            .unwrap_or((1, 1));
        let target = apogee_project_core_rect(
            core_tile.to,
            monitor_session.core_scroll_offset,
            monitor_session.core_atlas_width,
            screen_w,
            screen_h,
        )
        .rect;
        for member in &collapsed.members {
            let _ = st
                .ui
                .render_state
                .retarget_closing_window_animation_pull(*member, (target.cx, target.cy));
        }
    }
}

#[inline]
fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - (f * f * f) / 2.0
    }
}

/// Hit-test a monitor-local screen point against the open overview's tiles, using
/// each tile's current (animated) rect. Returns the node under the point, if any.
pub(crate) fn apogee_tile_at(
    st: &Halley,
    monitor: &str,
    sx: f32,
    sy: f32,
    now: Instant,
) -> Option<NodeId> {
    let session = st.input.interaction_state.apogee_session.as_ref()?;
    let monitor_session = session.monitor_session(monitor)?;
    let (screen_w, screen_h) = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| (space.width, space.height))
        .unwrap_or((1, 1));
    let eased = ease_in_out_cubic(session.progress(now));
    let mut hits: Vec<(NodeId, f32)> = Vec::new();
    for tile in monitor_session
        .core_tiles
        .iter()
        .chain(monitor_session.tiles.iter())
    {
        let is_core = matches!(tile.kind, ApogeeTileKind::Core);
        let projection = if session.phase == ApogeePhase::Closing {
            None
        } else if is_core {
            Some(apogee_project_core_rect(
                tile.to,
                monitor_session.core_scroll_offset,
                monitor_session.core_atlas_width,
                screen_w,
                screen_h,
            ))
        } else {
            Some(apogee_project_window_rect(tile.to))
        };
        let target = projection.map(|p| p.rect).unwrap_or(tile.to);
        let depth = projection.map(|p| p.depth).unwrap_or(1.0);
        let rect = tile.from.lerp(target, eased);
        let (half_w, half_h) = (rect.w * 0.5, rect.h * 0.5);
        let inside_bounds = sx >= rect.cx - half_w
            && sx <= rect.cx + half_w
            && sy >= rect.cy - half_h
            && sy <= rect.cy + half_h;
        let inside = inside_bounds;
        if inside {
            hits.push((tile.node_id, depth));
        }
    }
    hits.into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(node_id, _)| node_id)
}

/// Hit-test only window tiles for bounded live previews. Core markers can still
/// be clicked via `apogee_tile_at`, but they never need window texture refreshes.
pub(crate) fn apogee_window_tile_at(
    st: &Halley,
    monitor: &str,
    sx: f32,
    sy: f32,
    now: Instant,
) -> Option<NodeId> {
    let session = st.input.interaction_state.apogee_session.as_ref()?;
    let monitor_session = session.monitor_session(monitor)?;
    let eased = ease_in_out_cubic(session.progress(now));
    monitor_session
        .tiles
        .iter()
        .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
        .filter_map(|tile| {
            let projection = if session.phase == ApogeePhase::Closing {
                None
            } else {
                Some(apogee_project_window_rect(tile.to))
            };
            let target = projection.map(|p| p.rect).unwrap_or(tile.to);
            let depth = projection.map(|p| p.depth).unwrap_or(1.0);
            let rect = tile.from.lerp(target, eased);
            let half_w = rect.w * 0.5;
            let half_h = rect.h * 0.5;
            (sx >= rect.cx - half_w
                && sx <= rect.cx + half_w
                && sy >= rect.cy - half_h
                && sy <= rect.cy + half_h)
                .then_some((tile.node_id, depth))
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(node_id, _)| node_id)
}

pub(crate) fn apogee_region_for_point(screen_h: i32, sy: f32) -> ApogeeInteractionRegion {
    let core_band_bottom = (screen_h.max(1) as f32 * 0.24).max(120.0);
    if sy <= core_band_bottom {
        ApogeeInteractionRegion::CoreBar
    } else {
        ApogeeInteractionRegion::WindowRing
    }
}

/// Global (cross-monitor) origin of a monitor in screen space.
fn apogee_monitor_global_offset(st: &Halley, monitor: &str) -> (f32, f32) {
    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| (space.offset_x as f32, space.offset_y as f32))
        .unwrap_or((0.0, 0.0))
}

/// On-screen *local* center of an apogee tile within its monitor, using the same
/// projection as `apogee_tile_at` (Open phase) so a cursor warped here lands on the
/// tile. Returns `None` if the node isn't a tile in this session.
fn apogee_tile_local_center(
    monitor_session: &ApogeeMonitorSession,
    screen_w: i32,
    node_id: NodeId,
) -> Option<(f32, f32)> {
    if let Some(tile) = monitor_session
        .core_tiles
        .iter()
        .find(|tile| tile.node_id == node_id)
    {
        let p = apogee_project_core_rect(
            tile.to,
            monitor_session.core_scroll_offset,
            monitor_session.core_atlas_width,
            screen_w,
            0,
        );
        return Some((p.rect.cx, p.rect.cy));
    }
    monitor_session
        .tiles
        .iter()
        .find(|tile| tile.node_id == node_id && matches!(tile.kind, ApogeeTileKind::Window))
        .map(|tile| (tile.to.cx, tile.to.cy))
}

/// Global screen center of an apogee tile (any monitor). Mirrors the on-screen
/// projection so the keyboard warp drops the cursor exactly onto the tile.
pub(crate) fn apogee_tile_global_center(st: &Halley, node_id: NodeId) -> Option<(f32, f32)> {
    let session = st.input.interaction_state.apogee_session.as_ref()?;
    for monitor_session in &session.monitors {
        let screen_w = st
            .model
            .monitor_state
            .monitors
            .get(&monitor_session.monitor)
            .map(|space| space.width)
            .unwrap_or(1);
        if let Some((lx, ly)) = apogee_tile_local_center(monitor_session, screen_w, node_id) {
            let (ox, oy) = apogee_monitor_global_offset(st, &monitor_session.monitor);
            return Some((lx + ox, ly + oy));
        }
    }
    None
}

/// Pick the next apogee tile to move the cursor to when navigating with the arrow keys.
/// Every monitor's window mosaic and (scrolled) core rail are flattened into one
/// **global** screen field, so navigation crosses monitor boundaries naturally (no wrap)
/// and `Up`/`Down` step between the mosaic and the core rail. Navigation starts from the
/// highlighted tile, which mouse hover and keyboard warps both update. If nothing is
/// highlighted yet, the first arrow seeds the highlight at the main monitor's top-left
/// window tile.
pub(crate) fn apogee_navigate(
    st: &Halley,
    dir: halley_config::DirectionalAction,
) -> Option<NodeId> {
    use halley_config::DirectionalAction as D;
    let session = st.input.interaction_state.apogee_session.as_ref()?;
    let exclude = st.input.interaction_state.apogee_hover_node;
    let Some(exclude) = exclude else {
        return apogee_initial_keyboard_tile(st, session);
    };
    let (cx, cy) = apogee_tile_global_center(st, exclude).or_else(|| {
        st.input
            .interaction_state
            .cursor
            .last_screen_global
            .filter(|_| apogee_session_has_tile(session, exclude))
    })?;

    // (node, global_cx, global_cy) for every selectable tile across all monitors.
    let mut cands: Vec<(NodeId, f32, f32)> = Vec::new();
    for monitor_session in &session.monitors {
        let screen_w = st
            .model
            .monitor_state
            .monitors
            .get(&monitor_session.monitor)
            .map(|space| space.width)
            .unwrap_or(1);
        let (ox, oy) = apogee_monitor_global_offset(st, &monitor_session.monitor);
        for tile in &monitor_session.core_tiles {
            if tile.node_id == exclude {
                continue;
            }
            let p = apogee_project_core_rect(
                tile.to,
                monitor_session.core_scroll_offset,
                monitor_session.core_atlas_width,
                screen_w,
                0,
            );
            cands.push((tile.node_id, p.rect.cx + ox, p.rect.cy + oy));
        }
        for tile in monitor_session
            .tiles
            .iter()
            .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
        {
            if tile.node_id == exclude {
                continue;
            }
            cands.push((tile.node_id, tile.to.cx + ox, tile.to.cy + oy));
        }
    }

    // Keep tiles strictly in the travel direction; score primary-axis distance plus a
    // perpendicular penalty so the nearest aligned tile wins.
    let mut best: Option<(f32, NodeId)> = None;
    for (id, gx, gy) in cands {
        let dx = gx - cx;
        let dy = gy - cy;
        let (primary, perp) = match dir {
            D::Left => (-dx, dy.abs()),
            D::Right => (dx, dy.abs()),
            D::Up => (-dy, dx.abs()),
            D::Down => (dy, dx.abs()),
        };
        if primary <= 1.0 {
            continue;
        }
        let cost = primary + perp * 2.0;
        if best.is_none_or(|(b, _)| cost < b) {
            best = Some((cost, id));
        }
    }
    best.map(|(_, id)| id)
}

fn apogee_session_has_tile(session: &ApogeeSession, node_id: NodeId) -> bool {
    session.monitors.iter().any(|monitor_session| {
        monitor_session
            .core_tiles
            .iter()
            .chain(monitor_session.tiles.iter())
            .any(|tile| tile.node_id == node_id)
    })
}

fn apogee_initial_keyboard_tile(st: &Halley, session: &ApogeeSession) -> Option<NodeId> {
    let main_monitor = apogee_monitor_names(st).into_iter().find(|monitor| {
        session
            .monitor_session(monitor.as_str())
            .is_some_and(|session| !session.tiles.is_empty() || !session.core_tiles.is_empty())
    })?;
    let monitor_session = session.monitor_session(main_monitor.as_str())?;
    monitor_session
        .tiles
        .iter()
        .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
        .min_by(tile_top_left_order)
        .or_else(|| {
            monitor_session
                .core_tiles
                .iter()
                .min_by(tile_top_left_order)
        })
        .map(|tile| tile.node_id)
}

fn tile_top_left_order(a: &&ApogeeTile, b: &&ApogeeTile) -> Ordering {
    a.to.cy
        .total_cmp(&b.to.cy)
        .then_with(|| a.to.cx.total_cmp(&b.to.cx))
        .then_with(|| a.node_id.as_u64().cmp(&b.node_id.as_u64()))
}

/// Scroll the core rail of whichever monitor owns `node_id` so the core is on screen.
/// No-op for window tiles. Run after navigation lands on a (possibly scrolled-off) core,
/// before warping the cursor to it.
pub(crate) fn apogee_reveal_tile(st: &mut Halley, node_id: NodeId) {
    let monitor = st
        .input
        .interaction_state
        .apogee_session
        .as_ref()
        .and_then(|session| {
            session
                .monitors
                .iter()
                .find(|ms| ms.core_tiles.iter().any(|tile| tile.node_id == node_id))
                .map(|ms| ms.monitor.clone())
        });
    if let Some(monitor) = monitor {
        apogee_reveal_core(st, monitor.as_str(), node_id);
    }
}

/// Scroll the core rail so the given core tile is fully visible. No-op for window tiles
/// or when the rail can't scroll. Used after keyboard navigation lands on an off-screen
/// core so the selection is always on screen.
fn apogee_reveal_core(st: &mut Halley, monitor: &str, node_id: NodeId) {
    let screen_w = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| space.width.max(1) as f32)
        .unwrap_or(1.0);
    let Some(session) = st.input.interaction_state.apogee_session.as_mut() else {
        return;
    };
    let Some(monitor_session) = session.monitor_session_mut(monitor) else {
        return;
    };
    let max_offset = (monitor_session.core_atlas_width - screen_w).max(0.0);
    if max_offset <= 0.5 {
        return;
    }
    let Some(tile) = monitor_session
        .core_tiles
        .iter()
        .find(|tile| tile.node_id == node_id)
    else {
        return;
    };
    let half_w = tile.to.w * 0.5;
    let left = tile.to.cx - half_w;
    let right = tile.to.cx + half_w;
    let margin = 24.0;
    let mut offset = monitor_session.core_scroll_offset;
    if left - offset < margin {
        offset = left - margin;
    } else if right - offset > screen_w - margin {
        offset = right - screen_w + margin;
    }
    monitor_session.core_scroll_offset = offset.clamp(0.0, max_offset);
}

/// Begin closing Apogee with a pending selection. The selected tile is raised to
/// the top of the draw order for the close animation, and the actual focus /
/// activation is deferred until the close animation finishes (via `tick_apogee`)
/// so the desktop doesn't mutate underneath the fading overlay.
pub(crate) fn select_apogee_target(st: &mut Halley, node_id: NodeId, now: Instant) {
    if let Some(session) = st.input.interaction_state.apogee_session.as_mut() {
        for monitor_session in &mut session.monitors {
            if let Some(idx) = monitor_session.tiles.iter().position(|tile| {
                tile.node_id == node_id && matches!(tile.kind, ApogeeTileKind::Window)
            }) {
                let tile = monitor_session.tiles.remove(idx);
                monitor_session.tiles.push(tile);
                break;
            }
        }
        session.pending_selection = Some(node_id);
    }
    st.close_apogee(now);
}

/// Fly the camera to a picked overview tile and focus/activate it. Reuses the same
/// reveal path as clicking a node on the Field, so you land *at* the window.
#[allow(dead_code)]
pub(crate) fn activate_apogee_target(st: &mut Halley, node_id: NodeId, now: Instant) {
    activate_apogee_target_with_collapsed_clusters(st, node_id, now, Vec::new());
}

fn activate_apogee_target_with_collapsed_clusters(
    st: &mut Halley,
    node_id: NodeId,
    now: Instant,
    collapsed_clusters: Vec<ApogeeCollapsedCluster>,
) {
    // Raise the selected window's tile to the top of its monitor's draw order so
    // the close fly-back shows it coming forward, instead of animating back behind
    // its neighbours and only popping in front once the live (raised) window
    // renders after the transition ends.
    if let Some(session) = st.input.interaction_state.apogee_session.as_mut() {
        for monitor_session in &mut session.monitors {
            if let Some(idx) = monitor_session.tiles.iter().position(|tile| {
                tile.node_id == node_id && matches!(tile.kind, ApogeeTileKind::Window)
            }) {
                let tile = monitor_session.tiles.remove(idx);
                monitor_session.tiles.push(tile);
                break;
            }
        }
    }

    if st
        .model
        .field
        .node(node_id)
        .is_some_and(|node| node.kind == NodeKind::Surface)
    {
        let target_monitor = st.monitor_for_node_or_current(node_id);
        let target_cluster = st.model.field.cluster_id_for_member_public(node_id);
        if let Some(active_cid) = st.active_cluster_workspace_for_monitor(target_monitor.as_str())
            && target_cluster != Some(active_cid)
        {
            let _ = crate::compositor::clusters::system::exit_cluster_workspace_for_monitor(
                st,
                target_monitor.as_str(),
                now,
            );
        }
    }

    if crate::compositor::actions::window::focus_from_presentation_navigation(st, node_id, now)
        || crate::compositor::actions::window::focus_or_reveal_surface_node(st, node_id, now)
    {
        return;
    }
    // A cluster core: open the cluster (enter its workspace) instead of just
    // panning to it, when the knob is enabled.
    if st.runtime.tuning.apogee.open_cluster_on_select
        && st.model.field.cluster_id_for_core_public(node_id).is_some()
    {
        if let Some(collapsed) = collapsed_clusters
            .iter()
            .find(|cluster| cluster.core_id == node_id)
            && restore_apogee_collapsed_cluster(st, collapsed, now)
        {
            return;
        }
        let monitor = st.monitor_for_node_or_current(node_id);
        if st.focused_monitor() != monitor {
            st.focus_monitor_view(monitor.as_str(), now);
        }
        if crate::compositor::clusters::system::enter_cluster_workspace_by_core(
            st,
            node_id,
            monitor.as_str(),
            now,
        ) {
            return;
        }
    }
    // Cluster cores (and any non-surface node): focus + pan to its Field position.
    if let Some(pos) = st.model.field.node(node_id).map(|node| node.pos) {
        let monitor = st.monitor_for_node_or_current(node_id);
        if st.focused_monitor() != monitor {
            st.focus_monitor_view(monitor.as_str(), now);
        }
        st.set_interaction_focus(Some(node_id), 30_000, now);
        st.set_pan_restore_focus_target(node_id);
        let _ = st.animate_viewport_center_to(pos, now);
    }
}

fn restore_apogee_collapsed_cluster(
    st: &mut Halley,
    collapsed: &ApogeeCollapsedCluster,
    now: Instant,
) -> bool {
    if st.model.field.cluster_id_for_core_public(collapsed.core_id) != Some(collapsed.cluster_id) {
        return false;
    }
    let core_rect =
        st.model
            .field
            .node(collapsed.core_id)
            .map(|core| crate::animation::ClusterTileAnimRect {
                center: core.pos,
                size: core.footprint,
                alpha: 0.0,
            });
    for member in &collapsed.members {
        st.ui.render_state.remove_closing_window_animation(*member);
    }
    if st.focused_monitor() != collapsed.monitor {
        st.focus_monitor_view(collapsed.monitor.as_str(), now);
    }
    if !crate::compositor::clusters::system::enter_cluster_workspace_by_core(
        st,
        collapsed.core_id,
        collapsed.monitor.as_str(),
        now,
    ) {
        return false;
    }
    seed_apogee_cluster_restore_animation(st, collapsed, core_rect, now);
    true
}

fn seed_apogee_cluster_restore_animation(
    st: &mut Halley,
    collapsed: &ApogeeCollapsedCluster,
    core_rect: Option<crate::animation::ClusterTileAnimRect>,
    now: Instant,
) {
    if !st.runtime.tuning.cluster_animation_enabled() {
        return;
    }
    let duration_ms = match crate::compositor::clusters::system::active_cluster_layout_kind(st) {
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Stacking => {
            st.runtime.tuning.cluster_stacking_close_duration_ms()
        }
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling => {
            st.runtime.tuning.cluster_tiling_close_duration_ms()
        }
    };
    if matches!(
        st.runtime.tuning.cluster_layout_kind(),
        halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
    ) && st.runtime.tuning.tile_animation_enabled()
        && let Some(from) = core_rect
    {
        for member in &collapsed.members {
            if let Some(target) =
                st.active_cluster_tile_rect_for_member(collapsed.monitor.as_str(), *member)
            {
                crate::animation::set_cluster_tile_target_from_anim_rect(
                    st.ui.render_state.cluster_tile_tracks_mut(),
                    from,
                    *member,
                    target,
                    now,
                    duration_ms,
                    0,
                );
                st.request_window_animation_prewarm(*member, now);
            }
        }
        return;
    }
    for member in &collapsed.members {
        crate::compositor::workspace::state::mark_active_transition(st, *member, now, duration_ms);
    }
}

/// Collect the current monitor's nodes and lay them out into the mosaic.
fn build_apogee_tiles(
    st: &Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    now: Instant,
) -> (Vec<ApogeeTile>, Vec<ApogeeTile>, f32) {
    let view = OverlayView::from_halley(st);
    let scale_x = screen_w as f32 / view.camera_view_size.x.max(1.0);
    let scale_y = screen_h as f32 / view.camera_view_size.y.max(1.0);

    // When a cluster workspace is open on this monitor, the live desktop shows
    // that cluster's expanded layout. Apogee instead renders the *field* overview:
    // the opened cluster collapses back to its core icon (a marker tile), and the
    // regular field nodes that the workspace detached are surfaced again at their
    // field positions. This keeps Apogee a stable field map regardless of whether
    // a cluster happens to be opened.
    let active_cid = st.active_cluster_workspace_for_monitor(monitor);
    let active_members: std::collections::HashSet<NodeId> = active_cid
        .and_then(|cid| view.field.cluster(cid))
        .map(|cluster| cluster.members().iter().copied().collect())
        .unwrap_or_default();

    // (node, kind, collapsed, field_pos, aspect, weight, source_rect)
    let mut raw: Vec<(NodeId, ApogeeTileKind, bool, Vec2, f32, f32, TileRect)> = Vec::new();
    let mut core_raw: Vec<(NodeId, ApogeeTileKind, bool, Vec2, f32, f32, TileRect)> = Vec::new();
    for (node_id, node_monitor) in view.monitor_state.node_monitor.iter() {
        if node_monitor != monitor {
            continue;
        }
        let Some(node) = view.field.node(*node_id) else {
            continue;
        };
        // With a workspace open, drop its members (collapsed under the core) but
        // surface every other field-level node — including those the workspace
        // detached — by ignoring only the DETACHED flag. HIDDEN_BY_CLUSTER and
        // HIDDEN_EXPLICIT stay hidden, exactly as in the normal field view.
        let field_visible = if active_cid.is_some() {
            !active_members.contains(node_id)
                && !node.visibility.has(Visibility::HIDDEN_BY_CLUSTER)
                && !node.visibility.has(Visibility::HIDDEN_EXPLICIT)
        } else {
            view.field.is_visible(*node_id)
        };
        if !field_visible {
            continue;
        }
        let (kind, collapsed) = apogee_tile_class(&node.kind, &node.state);

        // Use the presentation visual rect (fullscreen or maximized) when the
        // node is in a presentation state, so the close animation flies back to
        // the correct on-screen geometry instead of the windowed field position.
        let presentation_visual = {
            let fs_visual =
                crate::compositor::fullscreen::system::fullscreen_visual_for_node_on_monitor_at(
                    st, *node_id, monitor, now,
                );
            let is_soft_suspended = st
                .model
                .fullscreen_state
                .fullscreen_soft_suspended_node
                .get(monitor)
                .copied()
                == Some(*node_id);
            let has_fs_anim = st
                .model
                .fullscreen_state
                .fullscreen_scale_anim
                .get(node_id)
                .is_some_and(|a| a.monitor == monitor);
            if is_soft_suspended && !has_fs_anim {
                None
            } else {
                fs_visual
            }
        }
        .or_else(|| {
            crate::compositor::workspace::state::maximized_visual_for_node_on_monitor_at(
                st, *node_id, monitor, now,
            )
        });

        let (src_center, src_size) = presentation_visual.unwrap_or((node.pos, node.footprint));
        let (cx, cy) = view.world_to_screen(screen_w, screen_h, src_center.x, src_center.y);
        let from = TileRect {
            cx: cx as f32,
            cy: cy as f32,
            w: (src_size.x * scale_x).max(8.0),
            h: (src_size.y * scale_y).max(8.0),
        };
        let preview_size = if matches!(kind, ApogeeTileKind::Window) {
            apogee_window_preview_size(&view, *node_id, node.intrinsic_size)
        } else {
            node.footprint
        };
        let aspect = window_aspect(&view, *node_id, preview_size);
        let weight = if matches!(kind, ApogeeTileKind::Window) {
            (preview_size.x * preview_size.y).max(1.0)
        } else {
            0.15
        };
        let entry = (*node_id, kind, collapsed, node.pos, aspect, weight, from);
        if matches!(kind, ApogeeTileKind::Core) {
            core_raw.push(entry);
        } else {
            raw.push(entry);
        }
    }

    // With a cluster workspace open, the cluster's core node is moved out of the
    // field's node map (it lives in the cluster's active workspace storage), so
    // the loop above can't see it. Re-surface it here as the cluster's collapsed
    // icon — flying in from its recorded field position — so the opened cluster
    // still appears in the overview, just as a single core marker.
    if let Some(cid) = active_cid
        && let Some(core_id) = view.field.cluster(cid).and_then(|c| c.core)
        && let Some(&core_pos) = st.model.cluster_state.workspace_core_positions.get(monitor)
    {
        let core_footprint = Vec2 { x: 48.0, y: 48.0 };
        let (cx, cy) = view.world_to_screen(screen_w, screen_h, core_pos.x, core_pos.y);
        let from = TileRect {
            cx: cx as f32,
            cy: cy as f32,
            w: (core_footprint.x * scale_x).max(8.0),
            h: (core_footprint.y * scale_y).max(8.0),
        };
        core_raw.push((
            core_id,
            ApogeeTileKind::Core,
            false,
            core_pos,
            1.0,
            0.15,
            from,
        ));
    }

    // Surface an active cluster workspace's overflow-bar members as window tiles.
    // They are HIDDEN_BY_CLUSTER on the desktop (so the loop above skips them) yet
    // still real workspace nodes; showing them lets the overview promote one into
    // the layout. They fly in from the overflow strip.
    // Skipped in field view (when a workspace is open on this monitor): the
    // cluster is shown collapsed as a single core icon, so its overflow members
    // are not surfaced either.
    if active_cid.is_none()
        && matches!(
            crate::compositor::clusters::system::active_cluster_layout_kind(st),
            halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
        )
        && let Some(overflow_ids) = st
            .model
            .cluster_state
            .cluster_overflow_members
            .get(monitor)
            .cloned()
    {
        let strip = st
            .model
            .cluster_state
            .cluster_overflow_rects
            .get(monitor)
            .copied();
        let already: std::collections::HashSet<NodeId> = raw.iter().map(|e| e.0).collect();
        for oid in overflow_ids {
            if already.contains(&oid) {
                continue;
            }
            let Some(node) = view.field.node(oid) else {
                continue;
            };
            let preview_size = apogee_window_preview_size(&view, oid, node.intrinsic_size);
            let aspect = window_aspect(&view, oid, preview_size);
            let weight = (preview_size.x * preview_size.y).max(1.0);
            let from = if let Some(strip) = strip {
                let side = strip.w.max(8.0);
                TileRect {
                    cx: strip.x + strip.w * 0.5,
                    cy: strip.y + strip.h * 0.5,
                    w: side,
                    h: side,
                }
            } else {
                let (cx, cy) = view.world_to_screen(screen_w, screen_h, node.pos.x, node.pos.y);
                TileRect {
                    cx: cx as f32,
                    cy: cy as f32,
                    w: (preview_size.x * scale_x).max(8.0),
                    h: (preview_size.y * scale_y).max(8.0),
                }
            };
            raw.push((
                oid,
                ApogeeTileKind::Window,
                false,
                node.pos,
                aspect,
                weight,
                from,
            ));
        }
    }

    if raw.is_empty() && core_raw.is_empty() {
        return (Vec::new(), Vec::new(), screen_w.max(1) as f32);
    }
    raw.sort_by(|a, b| {
        a.3.y
            .partial_cmp(&b.3.y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.3.x.partial_cmp(&b.3.x).unwrap_or(Ordering::Equal))
            .then_with(|| a.0.as_u64().cmp(&b.0.as_u64()))
    });
    core_raw.sort_by(|a, b| {
        a.3.x
            .partial_cmp(&b.3.x)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.as_u64().cmp(&b.0.as_u64()))
    });

    let items: Vec<ApogeeLayoutItem> = raw
        .iter()
        .map(
            |(node_id, kind, _, pos, aspect, weight, _)| ApogeeLayoutItem {
                field_pos: *pos,
                aspect: *aspect,
                marker: !matches!(kind, ApogeeTileKind::Window),
                stable_key: node_id.as_u64(),
                weight: *weight,
            },
        )
        .collect();

    let gap = view.tuning.apogee.gap.max(0.0);
    let core_bar_h = apogee_core_bar_height(screen_h);
    let window_area_h = (screen_h as f32 - core_bar_h).round().max(64.0) as i32;
    let slots = if items.is_empty() {
        Vec::new()
    } else {
        let mut slots = layout_mosaic(
            &items,
            screen_w,
            window_area_h,
            gap,
            view.tuning.apogee.max_rows.clamp(1, 5) as usize,
        );
        for slot in &mut slots {
            slot.cy += core_bar_h;
        }
        slots
    };
    let core_slots = layout_core_rail(core_raw.len(), screen_w, screen_h);
    let core_atlas_width = core_bar_width_for_slots(&core_slots, screen_w);

    let tiles = raw
        .into_iter()
        .zip(slots)
        .map(
            |((node_id, kind, collapsed, _, _, _, from), to)| ApogeeTile {
                node_id,
                kind,
                collapsed,
                from,
                to,
            },
        )
        .collect();
    let core_tiles = core_raw
        .into_iter()
        .zip(core_slots)
        .map(
            |((node_id, kind, collapsed, _, _, _, from), to)| ApogeeTile {
                node_id,
                kind,
                collapsed,
                from,
                to,
            },
        )
        .collect();
    (tiles, core_tiles, core_atlas_width)
}

fn apogee_tile_class(kind: &NodeKind, state: &NodeState) -> (ApogeeTileKind, bool) {
    match kind {
        NodeKind::Surface => (ApogeeTileKind::Window, matches!(state, NodeState::Node)),
        NodeKind::Core => (ApogeeTileKind::Core, false),
    }
}

fn layout_core_rail(count: usize, screen_w: i32, screen_h: i32) -> Vec<TileRect> {
    if count == 0 {
        return Vec::new();
    }
    let side = 68.0;
    let gap = 44.0;
    let step = side + gap;
    let compact_w = count as f32 * side + count.saturating_sub(1) as f32 * gap;
    let visible_w = screen_w as f32 * 0.76;
    let start_x = if compact_w <= visible_w {
        screen_w as f32 * 0.5 - compact_w * 0.5 + side * 0.5
    } else {
        screen_w as f32 * 0.08 + side * 0.5
    };
    let y = (screen_h as f32 * 0.125).max(54.0);
    (0..count)
        .map(|i| TileRect {
            cx: start_x + i as f32 * step,
            cy: y,
            w: side,
            h: side,
        })
        .collect()
}

fn core_bar_width_for_slots(slots: &[TileRect], screen_w: i32) -> f32 {
    if slots.is_empty() {
        return screen_w.max(1) as f32;
    }
    let min_x = slots
        .iter()
        .map(|slot| slot.cx - slot.w * 0.5)
        .fold(f32::MAX, f32::min);
    let max_x = slots
        .iter()
        .map(|slot| slot.cx + slot.w * 0.5)
        .fold(f32::MIN, f32::max);
    (max_x - min_x + 112.0).max(screen_w.max(1) as f32)
}

fn apogee_core_bar_height(screen_h: i32) -> f32 {
    // Tall enough to reserve room for a hovered cluster core to expand in place
    // into a small cluster viewport (see EXPANDED_CORE_* in the renderer) plus
    // its label, without spilling into the window mosaic below.
    (screen_h.max(1) as f32 * 0.215).clamp(140.0, 236.0)
}

pub(crate) fn apogee_project_window_rect(rect: TileRect) -> ApogeeProjection {
    ApogeeProjection { rect, depth: 1.0 }
}

pub(crate) fn apogee_project_core_rect(
    rect: TileRect,
    orbit_offset: f32,
    atlas_width: f32,
    screen_w: i32,
    screen_h: i32,
) -> ApogeeProjection {
    let screen_w = screen_w.max(1) as f32;
    let _ = screen_h;
    let max_offset = (atlas_width.max(screen_w) - screen_w).max(0.0);
    let cx = rect.cx - orbit_offset.clamp(0.0, max_offset);
    let rect = TileRect { cx, ..rect };
    ApogeeProjection { rect, depth: 2.0 }
}

fn window_aspect(view: &OverlayView<'_>, node_id: NodeId, footprint: Vec2) -> f32 {
    if let Some((_, _, w, h)) = view
        .render_state
        .cache
        .window_geometry
        .get(&node_id)
        .copied()
        && w >= 1.0
        && h >= 1.0
    {
        return (w / h).clamp(0.25, 4.5);
    }

    view.render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| cache.bbox)
        .map(|bbox| bbox.size.w.max(1) as f32 / bbox.size.h.max(1) as f32)
        .unwrap_or_else(|| footprint.x / footprint.y.max(1.0))
        .clamp(0.25, 4.5)
}

fn apogee_window_preview_size(view: &OverlayView<'_>, node_id: NodeId, fallback: Vec2) -> Vec2 {
    if let Some((_, _, w, h)) = view
        .render_state
        .cache
        .window_geometry
        .get(&node_id)
        .copied()
        && w >= 1.0
        && h >= 1.0
    {
        return Vec2 { x: w, y: h };
    }
    view.render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| cache.bbox)
        .map(|bbox| Vec2 {
            x: bbox.size.w.max(1) as f32,
            y: bbox.size.h.max(1) as f32,
        })
        .unwrap_or(fallback)
}

/// Spatial reading order: top-to-bottom by y band, left-to-right within a band.
/// Returns indices into `items`. Banding keeps roughly-aligned rows together so the
/// mosaic mirrors the field's geography instead of jittering on tiny y differences.
pub(crate) fn spatial_order(items: &[ApogeeLayoutItem]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    if items.len() < 2 {
        return order;
    }
    let (min_y, max_y) = items.iter().fold((f32::MAX, f32::MIN), |(lo, hi), item| {
        (lo.min(item.field_pos.y), hi.max(item.field_pos.y))
    });
    let span_y = (max_y - min_y).max(1.0);
    let approx_rows = (items.len() as f32).sqrt().ceil().max(1.0);
    let band = (span_y / approx_rows).max(1.0);
    order.sort_by(|&a, &b| {
        let band_a = ((items[a].field_pos.y - min_y) / band).floor() as i32;
        let band_b = ((items[b].field_pos.y - min_y) / band).floor() as i32;
        band_a.cmp(&band_b).then_with(|| {
            items[a]
                .field_pos
                .x
                .partial_cmp(&items[b].field_pos.x)
                .unwrap_or(Ordering::Equal)
                .then_with(|| items[a].stable_key.cmp(&items[b].stable_key))
        })
    });
    order
}

/// Pack items into a centered grid that fits the screen, preserving spatial order.
/// Each slot keeps the item's aspect (markers get a small square). Returns slots in
/// the same order as `items`.
pub(crate) fn layout_mosaic(
    items: &[ApogeeLayoutItem],
    screen_w: i32,
    screen_h: i32,
    gap: f32,
    max_rows: usize,
) -> Vec<TileRect> {
    let n = items.len();
    let mut out = vec![
        TileRect {
            cx: 0.0,
            cy: 0.0,
            w: 0.0,
            h: 0.0,
        };
        n
    ];
    if n == 0 {
        return out;
    }

    let margin = (gap * 2.0).max(32.0);
    let avail_w = (screen_w as f32 - margin * 2.0).max(64.0);
    let avail_h = (screen_h as f32 - margin * 2.0).max(64.0);
    if n == 1 {
        out[0] = single_window_mosaic_slot(items[0], screen_w, screen_h, avail_w, avail_h);
        return out;
    }
    let max_rows = max_rows.clamp(1, 5).min(n.max(1));
    let sizes = natural_mosaic_sizes(items, avail_w, avail_h, max_rows, gap);
    let order = packing_order(items, &sizes);
    let mut best: Option<PackAttempt> = None;

    for rows in 1..=max_rows {
        let pack_h = mosaic_pack_height(avail_h, rows, gap);
        let widths = packing_widths(&sizes, rows, avail_w, pack_h, gap);
        for width in widths {
            if let Some(attempt) =
                best_pack_for_width(items, &sizes, &order, width, pack_h, gap, rows, max_rows)
            {
                let replace = best
                    .as_ref()
                    .is_none_or(|current| attempt.score < current.score);
                if replace {
                    best = Some(attempt);
                }
            }
        }
    }

    let Some(best) = best else {
        return layout_mosaic_grid_fallback(items, screen_w, screen_h, gap);
    };

    let offset_x = screen_w as f32 * 0.5 - best.block_w * 0.5 - best.min_x;
    let offset_y = screen_h as f32 * 0.5 - best.block_h * 0.5 - best.min_y;
    for (idx, rect) in best.rects.into_iter().enumerate() {
        out[idx] = TileRect {
            cx: rect.cx + offset_x,
            cy: rect.cy + offset_y,
            w: rect.w,
            h: rect.h,
        };
    }
    out
}

fn single_window_mosaic_slot(
    item: ApogeeLayoutItem,
    screen_w: i32,
    screen_h: i32,
    avail_w: f32,
    avail_h: f32,
) -> TileRect {
    if item.marker {
        let side = avail_w.min(avail_h).clamp(48.0, 82.0);
        return TileRect {
            cx: screen_w as f32 * 0.5,
            cy: screen_h as f32 * 0.5,
            w: side,
            h: side,
        };
    }
    let aspect = item.aspect.clamp(0.25, 4.5);
    let max_w = (screen_w as f32 * 0.62).min(avail_w).max(64.0);
    let max_h = (screen_h as f32 * 0.56).min(avail_h).max(64.0);
    let mut w = max_w;
    let mut h = w / aspect;
    if h > max_h {
        h = max_h;
        w = h * aspect;
    }
    TileRect {
        cx: screen_w as f32 * 0.5,
        cy: screen_h as f32 * 0.5,
        w: w.clamp(64.0, max_w),
        h: h.clamp(64.0, max_h),
    }
}

#[derive(Clone, Copy, Debug)]
struct MosaicSize {
    w: f32,
    h: f32,
}

#[derive(Clone, Copy, Debug)]
struct FreeRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug)]
struct PackAttempt {
    rects: Vec<TileRect>,
    min_x: f32,
    min_y: f32,
    block_w: f32,
    block_h: f32,
    score: f32,
}

fn mosaic_pack_height(avail_h: f32, rows: usize, gap: f32) -> f32 {
    let rows = rows.max(1) as f32;
    let preferred = avail_h.max(160.0);
    let min_for_rows = rows * 74.0 + gap * (rows - 1.0);
    preferred.max(min_for_rows).min(avail_h)
}

fn natural_mosaic_sizes(
    items: &[ApogeeLayoutItem],
    avail_w: f32,
    pack_h: f32,
    rows: usize,
    gap: f32,
) -> Vec<MosaicSize> {
    let rows = rows.max(1) as f32;
    let row_gap = gap * (rows - 1.0).max(0.0);
    let nominal_h = ((pack_h - row_gap).max(64.0) / rows * 0.92).clamp(72.0, pack_h * 0.92);
    let marker_side = (nominal_h * 0.28).clamp(36.0, 82.0);
    let window_weights: Vec<f32> = items
        .iter()
        .filter(|item| !item.marker)
        .map(|item| item.weight.max(1.0))
        .collect();
    let avg_weight = if window_weights.is_empty() {
        1.0
    } else {
        window_weights.iter().sum::<f32>() / window_weights.len() as f32
    }
    .max(1.0);
    let base_area = nominal_h * nominal_h * 1.35;

    items
        .iter()
        .map(|item| {
            if item.marker {
                return MosaicSize {
                    w: marker_side,
                    h: marker_side,
                };
            }

            let aspect = item.aspect.clamp(0.25, 4.5);
            let weight = (item.weight.max(1.0) / avg_weight).sqrt().clamp(0.68, 1.45);
            let area = base_area * weight;
            let mut h = (area / aspect).sqrt();
            let mut w = h * aspect;
            let min_h = (nominal_h * 0.58).max(46.0);
            let max_h = (nominal_h * 1.46).min(pack_h * 0.92).max(min_h);
            if h < min_h {
                h = min_h;
                w = h * aspect;
            } else if h > max_h {
                h = max_h;
                w = h * aspect;
            }
            if w > avail_w * 0.92 {
                w = avail_w * 0.92;
                h = w / aspect;
            }
            w = w.max(48.0);
            h = h.max(36.0);
            MosaicSize { w, h }
        })
        .collect()
}

fn packing_order(items: &[ApogeeLayoutItem], sizes: &[MosaicSize]) -> Vec<usize> {
    let spatial = spatial_order(items);
    let mut rank = vec![0usize; items.len()];
    for (i, &idx) in spatial.iter().enumerate() {
        rank[idx] = i;
    }

    let mut order: Vec<usize> = (0..items.len()).collect();
    // Place larger previews first so the small node/core markers can behave like
    // grout pieces: they fill the remaining cuts instead of defining the atlas.
    order.sort_by(|&a, &b| {
        let area_a = sizes[a].w * sizes[a].h;
        let area_b = sizes[b].w * sizes[b].h;
        items[a]
            .marker
            .cmp(&items[b].marker)
            .then_with(|| area_b.partial_cmp(&area_a).unwrap_or(Ordering::Equal))
            .then_with(|| rank[a].cmp(&rank[b]))
            .then_with(|| items[a].stable_key.cmp(&items[b].stable_key))
    });
    order
}

fn packing_widths(
    sizes: &[MosaicSize],
    rows: usize,
    avail_w: f32,
    pack_h: f32,
    gap: f32,
) -> Vec<f32> {
    let rows = rows.max(1) as f32;
    let total_w =
        sizes.iter().map(|size| size.w).sum::<f32>() + gap * sizes.len().saturating_sub(1) as f32;
    let widest = sizes.iter().map(|size| size.w).fold(64.0, f32::max);
    let ideal = (total_w / rows * 1.08).max(widest).max(96.0).min(avail_w);
    let area_ideal = (sizes.iter().map(|size| size.w * size.h).sum::<f32>() / pack_h.max(1.0)
        * 1.22)
        .max(widest)
        .max(96.0)
        .min(avail_w);
    let max_w = avail_w.max(64.0);

    let mut widths = vec![
        ideal * 0.82,
        ideal * 0.94,
        ideal,
        ideal * 1.12,
        area_ideal,
        area_ideal * 1.16,
        avail_w * 0.92,
        avail_w,
    ];
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    widths.dedup_by(|a, b| (*a - *b).abs() < 8.0);
    widths
        .into_iter()
        .map(|width| width.clamp(widest.min(max_w), max_w))
        .collect()
}

fn best_pack_for_width(
    items: &[ApogeeLayoutItem],
    sizes: &[MosaicSize],
    order: &[usize],
    width: f32,
    avail_h: f32,
    gap: f32,
    rows: usize,
    max_rows: usize,
) -> Option<PackAttempt> {
    let widest = sizes
        .iter()
        .map(|size| size.w + gap)
        .fold(1.0_f32, f32::max);
    let mut lo = 0.12_f32;
    let mut hi = (width / widest).min(1.25).max(lo);
    let mut best = None;

    for _ in 0..16 {
        let mid = (lo + hi) * 0.5;
        match pack_scaled(items, sizes, order, width, avail_h, gap, mid) {
            Some(attempt) => {
                lo = mid;
                best = Some(attempt);
            }
            None => hi = mid,
        }
    }

    best.map(|mut attempt| {
        attempt.score = packing_score(&attempt, items.len(), width, avail_h, rows, max_rows);
        attempt
    })
}

fn pack_scaled(
    items: &[ApogeeLayoutItem],
    sizes: &[MosaicSize],
    order: &[usize],
    width: f32,
    avail_h: f32,
    gap: f32,
    scale: f32,
) -> Option<PackAttempt> {
    let mut rects = vec![
        TileRect {
            cx: 0.0,
            cy: 0.0,
            w: 0.0,
            h: 0.0,
        };
        items.len()
    ];
    let mut free = vec![FreeRect {
        x: 0.0,
        y: 0.0,
        w: width + gap,
        h: avail_h + gap,
    }];

    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    let mut content_area = 0.0_f32;

    for &idx in order {
        let mut w = sizes[idx].w * scale;
        let mut h = sizes[idx].h * scale;
        if items[idx].marker {
            let side = w.min(h).clamp(28.0, 96.0);
            w = side;
            h = side;
        }
        let need_w = w + gap;
        let need_h = h + gap;
        let slot = best_free_rect(&free, need_w, need_h)?;

        rects[idx] = TileRect {
            cx: slot.x + w * 0.5,
            cy: slot.y + h * 0.5,
            w,
            h,
        };
        min_x = min_x.min(slot.x);
        min_y = min_y.min(slot.y);
        max_x = max_x.max(slot.x + w);
        max_y = max_y.max(slot.y + h);
        content_area += w * h;

        split_free_rects(
            &mut free,
            FreeRect {
                x: slot.x,
                y: slot.y,
                w: need_w,
                h: need_h,
            },
        );
        prune_free_rects(&mut free);
    }

    let block_w = max_x - min_x;
    let block_h = max_y - min_y;
    if block_w > width + 0.5 || block_h > avail_h + 0.5 {
        return None;
    }

    let fill = content_area / (block_w * block_h).max(1.0);
    Some(PackAttempt {
        rects,
        min_x,
        min_y,
        block_w,
        block_h,
        score: 1.0 - fill,
    })
}

fn best_free_rect(free: &[FreeRect], need_w: f32, need_h: f32) -> Option<FreeRect> {
    free.iter()
        .filter(|rect| need_w <= rect.w + 0.5 && need_h <= rect.h + 0.5)
        .min_by(|a, b| {
            let score_a = free_rect_score(a, need_w, need_h);
            let score_b = free_rect_score(b, need_w, need_h);
            score_a.partial_cmp(&score_b).unwrap_or(Ordering::Equal)
        })
        .copied()
}

fn free_rect_score(rect: &FreeRect, need_w: f32, need_h: f32) -> (f32, f32, f32, f32, f32) {
    let leftover_w = (rect.w - need_w).max(0.0);
    let leftover_h = (rect.h - need_h).max(0.0);
    let short_side = leftover_w.min(leftover_h);
    let area_waste = rect.w * rect.h - need_w * need_h;

    // Top bands win first, which keeps growth horizontal; within a band, tight
    // leftover cuts win so small tiles fill real gaps instead of forcing new bands.
    (rect.y, short_side, area_waste, rect.x, rect.w)
}

fn split_free_rects(free: &mut Vec<FreeRect>, used: FreeRect) {
    let mut next = Vec::with_capacity(free.len() + 4);
    for rect in free.drain(..) {
        if !intersects_rect(rect, used) {
            next.push(rect);
            continue;
        }

        let rect_right = rect.x + rect.w;
        let rect_bottom = rect.y + rect.h;
        let used_right = used.x + used.w;
        let used_bottom = used.y + used.h;

        if used.x > rect.x {
            next.push(FreeRect {
                x: rect.x,
                y: rect.y,
                w: used.x - rect.x,
                h: rect.h,
            });
        }
        if used_right < rect_right {
            next.push(FreeRect {
                x: used_right,
                y: rect.y,
                w: rect_right - used_right,
                h: rect.h,
            });
        }
        if used.y > rect.y {
            next.push(FreeRect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: used.y - rect.y,
            });
        }
        if used_bottom < rect_bottom {
            next.push(FreeRect {
                x: rect.x,
                y: used_bottom,
                w: rect.w,
                h: rect_bottom - used_bottom,
            });
        }
    }

    next.retain(|rect| rect.w > 1.0 && rect.h > 1.0);
    sort_free_rects(&mut next);
    *free = next;
}

fn prune_free_rects(free: &mut Vec<FreeRect>) {
    let mut i = 0;
    while i < free.len() {
        let contained = (0..free.len()).any(|j| i != j && contains_rect(free[j], free[i]));
        if contained {
            free.swap_remove(i);
        } else {
            i += 1;
        }
    }
    sort_free_rects(free);
}

fn sort_free_rects(free: &mut [FreeRect]) {
    free.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(Ordering::Equal))
            .then_with(|| a.h.partial_cmp(&b.h).unwrap_or(Ordering::Equal))
            .then_with(|| a.w.partial_cmp(&b.w).unwrap_or(Ordering::Equal))
    });
}

fn contains_rect(a: FreeRect, b: FreeRect) -> bool {
    b.x >= a.x - 0.5
        && b.y >= a.y - 0.5
        && b.x + b.w <= a.x + a.w + 0.5
        && b.y + b.h <= a.y + a.h + 0.5
}

fn intersects_rect(a: FreeRect, b: FreeRect) -> bool {
    a.x < b.x + b.w - 0.5 && a.x + a.w > b.x + 0.5 && a.y < b.y + b.h - 0.5 && a.y + a.h > b.y + 0.5
}

fn packing_score(
    attempt: &PackAttempt,
    item_count: usize,
    avail_w: f32,
    avail_h: f32,
    rows: usize,
    max_rows: usize,
) -> f32 {
    let block_area = (attempt.block_w * attempt.block_h).max(1.0);
    let content_area = attempt
        .rects
        .iter()
        .map(|rect| rect.w * rect.h)
        .sum::<f32>();
    let fill = content_area / block_area;
    let block_aspect = attempt.block_w / attempt.block_h.max(1.0);
    let screen_aspect = avail_w / avail_h.max(1.0);
    let target_aspect = (screen_aspect * 0.95).max(1.35);
    let aspect_deficit = (target_aspect - block_aspect).max(0.0);
    let too_wide = (block_aspect - screen_aspect * 1.65).max(0.0) * 0.25;
    let area_frac = block_area / (avail_w * avail_h).max(1.0);
    let too_small = (0.62 - area_frac).max(0.0);
    let unused_w = (1.0 - attempt.block_w / avail_w.max(1.0)).max(0.0);
    let unused_h = (1.0 - attempt.block_h / avail_h.max(1.0)).max(0.0);
    let avg_h = attempt.rects.iter().map(|rect| rect.h).sum::<f32>() / item_count.max(1) as f32;
    let line_penalty = if item_count >= 3 && attempt.block_h < avg_h * 1.35 {
        if max_rows == 1 { 0.0 } else { 0.45 }
    } else {
        0.0
    };
    let row_budget = (rows as f32 / max_rows.max(1) as f32).clamp(0.0, 1.0);
    let row_penalty = if item_count >= 4 {
        row_budget * 0.10
    } else {
        0.0
    };

    aspect_deficit * 2.4
        + too_wide
        + (1.0 - fill) * 1.65
        + too_small * 1.25
        + unused_w * 0.55
        + unused_h * 0.45
        + line_penalty
        + row_penalty
}

fn layout_mosaic_grid_fallback(
    items: &[ApogeeLayoutItem],
    screen_w: i32,
    screen_h: i32,
    gap: f32,
) -> Vec<TileRect> {
    let n = items.len();
    let mut out = vec![
        TileRect {
            cx: 0.0,
            cy: 0.0,
            w: 0.0,
            h: 0.0,
        };
        n
    ];
    let margin = (gap * 2.0).max(32.0);
    let avail_w = (screen_w as f32 - margin * 2.0).max(64.0);
    let avail_h = (screen_h as f32 - margin * 2.0).max(64.0);
    let aspect_ratio = screen_w.max(1) as f32 / screen_h.max(1) as f32;
    let cols = (((n as f32) * aspect_ratio).sqrt().ceil() as usize).clamp(1, n);
    let rows = n.div_ceil(cols);
    let cell_w = avail_w / cols as f32;
    let cell_h = avail_h / rows as f32;
    let origin_x = margin;
    let origin_y = margin;

    let order = spatial_order(items);
    for (slot, &idx) in order.iter().enumerate() {
        let col = slot % cols;
        let row = slot / cols;
        let max_w = (cell_w - gap).max(8.0);
        let max_h = (cell_h - gap).max(8.0);
        let (w, h) = if items[idx].marker {
            let side = max_w.min(max_h).min(96.0);
            (side, side)
        } else {
            let aspect = items[idx].aspect.clamp(0.25, 4.5);
            let mut w = max_w;
            let mut h = w / aspect;
            if h > max_h {
                h = max_h;
                w = h * aspect;
            }
            (w, h)
        };
        out[idx] = TileRect {
            cx: origin_x + (col as f32 + 0.5) * cell_w,
            cy: origin_y + (row as f32 + 0.5) * cell_h,
            w,
            h,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(x: f32, y: f32) -> ApogeeLayoutItem {
        item_aspect(x, y, 1.5)
    }

    fn item_aspect(x: f32, y: f32, aspect: f32) -> ApogeeLayoutItem {
        ApogeeLayoutItem {
            field_pos: Vec2 { x, y },
            aspect,
            marker: false,
            stable_key: ((x as u64) << 24) ^ y as u64,
            weight: 1.0,
        }
    }

    fn marker(x: f32, y: f32) -> ApogeeLayoutItem {
        ApogeeLayoutItem {
            field_pos: Vec2 { x, y },
            aspect: 1.0,
            marker: true,
            stable_key: ((x as u64) << 24) ^ y as u64 ^ 0xffff,
            weight: 0.15,
        }
    }

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        tuning
    }

    fn single_monitor_tiling_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        tuning
    }

    fn test_apogee_tile(node_id: NodeId, cx: f32, cy: f32) -> ApogeeTile {
        ApogeeTile {
            node_id,
            kind: ApogeeTileKind::Window,
            collapsed: false,
            from: TileRect {
                cx,
                cy,
                w: 100.0,
                h: 80.0,
            },
            to: TileRect {
                cx,
                cy,
                w: 100.0,
                h: 80.0,
            },
        }
    }

    fn install_test_apogee_session(
        state: &mut Halley,
        monitors: Vec<ApogeeMonitorSession>,
        now: Instant,
    ) {
        state.input.interaction_state.apogee_session = Some(ApogeeSession {
            phase: ApogeePhase::Open,
            started_at: now,
            duration: std::time::Duration::from_millis(1),
            monitors,
            manual_progress: None,
            pending_selection: None,
            collapsed_clusters: Vec::new(),
        });
    }

    #[test]
    fn spatial_order_reads_top_left_to_bottom_right() {
        // Two rows of two, deliberately out of order in the input.
        let items = vec![
            item(1000.0, 1000.0), // bottom-right
            item(0.0, 0.0),       // top-left
            item(1000.0, 0.0),    // top-right
            item(0.0, 1000.0),    // bottom-left
        ];
        let order = spatial_order(&items);
        // top row first (the two y≈0 items), left before right within the row.
        assert_eq!(order, vec![1, 2, 3, 0]);
    }

    #[test]
    fn collapsed_surface_nodes_remain_window_previews() {
        let (kind, collapsed) = apogee_tile_class(&NodeKind::Surface, &NodeState::Node);

        assert_eq!(kind, ApogeeTileKind::Window);
        assert!(collapsed);
    }

    #[test]
    fn apogee_keyboard_navigation_initial_selection_uses_main_monitor_top_left_window() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());
        let now = Instant::now();
        let left_top_left = NodeId::new(1);
        let left_later = NodeId::new(2);
        let right_under_pointer = NodeId::new(3);
        install_test_apogee_session(
            &mut state,
            vec![
                ApogeeMonitorSession {
                    monitor: "right".to_string(),
                    core_scroll_offset: 0.0,
                    core_atlas_width: 800.0,
                    tiles: vec![test_apogee_tile(right_under_pointer, 120.0, 80.0)],
                    core_tiles: Vec::new(),
                },
                ApogeeMonitorSession {
                    monitor: "left".to_string(),
                    core_scroll_offset: 0.0,
                    core_atlas_width: 800.0,
                    tiles: vec![
                        test_apogee_tile(left_later, 110.0, 280.0),
                        test_apogee_tile(left_top_left, 90.0, 140.0),
                    ],
                    core_tiles: Vec::new(),
                },
            ],
            now,
        );
        state.input.interaction_state.cursor.last_screen_global = Some((920.0, 80.0));
        state.input.interaction_state.apogee_hover_node = None;

        assert_eq!(
            apogee_navigate(&state, halley_config::DirectionalAction::Right),
            Some(left_top_left)
        );
    }

    #[test]
    fn apogee_keyboard_navigation_moves_from_highlight_not_stale_pointer() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());
        let now = Instant::now();
        let selected = NodeId::new(1);
        let right_of_selected = NodeId::new(2);
        let stale_pointer_neighbor = NodeId::new(3);
        install_test_apogee_session(
            &mut state,
            vec![ApogeeMonitorSession {
                monitor: "left".to_string(),
                core_scroll_offset: 0.0,
                core_atlas_width: 800.0,
                tiles: vec![
                    test_apogee_tile(selected, 120.0, 220.0),
                    test_apogee_tile(right_of_selected, 330.0, 220.0),
                    test_apogee_tile(stale_pointer_neighbor, 720.0, 220.0),
                ],
                core_tiles: Vec::new(),
            }],
            now,
        );
        state.input.interaction_state.apogee_hover_node = Some(selected);
        state.input.interaction_state.cursor.last_screen_global = Some((690.0, 220.0));

        assert_eq!(
            apogee_navigate(&state, halley_config::DirectionalAction::Right),
            Some(right_of_selected)
        );
    }

    #[test]
    fn opening_apogee_creates_monitor_local_sessions_for_all_monitors() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, two_monitor_tuning());
        let left = state.model.field.spawn_surface(
            "left",
            Vec2 { x: 120.0, y: 120.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        let right = state.model.field.spawn_surface(
            "right",
            Vec2 { x: 920.0, y: 120.0 },
            Vec2 { x: 320.0, y: 220.0 },
        );
        state.assign_node_to_monitor(left, "left");
        state.assign_node_to_monitor(right, "right");

        state.open_apogee(Instant::now());
        let session = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("apogee session");
        assert!(session.monitor_session("left").is_some());
        assert!(session.monitor_session("right").is_some());
        assert!(session.monitor_session("left").is_some_and(|monitor| {
            monitor.tiles.iter().any(|tile| tile.node_id == left)
                && monitor.tiles.iter().all(|tile| tile.node_id != right)
        }));
        assert!(session.monitor_session("right").is_some_and(|monitor| {
            monitor.tiles.iter().any(|tile| tile.node_id == right)
                && monitor.tiles.iter().all(|tile| tile.node_id != left)
        }));
    }

    #[test]
    fn apogee_activation_raises_visible_target_above_fullscreen() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let now = Instant::now();
        let monitor = state.model.monitor_state.current_monitor.clone();
        let fullscreen = state.model.field.spawn_surface(
            "fullscreen",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(fullscreen, monitor.as_str());
        state.assign_node_to_monitor(target, monitor.as_str());
        state.enter_xdg_fullscreen(fullscreen, None, now);

        activate_apogee_target(&mut state, target, now);

        // Selecting a window in Apogee raises it above the fullscreen but does NOT
        // exit the fullscreen — only an explicit fullscreen/maximize action on the
        // target triggers mutual exclusivity.
        assert!(state.is_fullscreen_active(fullscreen));
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
        assert!(state.node_draws_above_fullscreen_on_monitor(target, monitor.as_str()));
    }

    #[test]
    fn select_apogee_target_defers_activation_until_close_completes() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let now = Instant::now();
        let monitor = state.model.monitor_state.current_monitor.clone();
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, monitor.as_str());
        let _ = state
            .model
            .field
            .set_state(target, halley_core::field::NodeState::Active);

        state.open_apogee(now);

        // Selection should start close but not activate immediately.
        select_apogee_target(&mut state, target, now);

        let session = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("apogee session still alive during close");
        assert!(matches!(session.phase, ApogeePhase::Closing));
        assert_eq!(session.pending_selection, Some(target));
        // Activation is deferred — focus must NOT have changed yet.
        assert_ne!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
    }

    #[test]
    fn tick_apogee_applies_pending_selection_on_close_completion() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, halley_config::RuntimeTuning::default());
        let now = Instant::now();
        let monitor = state.model.monitor_state.current_monitor.clone();
        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, monitor.as_str());
        let _ = state
            .model
            .field
            .set_state(target, halley_core::field::NodeState::Active);

        state.open_apogee(now);
        select_apogee_target(&mut state, target, now);

        // Advance time well past the close animation duration.
        let close_duration = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("session")
            .duration;
        let later = now + close_duration + std::time::Duration::from_millis(50);

        state.tick_apogee(later);

        // Session is cleared and activation has been applied.
        assert!(state.input.interaction_state.apogee_session.is_none());
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
    }

    #[test]
    fn cluster_cores_remain_marker_tiles() {
        let (kind, collapsed) = apogee_tile_class(&NodeKind::Core, &NodeState::Core);

        assert_eq!(kind, ApogeeTileKind::Core);
        assert!(!collapsed);
    }

    #[test]
    fn apogee_shows_field_view_when_cluster_workspace_is_open() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        // A standalone field window that the workspace will detach (hide).
        let loner = state.model.field.spawn_surface(
            "loner",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(loner, "monitor_a");

        // A two-member cluster, collapsed to a core, then opened as a workspace.
        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 560.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));

        state.open_apogee(now);
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );

        let session = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("apogee session");
        assert!(session.collapsed_clusters.iter().any(|cluster| {
            cluster.cluster_id == cid && cluster.core_id == core && cluster.monitor == "monitor_a"
        }));
        let monitor = session
            .monitor_session("monitor_a")
            .expect("monitor session");

        // The opened cluster's members are NOT surfaced as window tiles — it is
        // shown collapsed, as a single core icon in the core rail.
        assert!(
            monitor
                .tiles
                .iter()
                .all(|tile| tile.node_id != master && tile.node_id != stack)
        );
        assert!(monitor.core_tiles.iter().any(|tile| tile.node_id == core));

        // The detached standalone field window reappears at its field position:
        // Apogee renders the field overview, not the workspace.
        assert!(monitor.tiles.iter().any(|tile| tile.node_id == loner));
    }

    #[test]
    fn apogee_field_node_selection_leaves_opened_cluster_collapsed() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
        let now = Instant::now();

        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, "monitor_a");
        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 560.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));

        state.open_apogee(now);
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );
        select_apogee_target(&mut state, target, now);
        let close_duration = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("session")
            .duration;
        state.tick_apogee(now + close_duration + std::time::Duration::from_millis(50));

        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
    }

    #[test]
    fn apogee_core_selection_restores_cluster_collapsed_by_apogee() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
        let now = Instant::now();

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 560.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));

        state.open_apogee(now);
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );
        select_apogee_target(&mut state, core, now);
        let close_duration = state
            .input
            .interaction_state
            .apogee_session
            .as_ref()
            .expect("session")
            .duration;
        state.tick_apogee(now + close_duration + std::time::Duration::from_millis(50));

        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );
    }

    #[test]
    fn apogee_open_retargets_cluster_close_ghosts_to_apogee_core_tile() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut state = Halley::new_for_test(&dh, single_monitor_tiling_tuning());
        let now = Instant::now();
        let member = state.model.field.spawn_surface(
            "member",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let sibling = state.model.field.spawn_surface(
            "sibling",
            Vec2 { x: 560.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(member, "monitor_a");
        state.assign_node_to_monitor(sibling, "monitor_a");
        let cid = state
            .create_cluster(vec![member, sibling])
            .expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        state.ui.render_state.start_closing_window_animation(
            member,
            "monitor_a",
            now,
            500,
            halley_config::WindowCloseAnimationStyle::Shrink,
            vec![crate::window::ActiveBorderRect {
                x: 10,
                y: 20,
                w: 100,
                h: 80,
                inner_offset_x: 2.0,
                inner_offset_y: 2.0,
                inner_w: 96.0,
                inner_h: 76.0,
                alpha: 1.0,
                border_px: 2.0,
                corner_radius: 0.0,
                inner_corner_radius: 0.0,
                border_color: smithay::backend::renderer::Color32F::new(1.0, 1.0, 1.0, 1.0),
            }],
            Vec::new(),
            1.0,
            1.0,
            crate::window::CloseAnimationLayer::Top,
            Some((1.0, 2.0)),
            state.model.viewport.center,
        );
        let monitor_session = ApogeeMonitorSession {
            monitor: "monitor_a".to_string(),
            core_scroll_offset: 0.0,
            core_atlas_width: 800.0,
            tiles: Vec::new(),
            core_tiles: vec![ApogeeTile {
                node_id: core,
                kind: ApogeeTileKind::Core,
                collapsed: false,
                from: TileRect {
                    cx: 0.0,
                    cy: 0.0,
                    w: 48.0,
                    h: 48.0,
                },
                to: TileRect {
                    cx: 420.0,
                    cy: 72.0,
                    w: 68.0,
                    h: 68.0,
                },
            }],
        };
        let expected = apogee_project_core_rect(
            monitor_session.core_tiles[0].to,
            monitor_session.core_scroll_offset,
            monitor_session.core_atlas_width,
            800,
            600,
        )
        .rect;
        let collapsed = ApogeeCollapsedCluster {
            monitor: "monitor_a".to_string(),
            cluster_id: cid,
            core_id: core,
            members: vec![member],
        };

        retarget_apogee_collapsed_cluster_ghosts(&mut state, &[monitor_session], &[collapsed]);

        let anim = state
            .ui
            .render_state
            .window_animations
            .closing_window_animations
            .get(&member)
            .expect("closing animation");
        let crate::render::state::ClosingWindowAnimationKind::Window { pull_to, .. } = &anim.kind
        else {
            panic!("expected window animation");
        };
        assert_eq!(*pull_to, Some((expected.cx, expected.cy)));
    }

    #[test]
    fn apogee_activation_collapses_open_cluster_workspace_for_surface_target() {
        let dh = smithay::reexports::wayland_server::Display::<Halley>::new()
            .expect("display")
            .handle();
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        tuning.tty_viewports = vec![halley_config::ViewportOutputConfig {
            connector: "monitor_a".to_string(),
            enabled: true,
            offset_x: 0,
            offset_y: 0,
            width: 800,
            height: 600,
            refresh_rate: None,
            transform_degrees: 0,
            vrr: halley_config::ViewportVrrMode::Off,
            focus_ring: None,
        }];
        let mut state = Halley::new_for_test(&dh, tuning);
        let now = Instant::now();

        let target = state.model.field.spawn_surface(
            "target",
            Vec2 { x: 100.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(target, "monitor_a");

        let master = state.model.field.spawn_surface(
            "master",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        let stack = state.model.field.spawn_surface(
            "stack",
            Vec2 { x: 560.0, y: 100.0 },
            Vec2 { x: 320.0, y: 240.0 },
        );
        state.assign_node_to_monitor(master, "monitor_a");
        state.assign_node_to_monitor(stack, "monitor_a");
        let cid = state.create_cluster(vec![master, stack]).expect("cluster");
        let core = state.collapse_cluster(cid).expect("core");
        state.assign_node_to_monitor(core, "monitor_a");
        assert!(state.enter_cluster_workspace_by_core(core, "monitor_a", now));
        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            Some(cid)
        );

        activate_apogee_target(&mut state, target, now);

        assert_eq!(
            state.active_cluster_workspace_for_monitor("monitor_a"),
            None
        );
        assert_eq!(
            state.model.focus_state.primary_interaction_focus,
            Some(target)
        );
    }

    #[test]
    fn core_rail_uses_round_field_marker_size() {
        let slots = layout_core_rail(4, 1920, 1080);

        assert_eq!(slots.len(), 4);
        for slot in slots {
            assert_eq!(slot.w, 68.0);
            assert_eq!(slot.h, 68.0);
            assert!(slot.cy < 1080.0 * 0.20);
            assert!(slot.cx - slot.w * 0.5 >= 0.0);
            assert!(slot.cx + slot.w * 0.5 <= 1920.0);
        }
    }

    #[test]
    fn core_rail_is_flat_and_scrolls_without_wrapping() {
        let slots = layout_core_rail(24, 1920, 1080);
        let width = core_bar_width_for_slots(&slots, 1920);
        let slot = slots[12];
        let first = apogee_project_core_rect(slot, 0.0, width, 1920, 1080);
        let shifted = apogee_project_core_rect(slot, 120.0, width, 1920, 1080);
        let max_shifted = apogee_project_core_rect(slot, width * 4.0, width, 1920, 1080);

        assert_eq!(first.rect.cy, shifted.rect.cy);
        assert!(shifted.rect.cx < first.rect.cx);
        assert!(max_shifted.rect.cx <= shifted.rect.cx);
    }

    #[test]
    fn apogee_pointer_regions_split_core_bar_from_window_ring() {
        assert_eq!(
            apogee_region_for_point(1080, 80.0),
            ApogeeInteractionRegion::CoreBar
        );
        assert_eq!(
            apogee_region_for_point(1080, 420.0),
            ApogeeInteractionRegion::WindowRing
        );
    }

    #[test]
    fn window_projection_is_flat_destination_rect() {
        let slot = TileRect {
            cx: 960.0,
            cy: 540.0,
            w: 420.0,
            h: 240.0,
        };
        let projection = apogee_project_window_rect(slot);

        assert_eq!(projection.rect.cx, slot.cx);
        assert_eq!(projection.rect.cy, slot.cy);
        assert_eq!(projection.rect.w, slot.w);
        assert_eq!(projection.rect.h, slot.h);
    }

    #[test]
    fn window_mosaic_can_be_reserved_below_core_bar() {
        let items: Vec<ApogeeLayoutItem> = (0..7)
            .map(|i| item((i % 3) as f32 * 500.0, (i / 3) as f32 * 500.0))
            .collect();
        let core_bar_h = apogee_core_bar_height(1080);
        let mut slots = layout_mosaic(&items, 1920, (1080.0 - core_bar_h) as i32, 24.0, 3);
        for slot in &mut slots {
            slot.cy += core_bar_h;
        }

        assert_eq!(slots.len(), 7);
        for slot in slots {
            assert!(slot.cy - slot.h * 0.5 >= core_bar_h - 0.5);
        }
    }

    #[test]
    fn flat_mosaic_does_not_exceed_viewport_width() {
        let items = vec![
            item_aspect(0.0, 0.0, 2.8),
            item_aspect(500.0, 0.0, 0.55),
            item_aspect(1000.0, 0.0, 1.6),
            item_aspect(1500.0, 0.0, 2.2),
            item_aspect(0.0, 500.0, 0.75),
            item_aspect(500.0, 500.0, 1.9),
            item_aspect(1000.0, 500.0, 1.2),
            item_aspect(1500.0, 500.0, 2.5),
        ];
        let slots = layout_mosaic(&items, 1920, 900, 24.0, 3);

        assert!(packed_width(&slots) <= 1920.0 - 64.0 + 1.0);
    }

    #[test]
    fn flat_packing_width_candidates_never_exceed_viewport() {
        let sizes = vec![
            MosaicSize { w: 420.0, h: 180.0 },
            MosaicSize { w: 240.0, h: 320.0 },
            MosaicSize { w: 360.0, h: 220.0 },
        ];
        let widths = packing_widths(&sizes, 3, 1200.0, 700.0, 24.0);

        assert!(widths.iter().all(|width| *width <= 1200.0));
    }

    #[test]
    fn smaller_field_y_lands_in_an_earlier_row() {
        let items = vec![item(0.0, 2000.0), item(0.0, 0.0)];
        let slots = layout_mosaic(&items, 1920, 1080, 24.0, 3);
        // item[1] (y=0) should sit at or above item[0] (y=2000).
        assert!(slots[1].cy <= slots[0].cy);
    }

    #[test]
    fn markers_stay_compact_in_the_mosaic() {
        let items = vec![item(0.0, 0.0), marker(500.0, 0.0), item(1000.0, 0.0)];
        let slots = layout_mosaic(&items, 1920, 1080, 24.0, 3);

        assert!(slots[1].w <= 96.0);
        assert!(slots[1].h <= 96.0);
        assert!(slots[1].w < slots[0].w * 0.5);
        assert!(slots[1].w < slots[2].w * 0.5);
    }

    #[test]
    fn packed_tiles_keep_configured_gap() {
        let gap = 24.0;
        let items = vec![
            item_aspect(0.0, 0.0, 2.4),
            item_aspect(500.0, 0.0, 0.8),
            item_aspect(1000.0, 0.0, 1.6),
            item_aspect(1500.0, 0.0, 1.1),
            marker(2000.0, 0.0),
        ];
        let slots = layout_mosaic(&items, 1920, 1080, gap, 3);

        for (i, a) in slots.iter().enumerate() {
            for b in slots.iter().skip(i + 1) {
                let separated_x = a.cx + a.w * 0.5 + gap <= b.cx - b.w * 0.5 + 0.5
                    || b.cx + b.w * 0.5 + gap <= a.cx - a.w * 0.5 + 0.5;
                let separated_y = a.cy + a.h * 0.5 + gap <= b.cy - b.h * 0.5 + 0.5
                    || b.cy + b.h * 0.5 + gap <= a.cy - a.h * 0.5 + 0.5;

                assert!(separated_x || separated_y);
            }
        }
    }

    #[test]
    fn several_windows_prefer_a_horizontal_strip() {
        let items = vec![
            item(0.0, 0.0),
            item(500.0, 0.0),
            item(1000.0, 0.0),
            item(1500.0, 0.0),
        ];
        let slots = layout_mosaic(&items, 1920, 1080, 24.0, 3);
        let min_y = slots.iter().map(|slot| slot.cy).fold(f32::MAX, f32::min);
        let max_y = slots.iter().map(|slot| slot.cy).fold(f32::MIN, f32::max);
        let avg_h = slots.iter().map(|slot| slot.h).sum::<f32>() / slots.len() as f32;

        assert!(max_y - min_y <= avg_h * 1.20);
    }

    #[test]
    fn single_window_mosaic_stays_readable_not_fullscreen() {
        let items = vec![item(0.0, 0.0)];
        let slots = layout_mosaic(&items, 1920, 880, 24.0, 3);

        assert_eq!(slots.len(), 1);
        assert!(slots[0].w <= 1920.0 * 0.62 + 1.0);
        assert!(slots[0].h <= 880.0 * 0.56 + 1.0);
        assert_eq!(slots[0].cx, 960.0);
        assert_eq!(slots[0].cy, 440.0);
    }

    #[test]
    fn mixed_shapes_prefer_a_horizontal_atlas() {
        let items = vec![
            item_aspect(0.0, 0.0, 2.8),
            item_aspect(500.0, 0.0, 0.55),
            item_aspect(1000.0, 0.0, 1.6),
            item_aspect(1500.0, 0.0, 2.2),
            item_aspect(0.0, 500.0, 0.75),
            marker(500.0, 500.0),
            marker(1000.0, 500.0),
        ];
        let slots = layout_mosaic(&items, 1920, 1080, 24.0, 3);
        let min_x = slots
            .iter()
            .map(|slot| slot.cx - slot.w * 0.5)
            .fold(f32::MAX, f32::min);
        let max_x = slots
            .iter()
            .map(|slot| slot.cx + slot.w * 0.5)
            .fold(f32::MIN, f32::max);
        let min_y = slots
            .iter()
            .map(|slot| slot.cy - slot.h * 0.5)
            .fold(f32::MAX, f32::min);
        let max_y = slots
            .iter()
            .map(|slot| slot.cy + slot.h * 0.5)
            .fold(f32::MIN, f32::max);

        assert!((max_x - min_x) / (max_y - min_y) > 1.2);
    }

    #[test]
    fn adding_markers_does_not_force_a_new_tall_band() {
        let windows = vec![
            item_aspect(0.0, 0.0, 2.6),
            item_aspect(500.0, 0.0, 1.4),
            item_aspect(1000.0, 0.0, 0.7),
            item_aspect(1500.0, 0.0, 2.1),
        ];
        let mut with_markers = windows.clone();
        with_markers.push(marker(500.0, 500.0));
        with_markers.push(marker(1000.0, 500.0));

        let window_slots = layout_mosaic(&windows, 1920, 1080, 24.0, 3);
        let marker_slots = layout_mosaic(&with_markers, 1920, 1080, 24.0, 3);
        let window_h = packed_height(&window_slots);
        let marker_h = packed_height(&marker_slots);

        assert!(marker_h <= window_h + 140.0);
    }

    fn packed_height(slots: &[TileRect]) -> f32 {
        let min_y = slots
            .iter()
            .map(|slot| slot.cy - slot.h * 0.5)
            .fold(f32::MAX, f32::min);
        let max_y = slots
            .iter()
            .map(|slot| slot.cy + slot.h * 0.5)
            .fold(f32::MIN, f32::max);
        max_y - min_y
    }

    fn packed_width(slots: &[TileRect]) -> f32 {
        let min_x = slots
            .iter()
            .map(|slot| slot.cx - slot.w * 0.5)
            .fold(f32::MAX, f32::min);
        let max_x = slots
            .iter()
            .map(|slot| slot.cx + slot.w * 0.5)
            .fold(f32::MIN, f32::max);
        max_x - min_x
    }
}
