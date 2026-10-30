use smithay::utils::{Logical, Point, Rectangle, Size};

const EDGE_GRAB_DISTANCE: f64 = 7.0;
const CORNER_GRAB_DISTANCE: f64 = 12.0;
const MOVE_THRESHOLD: f64 = 4.0;
const MIN_REGION_SIZE: i32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Move,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug)]
struct Interaction {
    handle: Handle,
    start_pointer: Point<f64, Logical>,
    start_region: Rectangle<i32, Logical>,
    moving: bool,
}

/// Geometry and pointer policy for the interactive area selector.
///
/// It intentionally knows nothing about screenshots, IPC, or rendering.
/// Callers draw `region()`, route pointer events here while `is_active()`,
/// and retain the last geometry whenever the selection finishes.
#[derive(Debug, Default)]
pub struct RegionPicker {
    bounds: Option<Rectangle<i32, Logical>>,
    outputs: Vec<Rectangle<i32, Logical>>,
    region: Option<Rectangle<i32, Logical>>,
    remembered: Option<Rectangle<i32, Logical>>,
    interaction: Option<Interaction>,
}

impl RegionPicker {
    pub fn is_active(&self) -> bool {
        self.region.is_some()
    }

    pub fn region(&self) -> Option<Rectangle<i32, Logical>> {
        self.region
    }

    /// Move the region, or resize its bottom-right edge, using logical pixels.
    pub fn adjust(&mut self, dx: i32, dy: i32, resize: bool) -> bool {
        let (Some(region), Some(bounds)) = (self.region, self.bounds) else {
            return false;
        };
        let before = region;
        let interaction = Interaction {
            handle: if resize {
                Handle::BottomRight
            } else {
                Handle::Move
            },
            start_pointer: (0.0, 0.0).into(),
            start_region: region,
            moving: true,
        };
        let region = transform_region(interaction, (dx as f64, dy as f64).into(), bounds);
        self.region = Some(constrain_region(region, bounds, &self.outputs));
        self.interaction = None;
        self.region != Some(before)
    }

    pub fn begin(
        &mut self,
        outputs: Vec<Rectangle<i32, Logical>>,
        active_output: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let desktop_bounds = outputs
            .iter()
            .copied()
            .reduce(Rectangle::merge)
            .unwrap_or(active_output);
        self.bounds = Some(desktop_bounds);
        self.outputs = outputs;
        self.interaction = None;
        let initial = self
            .remembered
            .map(|region| constrain_region(region, desktop_bounds, &self.outputs))
            .unwrap_or_else(|| centered_half(active_output));
        let initial = constrain_region(initial, desktop_bounds, &self.outputs);
        self.region = Some(initial);
        initial
    }

    /// Keeps active and remembered geometry valid after an output-layout
    /// change. A rectangular selection may span adjacent outputs. Extents on
    /// an axis are limited only by the outputs covered on the other axis.
    pub fn update_outputs(&mut self, outputs: Vec<Rectangle<i32, Logical>>) {
        let Some(desktop_bounds) = outputs.iter().copied().reduce(Rectangle::merge) else {
            return;
        };
        self.bounds = Some(desktop_bounds);
        self.outputs = outputs;
        self.region = self
            .region
            .map(|region| constrain_region(region, desktop_bounds, &self.outputs));
        self.remembered = self
            .remembered
            .map(|region| constrain_region(region, desktop_bounds, &self.outputs));
        self.interaction = None;
    }

    /// Starts a potential move or resize. Every press is consumed while the
    /// picker is active, but presses outside the region deliberately do
    /// nothing.
    pub fn press(&mut self, pointer: Point<f64, Logical>) -> bool {
        let Some(region) = self.region else {
            return false;
        };
        self.interaction = hit_test(region, pointer).map(|handle| Interaction {
            handle,
            start_pointer: pointer,
            start_region: region,
            moving: handle != Handle::Move,
        });
        true
    }

    pub fn motion(&mut self, pointer: Point<f64, Logical>) -> bool {
        let (Some(bounds), Some(mut interaction)) = (self.bounds, self.interaction) else {
            return self.is_active();
        };
        let delta = pointer - interaction.start_pointer;
        if interaction.handle == Handle::Move && !interaction.moving {
            interaction.moving = delta.x.hypot(delta.y) >= MOVE_THRESHOLD;
            self.interaction = Some(interaction);
            if !interaction.moving {
                return true;
            }
        }

        self.region = Some(constrain_region(
            transform_region(interaction, delta, bounds),
            bounds,
            &self.outputs,
        ));
        true
    }

    pub fn release(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.interaction = None;
        true
    }

    pub fn accept(&mut self) -> Option<Rectangle<i32, Logical>> {
        self.finish()
    }

    pub fn cancel(&mut self) -> bool {
        self.finish().is_some()
    }

    fn finish(&mut self) -> Option<Rectangle<i32, Logical>> {
        self.interaction = None;
        let region = self.region.take()?;
        self.remembered = Some(match self.bounds {
            Some(bounds) => constrain_region(region, bounds, &self.outputs),
            None => region,
        });
        Some(region)
    }

    #[cfg(test)]
    fn remembered(&self) -> Option<Rectangle<i32, Logical>> {
        self.remembered
    }
}

fn centered_half(output: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let size = Size::from((
        (output.size.w / 2).max(MIN_REGION_SIZE).min(output.size.w),
        (output.size.h / 2).max(MIN_REGION_SIZE).min(output.size.h),
    ));
    Rectangle::new(
        (
            output.loc.x + (output.size.w - size.w) / 2,
            output.loc.y + (output.size.h - size.h) / 2,
        )
            .into(),
        size,
    )
}

fn clamp_region(
    region: Rectangle<i32, Logical>,
    bounds: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let size = Size::from((
        region.size.w.max(1).min(bounds.size.w.max(1)),
        region.size.h.max(1).min(bounds.size.h.max(1)),
    ));
    let x = region
        .loc
        .x
        .clamp(bounds.loc.x, bounds.loc.x + bounds.size.w - size.w);
    let y = region
        .loc
        .y
        .clamp(bounds.loc.y, bounds.loc.y + bounds.size.h - size.h);
    Rectangle::new((x, y).into(), size)
}

fn constrain_region(
    region: Rectangle<i32, Logical>,
    desktop_bounds: Rectangle<i32, Logical>,
    outputs: &[Rectangle<i32, Logical>],
) -> Rectangle<i32, Logical> {
    let mut region = clamp_region(region, desktop_bounds);
    if !outputs
        .iter()
        .any(|output| rectangles_overlap(region, *output))
    {
        return outputs
            .iter()
            .copied()
            .min_by_key(|output| distance_to_rect_squared(region.loc, *output))
            .map(|output| clamp_region(region, output))
            .unwrap_or(region);
    }

    // Resolve each axis from outputs covered on the other axis, taking the
    // *intersection* of their extents so every pixel of the selection is
    // backed by a real output. On a ragged desktop (a 1440-tall main beside a
    // 1200-tall secondary) a selection spanning both is capped to the common
    // 1200 band rather than reaching into the uncovered corner, which would
    // otherwise be saved as a transparent wedge.
    if let Some((top, bottom)) = covered_vertical_span(region, outputs) {
        region = clamp_vertical(region, top, bottom);
    }
    if let Some((left, right)) = covered_horizontal_span(region, outputs) {
        region = clamp_horizontal(region, left, right);
    }
    // Horizontal clamping can change which outputs support the vertical axis.
    if let Some((top, bottom)) = covered_vertical_span(region, outputs) {
        region = clamp_vertical(region, top, bottom);
    }
    region
}

/// Merges a set of half-open intervals into their disjoint covered runs.
fn merged_runs(mut intervals: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    intervals.retain(|(start, end)| start < end);
    intervals.sort_unstable();
    let mut runs: Vec<(i32, i32)> = Vec::new();
    for (start, end) in intervals {
        match runs.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => runs.push((start, end)),
        }
    }
    runs
}

/// Widest vertical span, containing the region's mid-line, that *every*
/// column of the region's x-range has backed by an output.
///
/// The x-range is split into slabs at output edges so that a column served by
/// a shorter output limits the result. This is what keeps a selection
/// spanning a 1440-tall main and a 1200-tall secondary out of the uncovered
/// corner. Returns `None` when no such span exists, which happens when the
/// outputs are tiled along this axis instead — the caller then relies on the
/// horizontal pass.
fn covered_vertical_span(
    region: Rectangle<i32, Logical>,
    outputs: &[Rectangle<i32, Logical>],
) -> Option<(i32, i32)> {
    let left = region.loc.x;
    let right = left + region.size.w;
    let probe = region.loc.y + region.size.h / 2;
    let mut edges = vec![left, right];
    for output in outputs {
        for edge in [output.loc.x, output.loc.x + output.size.w] {
            if edge > left && edge < right {
                edges.push(edge);
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();

    let mut span: Option<(i32, i32)> = None;
    for slab in edges.windows(2) {
        let (start, end) = (slab[0], slab[1]);
        let covering = outputs
            .iter()
            .filter(|output| output.loc.x <= start && output.loc.x + output.size.w >= end)
            .map(|output| (output.loc.y, output.loc.y + output.size.h))
            .collect::<Vec<_>>();
        let run = merged_runs(covering)
            .into_iter()
            .find(|(top, bottom)| (*top..*bottom).contains(&probe))?;
        span = Some(match span {
            Some((top, bottom)) => (top.max(run.0), bottom.min(run.1)),
            None => run,
        });
    }
    span.filter(|(top, bottom)| top < bottom)
}

/// Horizontal mirror of [`covered_vertical_span`], for outputs stacked
/// vertically where a narrower one limits the usable width.
fn covered_horizontal_span(
    region: Rectangle<i32, Logical>,
    outputs: &[Rectangle<i32, Logical>],
) -> Option<(i32, i32)> {
    let top = region.loc.y;
    let bottom = top + region.size.h;
    let probe = region.loc.x + region.size.w / 2;
    let mut edges = vec![top, bottom];
    for output in outputs {
        for edge in [output.loc.y, output.loc.y + output.size.h] {
            if edge > top && edge < bottom {
                edges.push(edge);
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();

    let mut span: Option<(i32, i32)> = None;
    for slab in edges.windows(2) {
        let (start, end) = (slab[0], slab[1]);
        let covering = outputs
            .iter()
            .filter(|output| output.loc.y <= start && output.loc.y + output.size.h >= end)
            .map(|output| (output.loc.x, output.loc.x + output.size.w))
            .collect::<Vec<_>>();
        let run = merged_runs(covering)
            .into_iter()
            .find(|(left, right)| (*left..*right).contains(&probe))?;
        span = Some(match span {
            Some((left, right)) => (left.max(run.0), right.min(run.1)),
            None => run,
        });
    }
    span.filter(|(left, right)| left < right)
}

fn clamp_vertical(
    region: Rectangle<i32, Logical>,
    top: i32,
    bottom: i32,
) -> Rectangle<i32, Logical> {
    let height = region.size.h.min((bottom - top).max(1));
    Rectangle::new(
        (region.loc.x, region.loc.y.clamp(top, bottom - height)).into(),
        (region.size.w, height).into(),
    )
}

fn clamp_horizontal(
    region: Rectangle<i32, Logical>,
    left: i32,
    right: i32,
) -> Rectangle<i32, Logical> {
    let width = region.size.w.min((right - left).max(1));
    Rectangle::new(
        (region.loc.x.clamp(left, right - width), region.loc.y).into(),
        (width, region.size.h).into(),
    )
}

fn rectangles_overlap(left: Rectangle<i32, Logical>, right: Rectangle<i32, Logical>) -> bool {
    intervals_overlap(
        left.loc.x,
        left.loc.x + left.size.w,
        right.loc.x,
        right.loc.x + right.size.w,
    ) && intervals_overlap(
        left.loc.y,
        left.loc.y + left.size.h,
        right.loc.y,
        right.loc.y + right.size.h,
    )
}

fn intervals_overlap(left_start: i32, left_end: i32, right_start: i32, right_end: i32) -> bool {
    left_start.max(right_start) < left_end.min(right_end)
}

fn distance_to_rect_squared(point: Point<i32, Logical>, rect: Rectangle<i32, Logical>) -> i64 {
    let right = rect.loc.x + rect.size.w - 1;
    let bottom = rect.loc.y + rect.size.h - 1;
    let dx = i64::from(point.x - point.x.clamp(rect.loc.x, right));
    let dy = i64::from(point.y - point.y.clamp(rect.loc.y, bottom));
    dx * dx + dy * dy
}

fn hit_test(region: Rectangle<i32, Logical>, pointer: Point<f64, Logical>) -> Option<Handle> {
    let left = f64::from(region.loc.x);
    let top = f64::from(region.loc.y);
    let right = f64::from(region.loc.x + region.size.w);
    let bottom = f64::from(region.loc.y + region.size.h);

    for (x, y, handle) in [
        (left, top, Handle::TopLeft),
        (right, top, Handle::TopRight),
        (left, bottom, Handle::BottomLeft),
        (right, bottom, Handle::BottomRight),
    ] {
        if (pointer.x - x).hypot(pointer.y - y) <= CORNER_GRAB_DISTANCE {
            return Some(handle);
        }
    }

    let within_x = pointer.x >= left && pointer.x <= right;
    let within_y = pointer.y >= top && pointer.y <= bottom;
    if within_x && (pointer.y - top).abs() <= EDGE_GRAB_DISTANCE {
        return Some(Handle::Top);
    }
    if within_x && (pointer.y - bottom).abs() <= EDGE_GRAB_DISTANCE {
        return Some(Handle::Bottom);
    }
    if within_y && (pointer.x - left).abs() <= EDGE_GRAB_DISTANCE {
        return Some(Handle::Left);
    }
    if within_y && (pointer.x - right).abs() <= EDGE_GRAB_DISTANCE {
        return Some(Handle::Right);
    }
    (within_x && within_y).then_some(Handle::Move)
}

fn transform_region(
    interaction: Interaction,
    delta: Point<f64, Logical>,
    bounds: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let dx = delta.x.round() as i32;
    let dy = delta.y.round() as i32;
    let start = interaction.start_region;
    if interaction.handle == Handle::Move {
        return clamp_region(
            Rectangle::new((start.loc.x + dx, start.loc.y + dy).into(), start.size),
            bounds,
        );
    }

    let mut left = start.loc.x;
    let mut top = start.loc.y;
    let mut right = start.loc.x + start.size.w;
    let mut bottom = start.loc.y + start.size.h;
    let bounds_right = bounds.loc.x + bounds.size.w;
    let bounds_bottom = bounds.loc.y + bounds.size.h;

    if matches!(
        interaction.handle,
        Handle::Left | Handle::TopLeft | Handle::BottomLeft
    ) {
        left = (left + dx).clamp(bounds.loc.x, right - MIN_REGION_SIZE);
    }
    if matches!(
        interaction.handle,
        Handle::Right | Handle::TopRight | Handle::BottomRight
    ) {
        right = (right + dx).clamp(left + MIN_REGION_SIZE, bounds_right);
    }
    if matches!(
        interaction.handle,
        Handle::Top | Handle::TopLeft | Handle::TopRight
    ) {
        top = (top + dy).clamp(bounds.loc.y, bottom - MIN_REGION_SIZE);
    }
    if matches!(
        interaction.handle,
        Handle::Bottom | Handle::BottomLeft | Handle::BottomRight
    ) {
        bottom = (bottom + dy).clamp(top + MIN_REGION_SIZE, bounds_bottom);
    }

    Rectangle::new((left, top).into(), (right - left, bottom - top).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_adjustment_moves_and_resizes_without_leaving_output() {
        let output = Rectangle::new((0, 0).into(), (800, 600).into());
        let mut picker = RegionPicker::default();
        let initial = picker.begin(vec![output], output);
        assert!(picker.adjust(10, 0, false));
        let moved = picker.region().unwrap();
        assert_eq!(moved.loc.x, initial.loc.x + 10);
        assert_eq!(moved.size, initial.size);
        assert!(picker.adjust(10, 10, true));
        let resized = picker.region().unwrap();
        assert_eq!(resized.loc, moved.loc);
        assert_eq!(resized.size.w, moved.size.w + 10);
        assert_eq!(resized.size.h, moved.size.h + 10);
        picker.adjust(10_000, 10_000, true);
        let limited = picker.region().unwrap();
        assert_eq!(limited.loc, moved.loc);
        assert!(limited.loc.x + limited.size.w <= 800);
        assert!(limited.loc.y + limited.size.h <= 600);
    }

    fn bounds() -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (2000, 1000).into())
    }

    fn output() -> Rectangle<i32, Logical> {
        Rectangle::new((1000, 0).into(), (1000, 1000).into())
    }

    #[test]
    fn first_region_is_centered_at_half_the_active_output() {
        let mut picker = RegionPicker::default();
        assert_eq!(
            picker.begin(vec![bounds()], output()),
            Rectangle::new((1250, 250).into(), (500, 500).into())
        );
    }

    #[test]
    fn clicking_outside_or_inside_without_dragging_is_a_no_op() {
        let mut picker = RegionPicker::default();
        let initial = picker.begin(vec![bounds()], output());

        assert!(picker.press((100.0, 100.0).into()));
        picker.motion((500.0, 500.0).into());
        picker.release();
        assert_eq!(picker.region(), Some(initial));

        picker.press((1400.0, 400.0).into());
        picker.release();
        assert_eq!(picker.region(), Some(initial));
    }

    #[test]
    fn moving_preserves_the_grab_point_after_a_small_threshold() {
        let mut picker = RegionPicker::default();
        let initial = picker.begin(vec![bounds()], output());
        picker.press((1300.0, 300.0).into());
        picker.motion((1302.0, 302.0).into());
        assert_eq!(picker.region(), Some(initial));

        picker.motion((1400.0, 350.0).into());
        assert_eq!(
            picker.region(),
            Some(Rectangle::new((1350, 300).into(), (500, 500).into()))
        );
    }

    #[test]
    fn corner_and_side_grips_resize_while_opposite_edges_stay_fixed() {
        let mut picker = RegionPicker::default();
        picker.begin(vec![bounds()], output());
        picker.press((1250.0, 250.0).into());
        picker.motion((1200.0, 200.0).into());
        picker.release();
        assert_eq!(
            picker.region(),
            Some(Rectangle::new((1200, 200).into(), (550, 550).into()))
        );

        picker.press((1750.0, 500.0).into());
        picker.motion((1800.0, 500.0).into());
        assert_eq!(
            picker.region(),
            Some(Rectangle::new((1200, 200).into(), (600, 550).into()))
        );
    }

    #[test]
    fn accepting_remembers_the_last_selection() {
        let mut picker = RegionPicker::default();
        let selected = picker.begin(vec![bounds()], output());
        assert_eq!(picker.accept(), Some(selected));
        assert_eq!(picker.remembered(), Some(selected));
    }

    #[test]
    fn cancelling_remembers_the_last_selection() {
        let mut picker = RegionPicker::default();
        let selected = picker.begin(vec![bounds()], output());

        assert!(picker.cancel());
        assert!(!picker.is_active());
        assert_eq!(picker.remembered(), Some(selected));
        assert!(!picker.cancel());
    }

    #[test]
    fn remembered_region_is_clamped_after_layout_changes() {
        let mut picker = RegionPicker::default();
        let selected = picker.begin(vec![bounds()], output());
        assert_eq!(picker.accept(), Some(selected));
        picker.update_outputs(vec![Rectangle::new((0, 0).into(), (800, 600).into())]);

        assert_eq!(
            picker.remembered(),
            Some(Rectangle::new((300, 100).into(), (500, 500).into()))
        );
    }

    #[test]
    fn selection_cannot_move_below_a_shorter_secondary_output() {
        let outputs = vec![
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let secondary = outputs[1];
        let mut picker = RegionPicker::default();
        let initial = picker.begin(outputs, secondary);
        let center = Point::from((
            f64::from(initial.loc.x + initial.size.w / 2),
            f64::from(initial.loc.y + initial.size.h / 2),
        ));

        picker.press(center);
        picker.motion((center.x, center.y + 1000.0).into());

        let selected = picker.region().unwrap();
        assert_eq!(selected.size, initial.size);
        assert_eq!(selected.loc.y + selected.size.h, 1200);
    }

    #[test]
    fn full_main_selection_is_not_limited_by_the_shorter_secondary() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = outputs[0];
        let desktop = Rectangle::new((0, 0).into(), (4480, 1440).into());

        assert_eq!(constrain_region(selected, desktop, &outputs), selected);
    }

    #[test]
    fn secondary_only_selection_is_limited_to_secondary_height() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = Rectangle::new((2560, 0).into(), (1920, 1440).into());
        let desktop = Rectangle::new((0, 0).into(), (4480, 1440).into());

        assert_eq!(
            constrain_region(selected, desktop, &outputs),
            Rectangle::new((2560, 0).into(), (1920, 1200).into())
        );
    }

    #[test]
    fn selection_spanning_uneven_outputs_stops_at_the_shorter_one() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = Rectangle::new((2000, 0).into(), (1000, 1440).into());
        let desktop = Rectangle::new((0, 0).into(), (4480, 1440).into());

        // The columns past x=2560 are only backed to y=1200, so the whole
        // selection is capped there rather than saving a transparent wedge.
        assert_eq!(
            constrain_region(selected, desktop, &outputs),
            Rectangle::new((2000, 0).into(), (1000, 1200).into())
        );
    }

    #[test]
    fn full_desktop_selection_excludes_the_uncovered_corner() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let desktop = Rectangle::new((0, 0).into(), (4480, 1440).into());

        assert_eq!(
            constrain_region(desktop, desktop, &outputs),
            Rectangle::new((0, 0).into(), (4480, 1200).into())
        );
    }

    #[test]
    fn resizing_across_the_boundary_keeps_a_single_covered_region() {
        let outputs = vec![
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let main = outputs[0];
        let mut picker = RegionPicker::default();
        let initial = picker.begin(outputs, main);

        // Grab the right edge and drag it well onto the secondary output.
        let right = f64::from(initial.loc.x + initial.size.w);
        let middle = f64::from(initial.loc.y + initial.size.h / 2);
        picker.press((right, middle).into());
        picker.motion((right + 1600.0, middle).into());

        let selected = picker.region().unwrap();
        assert!(
            selected.loc.x + selected.size.w > 2560,
            "region should extend onto the secondary output, got {selected:?}"
        );
        assert!(
            selected.loc.y + selected.size.h <= 1200,
            "region must stay inside the covered band, got {selected:?}"
        );
    }

    #[test]
    fn vertically_stacked_shorter_output_limits_only_its_own_width() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((0, 1440).into(), (1920, 1200).into()),
        ];
        let selected = Rectangle::new((0, 1440).into(), (2560, 1200).into());
        let desktop = Rectangle::new((0, 0).into(), (2560, 2640).into());

        assert_eq!(
            constrain_region(selected, desktop, &outputs),
            Rectangle::new((0, 1440).into(), (1920, 1200).into())
        );
    }
}
