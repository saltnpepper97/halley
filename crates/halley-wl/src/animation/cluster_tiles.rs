use std::collections::HashMap;
use std::time::{Duration, Instant};

use halley_core::field::{Field, NodeId, NodeState, Vec2, Visibility};
use halley_core::tiling::Rect;

use super::curves::ease_in_out_cubic;

/// A cluster tile with alpha at or below this is treated as fully invisible —
/// i.e. not yet shown / mid fade-out — and therefore a fresh "entering" tile
/// rather than a reflow of a visible one.
pub(crate) const ALPHA_INVISIBLE: f32 = 0.01;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClusterTileAnimRect {
    pub(crate) center: Vec2,
    pub(crate) size: Vec2,
    pub(crate) alpha: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClusterTileTrack {
    from: ClusterTileAnimRect,
    to: ClusterTileAnimRect,
    started_at: Instant,
    /// Delay before the track begins moving — used to stagger member entry so
    /// tiles cascade in rather than popping in unison.
    start_delay: Duration,
    duration: Duration,
}

impl ClusterTileTrack {
    #[inline]
    fn lifetime(&self) -> Duration {
        self.start_delay + self.duration
    }
}

pub(crate) type ClusterTileTracks = HashMap<NodeId, ClusterTileTrack>;

#[inline]
fn rect_center(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

#[inline]
fn anim_rect_from_tile_rect(rect: Rect, alpha: f32) -> ClusterTileAnimRect {
    ClusterTileAnimRect {
        center: rect_center(rect),
        size: Vec2 {
            x: rect.w.max(1.0),
            y: rect.h.max(1.0),
        },
        alpha: alpha.clamp(0.0, 1.0),
    }
}

#[inline]
fn tile_rect_from_anim_rect(rect: ClusterTileAnimRect) -> Rect {
    Rect {
        x: rect.center.x - rect.size.x * 0.5,
        y: rect.center.y - rect.size.y * 0.5,
        w: rect.size.x.max(1.0),
        h: rect.size.y.max(1.0),
    }
}

#[inline]
fn entry_rect_for_target(target: ClusterTileAnimRect) -> ClusterTileAnimRect {
    // Each member slides in horizontally from the left of its slot, near full
    // size (a slide, not a zoom), fading in. Combined with the per-member start
    // delay (cluster.tiling.stagger-ms) — slaves first, master last — this reads
    // as the windows being dealt into place one by one from the left.
    ClusterTileAnimRect {
        center: Vec2 {
            x: target.center.x - target.size.x * 0.6,
            y: target.center.y,
        },
        size: Vec2 {
            x: target.size.x * 0.96,
            y: target.size.y * 0.96,
        },
        alpha: 0.0,
    }
}

#[inline]
pub(crate) fn cluster_tile_rect_from_field(
    field: &Field,
    id: NodeId,
) -> Option<ClusterTileAnimRect> {
    let node = field.node(id)?;
    Some(ClusterTileAnimRect {
        center: node.pos,
        size: Vec2 {
            x: node.intrinsic_size.x.max(1.0),
            y: node.intrinsic_size.y.max(1.0),
        },
        alpha: if node.visibility.has(Visibility::HIDDEN_BY_CLUSTER)
            || node.state != NodeState::Active
        {
            0.0
        } else {
            1.0
        },
    })
}

#[inline]
fn nearly_eq(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.5
}

#[inline]
fn same_rect(a: ClusterTileAnimRect, b: ClusterTileAnimRect) -> bool {
    nearly_eq(a.center.x, b.center.x)
        && nearly_eq(a.center.y, b.center.y)
        && nearly_eq(a.size.x, b.size.x)
        && nearly_eq(a.size.y, b.size.y)
        && nearly_eq(a.alpha, b.alpha)
}

pub(crate) fn cluster_tile_rect_for_track(
    track: &ClusterTileTrack,
    now: Instant,
) -> ClusterTileAnimRect {
    let elapsed = now.saturating_duration_since(track.started_at);
    if elapsed <= track.start_delay {
        // Still waiting for this member's staggered turn — hold the entry pose.
        return track.from;
    }
    let elapsed = elapsed - track.start_delay;
    if elapsed >= track.duration {
        return track.to;
    }
    let t = (elapsed.as_secs_f32() / track.duration.as_secs_f32()).clamp(0.0, 1.0);
    let e = ease_in_out_cubic(t);
    ClusterTileAnimRect {
        center: Vec2 {
            x: track.from.center.x + (track.to.center.x - track.from.center.x) * e,
            y: track.from.center.y + (track.to.center.y - track.from.center.y) * e,
        },
        size: Vec2 {
            x: track.from.size.x + (track.to.size.x - track.from.size.x) * e,
            y: track.from.size.y + (track.to.size.y - track.from.size.y) * e,
        },
        alpha: (track.from.alpha + (track.to.alpha - track.from.alpha) * e).clamp(0.0, 1.0),
    }
}

pub(crate) fn set_cluster_tile_target(
    tracks: &mut ClusterTileTracks,
    current_rect: Option<ClusterTileAnimRect>,
    node_id: NodeId,
    target_rect: Rect,
    now: Instant,
    duration_ms: u64,
    start_delay_ms: u64,
) {
    let target = anim_rect_from_tile_rect(target_rect, 1.0);
    // The stagger delay only applies to a fresh entry (member was hidden). A
    // reflow of an already-visible tile must move immediately, with no delay.
    let mut entering = false;
    let current = tracks
        .get(&node_id)
        .map(|track| cluster_tile_rect_for_track(track, now))
        .or_else(|| {
            current_rect.map(|current| {
                if current.alpha <= ALPHA_INVISIBLE {
                    entering = true;
                    entry_rect_for_target(target)
                } else {
                    current
                }
            })
        })
        .unwrap_or_else(|| {
            entering = true;
            entry_rect_for_target(target)
        });

    if tracks
        .get(&node_id)
        .is_some_and(|track| same_rect(track.to, target))
    {
        return;
    }
    if same_rect(current, target) {
        tracks.remove(&node_id);
        return;
    }

    let start_delay = if entering {
        Duration::from_millis(start_delay_ms)
    } else {
        Duration::ZERO
    };
    tracks.insert(
        node_id,
        ClusterTileTrack {
            from: current,
            to: target,
            started_at: now,
            start_delay,
            duration: Duration::from_millis(duration_ms.max(1)),
        },
    );
}

pub(crate) fn set_cluster_tile_target_from_anim_rect(
    tracks: &mut ClusterTileTracks,
    from: ClusterTileAnimRect,
    node_id: NodeId,
    target_rect: Rect,
    now: Instant,
    duration_ms: u64,
    start_delay_ms: u64,
) {
    let target = anim_rect_from_tile_rect(target_rect, 1.0);
    if same_rect(from, target) {
        tracks.remove(&node_id);
        return;
    }
    tracks.insert(
        node_id,
        ClusterTileTrack {
            from,
            to: target,
            started_at: now,
            start_delay: Duration::from_millis(start_delay_ms),
            duration: Duration::from_millis(duration_ms.max(1)),
        },
    );
}

/// Pin a tile at a fixed pose (from == to) so it neither moves nor scales. Used to
/// hold a growing tile at its old slot while we wait for the client to commit the
/// bigger buffer — moving it before the buffer arrives would upscale the small
/// capture. The reflow replaces this with a real morph track once the buffer lands.
pub(crate) fn hold_cluster_tile_rect(
    tracks: &mut ClusterTileTracks,
    node_id: NodeId,
    rect: ClusterTileAnimRect,
    now: Instant,
) {
    tracks.insert(
        node_id,
        ClusterTileTrack {
            from: rect,
            to: rect,
            started_at: now,
            start_delay: Duration::ZERO,
            duration: Duration::from_millis(10_000),
        },
    );
}

pub(crate) fn cluster_tile_target_rect(
    tracks: &ClusterTileTracks,
    node_id: NodeId,
) -> Option<Rect> {
    tracks
        .get(&node_id)
        .map(|track| tile_rect_from_anim_rect(track.to))
}

pub(crate) fn cluster_tile_rect_for(
    tracks: &ClusterTileTracks,
    node_id: NodeId,
    now: Instant,
) -> Option<ClusterTileAnimRect> {
    tracks
        .get(&node_id)
        .map(|track| cluster_tile_rect_for_track(track, now))
}

pub(crate) fn retain_live_cluster_tile_tracks(
    tracks: &mut ClusterTileTracks,
    field: &Field,
    now: Instant,
) {
    tracks.retain(|id, track| {
        field.node(*id).is_some()
            && now.saturating_duration_since(track.started_at) < track.lifetime()
    });
}

pub(crate) fn cluster_tile_tracks_animating(tracks: &ClusterTileTracks, now: Instant) -> bool {
    tracks
        .values()
        .any(|track| now.saturating_duration_since(track.started_at) < track.lifetime())
}
