use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use halley_core::decay::{DecayClass, DecayTracker, TimedDecayPolicy};
use halley_core::field::{Field, NodeId, NodeState, Vec2};
use halley_core::viewport::FocusRing as CoreFocusRing;
use smithay::desktop::{PopupManager, Space, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shell::xdg::dialog::ToplevelDialogHint;

mod dynamics;
pub(crate) mod ipc;
mod keyboard_navigation;
mod movement;
mod session_ops;
pub(crate) use keyboard_navigation::{pan_field, transfer_window};

pub use ipc::handle_request;
pub(crate) use ipc::move_selected_direction;
pub(crate) use ipc::resize_selected_direction;
#[cfg(test)]
use session_ops::{
    centered_rect, landmark_reveal_delta, minimal_reveal_delta, physics_frame_delta,
};
pub use session_ops::{
    close, close_focused_on_output, collapse, focus_and_center_node, focus_cycle_node,
    focus_or_reveal_node, pan_after_close_restore, reconcile_landmarks, restore, restore_for_close,
    reveal_cluster_core, reveal_collapsed_node, tick_decay, toggle_focused_on_output,
};
pub(crate) use session_ops::{
    displace_landmarks_for_new_window, move_cluster_core_rigid, move_grabbed_body_rigid,
    reconcile_landmarks_for_zoom, resolve_new_cluster_core, restore_for_cluster_join,
    restore_landmarks_for_zoom, set_collapsed_output, tick_physics,
};

const OUTSIDE_THRESHOLD: f32 = 0.90;
pub const NODE_DIAMETER_PX: f32 = 51.0;

#[derive(Clone, Copy)]
struct PlacementContext<'a> {
    decorations: &'a halley_config::Decorations,
    font: &'a halley_config::Font,
    fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
}

impl PlacementContext<'_> {
    /// Presentation geometry is output-owned, not an obstacle in the Field.
    /// Pending entry/exit handoffs remain excluded as well.
    fn includes_window(self, surface: &WlSurface) -> bool {
        !self.fullscreen.is_fullscreen_or_pending(surface)
            && !self.maximize.is_maximized_or_pending(surface)
    }
}
pub const HOVER_PREVIEW_DWELL_MS: u64 = 1_500;
const LANDMARK_SLIDE_MS: u64 = 520;
const RELEASE_LOCK_MS: u64 = 350;

fn release_lock_deadline(now: Duration) -> Duration {
    now.saturating_add(Duration::from_millis(RELEASE_LOCK_MS))
}

fn release_lock_is_active(until: Duration, now: Duration) -> bool {
    now < until
}

fn hover_preview_ready(started_at: Option<Duration>, now: Duration) -> bool {
    started_at.is_some_and(|started_at| {
        now.saturating_sub(started_at) >= Duration::from_millis(HOVER_PREVIEW_DWELL_MS)
    })
}

fn logical_focus_after_collapse(
    focused: Option<NodeId>,
    collapsed: NodeId,
    client_was_focused: bool,
) -> Option<NodeId> {
    if client_was_focused || focused == Some(collapsed) {
        Some(collapsed)
    } else {
        focused
    }
}

fn participates_in_decay(collapsed: bool, attached: bool, cluster_member: bool) -> bool {
    attached && !collapsed && !cluster_member
}

#[derive(Clone, Copy)]
struct LandmarkSlide {
    from: Vec2,
    to: Vec2,
    started: Duration,
}

#[derive(Clone, Copy, Debug, Default)]
struct HoverPreviewState {
    node: Option<NodeId>,
    mix: f32,
}

fn advance_hover_preview(
    state: &mut HoverPreviewState,
    target: Option<NodeId>,
) -> Option<(NodeId, f32)> {
    if target.is_some() && target != state.node {
        state.node = target;
        state.mix = 0.0;
    }
    let target_mix = if target.is_some() { 1.0 } else { 0.0 };
    let rate = if target_mix > 0.5 { 0.30 } else { 0.14 };
    state.mix += (target_mix - state.mix) * rate;
    if (state.mix - target_mix).abs() < 0.002 {
        state.mix = target_mix;
    }
    if target.is_none() && state.mix <= 0.002 {
        state.mix = 0.0;
        state.node = None;
    }
    state.node.map(|id| (id, state.mix))
}

#[derive(Clone)]
pub struct NodeRecord {
    pub id: NodeId,
    pub window: Window,
    pub surface: WlSurface,
    pub output: String,
    pub geometry: Rectangle<i32, Logical>,
    pub collapsed: bool,
    pub attached: bool,
    pub title: String,
    pub app_id: Option<String>,
    pub collapsed_at: Duration,
    pub collapsed_stack_index: Option<usize>,
}

pub struct NodesState {
    pub field: Field,
    records: HashMap<NodeId, NodeRecord>,
    by_surface: HashMap<WlSurface, NodeId>,
    decay: DecayTracker,
    pub config: halley_config::Nodes,
    configured_nodes: halley_config::Nodes,
    system_color_scheme: halley_config::SystemColorScheme,
    pub decay_config: halley_config::Decay,
    pub focus_rings: halley_config::FocusRings,
    pub landmarks: halley_config::LandmarkPlacement,
    pub physics: halley_config::Physics,
    pub animation: halley_config::NodeAnimation,
    pub animations_enabled: bool,
    pub hovered: Option<NodeId>,
    hover_started_at: Option<Duration>,
    focused: Option<NodeId>,
    last_focus_ms: HashMap<NodeId, u64>,
    label_hover: RefCell<HashMap<NodeId, f32>>,
    preview_hover: RefCell<HashMap<String, HoverPreviewState>>,
    landmark_slides: RefCell<HashMap<NodeId, LandmarkSlide>>,
    physics_velocity: HashMap<NodeId, Vec2>,
    physics_last_tick: Duration,
    release_locks: HashMap<NodeId, Duration>,
    ring_preview_until: HashMap<String, Duration>,
    /// Positions a landmark held before camera zoom displaced it. Zooming back
    /// in may return the landmark home; any physical or explicit move rebases
    /// the position permanently. Runtime-only: never configured, sent over IPC,
    /// or written into persisted cluster metadata.
    zoom_homes: HashMap<NodeId, Vec2>,
}

impl NodesState {
    pub fn new(config: &halley_config::RuntimeConfig) -> Self {
        Self::new_with_color_scheme(config, halley_config::SystemColorScheme::NoPreference)
    }

    pub fn new_with_color_scheme(
        config: &halley_config::RuntimeConfig,
        system_color_scheme: halley_config::SystemColorScheme,
    ) -> Self {
        Self {
            field: Field::new(),
            records: HashMap::new(),
            by_surface: HashMap::new(),
            decay: DecayTracker::default(),
            config: config.nodes.resolved_for_system(system_color_scheme),
            configured_nodes: config.nodes,
            system_color_scheme,
            decay_config: config.decay,
            focus_rings: config.focus_rings.clone(),
            landmarks: halley_config::LandmarkPlacement {
                gap_px: config.field.gap,
            },
            physics: config.physics,
            animation: config.animations.node,
            animations_enabled: config.animations.enabled,
            hovered: None,
            hover_started_at: None,
            focused: None,
            last_focus_ms: HashMap::new(),
            label_hover: RefCell::new(HashMap::new()),
            preview_hover: RefCell::new(HashMap::new()),
            landmark_slides: RefCell::new(HashMap::new()),
            physics_velocity: HashMap::new(),
            physics_last_tick: crate::frame_clock::monotonic_now(),
            release_locks: HashMap::new(),
            ring_preview_until: HashMap::new(),
            zoom_homes: HashMap::new(),
        }
    }

    pub fn reload(&mut self, config: &halley_config::RuntimeConfig, now: Duration) -> bool {
        let ring_changed = self.focus_rings != config.focus_rings;
        let configured_outputs = self
            .records
            .values()
            .map(|record| record.output.clone())
            .chain(self.focus_rings.by_output.keys().cloned())
            .chain(config.focus_rings.by_output.keys().cloned())
            .collect::<HashSet<_>>();
        let changed_ring_outputs = configured_outputs
            .into_iter()
            .filter(|output| {
                self.focus_rings.for_output(output) != config.focus_rings.for_output(output)
            })
            .collect::<HashSet<_>>();
        let resolved_nodes = config.nodes.resolved_for_system(self.system_color_scheme);
        let redraw = ring_changed
            || self.config != resolved_nodes
            || self.configured_nodes != config.nodes
            || self.animation != config.animations.node
            || self.animations_enabled != config.animations.enabled;
        if ring_changed {
            let until = now.saturating_add(Duration::from_millis(1_500));
            for output in &changed_ring_outputs {
                self.ring_preview_until.insert(output.clone(), until);
            }
        }
        let next_landmarks = halley_config::LandmarkPlacement {
            gap_px: config.field.gap,
        };
        let landmarks_changed = self.landmarks != next_landmarks;
        let physics_changed = self.physics != config.physics;
        if physics_changed {
            self.physics_velocity.clear();
            self.physics_last_tick = now;
            self.release_locks.clear();
        }
        if self.decay_config != config.decay {
            self.decay.reset();
        } else if ring_changed {
            let changed_ids = self
                .records
                .values()
                .filter(|record| changed_ring_outputs.contains(&record.output))
                .map(|record| record.id)
                .collect::<Vec<_>>();
            for id in changed_ids {
                self.decay.remove(id);
            }
        }
        self.config = resolved_nodes;
        self.configured_nodes = config.nodes;
        self.decay_config = config.decay;
        self.focus_rings = config.focus_rings.clone();
        self.landmarks = next_landmarks;
        self.physics = config.physics;
        self.animation = config.animations.node;
        self.animations_enabled = config.animations.enabled;
        redraw || landmarks_changed || physics_changed
    }

    pub fn set_system_color_scheme(&mut self, scheme: halley_config::SystemColorScheme) -> bool {
        if self.system_color_scheme == scheme {
            return false;
        }
        self.system_color_scheme = scheme;
        let nodes = self.configured_nodes.resolved_for_system(scheme);
        let changed = self.config != nodes;
        self.config = nodes;
        changed
    }

    pub fn focus_ring_for_output(&self, output: &str) -> halley_config::FocusRing {
        self.focus_rings.for_output(output)
    }

    pub fn ring_is_previewed(&self, output: &str, now: Duration) -> bool {
        self.ring_preview_until
            .get(output)
            .is_some_and(|until| now < *until)
    }

    pub fn register_mapped(&mut self, space: &Space<Window>, surface: &WlSurface, now_ms: u64) {
        let Some(window) = space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == surface)
            })
            .cloned()
        else {
            return;
        };
        if crate::xwayland::is_override_redirect(&window) {
            return;
        }
        let Some(geometry) = space.element_geometry(&window) else {
            return;
        };
        let output = crate::wayland::window_output_name(&window).unwrap_or_default();
        let (title, app_id) = metadata(&window);
        if let Some(id) = self.by_surface.get(surface).copied() {
            // A re-mapped node is placed from live client geometry, so a home
            // remembered from before the unmap no longer describes it.
            self.commit_zoom_home(id);
            if let Some(record) = self.records.get_mut(&id) {
                record.window = window;
                record.output = output;
                record.geometry = geometry;
                record.attached = true;
                record.title = title.clone();
                record.app_id = app_id;
            }
            let _ = self.field.set_detached(id, false);
            if let Some(node) = self.field.node_mut(id) {
                node.label = title;
                node.pos = rect_center(geometry);
                node.intrinsic_size = vec_size(geometry);
            }
            let _ = self.field.touch(id, now_ms);
            return;
        }

        let id = self
            .field
            .spawn_surface(title.clone(), rect_center(geometry), vec_size(geometry));
        let _ = self.field.touch(id, now_ms);
        self.by_surface.insert(surface.clone(), id);
        self.records.insert(
            id,
            NodeRecord {
                id,
                window,
                surface: surface.clone(),
                output,
                geometry,
                collapsed: false,
                attached: true,
                title,
                app_id,
                collapsed_at: Duration::ZERO,
                collapsed_stack_index: None,
            },
        );
    }

    pub fn sync_from_space(&mut self, space: &Space<Window>) {
        let ids = self.records.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let Some(record) = self.records.get_mut(&id) else {
                continue;
            };
            if !record.attached {
                continue;
            }
            let (title, app_id) = metadata(&record.window);
            record.title = title.clone();
            record.app_id = app_id;
            if let Some(node) = self.field.node_mut(id) {
                node.label = title;
            }
            if record.collapsed {
                continue;
            }
            let Some(geometry) = space.element_geometry(&record.window) else {
                continue;
            };
            record.geometry = geometry;
            record.output = crate::wayland::window_output_name(&record.window).unwrap_or_default();
            if let Some(node) = self.field.node_mut(id) {
                node.pos = rect_center(geometry);
                node.intrinsic_size = vec_size(geometry);
                if node.state == NodeState::Active {
                    node.footprint = node.intrinsic_size;
                }
            }
        }
    }

    /// XWayland can replace a window's wl_surface when its X11 frame is
    /// restored. The managed X11 window and its Field node are still the same.
    pub(crate) fn reassociate_window_surface(
        &mut self,
        window: &Window,
        surface: &WlSurface,
    ) -> Option<WlSurface> {
        let record = self
            .records
            .values_mut()
            .find(|record| &record.window == window)?;
        if record.surface == *surface {
            return None;
        }
        let previous = std::mem::replace(&mut record.surface, surface.clone());
        self.by_surface.remove(&previous);
        self.by_surface.insert(surface.clone(), record.id);
        Some(previous)
    }

    pub fn mark_detached(&mut self, surface: &WlSurface) {
        let Some(id) = self.by_surface.get(surface).copied() else {
            return;
        };
        if let Some(record) = self.records.get_mut(&id) {
            record.attached = false;
        }
        let _ = self.field.set_detached(id, true);
        self.release_locks.remove(&id);
        self.zoom_homes.remove(&id);
        self.decay.remove(id);
        if self.focused == Some(id) {
            self.focused = None;
        }
        if self.hovered == Some(id) {
            self.set_hovered(None, crate::frame_clock::monotonic_now());
        }
    }

    pub fn remove_surface(&mut self, surface: &WlSurface) -> Option<NodeRecord> {
        let id = self.by_surface.remove(surface)?;
        self.label_hover.borrow_mut().remove(&id);
        self.preview_hover
            .borrow_mut()
            .retain(|_, state| state.node != Some(id));
        self.landmark_slides.borrow_mut().remove(&id);
        self.physics_velocity.remove(&id);
        self.release_locks.remove(&id);
        self.zoom_homes.remove(&id);
        self.decay.remove(id);
        self.last_focus_ms.remove(&id);
        if self.focused == Some(id) {
            self.focused = None;
        }
        if self.hovered == Some(id) {
            self.hovered = None;
            self.hover_started_at = None;
        }
        self.field.remove(id);
        self.records.remove(&id)
    }

    pub fn id_for_surface(&self, surface: &WlSurface) -> Option<NodeId> {
        self.by_surface.get(surface).copied()
    }

    pub fn focused(&self) -> Option<NodeId> {
        self.focused.filter(|id| {
            self.records
                .get(id)
                .is_some_and(|record| record.attached && self.field.is_visible(*id))
                || self
                    .field
                    .node(*id)
                    .is_some_and(|node| node.state == NodeState::Core)
        })
    }

    pub fn focused_on_output(&self, output: &str) -> Option<NodeId> {
        self.focused()
            .filter(|id| {
                self.records
                    .get(id)
                    .is_some_and(|record| record.output == output)
            })
            .or_else(|| {
                self.last_focus_ms
                    .iter()
                    .filter_map(|(id, focused_at)| {
                        self.records
                            .get(id)
                            .filter(|record| {
                                record.attached
                                    && record.output == output
                                    && self.field.is_visible(*id)
                            })
                            .map(|_| (*focused_at, id.as_u64(), *id))
                    })
                    .max_by_key(|(focused_at, stable_id, _)| (*focused_at, *stable_id))
                    .map(|(_, _, id)| id)
            })
    }

    pub fn last_focus_ms(&self) -> &HashMap<NodeId, u64> {
        &self.last_focus_ms
    }

    pub fn focus(&mut self, id: Option<NodeId>, now_ms: u64) -> bool {
        let next = id.filter(|id| {
            self.records
                .get(id)
                .is_some_and(|record| record.attached && self.field.is_visible(*id))
                || self
                    .field
                    .node(*id)
                    .is_some_and(|node| node.state == NodeState::Core)
        });
        if self.focused == next {
            return false;
        }
        self.focused = next;
        if let Some(id) = next {
            self.last_focus_ms.insert(id, now_ms);
        }
        true
    }

    pub fn focus_surface(&mut self, surface: &WlSurface, now_ms: u64) -> bool {
        self.focus(self.id_for_surface(surface), now_ms)
    }

    pub fn record(&self, id: NodeId) -> Option<&NodeRecord> {
        self.records.get(&id)
    }

    pub fn record_mut(&mut self, id: NodeId) -> Option<&mut NodeRecord> {
        self.records.get_mut(&id)
    }

    pub fn records(&self) -> impl Iterator<Item = &NodeRecord> {
        self.records.values()
    }

    pub fn clear_direct_motion(&mut self, id: NodeId) {
        self.landmark_slides.borrow_mut().remove(&id);
        self.physics_velocity.remove(&id);
        self.release_locks.remove(&id);
        // Explicit interaction, pinning, and output changes make the displayed
        // landmark position authoritative instead of a reversible zoom offset.
        self.zoom_homes.remove(&id);
    }

    /// Records where a landmark sat before camera zoom displaced it. Later
    /// zoom-out steps reflow from the displayed position without replacing the
    /// home, so one zoom gesture always returns to the same place.
    pub fn remember_zoom_home(&mut self, id: NodeId, position: Vec2) {
        self.zoom_homes.entry(id).or_insert(position);
    }

    pub fn zoom_home(&self, id: NodeId) -> Option<Vec2> {
        self.zoom_homes.get(&id).copied()
    }

    /// Makes the current displayed position permanent by discarding the home a
    /// physical or user-driven move has superseded.
    pub fn commit_zoom_home(&mut self, id: NodeId) {
        self.zoom_homes.remove(&id);
    }

    /// Retires homes for landmarks that no longer exist in the Field, so a
    /// removed node or dissolved cluster core cannot be restored later.
    pub fn retire_zoom_homes_without_field_node(&mut self) {
        self.zoom_homes
            .retain(|id, _| self.field.node(*id).is_some());
    }

    pub fn lock_released_window(&mut self, id: NodeId, now: Duration) {
        if self
            .records
            .get(&id)
            .is_some_and(|record| record.attached && !record.collapsed)
        {
            self.release_locks.insert(id, release_lock_deadline(now));
        }
    }

    fn expire_release_locks(&mut self, now: Duration) -> bool {
        let before = self.release_locks.len();
        self.release_locks
            .retain(|_, until| release_lock_is_active(*until, now));
        self.release_locks.len() != before
    }

    fn release_locked(&self, id: NodeId) -> bool {
        self.release_locks.contains_key(&id)
    }

    pub fn has_physics_on_output(&self, output: &str, now: Duration) -> bool {
        self.physics.enabled
            && (self.physics_velocity.keys().any(|id| {
                self.records
                    .get(id)
                    .is_some_and(|record| record.output == output)
            }) || self.release_locks.iter().any(|(id, until)| {
                release_lock_is_active(*until, now)
                    && self
                        .records
                        .get(id)
                        .is_some_and(|record| record.output == output)
            }))
    }

    pub fn collapsed_on_output(&self, output: &str) -> impl Iterator<Item = &NodeRecord> {
        self.records.values().filter(move |record| {
            record.collapsed
                && record.attached
                && record.output == output
                && self.field.is_visible(record.id)
        })
    }

    pub fn set_hovered(&mut self, hovered: Option<NodeId>, now: Duration) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        self.hover_started_at = hovered.map(|_| now);
        true
    }

    pub fn clear_hover(&mut self, now: Duration) -> bool {
        self.set_hovered(None, now)
    }

    pub fn label_hovered_on_output(&self, output: &str, now: Duration) -> Option<NodeId> {
        let id = self.hovered?;
        self.records
            .get(&id)
            .filter(|record| record.collapsed && record.attached && record.output == output)?;
        (!hover_preview_ready(self.hover_started_at, now)).then_some(id)
    }

    pub fn preview_hovered_on_output(&self, output: &str, now: Duration) -> Option<NodeId> {
        let id = self.hovered?;
        self.records
            .get(&id)
            .filter(|record| record.collapsed && record.attached && record.output == output)?;
        hover_preview_ready(self.hover_started_at, now).then_some(id)
    }

    pub fn preview_hover_mix(&self, output: &str, now: Duration) -> Option<(NodeId, f32)> {
        let target = self.preview_hovered_on_output(output, now);
        let mut states = self.preview_hover.borrow_mut();
        let state = states.entry(output.to_string()).or_default();
        advance_hover_preview(state, target)
    }

    pub fn hover_preview_visible_on_output(&self, output: &str, now: Duration) -> bool {
        self.preview_hovered_on_output(output, now).is_some()
            || self
                .preview_hover
                .borrow()
                .get(output)
                .is_some_and(|state| state.mix > 0.002)
    }

    pub fn is_animating_on_output(&self, output: &str, now: Duration) -> bool {
        let node_transition = self.animations_enabled
            && self.animation.enabled
            && self.animation.duration_ms > 0
            && self.collapsed_on_output(output).any(|record| {
                now.saturating_sub(record.collapsed_at)
                    < Duration::from_millis(u64::from(self.animation.duration_ms))
            });
        let states = self.label_hover.borrow();
        let hover_transition = self.config.show_labels == halley_config::NodeDisplayPolicy::Hover
            && self.collapsed_on_output(output).any(|record| {
                let mix = states.get(&record.id).copied().unwrap_or(0.0);
                let target = if self.label_hovered_on_output(output, now) == Some(record.id) {
                    1.0
                } else {
                    0.0
                };
                (mix - target).abs() > 0.002
            });
        let dwell_transition = self
            .hovered
            .and_then(|id| self.records.get(&id))
            .is_some_and(|record| {
                record.collapsed
                    && record.attached
                    && record.output == output
                    && !hover_preview_ready(self.hover_started_at, now)
            });
        let preview_target = self.preview_hovered_on_output(output, now);
        let preview_transition = self
            .preview_hover
            .borrow()
            .get(output)
            .is_some_and(|state| {
                if preview_target.is_some() && preview_target != state.node {
                    true
                } else {
                    let target = if preview_target.is_some() { 1.0 } else { 0.0 };
                    (state.mix - target).abs() > 0.002
                }
            })
            || (preview_target.is_some() && !self.preview_hover.borrow().contains_key(output));
        let icon_transition = self
            .collapsed_on_output(output)
            .any(|record| now.saturating_sub(record.collapsed_at) < Duration::from_millis(1_220));
        // Cluster cores share the landmark slide track even though they do not
        // have a `NodeRecord`. A slide is short, so redrawing every output while
        // any landmark is moving is both cheap and keeps core motion alive.
        let slide_transition = self.landmark_slides.borrow().values().any(|slide| {
            now.saturating_sub(slide.started) < Duration::from_millis(LANDMARK_SLIDE_MS)
        });
        node_transition
            || hover_transition
            || dwell_transition
            || preview_transition
            || icon_transition
            || slide_transition
            || self.has_physics_on_output(output, now)
    }

    pub fn label_hover_mix(&self, id: NodeId, highlighted: bool) -> f32 {
        let mut states = self.label_hover.borrow_mut();
        let mix = states.entry(id).or_insert(0.0);
        let target = if highlighted { 1.0 } else { 0.0 };
        let rate = if highlighted { 0.06 } else { 0.10 };
        *mix += (target - *mix) * rate;
        if (*mix - target).abs() < 0.002 {
            *mix = target;
        }
        *mix
    }

    pub fn landmark_position(&self, id: NodeId, target: Vec2, now: Duration) -> Vec2 {
        let mut slides = self.landmark_slides.borrow_mut();
        let Some(slide) = slides.get(&id).copied() else {
            return target;
        };
        let t =
            now.saturating_sub(slide.started).as_secs_f32() / (LANDMARK_SLIDE_MS as f32 / 1_000.0);
        if t >= 1.0 {
            slides.remove(&id);
            return target;
        }
        let denominator = 1.0 - 6.0 * (-5.0_f32).exp();
        let damped = ((1.0 - (1.0 + 5.0 * t) * (-5.0 * t).exp()) / denominator).clamp(0.0, 1.0);
        Vec2 {
            x: slide.from.x + (slide.to.x - slide.from.x) * damped,
            y: slide.from.y + (slide.to.y - slide.from.y) * damped,
        }
    }

    fn start_landmark_slide(&self, id: NodeId, from: Vec2, to: Vec2, now: Duration) {
        if from == to || !self.animations_enabled || !self.animation.enabled {
            self.landmark_slides.borrow_mut().remove(&id);
        } else {
            self.landmark_slides.borrow_mut().insert(
                id,
                LandmarkSlide {
                    from,
                    to,
                    started: now,
                },
            );
        }
    }

    pub fn hit_test(
        &self,
        output: &Output,
        output_geometry: Rectangle<i32, Logical>,
        camera: &halley_core::camera::Camera,
        screen: Point<f64, Logical>,
    ) -> Option<NodeId> {
        self.collapsed_on_output(&output.name())
            .filter_map(|record| {
                let node = self.field.node(record.id)?;
                let position = self.landmark_position(
                    record.id,
                    node.pos,
                    crate::frame_clock::monotonic_now(),
                );
                let center = screen_from_world(position, camera, output_geometry);
                let rect = marker_rect(
                    center,
                    &node.label,
                    self.config.show_labels,
                    self.hovered == Some(record.id),
                );
                rect.to_f64().contains(screen).then_some(record.id)
            })
            .max_by_key(|id| id.as_u64())
    }

    pub fn decay_candidates(
        &mut self,
        camera_centers: &HashMap<String, Vec2>,
        focused: Option<&WlSurface>,
        excluded: impl Fn(NodeId) -> bool,
        protected: impl Fn(&WlSurface) -> bool,
        now_ms: u64,
    ) -> Vec<NodeId> {
        if !self.decay_config.enabled {
            self.decay.reset();
            return Vec::new();
        }
        let policy = TimedDecayPolicy {
            inside_ms: self.decay_config.inside_delay_seconds.saturating_mul(1_000),
            outside_ms: self
                .decay_config
                .outside_delay_seconds
                .saturating_mul(1_000),
        };
        let records = self.records.values().cloned().collect::<Vec<_>>();
        let mut ready = Vec::new();
        for record in records {
            if !participates_in_decay(record.collapsed, record.attached, excluded(record.id)) {
                self.decay.remove(record.id);
                continue;
            }
            let class = if focused == Some(&record.surface) || protected(&record.surface) {
                DecayClass::Protected
            } else {
                let configured_ring = self.focus_rings.for_output(&record.output);
                let ring = CoreFocusRing::new(
                    configured_ring.radius_x,
                    configured_ring.radius_y,
                    configured_ring.offset_x,
                    configured_ring.offset_y,
                );
                let center = camera_centers
                    .get(&record.output)
                    .copied()
                    .unwrap_or(Vec2 { x: 0.0, y: 0.0 });
                let node = self.field.node(record.id).expect("record has field node");
                if ring.outside_fraction(center, node.pos, node.intrinsic_size) >= OUTSIDE_THRESHOLD
                {
                    DecayClass::OutsideRing
                } else {
                    DecayClass::InsideRing
                }
            };
            if self
                .decay
                .update(record.id, class, now_ms, policy)
                .is_some()
            {
                ready.push(record.id);
            }
        }
        ready
    }

    pub fn set_collapsed(&mut self, id: NodeId, collapsed: bool, now_ms: u64) -> bool {
        let Some(record) = self.records.get_mut(&id) else {
            return false;
        };
        record.collapsed = collapsed;
        self.decay.remove(id);
        self.release_locks.remove(&id);
        if collapsed {
            self.field.set_state(id, NodeState::Node)
        } else {
            self.landmark_slides.borrow_mut().remove(&id);
            self.field.touch(id, now_ms)
        }
    }

    fn nearest_free_position(
        &self,
        id: NodeId,
        desired: Vec2,
        scale: f32,
        occupied_cores: &[Vec2],
        chrome: PlacementContext<'_>,
    ) -> Vec2 {
        let output = self
            .record(id)
            .map(|record| record.output.as_str())
            .unwrap_or_default();
        let scale = scale.max(0.05);
        let node_spacing = (NODE_DIAMETER_PX + self.landmarks.gap_px) / scale;
        let window_clearance = (NODE_DIAMETER_PX * 0.5 + self.landmarks.gap_px) / scale;
        let mut occupied_nodes = self
            .records
            .values()
            .filter(|record| {
                record.id != id && record.collapsed && record.attached && record.output == output
            })
            .filter_map(|record| self.field.node(record.id).map(|node| node.pos))
            .collect::<Vec<_>>();
        occupied_nodes.extend_from_slice(occupied_cores);
        let occupied_windows = self
            .records
            .values()
            .filter(|record| {
                record.id != id
                    && !record.collapsed
                    && record.attached
                    && record.output == output
                    && chrome.includes_window(&record.surface)
            })
            .map(|record| {
                crate::titlebar::outer_rect_for_client(
                    &record.window,
                    record.geometry,
                    chrome.decorations,
                    chrome.font,
                )
            })
            .collect::<Vec<_>>();
        nearest_free_landmark(
            desired,
            &occupied_nodes,
            &occupied_windows,
            node_spacing,
            window_clearance,
        )
    }

    fn nearest_free_active_rect(
        &self,
        id: NodeId,
        desired: Rectangle<i32, Logical>,
        output: &str,
        scale: f32,
        occupied_cores: &[Vec2],
        chrome: PlacementContext<'_>,
    ) -> Rectangle<i32, Logical> {
        let clearance = (NODE_DIAMETER_PX * 0.5 + self.landmarks.gap_px) / scale.max(0.05);
        let mut nodes = self
            .records
            .values()
            .filter(|record| {
                record.id != id && record.collapsed && record.attached && record.output == output
            })
            .filter_map(|record| self.field.node(record.id).map(|node| node.pos))
            .collect::<Vec<_>>();
        nodes.extend_from_slice(occupied_cores);
        let Some(record) = self.record(id) else {
            return desired;
        };
        let outer = crate::titlebar::outer_rect_for_client(
            &record.window,
            desired,
            chrome.decorations,
            chrome.font,
        );
        let placed = nearest_free_window_rect(outer, &nodes, clearance);
        Rectangle::new(desired.loc + (placed.loc - outer.loc), desired.size)
    }
}

fn nearest_free_landmark(
    desired: Vec2,
    occupied_nodes: &[Vec2],
    occupied_windows: &[Rectangle<i32, Logical>],
    node_spacing: f32,
    window_clearance: f32,
) -> Vec2 {
    let free = |candidate: Vec2| {
        occupied_nodes.iter().all(|other| {
            let dx = candidate.x - other.x;
            let dy = candidate.y - other.y;
            dx * dx + dy * dy >= node_spacing * node_spacing
        }) && occupied_windows
            .iter()
            .all(|rect| !circle_intersects_rect(candidate, window_clearance, *rect))
    };
    if free(desired) {
        return desired;
    }
    for radius in 1_i32.. {
        for x in -radius..=radius {
            for y in -radius..=radius {
                if x.abs() != radius && y.abs() != radius {
                    continue;
                }
                let candidate = Vec2 {
                    x: desired.x + x as f32 * node_spacing,
                    y: desired.y + y as f32 * node_spacing,
                };
                if free(candidate) {
                    return candidate;
                }
            }
        }
    }
    unreachable!("unbounded landmark search must eventually find free space")
}

fn circle_intersects_rect(center: Vec2, radius: f32, rect: Rectangle<i32, Logical>) -> bool {
    let min_x = rect.loc.x as f32;
    let min_y = rect.loc.y as f32;
    let max_x = min_x + rect.size.w as f32;
    let max_y = min_y + rect.size.h as f32;
    let nearest_x = center.x.clamp(min_x, max_x);
    let nearest_y = center.y.clamp(min_y, max_y);
    let dx = center.x - nearest_x;
    let dy = center.y - nearest_y;
    dx * dx + dy * dy < radius * radius
}

fn nearest_free_window_rect(
    desired: Rectangle<i32, Logical>,
    nodes: &[Vec2],
    clearance: f32,
) -> Rectangle<i32, Logical> {
    let free = |candidate: Rectangle<i32, Logical>| {
        nodes
            .iter()
            .all(|node| !circle_intersects_rect(*node, clearance, candidate))
    };
    if free(desired) {
        return desired;
    }
    let step = (clearance * 2.0).round().max(1.0) as i32;
    for radius in 1_i32.. {
        for x in -radius..=radius {
            for y in -radius..=radius {
                if x.abs() != radius && y.abs() != radius {
                    continue;
                }
                let candidate = Rectangle::new(
                    desired.loc + Point::<i32, Logical>::from((x * step, y * step)),
                    desired.size,
                );
                if free(candidate) {
                    return candidate;
                }
            }
        }
    }
    unreachable!("unbounded active-window search must eventually find free space")
}

pub fn marker_rect(
    center: Point<i32, Logical>,
    _label: &str,
    _labels: halley_config::NodeDisplayPolicy,
    _hovered: bool,
) -> Rectangle<i32, Logical> {
    let diameter = NODE_DIAMETER_PX.round() as i32;
    Rectangle::new(
        (center.x - diameter / 2, center.y - diameter / 2).into(),
        (diameter, diameter).into(),
    )
}

pub fn screen_from_world(
    world: Vec2,
    camera: &halley_core::camera::Camera,
    output_geometry: Rectangle<i32, Logical>,
) -> Point<i32, Logical> {
    let scale = crate::presentation::camera::scale(camera);
    let global_center = crate::presentation::camera::global_center(
        (camera.center.x, camera.center.y).into(),
        output_geometry,
    );
    (
        output_geometry.loc.x
            + output_geometry.size.w / 2
            + ((world.x - global_center.x) * scale).round() as i32,
        output_geometry.loc.y
            + output_geometry.size.h / 2
            + ((world.y - global_center.y) * scale).round() as i32,
    )
        .into()
}

pub fn send_hover_preview_frame(
    nodes: &NodesState,
    output: &Output,
    elapsed: Duration,
    now: Duration,
    sequence: u32,
) {
    let Some(id) = nodes.preview_hovered_on_output(&output.name(), now) else {
        return;
    };
    let Some(record) = nodes.record(id) else {
        return;
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

fn metadata(window: &Window) -> (String, Option<String>) {
    if let Some(toplevel) = window.toplevel() {
        return with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .map(|data| {
                    let app_id = data.app_id.clone();
                    let title = data
                        .title
                        .clone()
                        .or_else(|| app_id.clone())
                        .unwrap_or_else(|| "Untitled".to_string());
                    (title, app_id)
                })
                .unwrap_or_else(|| ("Untitled".to_string(), None))
        });
    }
    if let Some((title, class)) = crate::xwayland::metadata(window) {
        let app_id = (!class.is_empty()).then_some(class);
        return (
            if title.is_empty() {
                app_id.clone().unwrap_or_else(|| "Untitled".to_string())
            } else {
                title
            },
            app_id,
        );
    }
    ("Untitled".to_string(), None)
}

fn rect_center(rect: Rectangle<i32, Logical>) -> Vec2 {
    Vec2 {
        x: rect.loc.x as f32 + rect.size.w as f32 / 2.0,
        y: rect.loc.y as f32 + rect.size.h as f32 / 2.0,
    }
}

fn vec_size(rect: Rectangle<i32, Logical>) -> Vec2 {
    Vec2 {
        x: rect.size.w.max(1) as f32,
        y: rect.size.h.max(1) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HoverPreviewState, advance_hover_preview, centered_rect, hover_preview_ready,
        landmark_reveal_delta, logical_focus_after_collapse, minimal_reveal_delta,
        nearest_free_landmark, nearest_free_window_rect, participates_in_decay,
        physics_frame_delta, release_lock_deadline, release_lock_is_active,
    };
    use halley_core::field::{NodeId, NodeState, Vec2};
    use smithay::utils::{Logical, Rectangle};
    use std::time::Duration;

    #[test]
    fn physics_frame_delta_never_invents_time_for_fast_input() {
        let last = Duration::from_secs(10);
        assert!(
            (physics_frame_delta(last, last + Duration::from_millis(1)) - 0.001).abs()
                < f32::EPSILON
        );
        assert_eq!(physics_frame_delta(last, last), 0.0);
        assert_eq!(
            physics_frame_delta(last, last + Duration::from_secs(1)),
            1.0 / 30.0
        );
    }

    #[test]
    fn released_window_lock_expires_at_old_halley_deadline() {
        let now = Duration::from_secs(10);
        let until = release_lock_deadline(now);
        assert_eq!(until, now + Duration::from_millis(350));
        assert!(release_lock_is_active(
            until,
            now + Duration::from_millis(349)
        ));
        assert!(!release_lock_is_active(until, until));
    }

    #[test]
    fn hover_preview_starts_at_the_old_halley_dwell_boundary() {
        let started = Duration::from_secs(10);
        assert!(!hover_preview_ready(
            Some(started),
            started + Duration::from_millis(1_499)
        ));
        assert!(hover_preview_ready(
            Some(started),
            started + Duration::from_millis(1_500)
        ));
        assert!(!hover_preview_ready(None, Duration::from_secs(20)));
    }

    #[test]
    fn hover_preview_switch_resets_and_leave_fades_the_previous_node() {
        let first = NodeId::new(1);
        let second = NodeId::new(2);
        let mut state = HoverPreviewState::default();

        assert_eq!(
            advance_hover_preview(&mut state, Some(first)),
            Some((first, 0.3))
        );
        let (_, advanced) = advance_hover_preview(&mut state, Some(first)).unwrap();
        assert!(advanced > 0.3);
        assert_eq!(
            advance_hover_preview(&mut state, Some(second)),
            Some((second, 0.3))
        );
        let (_, fading) = advance_hover_preview(&mut state, None).unwrap();
        assert!(fading < 0.3);
    }

    #[test]
    fn collapsing_focused_window_preserves_its_logical_node_focus() {
        let collapsed = NodeId::new(7);
        let other = NodeId::new(9);
        assert_eq!(
            logical_focus_after_collapse(Some(other), collapsed, true),
            Some(collapsed)
        );
        assert_eq!(
            logical_focus_after_collapse(Some(collapsed), collapsed, false),
            Some(collapsed)
        );
    }

    #[test]
    fn collapsing_unfocused_window_does_not_steal_focus() {
        let collapsed = NodeId::new(7);
        let focused = NodeId::new(9);
        assert_eq!(
            logical_focus_after_collapse(Some(focused), collapsed, false),
            Some(focused)
        );
        assert_eq!(logical_focus_after_collapse(None, collapsed, false), None);
    }

    #[test]
    fn cluster_members_never_participate_in_decay() {
        assert!(participates_in_decay(false, true, false));
        assert!(!participates_in_decay(false, true, true));
        assert!(!participates_in_decay(true, true, false));
        assert!(!participates_in_decay(false, false, false));
    }

    #[test]
    fn logical_cluster_core_can_hold_focus_without_a_window_record() {
        let mut nodes = super::NodesState::new(&halley_config::RuntimeConfig::default());
        let core =
            nodes
                .field
                .spawn_surface("core", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 48.0, y: 48.0 });
        assert!(nodes.field.set_state(core, NodeState::Core));

        assert!(nodes.focus(Some(core), 10));
        assert_eq!(nodes.focused(), Some(core));
    }

    #[test]
    fn zoom_home_records_only_the_first_zoom_displacement() {
        let mut nodes = super::NodesState::new(&halley_config::RuntimeConfig::default());
        let id = nodes.field.spawn_surface(
            "landmark",
            Vec2 { x: 100.0, y: 40.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        let home = Vec2 { x: 100.0, y: 40.0 };

        nodes.remember_zoom_home(id, home);
        // A second zoom-out step moves the landmark again but must not replace
        // the home the first step recorded.
        nodes.remember_zoom_home(id, Vec2 { x: 300.0, y: 40.0 });

        assert_eq!(nodes.zoom_home(id), Some(home));
    }

    #[test]
    fn explicit_landmark_motion_commits_the_remembered_zoom_home() {
        let mut nodes = super::NodesState::new(&halley_config::RuntimeConfig::default());
        let dragged = nodes.field.spawn_surface(
            "dragged",
            Vec2 { x: 100.0, y: 40.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        let pushed = nodes.field.spawn_surface(
            "pushed",
            Vec2 { x: 200.0, y: 40.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        nodes.remember_zoom_home(dragged, Vec2 { x: 0.0, y: 0.0 });
        nodes.remember_zoom_home(pushed, Vec2 { x: 90.0, y: 0.0 });

        // Pinning, output transfer, and drag start all route through this.
        nodes.clear_direct_motion(dragged);
        nodes.commit_zoom_home(pushed);

        assert_eq!(nodes.zoom_home(dragged), None);
        assert_eq!(nodes.zoom_home(pushed), None);
    }

    #[test]
    fn a_disappearing_landmark_retires_its_zoom_home() {
        let mut nodes = super::NodesState::new(&halley_config::RuntimeConfig::default());
        let surviving = nodes.field.spawn_surface(
            "surviving",
            Vec2 { x: 100.0, y: 40.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        let removed = nodes.field.spawn_surface(
            "removed",
            Vec2 { x: 200.0, y: 40.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        nodes.remember_zoom_home(surviving, Vec2 { x: 100.0, y: 40.0 });
        nodes.remember_zoom_home(removed, Vec2 { x: 200.0, y: 40.0 });

        assert!(nodes.field.remove(removed).is_some());
        nodes.retire_zoom_homes_without_field_node();

        assert_eq!(nodes.zoom_home(removed), None);
        assert_eq!(nodes.zoom_home(surviving), Some(Vec2 { x: 100.0, y: 40.0 }));
    }

    #[test]
    fn cluster_core_landmark_slide_keeps_frames_running_without_a_window_record() {
        let mut nodes = super::NodesState::new(&halley_config::RuntimeConfig::default());
        nodes.animations_enabled = true;
        nodes.animation.enabled = true;
        let core = nodes.field.spawn_surface(
            "core",
            Vec2 { x: 200.0, y: 100.0 },
            Vec2 { x: 48.0, y: 48.0 },
        );
        assert!(nodes.field.set_state(core, NodeState::Core));
        let started = Duration::from_secs(1);
        nodes.start_landmark_slide(
            core,
            Vec2 { x: 20.0, y: 10.0 },
            Vec2 { x: 200.0, y: 100.0 },
            started,
        );

        assert!(nodes.is_animating_on_output("DP-1", started));
        assert_eq!(
            nodes.landmark_position(core, Vec2 { x: 200.0, y: 100.0 }, started),
            Vec2 { x: 20.0, y: 10.0 }
        );
        assert!(!nodes.is_animating_on_output(
            "DP-1",
            started + Duration::from_millis(super::LANDMARK_SLIDE_MS),
        ));
    }

    #[test]
    fn collapsed_node_reveal_uses_its_full_restored_window_bounds() {
        let viewport = Rectangle::<i32, Logical>::new((0, 0).into(), (1_000, 700).into());
        let restored = centered_rect(
            Vec2 {
                x: 1_100.0,
                y: 350.0,
            },
            (400, 300).into(),
        );

        assert_eq!(
            restored,
            Rectangle::new((900, 200).into(), (400, 300).into())
        );
        assert_eq!(
            minimal_reveal_delta(viewport, restored, 24),
            Vec2 { x: 324.0, y: 0.0 }
        );
    }

    #[test]
    fn offscreen_cluster_core_gets_a_minimal_zoom_aware_reveal() {
        let viewport = Rectangle::<i32, Logical>::new((0, 0).into(), (1_000, 700).into());
        assert_eq!(
            landmark_reveal_delta(
                viewport,
                Vec2 {
                    x: 1_100.0,
                    y: 350.0,
                },
                super::NODE_DIAMETER_PX,
                0.5,
            ),
            Vec2 { x: 199.0, y: 0.0 }
        );
    }

    #[test]
    fn bearings_only_pan_enough_to_reveal_a_live_window() {
        let viewport = Rectangle::<i32, Logical>::new((0, 0).into(), (1_000, 700).into());
        assert_eq!(
            minimal_reveal_delta(
                viewport,
                Rectangle::new((1_050, 200).into(), (300, 200).into()),
                24,
            ),
            Vec2 { x: 374.0, y: 0.0 }
        );
        assert_eq!(
            minimal_reveal_delta(
                viewport,
                Rectangle::new((100, 100).into(), (300, 200).into()),
                24,
            ),
            Vec2 { x: 0.0, y: 0.0 }
        );
    }

    #[test]
    fn placement_keeps_the_requested_position_when_it_is_free() {
        let desired = Vec2 { x: 10.0, y: 20.0 };
        assert_eq!(
            nearest_free_landmark(desired, &[], &[], 71.0, 45.5),
            desired
        );
    }

    #[test]
    fn placement_moves_an_overlapping_node_to_the_nearest_grid_ring() {
        let desired = Vec2 { x: 10.0, y: 20.0 };
        let spacing = 71.0;
        let placed = nearest_free_landmark(desired, &[desired], &[], spacing, 45.5);
        let dx = placed.x - desired.x;
        let dy = placed.y - desired.y;
        assert!(dx * dx + dy * dy >= spacing * spacing);
        assert!(dx.abs() <= spacing && dy.abs() <= spacing);
    }

    #[test]
    fn active_window_moves_away_from_a_landmark() {
        let desired = Rectangle::<i32, Logical>::new((0, 0).into(), (200, 120).into());
        let placed = nearest_free_window_rect(desired, &[Vec2 { x: 100.0, y: 60.0 }], 45.5);
        assert_ne!(placed, desired);
    }

    #[test]
    fn landmark_placement_treats_active_window_as_a_blocker() {
        let desired = Vec2 { x: 100.0, y: 60.0 };
        let window = Rectangle::<i32, Logical>::new((0, 0).into(), (200, 120).into());
        assert_ne!(
            nearest_free_landmark(desired, &[], &[window], 71.0, 45.5),
            desired
        );
    }
}
