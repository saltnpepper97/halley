//! The space-filling mosaic used by Arrange.
//!
//! The screen is cut into pieces, one per window, so the windows cover it with
//! only the Field gap between them, as a tiled layout does. Which cuts are
//! made is chosen from the windows themselves rather than from a fixed shape
//! such as master and stack: every way of cutting the windows apart in Field
//! order is scored, and the layout whose pieces are closest to each window's
//! own shape and share of the screen wins. A wide window therefore tends to
//! get a wide band and a tall one a column, bigger windows get more room, and
//! the focused window gets a modest bonus. Minimum sizes are hard limits the
//! cut lines move to satisfy.

use std::collections::HashMap;
use std::rc::Rc;

use super::mosaic::{Item, Rect};

/// The focused window's extra share of the screen. Kept modest so focus makes
/// its piece bigger without turning every layout into master and stack.
pub(crate) const FOCUSED_SHARE: f32 = 1.4;

/// Up to this many windows every split position is tried; beyond it only the
/// splits nearest a balanced one are, which keeps the search fast.
const EXHAUSTIVE_LIMIT: usize = 6;

/// How much a piece's area straying from its share costs, relative to its
/// shape straying from the window's. Strong enough that bigger and focused
/// windows visibly get more room instead of every cut landing in the middle.
const AREA_COST: f32 = 2.0;

/// Cost of missing a minimum size; large enough that any layout which meets
/// every minimum beats one that does not.
const MIN_SIZE_COST: f32 = 100.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// A vertical cut: pieces side by side.
    X,
    /// A horizontal cut: pieces stacked.
    Y,
}

#[derive(Clone, Copy, Debug)]
struct Area {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Solver<'a> {
    items: &'a [Item],
    shares: Vec<f32>,
    total_share: f32,
    screen_area: f32,
    gap: f32,
    /// Area sizes are rounded to this many pixels for memoization.
    quantum: (f32, f32),
    memo: HashMap<(u128, i32, i32), Solution>,
}

/// How a set of windows is cut up. Cuts are stored as fractions, so a plan
/// found for one area size applies to a slightly different one.
enum Plan {
    Leaf(usize),
    Cut {
        axis: Axis,
        fraction: f32,
        first: Rc<Plan>,
        second: Rc<Plan>,
    },
}

#[derive(Clone)]
struct Solution {
    cost: f32,
    plan: Rc<Plan>,
}

/// Lays `items` out over a `screen_w` by `screen_h` area with `gap` between
/// pieces. The area is filled edge to edge; callers inset it for an outer gap.
pub(crate) fn partition(
    items: &[Item],
    focused: Option<usize>,
    screen_w: i32,
    screen_h: i32,
    gap: f32,
) -> Vec<Rect> {
    let n = items.len();
    let mut out = vec![Rect::default(); n];
    if n == 0 || screen_w <= 0 || screen_h <= 0 {
        return out;
    }
    let avg_weight = items.iter().map(|item| item.weight.max(1.0)).sum::<f32>() / n as f32;
    // Damped as in the packer, so a window with a huge default size (Steam)
    // gets more room, not the whole screen.
    let shares = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let base = (item.weight.max(1.0) / avg_weight.max(1.0))
                .sqrt()
                .clamp(0.68, 1.45);
            if focused == Some(index) {
                base * FOCUSED_SHARE
            } else {
                base
            }
        })
        .collect::<Vec<_>>();
    let mut solver = Solver {
        items,
        total_share: shares.iter().sum(),
        shares,
        screen_area: screen_w as f32 * screen_h as f32,
        gap: gap.max(0.0),
        quantum: (
            (screen_w as f32 / MEMO_STEPS).max(1.0),
            (screen_h as f32 / MEMO_STEPS).max(1.0),
        ),
        memo: HashMap::new(),
    };
    let all = (0..n).collect::<Vec<_>>();
    let area = Area {
        x: 0.0,
        y: 0.0,
        w: screen_w as f32,
        h: screen_h as f32,
    };
    let solution = solver.solve(&all, area);
    place(&solution.plan, area, gap.max(0.0), &mut out);
    out
}

/// Area sizes per screen dimension that memoization tells apart.
const MEMO_STEPS: f32 = 40.0;

fn place(plan: &Plan, area: Area, gap: f32, out: &mut [Rect]) {
    match plan {
        Plan::Leaf(index) => {
            out[*index] = Rect {
                cx: area.x + area.w * 0.5,
                cy: area.y + area.h * 0.5,
                w: area.w,
                h: area.h,
            };
        }
        Plan::Cut {
            axis,
            fraction,
            first,
            second,
        } => {
            let (first_area, second_area) = cut(area, *axis, *fraction, gap);
            place(first, first_area, gap, out);
            place(second, second_area, gap, out);
        }
    }
}

fn cut(area: Area, axis: Axis, fraction: f32, gap: f32) -> (Area, Area) {
    let length = match axis {
        Axis::X => area.w,
        Axis::Y => area.h,
    };
    let avail = (length - gap).max(2.0);
    let first_len = (avail * fraction).clamp(1.0, avail - 1.0);
    let second_len = avail - first_len;
    match axis {
        Axis::X => (
            Area {
                w: first_len,
                ..area
            },
            Area {
                x: area.x + first_len + gap,
                w: second_len,
                ..area
            },
        ),
        Axis::Y => (
            Area {
                h: first_len,
                ..area
            },
            Area {
                y: area.y + first_len + gap,
                h: second_len,
                ..area
            },
        ),
    }
}

impl Solver<'_> {
    fn solve(&mut self, set: &[usize], area: Area) -> Solution {
        let key = memo_key(set, area, self.quantum);
        if let Some(key) = key
            && let Some(found) = self.memo.get(&key)
        {
            return found.clone();
        }
        let solution = self.solve_uncached(set, area);
        if let Some(key) = key {
            self.memo.insert(key, solution.clone());
        }
        solution
    }

    fn solve_uncached(&mut self, set: &[usize], area: Area) -> Solution {
        if set.len() == 1 {
            return Solution {
                cost: self.piece_cost(set[0], area.w, area.h),
                plan: Rc::new(Plan::Leaf(set[0])),
            };
        }

        let mut best: Option<Solution> = None;
        for axis in [Axis::X, Axis::Y] {
            let sorted = self.field_order(set, axis);
            let (length, cross) = match axis {
                Axis::X => (area.w, area.h),
                Axis::Y => (area.h, area.w),
            };
            let avail = length - self.gap;
            if avail < 2.0 {
                continue;
            }
            for split in self.split_points(&sorted) {
                let (first, second) = sorted.split_at(split);
                for fraction in self.cut_fractions(first, second, avail, cross, axis) {
                    let (first_area, second_area) = cut(area, axis, fraction, self.gap);
                    let a = self.solve(first, first_area);
                    let b = self.solve(second, second_area);
                    let cost = a.cost + b.cost;
                    if best.as_ref().is_some_and(|best| cost >= best.cost) {
                        continue;
                    }
                    best = Some(Solution {
                        cost,
                        plan: Rc::new(Plan::Cut {
                            axis,
                            fraction,
                            first: a.plan,
                            second: b.plan,
                        }),
                    });
                }
            }
        }
        best.unwrap_or_else(|| self.equal_columns(set, area))
    }

    /// The windows in Field order along `axis`, so a cut keeps windows on
    /// the side of the screen they were already on.
    fn field_order(&self, set: &[usize], axis: Axis) -> Vec<usize> {
        let mut sorted = set.to_vec();
        sorted.sort_by(|&a, &b| {
            let (a_item, b_item) = (&self.items[a], &self.items[b]);
            let (a_main, a_cross, b_main, b_cross) = match axis {
                Axis::X => (a_item.x, a_item.y, b_item.x, b_item.y),
                Axis::Y => (a_item.y, a_item.x, b_item.y, b_item.x),
            };
            a_main
                .total_cmp(&b_main)
                .then(a_cross.total_cmp(&b_cross))
                .then(a_item.stable_key.cmp(&b_item.stable_key))
        });
        sorted
    }

    fn split_points(&self, sorted: &[usize]) -> Vec<usize> {
        let n = sorted.len();
        if n <= EXHAUSTIVE_LIMIT {
            return (1..n).collect();
        }
        // The split whose two sides' shares are closest to even, and its
        // neighbours.
        let total = sorted.iter().map(|&index| self.shares[index]).sum::<f32>();
        let mut running = 0.0;
        let mut balanced = 1;
        let mut closest = f32::MAX;
        for (split, &index) in sorted.iter().enumerate().take(n - 1) {
            running += self.shares[index];
            let distance = (running - total * 0.5).abs();
            if distance < closest {
                closest = distance;
                balanced = split + 1;
            }
        }
        let mut points = vec![balanced];
        if balanced > 1 {
            points.push(balanced - 1);
        }
        if balanced + 1 < n {
            points.push(balanced + 1);
        }
        points
    }

    /// Where along the cut's axis to try cutting, as a fraction of the space
    /// after the gap: in proportion to the two sides' shares, down the
    /// middle, and where a lone window on either side keeps its own shape.
    /// Every candidate is moved, when possible, so both sides meet their
    /// minimum sizes.
    fn cut_fractions(
        &self,
        first: &[usize],
        second: &[usize],
        avail: f32,
        cross: f32,
        axis: Axis,
    ) -> Vec<f32> {
        let share = |set: &[usize]| set.iter().map(|&index| self.shares[index]).sum::<f32>();
        let proportional = share(first) / (share(first) + share(second)).max(f32::EPSILON);
        let mut fractions = vec![proportional];
        if first.len() + second.len() <= EXHAUSTIVE_LIMIT {
            fractions.push(0.5);
            let natural_len = |index: usize| {
                let aspect = self.items[index].aspect.clamp(0.25, 4.5);
                match axis {
                    Axis::X => cross * aspect,
                    Axis::Y => cross / aspect,
                }
            };
            if let [index] = first {
                fractions.push(natural_len(*index) / avail);
            }
            if let [index] = second {
                fractions.push(1.0 - natural_len(*index) / avail);
            }
        }

        let min_len = |set: &[usize]| {
            set.iter()
                .map(|&index| match axis {
                    Axis::X => self.items[index].min_w,
                    Axis::Y => self.items[index].min_h,
                })
                .fold(0.0_f32, f32::max)
        };
        let low = min_len(first) / avail;
        let high = 1.0 - min_len(second) / avail;
        let mut fractions = fractions
            .into_iter()
            .map(|fraction| {
                let fraction = fraction.clamp(0.12, 0.88);
                if low <= high {
                    fraction.clamp(low, high)
                } else {
                    fraction
                }
            })
            .collect::<Vec<_>>();
        fractions.sort_by(f32::total_cmp);
        fractions.dedup_by(|a, b| (*a - *b).abs() < 0.005);
        fractions
    }

    fn piece_cost(&self, index: usize, w: f32, h: f32) -> f32 {
        let item = &self.items[index];
        let w = w.max(1.0);
        let h = h.max(1.0);
        let shape = ((w / h) / item.aspect.clamp(0.25, 4.5)).ln();
        let target = self.screen_area * self.shares[index] / self.total_share.max(f32::EPSILON);
        let size = ((w * h) / target.max(1.0)).ln();
        let mut cost = shape * shape + AREA_COST * size * size;
        for (have, need) in [(w, item.min_w), (h, item.min_h)] {
            if need > 0.0 && have + 0.5 < need {
                cost += MIN_SIZE_COST * (1.0 + (need - have) / need);
            }
        }
        cost
    }

    /// Only reached when the area is too small to cut at all: even columns
    /// in Field order.
    fn equal_columns(&self, set: &[usize], area: Area) -> Solution {
        let sorted = self.field_order(set, Axis::X);
        let mut plan = Rc::new(Plan::Leaf(sorted[sorted.len() - 1]));
        for (taken, &index) in sorted.iter().enumerate().rev().skip(1) {
            let remaining = (sorted.len() - taken) as f32;
            plan = Rc::new(Plan::Cut {
                axis: Axis::X,
                fraction: 1.0 / remaining,
                first: Rc::new(Plan::Leaf(index)),
                second: plan,
            });
        }
        let mut pieces = vec![Rect::default(); self.items.len()];
        place(&plan, area, self.gap, &mut pieces);
        let cost = set
            .iter()
            .map(|&index| self.piece_cost(index, pieces[index].w, pieces[index].h))
            .sum();
        Solution { cost, plan }
    }
}

/// Solutions depend only on which windows and the area's size, so the same
/// sub-problem reached through different cuts is solved once. Sizes are
/// rounded so near-identical areas share a plan too.
fn memo_key(set: &[usize], area: Area, quantum: (f32, f32)) -> Option<(u128, i32, i32)> {
    let mut mask = 0_u128;
    for &index in set {
        if index >= 128 {
            return None;
        }
        mask |= 1 << index;
    }
    Some((
        mask,
        (area.w / quantum.0).round() as i32,
        (area.h / quantum.1).round() as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(x: f32, y: f32, w: f32, h: f32, key: u64) -> Item {
        Item {
            x,
            y,
            aspect: w / h,
            stable_key: key,
            weight: w * h,
            min_w: 0.0,
            min_h: 0.0,
        }
    }

    fn left(rect: &Rect) -> f32 {
        rect.cx - rect.w * 0.5
    }

    fn top(rect: &Rect) -> f32 {
        rect.cy - rect.h * 0.5
    }

    #[test]
    fn three_windows_give_the_left_one_the_full_height() {
        // One window on the left, two stacked on the right, all the same size.
        let items = [
            item(0.0, 400.0, 800.0, 600.0, 1),
            item(1200.0, 0.0, 800.0, 600.0, 2),
            item(1200.0, 800.0, 800.0, 600.0, 3),
        ];
        let slots = partition(&items, None, 2520, 1360, 20.0);
        assert!((slots[0].h - 1360.0).abs() < 1.0, "{:?}", slots[0]);
        assert!(left(&slots[0]) < 1.0);
        for right in &slots[1..] {
            assert!(left(right) > slots[0].w, "{right:?}");
        }
        assert!(top(&slots[1]) < top(&slots[2]));
    }

    #[test]
    fn a_wide_window_with_a_minimum_size_gets_a_band_not_the_screen() {
        // Steam-like: a big default size and a hard minimum, above three
        // ordinary windows.
        let mut steam = item(800.0, 0.0, 1280.0, 800.0, 1);
        steam.min_w = 1010.0;
        steam.min_h = 700.0;
        let items = [
            steam,
            item(0.0, 900.0, 800.0, 600.0, 2),
            item(900.0, 900.0, 800.0, 600.0, 3),
            item(1800.0, 900.0, 800.0, 600.0, 4),
        ];
        let slots = partition(&items, None, 2520, 1360, 20.0);
        assert!(
            slots[0].w >= 1010.0 && slots[0].h >= 700.0,
            "{:?}",
            slots[0]
        );
        let screen = 2520.0 * 1360.0;
        assert!(slots[0].w * slots[0].h < screen * 0.6, "{:?}", slots[0]);
        assert!(slots[0].w > slots[0].h, "Steam stays wide: {:?}", slots[0]);
    }

    #[test]
    fn many_windows_are_laid_out_quickly() {
        let items = (0..24)
            .map(|index| {
                item(
                    (index % 6) as f32 * 500.0,
                    (index / 6) as f32 * 400.0,
                    800.0,
                    600.0,
                    index,
                )
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let slots = partition(&items, Some(3), 2520, 1360, 20.0);
        assert_eq!(slots.len(), 24);
        assert!(slots.iter().all(|slot| slot.w >= 1.0 && slot.h >= 1.0));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
