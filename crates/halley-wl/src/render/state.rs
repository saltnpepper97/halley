use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterCycleDirection;
use halley_core::field::{Field, NodeId, Vec2};
use halley_core::tiling::Rect;

use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};
use smithay::utils::{Logical, Rectangle};

use crate::animation::{Animator, ClusterTileTracks};
use crate::overlay::{
    ClusterBloomAnimSnapshot, ClusterBloomAnimState, ExitConfirmOverlaySnapshot,
    ExitConfirmOverlayState, OverlayActionHint, OverlayBannerSnapshot, OverlayBannerState,
    OverlayToastSnapshot, OverlayToastState,
};
use crate::render::text::UiTextRenderer;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WindowOffscreenKey {
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
pub(crate) struct WindowOffscreenCache {
    /// Native 1.0x surface-tree bbox size used to build the offscreen image.
    pub key: WindowOffscreenKey,

    /// Set when the cached offscreen image should be rebuilt before use.
    pub dirty: bool,

    /// Last frame this cache entry was touched.
    pub last_used_at: Option<Instant>,

    /// Cached 1.0x surface-tree render target for zoomed compositing.
    pub texture: Option<GlesTexture>,

    /// Logical bbox paired with the cached texture.
    pub bbox: Option<Rectangle<i32, Logical>>,

    /// True once the cached offscreen image contains actual surface content.
    pub has_content: bool,
}

impl WindowOffscreenCache {
    #[inline]
    pub(crate) fn matches_size(&self, width: i32, height: i32) -> bool {
        self.key.width == width && self.key.height == height
    }

    #[inline]
    pub(crate) fn set_size(&mut self, width: i32, height: i32) {
        self.key = WindowOffscreenKey { width, height };
    }

    #[inline]
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    #[inline]
    pub(crate) fn mark_clean(&mut self, now: Instant) {
        self.dirty = false;
        self.last_used_at = Some(now);
    }

    #[inline]
    pub(crate) fn touch(&mut self, now: Instant) {
        self.last_used_at = Some(now);
    }
}

#[derive(Clone)]
pub(crate) struct NodeAppIconTexture {
    pub texture: GlesTexture,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone)]
pub(crate) enum NodeAppIconCacheEntry {
    Ready(NodeAppIconTexture),
    Missing,
}

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

pub(crate) struct RenderState {
    pub animator: Animator,

    pub(crate) node_app_icon_cache: HashMap<String, NodeAppIconCacheEntry>,
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
    pub(crate) stack_cycle_transition: HashMap<String, StackCycleTransitionState>,
    pub(crate) ui_text: RefCell<UiTextRenderer>,
    pub(crate) node_circle_texture: Option<GlesTexture>,
    pub(crate) node_circle_program: Option<GlesTexProgram>,
    pub(crate) node_square_program: Option<GlesTexProgram>,
    pub(crate) node_squircle_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_rounded_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_rounded_program_failed: bool,
    pub(crate) ui_rect_square_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_square_program_failed: bool,
    pub(crate) window_texture_program: Option<GlesTexProgram>,
    pub(crate) window_texture_program_failed: bool,
    pub(crate) surface_clip_program: Option<GlesTexProgram>,
    pub(crate) surface_clip_program_failed: bool,
    pub(crate) ui_text_program: Option<GlesTexProgram>,
    pub(crate) ui_text_program_failed: bool,

    pub(crate) zoom_nominal_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_fallback: HashSet<NodeId>,
    pub(crate) zoom_resize_reject_streak: HashMap<NodeId, u8>,
    pub(crate) zoom_last_observed_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_static_streak: HashMap<NodeId, u8>,

    pub(crate) render_last_tick: Instant,

    pub(crate) bbox_loc: HashMap<NodeId, (f32, f32)>,
    pub(crate) window_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) window_offscreen_cache: HashMap<NodeId, WindowOffscreenCache>,
}

impl RenderState {
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

    pub(crate) fn ui_rect_program(&self, rounded: bool) -> Option<&GlesTexProgram> {
        if rounded {
            self.ui_rect_rounded_program.as_ref()
        } else {
            self.ui_rect_square_program.as_ref()
        }
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
        let toast = self.overlay_toast.entry(monitor.to_string()).or_default();
        toast.message = Some(message.to_string());
        toast.visible_until_ms = now_ms.saturating_add(duration_ms.max(1));
        if toast.mix < 0.12 {
            toast.mix = 0.0;
        }
    }

    pub(crate) fn overlay_toast_snapshot(
        &mut self,
        monitor: &str,
        now_ms: u64,
    ) -> Option<OverlayToastSnapshot> {
        let toast = self.overlay_toast.get_mut(monitor)?;
        let target = if toast.message.is_some() && now_ms < toast.visible_until_ms {
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

    pub(crate) fn ensure_window_offscreen_cache(
        &mut self,
        node_id: NodeId,
        width: i32,
        height: i32,
        now: Instant,
    ) -> &mut WindowOffscreenCache {
        let width = width.max(1);
        let height = height.max(1);
        let cache = self.window_offscreen_cache.entry(node_id).or_default();
        if !cache.matches_size(width, height) {
            cache.set_size(width, height);
            cache.texture = None;
            cache.bbox = None;
            cache.has_content = false;
            cache.mark_dirty();
        }

        cache.touch(now);
        self.window_offscreen_cache
            .get_mut(&node_id)
            .expect("offscreen cache should exist after ensure")
    }

    pub(crate) fn mark_window_offscreen_dirty(&mut self, node_id: NodeId) {
        if let Some(cache) = self.window_offscreen_cache.get_mut(&node_id) {
            cache.mark_dirty();
        }
    }

    pub(crate) fn clear_window_offscreen_cache_for(&mut self, node_id: NodeId) {
        self.window_offscreen_cache.remove(&node_id);
    }

    pub(crate) fn prune_window_offscreen_cache(&mut self, alive: &HashSet<NodeId>, now: Instant) {
        self.window_offscreen_cache.retain(|id, cache| {
            alive.contains(id)
                && cache
                    .last_used_at
                    .is_none_or(|t| now.saturating_duration_since(t).as_secs() < 5)
        });
    }

    pub(crate) fn invalidate_ui_text_cache(&mut self) {
        self.ui_text.get_mut().clear();
    }

    pub(crate) fn prune_ui_text_cache(&mut self, now: Instant) {
        self.ui_text.get_mut().prune(now);
    }
}
