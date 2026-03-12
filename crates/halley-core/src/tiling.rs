// Weighted tiling for an Active cluster, with an optional "Swap Bay" when n > MAX_MAJOR.
// - Field-space overlap rules are enforced elsewhere; this module only lays out within a
//   cluster's active rect.
// - In Tiled mode: NO overlap, ever.
// - In Stacked mode: (separate module) overlap allowed.
//
// Integration notes:
// - Replace NodeId with your real NodeId type.
// - Replace Rect with your existing geometry type if you already have one.
//
// Author: Dustin Pilgrim
// License: GPL-3.0-only (match your repo)

#![allow(dead_code)]

use core::cmp::Ordering;
use std::collections::HashMap;

pub type NodeId = u64;

pub const MAX_MAJOR: usize = 4;

/// A simple float rect. Replace with your own if you already have one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, pad: f32) -> Rect {
        Rect {
            x: self.x + pad,
            y: self.y + pad,
            w: (self.w - 2.0 * pad).max(0.0),
            h: (self.h - 2.0 * pad).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Primary,
    Secondary,
    Bay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: NodeId,
    pub kind: TileKind,
    pub rect: Rect,
}

/// Output of the tiled layout solver.
#[derive(Clone, Debug, PartialEq)]
pub struct TilingOutput {
    /// Major tiles (primary + secondaries). These are the usable windows.
    pub majors: Vec<Tile>,
    /// Visible bay tiles (icons/thumbnails) if n > MAX_MAJOR.
    pub bay: Vec<Tile>,
    /// The bay viewport rect (strip). `None` if there is no bay.
    pub bay_viewport: Option<Rect>,
    /// Major viewport rect (excluding bay strip, if present).
    pub major_viewport: Rect,
}

/// Layout knobs. Keep these stable to avoid the layout "jumping" when windows are added.
#[derive(Clone, Copy, Debug)]
pub struct TilingParams {
    /// If n > MAX_MAJOR, we allocate a bay strip. This is the fraction of total width.
    pub bay_width_frac: f32,
    /// Minimum bay width in pixels (prevents tiny unusable bay).
    pub bay_min_w: f32,
    /// Padding inside the major viewport.
    pub major_pad: f32,
    /// Padding inside the bay viewport.
    pub bay_pad: f32,
    /// Bay grid columns (icon squares). 2 is a good default for a thin strip.
    pub bay_cols: usize,
    /// Spacing between bay squares.
    pub bay_gap: f32,
    /// Min/max clamp for primary column width fraction (within major viewport).
    pub primary_min_frac: f32,
    pub primary_max_frac: f32,
}

impl Default for TilingParams {
    fn default() -> Self {
        Self {
            bay_width_frac: 0.18,
            bay_min_w: 110.0,
            major_pad: 10.0,
            bay_pad: 10.0,
            bay_cols: 2,
            bay_gap: 8.0,
            // Primary should feel "dominant" but not swallow everything.
            primary_min_frac: 0.45,
            primary_max_frac: 0.75,
        }
    }
}

/// Weights are only meaningful for "majors". Bay membership is separate.
/// Your compositor can feed weights from:
/// - user adjustments (strongest)
/// - config roles (strong)
/// - intrinsic_size/focus recency (weak hints)
#[derive(Clone, Debug)]
pub struct WeightModel {
    /// Per-node weight (any positive float). Missing nodes default to 1.0.
    pub weights: HashMap<NodeId, f32>,
}

impl WeightModel {
    pub fn new() -> Self {
        Self {
            weights: HashMap::new(),
        }
    }

    pub fn weight_of(&self, id: NodeId) -> f32 {
        self.weights.get(&id).copied().unwrap_or(1.0).max(0.0001)
    }
}

impl Default for WeightModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Select majors/bay deterministically.
///
/// `recency_order` is most-recent-first (index 0 = newest). If you don't have MRU yet,
/// pass `ids` in a stable order and it will remain stable.
///
/// Strategy:
/// - If n <= MAX_MAJOR: all majors, no bay.
/// - Else: choose top MAX_MAJOR by (weight desc, recency asc), remainder in bay.
///
/// IMPORTANT UX RULE:
/// - When adding a new window, you may want to bias it to start in bay unless focused.
///   That policy should live above this function (cluster policy/input), not here.
pub fn select_majors_and_bay(
    ids: &[NodeId],
    weight_model: &WeightModel,
    recency_order: &[NodeId],
) -> (Vec<NodeId>, Vec<NodeId>) {
    if ids.len() <= MAX_MAJOR {
        return (ids.to_vec(), Vec::new());
    }

    // Build recency rank map: lower is newer (better).
    let mut rank: HashMap<NodeId, usize> = HashMap::with_capacity(recency_order.len());
    for (i, &id) in recency_order.iter().enumerate() {
        rank.insert(id, i);
    }
    let fallback_rank_base = recency_order.len() + 10_000;

    let mut sorted = ids.to_vec();
    sorted.sort_by(|&a, &b| {
        let wa = weight_model.weight_of(a);
        let wb = weight_model.weight_of(b);

        // Descending weight
        match wb.partial_cmp(&wa).unwrap_or(Ordering::Equal) {
            Ordering::Equal => {
                // Ascending recency rank
                let ra = *rank.get(&a).unwrap_or(&fallback_rank_base);
                let rb = *rank.get(&b).unwrap_or(&fallback_rank_base);
                ra.cmp(&rb)
            }
            other => other,
        }
    });

    let majors: Vec<NodeId> = sorted.iter().copied().take(MAX_MAJOR).collect();
    let bay: Vec<NodeId> = sorted.iter().copied().skip(MAX_MAJOR).collect();
    (majors, bay)
}

/// Compute a tiled layout (weighted) for majors + optional bay.
///
/// `focused` should be a node inside the cluster (member). If it's in majors,
/// it becomes primary; otherwise the first major becomes primary.
///
/// `bay_scroll_rows` allows a scrollable bay (row offset). For v1, just pass 0.
pub fn layout_weighted_tiling(
    cluster_rect: Rect,
    members: &[NodeId],
    focused: Option<NodeId>,
    weight_model: &WeightModel,
    recency_order: &[NodeId],
    params: TilingParams,
    bay_scroll_rows: usize,
) -> TilingOutput {
    let (majors, bay_all) = select_majors_and_bay(members, weight_model, recency_order);

    // Split bay strip if needed.
    let has_bay = !bay_all.is_empty();
    let (major_viewport, bay_viewport) = if has_bay {
        let mut bay_w = (cluster_rect.w * params.bay_width_frac).max(params.bay_min_w);
        bay_w = bay_w.min(cluster_rect.w * 0.45); // never let bay dominate
        let major_w = (cluster_rect.w - bay_w).max(0.0);

        let major = Rect {
            x: cluster_rect.x,
            y: cluster_rect.y,
            w: major_w,
            h: cluster_rect.h,
        };
        let bay = Rect {
            x: cluster_rect.x + major_w,
            y: cluster_rect.y,
            w: bay_w,
            h: cluster_rect.h,
        };
        (major, Some(bay))
    } else {
        (cluster_rect, None)
    };

    let major = major_viewport.inset(params.major_pad);

    // Determine primary.
    let primary_id = match focused {
        Some(f) if majors.contains(&f) => f,
        _ => majors.first().copied().unwrap_or(0),
    };

    // Normalize weights among majors.
    let major_weight_sum: f32 = majors.iter().map(|&id| weight_model.weight_of(id)).sum();

    // Special case: 0 members
    if majors.is_empty() {
        return TilingOutput {
            majors: vec![],
            bay: vec![],
            bay_viewport,
            major_viewport,
        };
    }

    // Single window: it takes all major area.
    if majors.len() == 1 {
        return TilingOutput {
            majors: vec![Tile {
                id: primary_id,
                kind: TileKind::Primary,
                rect: major,
            }],
            bay: layout_bay_tiles(bay_viewport, &bay_all, params, bay_scroll_rows),
            bay_viewport,
            major_viewport,
        };
    }

    // Split major area into primary column and secondary column.
    let primary_w_norm = weight_model.weight_of(primary_id) / major_weight_sum.max(0.0001);

    // Clamp primary fraction to avoid extreme layouts.
    let primary_frac = clamp(
        primary_w_norm,
        params.primary_min_frac,
        params.primary_max_frac,
    );

    let primary_w = major.w * primary_frac;
    let secondary_w = (major.w - primary_w).max(0.0);

    let primary_rect = Rect {
        x: major.x,
        y: major.y,
        w: primary_w,
        h: major.h,
    };
    let secondary_rect = Rect {
        x: major.x + primary_w,
        y: major.y,
        w: secondary_w,
        h: major.h,
    };

    // Secondary ids (in stable order: majors order with primary removed).
    let secondary_ids: Vec<NodeId> = majors
        .iter()
        .copied()
        .filter(|&id| id != primary_id)
        .collect();

    // Normalize weights for secondaries.
    let secondary_weight_sum: f32 = secondary_ids
        .iter()
        .map(|&id| weight_model.weight_of(id))
        .sum();

    // Lay out secondaries as vertical stack in secondary column.
    let mut tiles = Vec::with_capacity(majors.len());
    tiles.push(Tile {
        id: primary_id,
        kind: TileKind::Primary,
        rect: primary_rect,
    });

    if secondary_ids.is_empty() || secondary_rect.w <= 0.0 {
        // Edge case: no room for secondaries; primary takes all.
        tiles[0].rect = major;
    } else {
        let mut y = secondary_rect.y;
        for (i, &id) in secondary_ids.iter().enumerate() {
            let w = weight_model.weight_of(id);
            let frac = (w / secondary_weight_sum.max(0.0001)).max(0.0);

            // Last one takes remaining height to avoid float gaps.
            let h = if i + 1 == secondary_ids.len() {
                secondary_rect.bottom() - y
            } else {
                secondary_rect.h * frac
            };

            let rect = Rect {
                x: secondary_rect.x,
                y,
                w: secondary_rect.w,
                h: h.max(0.0),
            };
            tiles.push(Tile {
                id,
                kind: TileKind::Secondary,
                rect,
            });
            y += h;
        }
    }

    // (Optional) ensure stable ordering: primary first, then secondaries in order.
    // This is already the case.

    TilingOutput {
        majors: tiles,
        bay: layout_bay_tiles(bay_viewport, &bay_all, params, bay_scroll_rows),
        bay_viewport,
        major_viewport,
    }
}

fn layout_bay_tiles(
    bay_viewport: Option<Rect>,
    bay_all: &[NodeId],
    params: TilingParams,
    bay_scroll_rows: usize,
) -> Vec<Tile> {
    let Some(bay_vp) = bay_viewport else {
        return vec![];
    };
    if bay_all.is_empty() {
        return vec![];
    }

    let bay = bay_vp.inset(params.bay_pad);
    let cols = params.bay_cols.max(1);

    // Compute square size
    let total_gap_w = params.bay_gap * (cols.saturating_sub(1)) as f32;
    let cell_w = ((bay.w - total_gap_w) / cols as f32).max(0.0);

    // Square cell: use min(cell_w, a height-derived value if you want)
    let cell = cell_w;

    let rows_visible = if cell <= 0.0 {
        0
    } else {
        // How many full rows fit?
        ((bay.h + params.bay_gap) / (cell + params.bay_gap)).floor() as usize
    };

    if rows_visible == 0 {
        return vec![];
    }

    let first_index = bay_scroll_rows * cols;
    let max_items = rows_visible * cols;

    let slice = bay_all.iter().copied().skip(first_index).take(max_items);

    let mut out = Vec::new();
    for (i, id) in slice.enumerate() {
        let col = i % cols;
        let row = i / cols;

        let x = bay.x + col as f32 * (cell + params.bay_gap);
        let y = bay.y + row as f32 * (cell + params.bay_gap);

        out.push(Tile {
            id,
            kind: TileKind::Bay,
            rect: Rect {
                x,
                y,
                w: cell,
                h: cell,
            },
        });
    }

    out
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<NodeId> {
        (0..n).collect()
    }

    #[test]
    fn n_le_4_has_no_bay() {
        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 600.0,
        };
        let members = ids(4);
        let wm = WeightModel::new();
        let out = layout_weighted_tiling(
            cluster,
            &members,
            Some(0),
            &wm,
            &members,
            TilingParams::default(),
            0,
        );
        assert!(out.bay.is_empty());
        assert!(out.bay_viewport.is_none());
        assert_eq!(out.majors.len(), 4);
    }

    #[test]
    fn n_gt_4_has_bay_and_4_majors() {
        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 700.0,
        };
        let members = ids(9);
        let wm = WeightModel::new();
        let out = layout_weighted_tiling(
            cluster,
            &members,
            Some(0),
            &wm,
            &members,
            TilingParams::default(),
            0,
        );
        assert!(out.bay_viewport.is_some());
        assert_eq!(out.majors.len(), 4);
        assert!(!out.bay.is_empty());
    }

    #[test]
    fn primary_is_focused_when_in_majors() {
        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 700.0,
        };
        let members = ids(4);
        let wm = WeightModel::new();
        let out = layout_weighted_tiling(
            cluster,
            &members,
            Some(2),
            &wm,
            &members,
            TilingParams::default(),
            0,
        );

        let primary = out
            .majors
            .iter()
            .find(|t| t.kind == TileKind::Primary)
            .unwrap();
        assert_eq!(primary.id, 2);
    }

    #[test]
    fn no_overlap_in_output_rects() {
        // Crude overlap test: ensure primary + secondaries do not overlap in area.
        fn overlaps(a: Rect, b: Rect) -> bool {
            let ax2 = a.x + a.w;
            let ay2 = a.y + a.h;
            let bx2 = b.x + b.w;
            let by2 = b.y + b.h;
            !(ax2 <= b.x || bx2 <= a.x || ay2 <= b.y || by2 <= a.y)
        }

        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            w: 1400.0,
            h: 800.0,
        };
        let members = ids(4);
        let wm = WeightModel::new();
        let out = layout_weighted_tiling(
            cluster,
            &members,
            Some(0),
            &wm,
            &members,
            TilingParams::default(),
            0,
        );

        for i in 0..out.majors.len() {
            for j in (i + 1)..out.majors.len() {
                assert!(!overlaps(out.majors[i].rect, out.majors[j].rect));
            }
        }
    }

    #[test]
    fn weight_influences_primary_width() {
        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 600.0,
        };
        let members = ids(4);

        let mut wm = WeightModel::new();
        wm.weights.insert(0, 10.0);
        wm.weights.insert(1, 1.0);
        wm.weights.insert(2, 1.0);
        wm.weights.insert(3, 1.0);

        let out = layout_weighted_tiling(
            cluster,
            &members,
            Some(0),
            &wm,
            &members,
            TilingParams::default(),
            0,
        );
        let primary = out
            .majors
            .iter()
            .find(|t| t.kind == TileKind::Primary)
            .unwrap();
        // Primary should be clamped but significantly wide
        assert!(primary.rect.w > 500.0);
    }
}
