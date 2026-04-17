use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::field::{NodeId, Vec2};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::utils::{Logical, Rectangle};

use crate::text::UiTextRenderer;

use super::RenderState;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WindowOffscreenKey {
    pub width: i32,
    pub height: i32,
}

#[derive(Default)]
pub(crate) struct WindowOffscreenCache {
    pub key: WindowOffscreenKey,
    pub dirty: bool,
    pub last_used_at: Option<Instant>,
    pub texture: Option<GlesTexture>,
    pub bbox: Option<Rectangle<i32, Logical>>,
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

#[derive(Default)]
pub(crate) struct ClusterCoreIconCache {
    pub(crate) focused_color: [u8; 4],
    pub(crate) unfocused_color: [u8; 4],
    pub(crate) focused: Option<NodeAppIconTexture>,
    pub(crate) unfocused: Option<NodeAppIconTexture>,
}

#[derive(Default)]
pub(crate) struct ScreenshotMenuIconCache {
    pub(crate) active_color: [u8; 4],
    pub(crate) inactive_color: [u8; 4],
    pub(crate) region_active: Option<NodeAppIconTexture>,
    pub(crate) region_inactive: Option<NodeAppIconTexture>,
    pub(crate) screen_active: Option<NodeAppIconTexture>,
    pub(crate) screen_inactive: Option<NodeAppIconTexture>,
    pub(crate) window_active: Option<NodeAppIconTexture>,
    pub(crate) window_inactive: Option<NodeAppIconTexture>,
}

#[derive(Default)]
pub(crate) struct RenderCacheState {
    pub(crate) node_app_icon_cache: HashMap<String, NodeAppIconCacheEntry>,
    pub(crate) cluster_core_icon_cache: ClusterCoreIconCache,
    pub(crate) screenshot_menu_icon_cache: ScreenshotMenuIconCache,
    pub(crate) ui_text: RefCell<UiTextRenderer>,
    pub(crate) zoom_nominal_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_fallback: HashSet<NodeId>,
    pub(crate) zoom_resize_reject_streak: HashMap<NodeId, u8>,
    pub(crate) zoom_last_observed_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_static_streak: HashMap<NodeId, u8>,
    pub(crate) bbox_loc: HashMap<NodeId, (f32, f32)>,
    pub(crate) window_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) window_offscreen_cache: HashMap<NodeId, WindowOffscreenCache>,
}

impl RenderState {
    pub(crate) fn ensure_window_offscreen_cache(
        &mut self,
        node_id: NodeId,
        width: i32,
        height: i32,
        now: Instant,
    ) -> &mut WindowOffscreenCache {
        let width = width.max(1);
        let height = height.max(1);
        let cache = self
            .cache
            .window_offscreen_cache
            .entry(node_id)
            .or_default();
        if !cache.matches_size(width, height) {
            cache.set_size(width, height);
            cache.texture = None;
            cache.bbox = None;
            cache.has_content = false;
            cache.mark_dirty();
        }

        cache.touch(now);
        self.cache
            .window_offscreen_cache
            .get_mut(&node_id)
            .expect("offscreen cache should exist after ensure")
    }

    pub(crate) fn mark_window_offscreen_dirty(&mut self, node_id: NodeId) {
        if let Some(cache) = self.cache.window_offscreen_cache.get_mut(&node_id) {
            cache.mark_dirty();
        }
    }

    pub(crate) fn clear_window_offscreen_cache_for(&mut self, node_id: NodeId) {
        self.cache.window_offscreen_cache.remove(&node_id);
    }

    pub(crate) fn clear_window_offscreen_caches(&mut self) {
        self.cache.window_offscreen_cache.clear();
    }

    pub(crate) fn prune_window_offscreen_cache(&mut self, alive: &HashSet<NodeId>, now: Instant) {
        self.cache.window_offscreen_cache.retain(|id, cache| {
            alive.contains(id)
                && cache
                    .last_used_at
                    .is_none_or(|t| now.saturating_duration_since(t).as_secs() < 5)
        });
    }

    pub(crate) fn invalidate_ui_text_cache(&mut self) {
        self.cache.ui_text.get_mut().clear();
    }

    pub(crate) fn prune_ui_text_cache(&mut self, now: Instant) {
        self.cache.ui_text.get_mut().prune(now);
    }
}
