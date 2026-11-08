use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    pub x: f32,
    pub y: f32,
    pub aspect: f32,
    pub stable_key: u64,
    pub weight: f32,
    /// The smallest slot this item accepts, such as a window's minimum size.
    /// Zero means no limit. The packer shrinks other items instead of this
    /// one, so a constrained window never overlaps its neighbours.
    pub min_w: f32,
    pub min_h: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rect {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

/// The Halley mosaic packer, used by Arrange and Cluster Composer. Items retain
/// field-space reading order while larger/aspect-heavy windows are placed
/// first into a MaxRects atlas, allowing small tiles to fill cuts without
/// overlapping.
pub(crate) fn mosaic(
    items: &[Item],
    screen_w: i32,
    screen_h: i32,
    gap: f32,
    max_rows: usize,
) -> Vec<Rect> {
    let n = items.len();
    let mut out = vec![Rect::default(); n];
    if n == 0 {
        return out;
    }

    let margin = (gap * 2.0).max(32.0);
    let avail_w = (screen_w as f32 - margin * 2.0).max(64.0);
    let avail_h = (screen_h as f32 - margin * 2.0).max(64.0);
    if n == 1 {
        out[0] = single_slot(items[0], screen_w, screen_h, avail_w, avail_h);
        return out;
    }

    let max_rows = max_rows.clamp(1, 5).min(n);
    let sizes = natural_sizes(items, avail_w, avail_h, max_rows, gap);
    let order = packing_order(items, &sizes);
    let mut best: Option<PackAttempt> = None;

    for rows in 1..=max_rows {
        let pack_h = pack_height(avail_h, rows, gap);
        for width in packing_widths(&sizes, rows, avail_w, pack_h, gap) {
            if let Some(attempt) =
                best_pack_for_width(items, &sizes, &order, width, pack_h, gap, rows, max_rows)
                && best
                    .as_ref()
                    .is_none_or(|current| attempt.score < current.score)
            {
                best = Some(attempt);
            }
        }
    }

    let Some(best) = best else {
        return grid_fallback(items, screen_w, screen_h, gap);
    };
    let offset_x = screen_w as f32 * 0.5 - best.block_w * 0.5 - best.min_x;
    let offset_y = screen_h as f32 * 0.5 - best.block_h * 0.5 - best.min_y;
    for (index, rect) in best.rects.into_iter().enumerate() {
        out[index] = Rect {
            cx: rect.cx + offset_x,
            cy: rect.cy + offset_y,
            ..rect
        };
    }
    out
}

/// Grows a packed mosaic until it uses the whole screen, for callers that
/// want the space more than the packer's breathing room (Arrange).
///
/// First the whole block scales up about its centre until it meets the
/// screen edges. Each tile is treated as a cell that owns half the gap on
/// every side, so scaling cells keeps the gaps constant and cannot create an
/// overlap. Then each tile grows into free space beside it, alternating
/// horizontal and vertical passes, until it meets a neighbour, the screen
/// edge, or `max_stretch`: how far its shape may drift from the item's own
/// aspect (0.25 allows a quarter wider or taller). Tiles only ever grow, so
/// minimum sizes stay satisfied. A single item is left as the packer placed
/// it, since filling it would only reproduce maximize.
pub(crate) fn fill(rects: &mut [Rect], items: &[Item], screen_w: i32, screen_h: i32, gap: f32) {
    const MAX_STRETCH: f32 = 0.25;
    const PASSES: usize = 3;
    if rects.len() < 2 || rects.len() != items.len() {
        return;
    }
    let half = gap.max(0.0) * 0.5;
    // Cell space: the screen grows by half a gap so edge tiles reach it.
    let bounds = FreeRect {
        x: -half,
        y: -half,
        w: screen_w as f32 + gap.max(0.0),
        h: screen_h as f32 + gap.max(0.0),
    };
    let mut cells = rects
        .iter()
        .map(|rect| FreeRect {
            x: rect.cx - rect.w * 0.5 - half,
            y: rect.cy - rect.h * 0.5 - half,
            w: rect.w + half * 2.0,
            h: rect.h + half * 2.0,
        })
        .collect::<Vec<_>>();

    scale_cells_to_bounds(&mut cells, bounds);
    // Sizes the packer chose (the focused window's larger slot, bigger
    // windows over smaller ones) must survive filling, so each tile may gain
    // at most `MAX_STRETCH` in area over its scaled size.
    let max_areas = cells
        .iter()
        .map(|cell| (cell.w - half * 2.0) * (cell.h - half * 2.0) * (1.0 + MAX_STRETCH))
        .collect::<Vec<_>>();

    let order = spatial_order(items);
    // Grow, then centre what could not grow so its neighbours get a share of
    // the leftover space on the next pass. Ending on a centring step keeps
    // any space that remains balanced instead of piled on one side.
    for _ in 0..PASSES {
        for axis in [Axis::X, Axis::Y] {
            for &index in &order {
                let limits = GrowthLimits {
                    aspect: items[index].aspect,
                    max_stretch: MAX_STRETCH,
                    max_area: max_areas[index],
                };
                grow_cell(&mut cells, index, bounds, half, limits, axis);
            }
            for &index in &order {
                centre_cell(&mut cells, index, bounds, axis);
            }
        }
    }

    for (rect, cell) in rects.iter_mut().zip(cells) {
        let w = (cell.w - half * 2.0).max(1.0);
        let h = (cell.h - half * 2.0).max(1.0);
        *rect = Rect {
            cx: cell.x + cell.w * 0.5,
            cy: cell.y + cell.h * 0.5,
            w,
            h,
        };
    }
}

fn scale_cells_to_bounds(cells: &mut [FreeRect], bounds: FreeRect) {
    let min_x = cells.iter().map(|cell| cell.x).fold(f32::MAX, f32::min);
    let min_y = cells.iter().map(|cell| cell.y).fold(f32::MAX, f32::min);
    let max_x = cells
        .iter()
        .map(|cell| cell.x + cell.w)
        .fold(f32::MIN, f32::max);
    let max_y = cells
        .iter()
        .map(|cell| cell.y + cell.h)
        .fold(f32::MIN, f32::max);
    let block_w = max_x - min_x;
    let block_h = max_y - min_y;
    if block_w <= 0.0 || block_h <= 0.0 {
        return;
    }
    let scale = (bounds.w / block_w).min(bounds.h / block_h);
    if scale <= 1.0 {
        return;
    }
    // Centre the scaled block on the screen.
    let offset_x = bounds.x + (bounds.w - block_w * scale) * 0.5;
    let offset_y = bounds.y + (bounds.h - block_h * scale) * 0.5;
    for cell in cells {
        cell.x = offset_x + (cell.x - min_x) * scale;
        cell.y = offset_y + (cell.y - min_y) * scale;
        cell.w *= scale;
        cell.h *= scale;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// Extends one cell along `axis` into the free space on both sides, sharing
/// the growth evenly so the tile stays near where the packer put it.
#[derive(Clone, Copy)]
struct GrowthLimits {
    /// The item's own width/height ratio.
    aspect: f32,
    /// How far the tile's shape may drift from `aspect`.
    max_stretch: f32,
    /// The largest visible tile area allowed.
    max_area: f32,
}

fn grow_cell(
    cells: &mut [FreeRect],
    index: usize,
    bounds: FreeRect,
    half_gap: f32,
    limits: GrowthLimits,
    axis: Axis,
) {
    let cell = cells[index];
    let aspect = limits.aspect.clamp(0.25, 4.5);
    let max_stretch = limits.max_stretch;
    let (before, after) = free_room(cells, index, bounds, axis);
    let (start, end, span_start, span_end) = match axis {
        Axis::X => (cell.x, cell.x + cell.w, cell.y, cell.y + cell.h),
        Axis::Y => (cell.y, cell.y + cell.h, cell.x, cell.x + cell.w),
    };

    // The visible tile, not the cell, carries the window's shape.
    let tile_len = (end - start) - half_gap * 2.0;
    let tile_cross = (span_end - span_start) - half_gap * 2.0;
    let shape_limit = match axis {
        Axis::X => tile_cross * aspect * (1.0 + max_stretch),
        Axis::Y => tile_cross / aspect * (1.0 + max_stretch),
    };
    let area_limit = limits.max_area / tile_cross.max(1.0);
    let want = (shape_limit.min(area_limit) - tile_len)
        .max(0.0)
        .min(before + after);
    if want <= ROOM_EPS {
        return;
    }
    let mut grow_before = (want * 0.5).min(before);
    let grow_after = (want - grow_before).min(after);
    grow_before = (want - grow_after).min(before);

    let cell = &mut cells[index];
    match axis {
        Axis::X => {
            cell.x -= grow_before;
            cell.w += grow_before + grow_after;
        }
        Axis::Y => {
            cell.y -= grow_before;
            cell.h += grow_before + grow_after;
        }
    }
}

/// Moves a cell to the middle of the free space around it along `axis`, so
/// a tile that has stopped growing hands the leftover space to its
/// neighbours evenly instead of leaving it all on one side.
fn centre_cell(cells: &mut [FreeRect], index: usize, bounds: FreeRect, axis: Axis) {
    let (before, after) = free_room(cells, index, bounds, axis);
    let shift = (after - before) * 0.5;
    if shift.abs() <= ROOM_EPS {
        return;
    }
    match axis {
        Axis::X => cells[index].x += shift,
        Axis::Y => cells[index].y += shift,
    }
}

const ROOM_EPS: f32 = 0.5;

/// Free distance from a cell to the nearest neighbour (or the bounds) on
/// each side along `axis`, counting only neighbours that share its span on
/// the other axis.
fn free_room(cells: &[FreeRect], index: usize, bounds: FreeRect, axis: Axis) -> (f32, f32) {
    const EPS: f32 = ROOM_EPS;
    let cell = cells[index];
    let (start, end, span_start, span_end, lo, hi) = match axis {
        Axis::X => (
            cell.x,
            cell.x + cell.w,
            cell.y,
            cell.y + cell.h,
            bounds.x,
            bounds.x + bounds.w,
        ),
        Axis::Y => (
            cell.y,
            cell.y + cell.h,
            cell.x,
            cell.x + cell.w,
            bounds.y,
            bounds.y + bounds.h,
        ),
    };
    let mut before = start - lo;
    let mut after = hi - end;
    for (other_index, other) in cells.iter().enumerate() {
        if other_index == index {
            continue;
        }
        let (o_start, o_end, o_span_start, o_span_end) = match axis {
            Axis::X => (other.x, other.x + other.w, other.y, other.y + other.h),
            Axis::Y => (other.y, other.y + other.h, other.x, other.x + other.w),
        };
        if o_span_start >= span_end - EPS || o_span_end <= span_start + EPS {
            continue;
        }
        if o_end <= start + EPS {
            before = before.min(start - o_end);
        } else if o_start >= end - EPS {
            after = after.min(o_start - end);
        }
    }
    (before.max(0.0), after.max(0.0))
}

fn single_slot(item: Item, screen_w: i32, screen_h: i32, avail_w: f32, avail_h: f32) -> Rect {
    let aspect = item.aspect.clamp(0.25, 4.5);
    let max_w = (screen_w as f32 * 0.62).min(avail_w).max(64.0);
    let max_h = (screen_h as f32 * 0.56).min(avail_h).max(64.0);
    let mut w = max_w;
    let mut h = w / aspect;
    if h > max_h {
        h = max_h;
        w = h * aspect;
    }
    Rect {
        cx: screen_w as f32 * 0.5,
        cy: screen_h as f32 * 0.5,
        w: w.clamp(64.0, max_w),
        h: h.clamp(64.0, max_h),
    }
}

#[derive(Clone, Copy, Debug)]
struct Size {
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
    rects: Vec<Rect>,
    min_x: f32,
    min_y: f32,
    block_w: f32,
    block_h: f32,
    score: f32,
}

fn pack_height(avail_h: f32, rows: usize, gap: f32) -> f32 {
    let rows = rows.max(1) as f32;
    let min_for_rows = rows * 74.0 + gap * (rows - 1.0);
    avail_h.max(160.0).max(min_for_rows).min(avail_h)
}

fn natural_sizes(items: &[Item], avail_w: f32, pack_h: f32, rows: usize, gap: f32) -> Vec<Size> {
    let rows = rows.max(1) as f32;
    let row_gap = gap * (rows - 1.0).max(0.0);
    let nominal_h = ((pack_h - row_gap).max(64.0) / rows * 0.92).clamp(72.0, pack_h * 0.92);
    let avg_weight =
        (items.iter().map(|item| item.weight.max(1.0)).sum::<f32>() / items.len() as f32).max(1.0);
    let base_area = nominal_h * nominal_h * 1.35;

    items
        .iter()
        .map(|item| {
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
            Size {
                w: w.max(48.0),
                h: h.max(36.0),
            }
        })
        .collect()
}

fn spatial_order(items: &[Item]) -> Vec<usize> {
    let mut order = (0..items.len()).collect::<Vec<_>>();
    if items.len() < 2 {
        return order;
    }
    let (min_y, max_y) = items.iter().fold((f32::MAX, f32::MIN), |(lo, hi), item| {
        (lo.min(item.y), hi.max(item.y))
    });
    let span_y = (max_y - min_y).max(1.0);
    let band = (span_y / (items.len() as f32).sqrt().ceil().max(1.0)).max(1.0);
    order.sort_by(|&a, &b| {
        let band_a = ((items[a].y - min_y) / band).floor() as i32;
        let band_b = ((items[b].y - min_y) / band).floor() as i32;
        band_a.cmp(&band_b).then_with(|| {
            items[a]
                .x
                .partial_cmp(&items[b].x)
                .unwrap_or(Ordering::Equal)
                .then_with(|| items[a].stable_key.cmp(&items[b].stable_key))
        })
    });
    order
}

fn packing_order(items: &[Item], sizes: &[Size]) -> Vec<usize> {
    let spatial = spatial_order(items);
    let mut rank = vec![0; items.len()];
    for (index, item) in spatial.into_iter().enumerate() {
        rank[item] = index;
    }
    let mut order = (0..items.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        let area_a = sizes[a].w * sizes[a].h;
        let area_b = sizes[b].w * sizes[b].h;
        area_b
            .partial_cmp(&area_a)
            .unwrap_or(Ordering::Equal)
            .then_with(|| rank[a].cmp(&rank[b]))
            .then_with(|| items[a].stable_key.cmp(&items[b].stable_key))
    });
    order
}

fn packing_widths(sizes: &[Size], rows: usize, avail_w: f32, pack_h: f32, gap: f32) -> Vec<f32> {
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
        .map(|width| width.clamp(widest.min(avail_w.max(64.0)), avail_w.max(64.0)))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn best_pack_for_width(
    items: &[Item],
    sizes: &[Size],
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
    items: &[Item],
    sizes: &[Size],
    order: &[usize],
    width: f32,
    avail_h: f32,
    gap: f32,
    scale: f32,
) -> Option<PackAttempt> {
    let mut rects = vec![Rect::default(); items.len()];
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

    for &index in order {
        let w = (sizes[index].w * scale).max(items[index].min_w);
        let h = (sizes[index].h * scale).max(items[index].min_h);
        let used = best_free_rect(&free, w + gap, h + gap)?;
        rects[index] = Rect {
            cx: used.x + w * 0.5,
            cy: used.y + h * 0.5,
            w,
            h,
        };
        min_x = min_x.min(used.x);
        min_y = min_y.min(used.y);
        max_x = max_x.max(used.x + w);
        max_y = max_y.max(used.y + h);
        content_area += w * h;
        split_free_rects(
            &mut free,
            FreeRect {
                x: used.x,
                y: used.y,
                w: w + gap,
                h: h + gap,
            },
        );
        prune_free_rects(&mut free);
    }

    let block_w = max_x - min_x;
    let block_h = max_y - min_y;
    if block_w > width + 0.5 || block_h > avail_h + 0.5 {
        return None;
    }
    Some(PackAttempt {
        rects,
        min_x,
        min_y,
        block_w,
        block_h,
        score: 1.0 - content_area / (block_w * block_h).max(1.0),
    })
}

fn best_free_rect(free: &[FreeRect], need_w: f32, need_h: f32) -> Option<FreeRect> {
    free.iter()
        .filter(|rect| need_w <= rect.w + 0.5 && need_h <= rect.h + 0.5)
        .min_by(|a, b| {
            free_rect_score(a, need_w, need_h)
                .partial_cmp(&free_rect_score(b, need_w, need_h))
                .unwrap_or(Ordering::Equal)
        })
        .copied()
}

fn free_rect_score(rect: &FreeRect, need_w: f32, need_h: f32) -> (f32, f32, f32, f32, f32) {
    let leftover_w = (rect.w - need_w).max(0.0);
    let leftover_h = (rect.h - need_h).max(0.0);
    (
        rect.y,
        leftover_w.min(leftover_h),
        rect.w * rect.h - need_w * need_h,
        rect.x,
        rect.w,
    )
}

fn split_free_rects(free: &mut Vec<FreeRect>, used: FreeRect) {
    let mut next = Vec::with_capacity(free.len() + 4);
    for rect in free.drain(..) {
        if !intersects(rect, used) {
            next.push(rect);
            continue;
        }
        let right = rect.x + rect.w;
        let bottom = rect.y + rect.h;
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
        if used_right < right {
            next.push(FreeRect {
                x: used_right,
                y: rect.y,
                w: right - used_right,
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
        if used_bottom < bottom {
            next.push(FreeRect {
                x: rect.x,
                y: used_bottom,
                w: rect.w,
                h: bottom - used_bottom,
            });
        }
    }
    next.retain(|rect| rect.w > 1.0 && rect.h > 1.0);
    sort_free(&mut next);
    *free = next;
}

fn prune_free_rects(free: &mut Vec<FreeRect>) {
    let mut index = 0;
    while index < free.len() {
        if (0..free.len()).any(|other| index != other && contains(free[other], free[index])) {
            free.swap_remove(index);
        } else {
            index += 1;
        }
    }
    sort_free(free);
}

fn sort_free(free: &mut [FreeRect]) {
    free.sort_by(|a, b| {
        a.y.total_cmp(&b.y)
            .then_with(|| a.x.total_cmp(&b.x))
            .then_with(|| a.h.total_cmp(&b.h))
            .then_with(|| a.w.total_cmp(&b.w))
    });
}

fn contains(a: FreeRect, b: FreeRect) -> bool {
    b.x >= a.x - 0.5
        && b.y >= a.y - 0.5
        && b.x + b.w <= a.x + a.w + 0.5
        && b.y + b.h <= a.y + a.h + 0.5
}

fn intersects(a: FreeRect, b: FreeRect) -> bool {
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
    let aspect_deficit = ((screen_aspect * 0.95).max(1.35) - block_aspect).max(0.0);
    let too_wide = (block_aspect - screen_aspect * 1.65).max(0.0) * 0.25;
    let area_frac = block_area / (avail_w * avail_h).max(1.0);
    let too_small = (0.62 - area_frac).max(0.0);
    let unused_w = (1.0 - attempt.block_w / avail_w.max(1.0)).max(0.0);
    let unused_h = (1.0 - attempt.block_h / avail_h.max(1.0)).max(0.0);
    let avg_h = attempt.rects.iter().map(|rect| rect.h).sum::<f32>() / item_count.max(1) as f32;
    let line_penalty = if item_count >= 3 && attempt.block_h < avg_h * 1.35 && max_rows != 1 {
        0.45
    } else {
        0.0
    };
    let row_penalty = if item_count >= 4 {
        (rows as f32 / max_rows.max(1) as f32).clamp(0.0, 1.0) * 0.10
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

fn grid_fallback(items: &[Item], screen_w: i32, screen_h: i32, gap: f32) -> Vec<Rect> {
    let n = items.len();
    let mut out = vec![Rect::default(); n];
    let margin = (gap * 2.0).max(32.0);
    let avail_w = (screen_w as f32 - margin * 2.0).max(64.0);
    let avail_h = (screen_h as f32 - margin * 2.0).max(64.0);
    let screen_aspect = screen_w.max(1) as f32 / screen_h.max(1) as f32;
    let columns = (((n as f32) * screen_aspect).sqrt().ceil() as usize).clamp(1, n);
    let rows = n.div_ceil(columns);
    let cell_w = avail_w / columns as f32;
    let cell_h = avail_h / rows as f32;
    for (slot, &index) in spatial_order(items).iter().enumerate() {
        let max_w = (cell_w - gap).max(8.0);
        let max_h = (cell_h - gap).max(8.0);
        let aspect = items[index].aspect.clamp(0.25, 4.5);
        let mut w = max_w;
        let mut h = w / aspect;
        if h > max_h {
            h = max_h;
            w = h * aspect;
        }
        out[index] = Rect {
            cx: margin + (slot % columns) as f32 * cell_w + cell_w * 0.5,
            cy: margin + (slot / columns) as f32 * cell_h + cell_h * 0.5,
            w,
            h,
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(x: f32, y: f32, aspect: f32) -> Item {
        Item {
            x,
            y,
            aspect,
            stable_key: x.to_bits() as u64 ^ ((y.to_bits() as u64) << 32),
            weight: 800.0 * 600.0,
            min_w: 0.0,
            min_h: 0.0,
        }
    }

    #[test]
    fn mosaic_is_bounded_and_non_overlapping() {
        let items = vec![
            item(0.0, 0.0, 16.0 / 9.0),
            item(500.0, 20.0, 4.0 / 3.0),
            item(40.0, 500.0, 1.0),
            item(600.0, 550.0, 2.0),
        ];
        let slots = mosaic(&items, 1920, 840, 24.0, 3);
        for slot in &slots {
            assert!(slot.cx - slot.w * 0.5 >= 0.0);
            assert!(slot.cy - slot.h * 0.5 >= 0.0);
            assert!(slot.cx + slot.w * 0.5 <= 1920.0);
            assert!(slot.cy + slot.h * 0.5 <= 840.0);
        }
        for (index, a) in slots.iter().enumerate() {
            for b in &slots[index + 1..] {
                let overlap = a.cx - a.w * 0.5 < b.cx + b.w * 0.5
                    && a.cx + a.w * 0.5 > b.cx - b.w * 0.5
                    && a.cy - a.h * 0.5 < b.cy + b.h * 0.5
                    && a.cy + a.h * 0.5 > b.cy - b.h * 0.5;
                assert!(!overlap);
            }
        }
    }
}
