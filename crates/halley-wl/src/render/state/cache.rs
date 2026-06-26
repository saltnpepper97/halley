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
    /// Resolution/decode has been handed to the background loader; renderers draw
    /// the fallback glyph until a `drain` swaps this for `Ready`/`Missing`.
    Pending,
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
pub(crate) struct PinIconCache {
    pub(crate) color: [u8; 4],
    pub(crate) icon: Option<NodeAppIconTexture>,
}

/// The cluster glyph tinted to the bearing chip text colour, drawn on a cluster
/// core's bearing chip in place of the app-icon fallback. Rebuilt only when the
/// chip text colour changes.
#[derive(Default)]
pub(crate) struct BearingClusterIconCache {
    pub(crate) color: [u8; 4],
    pub(crate) icon: Option<NodeAppIconTexture>,
}

#[derive(Default)]
pub(crate) struct RenderCacheState {
    pub(crate) node_app_icon_cache: HashMap<String, NodeAppIconCacheEntry>,
    /// Background worker that resolves + decodes app icons off the render thread.
    /// Lazily spawned on first cache miss (see `crate::render::app_icon`).
    pub(crate) app_icon_loader: Option<crate::render::app_icon::AppIconLoader>,
    pub(crate) cluster_core_icon_cache: ClusterCoreIconCache,
    pub(crate) screenshot_menu_icon_cache: ScreenshotMenuIconCache,
    pub(crate) pin_icon_cache: PinIconCache,
    pub(crate) bearing_cluster_icon_cache: BearingClusterIconCache,
    pub(crate) ui_text: RefCell<UiTextRenderer>,
    pub(crate) zoom_nominal_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_fallback: HashSet<NodeId>,
    pub(crate) zoom_resize_reject_streak: HashMap<NodeId, u8>,
    pub(crate) zoom_last_observed_size: HashMap<NodeId, Vec2>,
    pub(crate) zoom_resize_static_streak: HashMap<NodeId, u8>,
    pub(crate) bbox_loc: HashMap<NodeId, (f32, f32)>,
    pub(crate) window_geometry: HashMap<NodeId, (f32, f32, f32, f32)>,
    pub(crate) window_offscreen_cache: HashMap<NodeId, WindowOffscreenCache>,
    /// Frozen alt+tab neighbour previews. Separate from `window_offscreen_cache`
    /// (which the main render pass keeps live for on-screen windows) so a
    /// non-selected card shows a single still instead of animating. Only the
    /// selected focus-cycle card reads the live `window_offscreen_cache`.
    pub(crate) focus_cycle_still: HashMap<NodeId, WindowOffscreenCache>,
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

    /// Get-or-create the frozen focus-cycle still entry for `node_id`, resetting
    /// it when the captured size no longer matches (so a resized neighbour
    /// recaptures a fresh still).
    pub(crate) fn ensure_focus_cycle_still(
        &mut self,
        node_id: NodeId,
        width: i32,
        height: i32,
        now: Instant,
    ) -> &mut WindowOffscreenCache {
        let width = width.max(1);
        let height = height.max(1);
        let cache = self.cache.focus_cycle_still.entry(node_id).or_default();
        if !cache.matches_size(width, height) {
            cache.set_size(width, height);
            cache.texture = None;
            cache.bbox = None;
            cache.has_content = false;
            cache.mark_dirty();
        }
        cache.touch(now);
        self.cache
            .focus_cycle_still
            .get_mut(&node_id)
            .expect("focus-cycle still should exist after ensure")
    }

    /// Drop a single node's frozen still (e.g. when it becomes the selected card,
    /// so a fresh still is taken once it is demoted to a neighbour again).
    pub(crate) fn clear_focus_cycle_still_for(&mut self, node_id: NodeId) {
        self.cache.focus_cycle_still.remove(&node_id);
    }

    /// Drop frozen stills for nodes no longer in the visible alt+tab slots.
    pub(crate) fn prune_focus_cycle_still(&mut self, keep: &HashSet<NodeId>) {
        self.cache
            .focus_cycle_still
            .retain(|id, _| keep.contains(id));
    }

    /// Drop every frozen still (focus-cycle session ended).
    pub(crate) fn clear_focus_cycle_still(&mut self) {
        self.cache.focus_cycle_still.clear();
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

    pub(crate) fn prune_window_offscreen_cache(
        &mut self,
        alive: &HashSet<NodeId>,
        keep_warm: &HashSet<NodeId>,
        now: Instant,
    ) {
        self.cache.window_offscreen_cache.retain(|id, cache| {
            // Cluster members are kept warm regardless of the idle TTL so a
            // collapse → later re-open is always a cache hit (no synchronous
            // texture rebuild spike at open). Closed windows are still gated by
            // `alive` and freed.
            alive.contains(id)
                && (keep_warm.contains(id)
                    || cache
                        .last_used_at
                        .is_none_or(|t| now.saturating_duration_since(t).as_secs() < 5))
        });
    }

    pub(crate) fn invalidate_ui_text_cache(&mut self) {
        self.cache.ui_text.get_mut().clear();
    }

    pub(crate) fn prune_ui_text_cache(&mut self, now: Instant) {
        self.cache.ui_text.get_mut().prune(now);
    }
}
