mod cache;
mod gpu;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_config::WindowCloseAnimationStyle;
use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterCycleDirection;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::tiling::Rect;

use crate::animation::{Animator, ClusterTileTracks};
use crate::overlay::{
    ClusterBloomAnimSnapshot, ClusterBloomAnimState, ExitConfirmOverlaySnapshot,
    ExitConfirmOverlayState, OverlayActionHint, OverlayBannerSnapshot, OverlayBannerState,
    OverlayToastKind, OverlayToastSnapshot, OverlayToastState,
};
use crate::window::{ActiveBorderRect, OffscreenNodeTexture};

const LANDMARK_SLIDE_DURATION_MS: u64 = 520;

pub(crate) use cache::{
    ClusterCoreIconCache, NodeAppIconCacheEntry, NodeAppIconTexture, PinIconCache,
    RenderCacheState, ScreenshotMenuIconCache,
};
pub(crate) use gpu::RenderGpuState;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PreviewHoverState {
    pub(crate) node: Option<NodeId>,
    pub(crate) mix: f32,
    pub(crate) overlay_anchor: Option<((i32, i32), bool)>,
}

#[derive(Clone, Debug)]
pub(crate) struct StackCycleTransitionState {
    pub(crate) direction: ClusterCycleDirection,
    pub(crate) started_at: Instant,
    pub(crate) duration_ms: u64,
    pub(crate) old_visible: Vec<NodeId>,
    pub(crate) new_visible: Vec<NodeId>,
    pub(crate) source_rects: Option<HashMap<NodeId, Rect>>,
}

#[derive(Clone, Debug)]
pub(crate) struct StackCycleTransitionSnapshot {
    pub(crate) direction: ClusterCycleDirection,
    pub(crate) progress: f32,
    pub(crate) old_visible: Vec<NodeId>,
    pub(crate) new_visible: Vec<NodeId>,
    pub(crate) source_rects: Option<HashMap<NodeId, Rect>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RaiseAnimationState {
    pub(crate) started_at: Instant,
    pub(crate) duration_ms: u64,
    pub(crate) scale: f32,
    pub(crate) shadow_boost: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RaiseAnimationSnapshot {
    pub(crate) scale: f32,
    pub(crate) shadow_boost: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LandmarkSlideAnimationState {
    pub(crate) from: Vec2,
    pub(crate) to: Vec2,
    pub(crate) started_at: Instant,
}

#[derive(Clone)]
pub(crate) enum ClosingWindowAnimationKind {
    Window {
        style: WindowCloseAnimationStyle,
        border_rects: Vec<ActiveBorderRect>,
        offscreen_textures: Vec<OffscreenNodeTexture>,
    },
    Node {
        pos: Vec2,
        label: String,
        state: halley_core::field::NodeState,
    },
}

#[derive(Clone)]
pub(crate) struct ClosingWindowAnimationState {
    pub(crate) monitor: String,
    pub(crate) started_at: Instant,
    pub(crate) duration_ms: u64,
    pub(crate) kind: ClosingWindowAnimationKind,
}

#[derive(Clone)]
pub(crate) struct ClosingWindowAnimationSnapshot {
    pub(crate) progress: f32,
    pub(crate) kind: ClosingWindowAnimationKind,
}

pub(crate) struct RenderState {
    pub animator: Animator,

    pub(crate) cache: RenderCacheState,
    pub(crate) node_hover_mix: HashMap<NodeId, f32>,
    pub(crate) node_preview_hover: HashMap<String, PreviewHoverState>,
    pub(crate) bearings_visible: bool,
    pub(crate) bearings_mix: HashMap<String, f32>,
    pub(crate) cluster_tile_tracks: ClusterTileTracks,
    pub(crate) cluster_tile_entry_pending: HashSet<NodeId>,
    pub(crate) cluster_tile_frozen_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) cluster_bloom_mix: HashMap<String, ClusterBloomAnimState>,
    pub(crate) overlay_banner: HashMap<String, OverlayBannerState>,
    pub(crate) overlay_toast: HashMap<String, OverlayToastState>,
    pub(crate) overlay_exit_confirm: HashMap<String, ExitConfirmOverlayState>,
    pub(crate) closing_window_animations: HashMap<NodeId, ClosingWindowAnimationState>,
    pub(crate) stack_cycle_transition: HashMap<String, StackCycleTransitionState>,
    pub(crate) raise_animations: HashMap<NodeId, RaiseAnimationState>,
    pub(crate) landmark_slide_animations: HashMap<NodeId, LandmarkSlideAnimationState>,
    pub(crate) gpu: RenderGpuState,

    pub(crate) render_last_tick: Instant,
}

impl RenderState {
    pub(crate) fn start_closing_window_animation(
        &mut self,
        node_id: NodeId,
        monitor: &str,
        now: Instant,
        duration_ms: u64,
        style: WindowCloseAnimationStyle,
        border_rects: Vec<ActiveBorderRect>,
        offscreen_textures: Vec<OffscreenNodeTexture>,
    ) {
        if border_rects.is_empty() && offscreen_textures.is_empty() {
            return;
        }
        self.closing_window_animations.insert(
            node_id,
            ClosingWindowAnimationState {
                monitor: monitor.to_string(),
                started_at: now,
                duration_ms: duration_ms.max(1),
                kind: ClosingWindowAnimationKind::Window {
                    style,
                    border_rects,
                    offscreen_textures,
                },
            },
        );
    }

    pub(crate) fn start_raise_animation(
        &mut self,
        node_id: NodeId,
        now: Instant,
        duration_ms: u64,
        scale: f32,
        shadow_boost: f32,
    ) {
        self.raise_animations.insert(
            node_id,
            RaiseAnimationState {
                started_at: now,
                duration_ms: duration_ms.max(1),
                scale: scale.max(1.0),
                shadow_boost: shadow_boost.clamp(0.0, 1.0),
            },
        );
    }

    pub(crate) fn start_landmark_slide_animation(
        &mut self,
        node_id: NodeId,
        from: Vec2,
        to: Vec2,
        now: Instant,
    ) {
        if (from.x - to.x).abs() <= 0.5 && (from.y - to.y).abs() <= 0.5 {
            return;
        }
        self.landmark_slide_animations.insert(
            node_id,
            LandmarkSlideAnimationState {
                from,
                to,
                started_at: now,
            },
        );
    }

    pub(crate) fn landmark_slide_active_for_monitor(
        &self,
        field: &Field,
        node_monitor: &HashMap<NodeId, String>,
        monitor: &str,
        now: Instant,
    ) -> bool {
        self.landmark_slide_animations.iter().any(|(&id, anim)| {
            (now.saturating_duration_since(anim.started_at).as_millis() as u64)
                < LANDMARK_SLIDE_DURATION_MS
                && field.is_visible(id)
                && !node_monitor
                    .get(&id)
                    .is_some_and(|node_monitor| node_monitor != monitor)
        })
    }

    pub(crate) fn landmark_slide_position(
        &mut self,
        node_id: NodeId,
        fallback: Vec2,
        now: Instant,
    ) -> Vec2 {
        let Some(anim) = self.landmark_slide_animations.get(&node_id).copied() else {
            return fallback;
        };
        let elapsed_ms = now.saturating_duration_since(anim.started_at).as_millis() as u64;
        if elapsed_ms >= LANDMARK_SLIDE_DURATION_MS {
            self.landmark_slide_animations.remove(&node_id);
            return fallback;
        }
        let t = (elapsed_ms as f32 / LANDMARK_SLIDE_DURATION_MS as f32).clamp(0.0, 1.0);
        let end = 1.0 - (1.0 + 5.0) * (-5.0f32).exp();
        let damped = ((1.0 - (1.0 + 5.0 * t) * (-5.0 * t).exp()) / end).clamp(0.0, 1.0);
        Vec2 {
            x: anim.from.x + (anim.to.x - anim.from.x) * damped,
            y: anim.from.y + (anim.to.y - anim.from.y) * damped,
        }
    }

    pub(crate) fn raise_animation_active_for_monitor(
        &self,
        field: &Field,
        node_monitor: &HashMap<NodeId, String>,
        monitor: &str,
        now: Instant,
    ) -> bool {
        self.raise_animations.iter().any(|(&id, anim)| {
            (now.saturating_duration_since(anim.started_at).as_millis() as u64) < anim.duration_ms
                && field.is_visible(id)
                && node_monitor
                    .get(&id)
                    .is_some_and(|node_monitor| node_monitor == monitor)
        })
    }

    pub(crate) fn raise_animation_for(
        &mut self,
        node_id: NodeId,
        now: Instant,
    ) -> RaiseAnimationSnapshot {
        let Some(anim) = self.raise_animations.get(&node_id).copied() else {
            return RaiseAnimationSnapshot {
                scale: 1.0,
                shadow_boost: 0.0,
            };
        };
        let elapsed_ms = now.saturating_duration_since(anim.started_at).as_millis() as u64;
        if elapsed_ms >= anim.duration_ms {
            self.raise_animations.remove(&node_id);
            return RaiseAnimationSnapshot {
                scale: 1.0,
                shadow_boost: 0.0,
            };
        }
        let t = (elapsed_ms as f32 / anim.duration_ms as f32).clamp(0.0, 1.0);
        let pulse = (1.0 - t).powi(2);
        RaiseAnimationSnapshot {
            scale: 1.0 + (anim.scale - 1.0) * pulse,
            shadow_boost: anim.shadow_boost * pulse,
        }
    }

    pub(crate) fn start_closing_node_animation(
        &mut self,
        node_id: NodeId,
        monitor: &str,
        now: Instant,
        duration_ms: u64,
        pos: Vec2,
        label: String,
        state: halley_core::field::NodeState,
    ) {
        self.closing_window_animations.insert(
            node_id,
            ClosingWindowAnimationState {
                monitor: monitor.to_string(),
                started_at: now,
                duration_ms: duration_ms.max(1),
                kind: ClosingWindowAnimationKind::Node { pos, label, state },
            },
        );
    }

    pub(crate) fn closing_window_animation_active_for_monitor(
        &self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        self.closing_window_animations.values().any(|state| {
            state.monitor == monitor
                && (now.saturating_duration_since(state.started_at).as_millis() as u64)
                    < state.duration_ms
        })
    }

    pub(crate) fn closing_window_animation_snapshots(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> Vec<ClosingWindowAnimationSnapshot> {
        self.closing_window_animations.retain(|_, state| {
            (now.saturating_duration_since(state.started_at).as_millis() as u64) < state.duration_ms
        });
        self.closing_window_animations
            .values()
            .filter(|state| state.monitor == monitor)
            .map(|state| {
                let elapsed_ms = now.saturating_duration_since(state.started_at).as_millis() as u64;
                ClosingWindowAnimationSnapshot {
                    progress: (elapsed_ms as f32 / state.duration_ms.max(1) as f32).clamp(0.0, 1.0),
                    kind: state.kind.clone(),
                }
            })
            .collect()
    }

    pub(crate) fn start_stack_cycle_transition(
        &mut self,
        monitor: &str,
        direction: ClusterCycleDirection,
        old_visible: Vec<NodeId>,
        new_visible: Vec<NodeId>,
        now: Instant,
        duration_ms: u64,
    ) {
        self.start_stack_cycle_transition_from_rects(
            monitor,
            direction,
            old_visible,
            new_visible,
            HashMap::new(),
            now,
            duration_ms,
        );
    }

    pub(crate) fn start_stack_cycle_transition_from_rects(
        &mut self,
        monitor: &str,
        direction: ClusterCycleDirection,
        old_visible: Vec<NodeId>,
        new_visible: Vec<NodeId>,
        source_rects: HashMap<NodeId, Rect>,
        now: Instant,
        duration_ms: u64,
    ) {
        if old_visible == new_visible && source_rects.is_empty() {
            self.stack_cycle_transition.remove(monitor);
            return;
        }
        self.stack_cycle_transition.insert(
            monitor.to_string(),
            StackCycleTransitionState {
                direction,
                started_at: now,
                duration_ms: duration_ms.max(1),
                old_visible,
                new_visible,
                source_rects: (!source_rects.is_empty()).then_some(source_rects),
            },
        );
    }

    pub(crate) fn stack_cycle_transition_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> Option<StackCycleTransitionSnapshot> {
        let state = self.stack_cycle_transition.get(monitor)?.clone();
        let elapsed_ms = now.saturating_duration_since(state.started_at).as_millis() as u64;
        if elapsed_ms >= state.duration_ms {
            self.stack_cycle_transition.remove(monitor);
            return None;
        }
        Some(StackCycleTransitionSnapshot {
            direction: state.direction,
            progress: (elapsed_ms as f32 / state.duration_ms as f32).clamp(0.0, 1.0),
            old_visible: state.old_visible,
            new_visible: state.new_visible,
            source_rects: state.source_rects,
        })
    }

    pub(crate) fn anim_track_elapsed_for(
        &self,
        id: NodeId,
        state: halley_core::field::NodeState,
        now: Instant,
    ) -> Option<std::time::Duration> {
        self.animator.track_elapsed_for(id, state, now)
    }

    pub(crate) fn node_label_hover_mix(&mut self, id: NodeId, hovered: bool) -> f32 {
        let target = if hovered { 1.0 } else { 0.0 };
        let mix = self.node_hover_mix.entry(id).or_insert(target);
        let k = if hovered { 0.06 } else { 0.10 };
        *mix += (target - *mix) * k;
        if (*mix - target).abs() < 0.01 {
            *mix = target;
        }
        *mix
    }

    pub(crate) fn node_preview_hover_anim_for_monitor(
        &mut self,
        monitor: &str,
        hovered: Option<NodeId>,
    ) -> Option<(NodeId, f32)> {
        let state = self
            .node_preview_hover
            .entry(monitor.to_string())
            .or_default();
        if hovered.is_some() && hovered != state.node {
            state.node = hovered;
            state.mix = 0.0;
            state.overlay_anchor = None;
        }
        let target = if hovered.is_some() { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.30 } else { 0.14 };
        state.mix += (target - state.mix) * k;
        if (state.mix - target).abs() < 0.002 {
            state.mix = target;
        }
        if target <= 0.0 && state.mix <= 0.002 {
            state.mix = 0.0;
            state.node = None;
            state.overlay_anchor = None;
        }
        state.node.map(|id| (id, state.mix))
    }

    pub(crate) fn bearings_visible(&self) -> bool {
        self.bearings_visible
    }

    pub(crate) fn set_bearings_visible(&mut self, visible: bool) -> bool {
        if self.bearings_visible == visible {
            return false;
        }
        self.bearings_visible = visible;
        true
    }

    pub(crate) fn toggle_bearings_visible(&mut self) -> bool {
        let next = !self.bearings_visible;
        self.set_bearings_visible(next);
        next
    }

    pub(crate) fn bearings_mix_for_monitor(&mut self, monitor: &str) -> f32 {
        let target = if self.bearings_visible { 1.0 } else { 0.0 };
        let mix = self
            .bearings_mix
            .entry(monitor.to_string())
            .or_insert(target);
        if target > 0.5 {
            *mix += (target - *mix) * 0.18;
        } else {
            *mix *= 0.72;
        }
        if (*mix - target).abs() < 0.004 {
            *mix = target;
        }
        if target <= 0.0 && *mix <= 0.02 {
            *mix = 0.0;
        }
        *mix
    }

    pub(crate) fn cluster_bloom_snapshot_for_monitor(
        &mut self,
        monitor: &str,
        target_cluster: Option<ClusterId>,
    ) -> Option<ClusterBloomAnimSnapshot> {
        let state = self
            .cluster_bloom_mix
            .entry(monitor.to_string())
            .or_default();
        if let Some(cid) = target_cluster
            && state.cluster_id != Some(cid)
        {
            state.cluster_id = Some(cid);
            if state.mix < 0.08 {
                state.mix = 0.0;
            }
        }
        state.visible = target_cluster.is_some();
        let target = if state.visible { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.26 } else { 0.22 };
        state.mix += (target - state.mix) * k;
        if (state.mix - target).abs() < 0.01 {
            state.mix = target;
        }
        if target <= 0.0 && state.mix <= 0.01 {
            state.cluster_id = None;
            return None;
        }
        state.cluster_id.map(|cluster_id| ClusterBloomAnimSnapshot {
            cluster_id,
            mix: state.mix.clamp(0.0, 1.0),
        })
    }

    pub(crate) fn set_persistent_mode_banner(
        &mut self,
        monitor: &str,
        title: &str,
        subtitle: Option<&str>,
        actions: &[OverlayActionHint],
    ) {
        let state = self
            .overlay_banner
            .entry(monitor.to_string())
            .or_insert_with(|| OverlayBannerState {
                title: String::new(),
                subtitle: None,
                actions: Vec::new(),
                visible: false,
                mix: 0.0,
            });
        state.title = title.to_string();
        state.subtitle = subtitle.map(str::to_string);
        state.actions = actions.to_vec();
        state.visible = true;
    }

    pub(crate) fn clear_persistent_mode_banner(&mut self, monitor: &str) {
        if let Some(state) = self.overlay_banner.get_mut(monitor) {
            state.visible = false;
        }
    }

    pub(crate) fn persistent_mode_banner_snapshot(
        &mut self,
        monitor: &str,
    ) -> Option<OverlayBannerSnapshot> {
        let state = self.overlay_banner.get_mut(monitor)?;
        let target = if state.visible { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.36 } else { 0.24 };
        state.mix += (target - state.mix) * k;
        if (state.mix - target).abs() < 0.015 {
            state.mix = target;
        }
        if target <= 0.0 && state.mix <= 0.015 {
            self.overlay_banner.remove(monitor);
            return None;
        }
        Some(OverlayBannerSnapshot {
            title: state.title.clone(),
            subtitle: state.subtitle.clone(),
            actions: state.actions.clone(),
            mix: state.mix,
        })
    }

    pub(crate) fn show_overlay_toast(
        &mut self,
        monitor: &str,
        message: &str,
        duration_ms: u64,
        now_ms: u64,
    ) {
        self.show_overlay_toast_with_kind(
            monitor,
            message,
            OverlayToastKind::Info,
            duration_ms,
            now_ms,
        );
    }

    pub(crate) fn show_overlay_error_toast(
        &mut self,
        monitor: &str,
        message: &str,
        duration_ms: u64,
        now_ms: u64,
    ) {
        self.show_overlay_toast_with_kind(
            monitor,
            message,
            OverlayToastKind::Error,
            duration_ms,
            now_ms,
        );
    }

    fn show_overlay_toast_with_kind(
        &mut self,
        monitor: &str,
        message: &str,
        kind: OverlayToastKind,
        duration_ms: u64,
        now_ms: u64,
    ) {
        let toast = self.overlay_toast.entry(monitor.to_string()).or_default();
        toast.message = Some(message.to_string());
        toast.kind = kind;
        toast.duration_ms = duration_ms.max(1);
        toast.hovered = false;
        toast.scroll_x = 0;
        toast.scroll_y = 0;
        toast.visible_until_ms = now_ms.saturating_add(toast.duration_ms);
        if toast.mix < 0.12 {
            toast.mix = 0.0;
        }
    }

    pub(crate) fn adjust_overlay_error_toast_scroll(
        &mut self,
        monitor: &str,
        dx: i32,
        dy: i32,
        max_x: i32,
        max_y: i32,
    ) -> bool {
        let Some(toast) = self.overlay_toast.get_mut(monitor) else {
            return false;
        };
        if !matches!(toast.kind, OverlayToastKind::Error) || toast.message.is_none() {
            return false;
        }
        let prev = (toast.scroll_x, toast.scroll_y);
        toast.scroll_x = toast.scroll_x.saturating_add(dx).clamp(0, max_x.max(0));
        toast.scroll_y = toast.scroll_y.saturating_add(dy).clamp(0, max_y.max(0));
        prev != (toast.scroll_x, toast.scroll_y)
    }

    pub(crate) fn set_overlay_error_toast_hovered(
        &mut self,
        monitor: &str,
        hovered: bool,
        now_ms: u64,
    ) {
        let Some(toast) = self.overlay_toast.get_mut(monitor) else {
            return;
        };
        if !matches!(toast.kind, OverlayToastKind::Error) || toast.message.is_none() {
            return;
        }
        let was_hovered = toast.hovered;
        toast.hovered = hovered;
        if was_hovered && !hovered {
            toast.visible_until_ms = now_ms.saturating_add(toast.duration_ms.max(1));
        }
    }

    pub(crate) fn dismiss_overlay_error_toast(&mut self, monitor: &str) -> bool {
        let Some(toast) = self.overlay_toast.get(monitor) else {
            return false;
        };
        if !matches!(toast.kind, OverlayToastKind::Error) {
            return false;
        }
        self.overlay_toast.remove(monitor).is_some()
    }

    pub(crate) fn overlay_toast_snapshot(
        &mut self,
        monitor: &str,
        now_ms: u64,
    ) -> Option<OverlayToastSnapshot> {
        let toast = self.overlay_toast.get_mut(monitor)?;
        let target = if toast.message.is_some()
            && (now_ms < toast.visible_until_ms
                || (matches!(toast.kind, OverlayToastKind::Error) && toast.hovered))
        {
            1.0
        } else {
            0.0
        };
        let k = if target > 0.5 { 0.40 } else { 0.26 };
        toast.mix += (target - toast.mix) * k;
        if (toast.mix - target).abs() < 0.015 {
            toast.mix = target;
        }
        if target <= 0.0 && toast.mix <= 0.015 {
            self.overlay_toast.remove(monitor);
            return None;
        }
        Some(OverlayToastSnapshot {
            message: toast.message.clone().unwrap_or_default(),
            kind: toast.kind,
            scroll_x: toast.scroll_x,
            scroll_y: toast.scroll_y,
            mix: toast.mix,
        })
    }

    pub(crate) fn show_exit_confirm(&mut self, monitor: &str) {
        let exit_confirm = self
            .overlay_exit_confirm
            .entry(monitor.to_string())
            .or_default();
        exit_confirm.visible = true;
        if exit_confirm.mix < 0.12 {
            exit_confirm.mix = 0.0;
        }
    }

    pub(crate) fn clear_exit_confirm(&mut self, monitor: &str) {
        if let Some(exit_confirm) = self.overlay_exit_confirm.get_mut(monitor) {
            exit_confirm.visible = false;
        }
    }

    pub(crate) fn exit_confirm_visible(&self) -> bool {
        self.overlay_exit_confirm
            .values()
            .any(|state| state.visible)
    }

    pub(crate) fn exit_confirm_snapshot(
        &mut self,
        monitor: &str,
    ) -> Option<ExitConfirmOverlaySnapshot> {
        let exit_confirm = self.overlay_exit_confirm.get_mut(monitor)?;
        let target = if exit_confirm.visible { 1.0 } else { 0.0 };
        let k = if target > 0.5 { 0.34 } else { 0.24 };
        exit_confirm.mix += (target - exit_confirm.mix) * k;
        if (exit_confirm.mix - target).abs() < 0.015 {
            exit_confirm.mix = target;
        }
        if target <= 0.0 && exit_confirm.mix <= 0.015 {
            self.overlay_exit_confirm.remove(monitor);
            return None;
        }
        Some(ExitConfirmOverlaySnapshot {
            mix: exit_confirm.mix,
        })
    }

    pub(crate) fn tick_animator_frame(&mut self, field: &Field, now: Instant) {
        self.animator.observe_field(field, now);
    }
}
