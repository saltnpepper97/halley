use halley_core::camera::Camera;
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{Session, SessionDriver};

const INFEASIBLE_COST: i64 = i64::MAX / 16;
const UNREACHED_COST: i64 = i64::MAX / 4;

#[derive(Clone, Debug)]
struct ArrangeTransaction {
    restores: Vec<crate::presentation::maximize::FieldRestore>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ArrangeTransactions {
    by_output: std::collections::HashMap<String, ArrangeTransaction>,
}

impl ArrangeTransactions {
    fn take(&mut self, output: &str) -> Option<ArrangeTransaction> {
        self.by_output.remove(output)
    }

    fn insert(
        &mut self,
        output: String,
        restores: Vec<crate::presentation::maximize::FieldRestore>,
    ) {
        self.by_output
            .insert(output, ArrangeTransaction { restores });
    }

    /// A deliberate edit to any participant accepts the current layout as
    /// ordinary Field geometry. Forget the entire output's restore snapshot,
    /// rather than letting a later toggle overwrite that newer choice.
    pub(crate) fn invalidate_surface(&mut self, surface: &WlSurface) {
        self.by_output.retain(|_, transaction| {
            !transaction
                .restores
                .iter()
                .any(|restore| restore.surface == *surface)
        });
    }

    pub(crate) fn contains_surface(&self, surface: &WlSurface) -> bool {
        self.by_output.values().any(|transaction| {
            transaction
                .restores
                .iter()
                .any(|restore| restore.surface == *surface)
        })
    }

    pub(crate) fn active_on(&self, output: &str) -> bool {
        self.by_output.contains_key(output)
    }
}

#[derive(Clone)]
struct Candidate {
    window: Window,
    surface: WlSurface,
    current: Rectangle<i32, Logical>,
    center: Point<i32, Logical>,
}

pub(crate) fn arrange_visible<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
) -> bool {
    if let Some(transaction) = session.interactions.field_arrange.take(output_name) {
        return restore_transaction(session, output_name, transaction);
    }
    if session.clusters.active_on(output_name).is_some()
        || !matches!(session.interactions.grab, crate::input::grab::Grab::None)
    {
        return false;
    }
    let Some(output) = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
    else {
        return false;
    };
    let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    let Some(camera) = session.cameras.get(output_name) else {
        return false;
    };
    let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
    let Some((visible_area, _)) = visible_work_area_outer(camera, output_geometry, work_area, 0)
    else {
        return false;
    };
    let configured_gap = session.settings.field.gap.ceil() as i32;
    let Some((layout_outer, gap)) =
        visible_work_area_outer(camera, output_geometry, work_area, configured_gap)
    else {
        return false;
    };

    session.nodes.sync_from_space(&session.wayland.space);
    let mut candidates = session
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && !record.collapsed
                && record.output == output_name
                && session.nodes.field.is_visible(record.id)
                && !session.clusters.is_member(record.id)
                && !super::node_user_pinned(session, record.id)
                && !session.fullscreen.is_fullscreen_or_pending(&record.surface)
                && !session.maximize.contains(&record.surface)
                && !crate::input::grab::belongs_to_surface(
                    &session.interactions.grab,
                    &record.surface,
                )
        })
        .filter_map(|record| {
            let current = session.wayland.space.element_geometry(&record.window)?;
            let outer = crate::titlebar::outer_rect_for_client(
                &record.window,
                current,
                &session.settings.decorations,
                &session.settings.font,
            );
            let center = rect_center(outer);
            visible_area.contains(center).then(|| Candidate {
                window: record.window.clone(),
                surface: record.surface.clone(),
                current,
                center,
            })
        })
        .collect::<Vec<_>>();

    let assignment = loop {
        if candidates.len() < 2 {
            return false;
        }
        let Some(region_variants) = mosaic_region_variants(layout_outer, candidates.len(), gap)
        else {
            return false;
        };
        let mut best_removal_costs = None;
        let mut best_feasible_cells = 0usize;
        let mut selected = None;
        for regions in region_variants {
            let target_clients = candidates
                .iter()
                .map(|candidate| {
                    regions
                        .iter()
                        .map(|region| {
                            crate::titlebar::client_rect_for_outer(
                                &candidate.window,
                                *region,
                                &session.settings.decorations,
                                &session.settings.font,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let costs = candidates
                .iter()
                .enumerate()
                .map(|(candidate_index, candidate)| {
                    regions
                        .iter()
                        .enumerate()
                        .map(|(region_index, region)| {
                            let target = target_clients[candidate_index][region_index];
                            if !window_size_is_accepted(&candidate.window, target.size) {
                                INFEASIBLE_COST
                            } else {
                                let feasible_ceiling =
                                    INFEASIBLE_COST / (candidates.len() as i64 + 1) - 1;
                                squared_distance(candidate.center, rect_center(*region))
                                    .min(feasible_ceiling)
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let feasible_cells = costs
                .iter()
                .flatten()
                .filter(|cost| **cost < INFEASIBLE_COST)
                .count();
            if feasible_cells > best_feasible_cells || best_removal_costs.is_none() {
                best_feasible_cells = feasible_cells;
                best_removal_costs = Some(costs.clone());
            }
            if let Some(assignment) = feasible_assignment(&costs) {
                selected = Some(
                    assignment
                        .into_iter()
                        .enumerate()
                        .map(|(candidate, region)| (candidate, target_clients[candidate][region]))
                        .collect::<Vec<_>>(),
                );
                break;
            }
        }
        if let Some(selected) = selected {
            break selected;
        }

        let costs = best_removal_costs.expect("a mosaic has at least one layout variant");
        let remove = costs
            .iter()
            .enumerate()
            .min_by_key(|(index, row)| {
                (
                    row.iter().filter(|cost| **cost < INFEASIBLE_COST).count(),
                    std::cmp::Reverse(
                        candidates[*index].current.size.w as i64
                            * candidates[*index].current.size.h as i64,
                    ),
                )
            })
            .map(|(index, _)| index)
            .expect("candidate list is non-empty");
        candidates.remove(remove);
    };

    let restores = candidates
        .iter()
        .map(|candidate| crate::presentation::maximize::FieldRestore {
            surface: candidate.surface.clone(),
            geometry: candidate.current,
            output: output_name.to_string(),
        })
        .collect::<Vec<_>>();
    let now = crate::frame_clock::monotonic_now();
    let transitions = assignment
        .into_iter()
        .filter_map(|(candidate_index, target)| {
            let candidate = &candidates[candidate_index];
            let current_visual =
                super::presented_window_rect(session, &candidate.window, &output, now)?;
            let request = crate::presentation::maximize::FieldRestore {
                surface: candidate.surface.clone(),
                geometry: target,
                output: output_name.to_string(),
            };
            let target_visual = field_visual_rect(session, &output, target)?;
            Some((
                candidate.window.clone(),
                request,
                current_visual,
                target_visual,
            ))
        })
        .collect::<Vec<_>>();
    if transitions.len() < 2 {
        return false;
    }

    // Arrange owns the foreground once, like maximize: a partially visible or
    // size-constrained Field window that was not selected must not ride over
    // the mosaic. Preserve the participants' existing relative depth and do
    // not disturb keyboard focus or activation while moving the group above
    // every non-participant.
    let arranged_windows = session
        .wayland
        .space
        .elements()
        .filter(|window| {
            candidates
                .iter()
                .any(|candidate| &candidate.window == *window)
        })
        .cloned()
        .collect::<Vec<_>>();
    crate::window::raise_managed_group(&mut session.wayland, &arranged_windows);

    // Publish the exact restore set before issuing any client configure. This
    // makes a rapid second Mod+A a valid reversal even before clients commit
    // their arranged sizes.
    session
        .interactions
        .field_arrange
        .insert(output_name.to_string(), restores);
    for (window, request, current_visual, target_visual) in transitions {
        capture_arrange_texture(
            session,
            &window,
            &request.surface,
            request.geometry.size,
            now,
        );
        session.window_animations.arrange(
            request.surface.clone(),
            now,
            current_visual,
            target_visual,
        );
        super::configure_field_geometry(session, &request);
    }
    session.request_output_redraw(&output);
    true
}

fn capture_arrange_texture<D: SessionDriver>(
    session: &mut Session<D>,
    window: &Window,
    surface: &WlSurface,
    target_size: Size<i32, Logical>,
    now: std::time::Duration,
) {
    let preserve_existing = session.window_animations.is_arranging(surface, now);
    let textures = &mut session.render.arrange_textures;
    let capture = session.driver.with_renderer(|renderer| {
        textures.capture(renderer, window, target_size, preserve_existing)
    });
    if let Err(err) = capture {
        eventline::warn!("field arrange: failed to capture outgoing window texture: {err}");
    }
}

pub(crate) fn undo_last<D: SessionDriver>(session: &mut Session<D>, output_name: &str) -> bool {
    let Some(transaction) = session.interactions.field_arrange.take(output_name) else {
        return false;
    };
    restore_transaction(session, output_name, transaction)
}

fn restore_transaction<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
    transaction: ArrangeTransaction,
) -> bool {
    let now = crate::frame_clock::monotonic_now();
    let mut restored = false;
    for request in transaction.restores {
        let Some((id, window, current_output_name)) = session
            .nodes
            .id_for_surface(&request.surface)
            .and_then(|id| {
                session
                    .nodes
                    .record(id)
                    .map(|record| (id, record.window.clone(), record.output.clone()))
            })
        else {
            continue;
        };
        let eligible = session.nodes.record(id).is_some_and(|record| {
            record.attached
                && !record.collapsed
                && !session.clusters.is_member(id)
                && !session.fullscreen.is_fullscreen_or_pending(&record.surface)
                && !session.maximize.contains(&record.surface)
        });
        if !eligible {
            continue;
        }
        let current_output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == current_output_name)
            .cloned();
        let target_output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == request.output)
            .cloned();
        let transition = current_output
            .as_ref()
            .and_then(|output| super::presented_window_rect(session, &window, output, now))
            .zip(
                target_output
                    .as_ref()
                    .and_then(|output| field_visual_rect(session, output, request.geometry)),
            );
        if let Some((current_visual, target_visual)) = transition {
            capture_arrange_texture(
                session,
                &window,
                &request.surface,
                request.geometry.size,
                now,
            );
            session.window_animations.arrange(
                request.surface.clone(),
                now,
                current_visual,
                target_visual,
            );
        }
        super::configure_field_geometry(session, &request);
        restored = true;
    }
    if restored {
        let output = session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == output_name)
            .cloned();
        if let Some(output) = output {
            session.request_output_redraw(&output);
        } else {
            session.request_redraw();
        }
    }
    restored
}

fn field_visual_rect<D: SessionDriver>(
    session: &Session<D>,
    output: &smithay::output::Output,
    geometry: Rectangle<i32, Logical>,
) -> Option<Rectangle<i32, smithay::utils::Physical>> {
    let output_geometry = session.wayland.space.output_geometry(output)?;
    let view = session.cameras.view(&output.name())?;
    Some(crate::render::camera_rect(
        geometry.to_physical(1),
        crate::presentation::camera::global_center(view.center, output_geometry),
        output_geometry.size.to_physical(1),
        view.scale,
    ))
}

fn window_size_is_accepted(window: &Window, requested: Size<i32, Logical>) -> bool {
    if requested.w < 1 || requested.h < 1 {
        return false;
    }
    if crate::xwayland::is_x11(window) {
        return crate::xwayland::constrain_window_size(window, requested) == requested;
    }
    let Some(toplevel) = window.toplevel() else {
        return false;
    };
    smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        let mut cached = states
            .cached_state
            .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
        let state = cached.current();
        let minimum_ok = (state.min_size.w <= 0 || requested.w >= state.min_size.w)
            && (state.min_size.h <= 0 || requested.h >= state.min_size.h);
        let maximum_ok = (state.max_size.w <= 0 || requested.w <= state.max_size.w)
            && (state.max_size.h <= 0 || requested.h <= state.max_size.h);
        minimum_ok && maximum_ok
    })
}

fn visible_work_area_outer(
    camera: &Camera,
    output_geometry: Rectangle<i32, Logical>,
    work_area: Rectangle<i32, Logical>,
    gap: i32,
) -> Option<(Rectangle<i32, Logical>, i32)> {
    let gap = gap.max(0);
    let width = work_area.size.w.checked_sub(gap.checked_mul(2)?)?;
    let height = work_area.size.h.checked_sub(gap.checked_mul(2)?)?;
    if width < 2 || height < 2 {
        return None;
    }
    let screen_outer = Rectangle::new(
        output_geometry.loc + work_area.loc + Point::from((gap, gap)),
        (width, height).into(),
    );
    let top_left = crate::input::grab::screen_to_world_on_output(
        (f64::from(screen_outer.loc.x), f64::from(screen_outer.loc.y)),
        camera,
        output_geometry,
    );
    let bottom_right = crate::input::grab::screen_to_world_on_output(
        (
            f64::from(screen_outer.loc.x + screen_outer.size.w),
            f64::from(screen_outer.loc.y + screen_outer.size.h),
        ),
        camera,
        output_geometry,
    );
    let left = top_left.x.round() as i32;
    let top = top_left.y.round() as i32;
    let right = bottom_right.x.round() as i32;
    let bottom = bottom_right.y.round() as i32;
    let outer = Rectangle::new(
        (left, top).into(),
        (right.checked_sub(left)?, bottom.checked_sub(top)?).into(),
    );
    if outer.size.w < 2 || outer.size.h < 2 {
        return None;
    }
    let scale = crate::input::zoom::scale(camera).max(0.05);
    let world_gap = ((gap as f32) / scale).ceil() as i32;
    Some((outer, world_gap))
}

fn mosaic_regions(
    outer: Rectangle<i32, Logical>,
    count: usize,
    gap: i32,
) -> Option<Vec<Rectangle<i32, Logical>>> {
    if count < 2 {
        return None;
    }
    if count == 3 {
        let columns = split_axis(outer.loc.x, outer.size.w, 2, gap)?;
        let right_rows = split_axis(outer.loc.y, outer.size.h, 2, gap)?;
        return Some(vec![
            Rectangle::new(
                (columns[0].0, outer.loc.y).into(),
                (columns[0].1, outer.size.h).into(),
            ),
            Rectangle::new(
                (columns[1].0, right_rows[0].0).into(),
                (columns[1].1, right_rows[0].1).into(),
            ),
            Rectangle::new(
                (columns[1].0, right_rows[1].0).into(),
                (columns[1].1, right_rows[1].1).into(),
            ),
        ]);
    }

    let columns = ceil_sqrt(count);
    let rows = count.div_ceil(columns);
    let row_heights = split_axis(outer.loc.y, outer.size.h, rows, gap)?;
    let base = count / rows;
    let extra = count % rows;
    let mut regions = Vec::with_capacity(count);
    for (row, (top, height)) in row_heights.into_iter().enumerate() {
        let in_row = base + usize::from(row < extra);
        let widths = split_axis(outer.loc.x, outer.size.w, in_row, gap)?;
        regions.extend(
            widths
                .into_iter()
                .map(|(left, width)| Rectangle::new((left, top).into(), (width, height).into())),
        );
    }
    Some(regions)
}

/// Keeps the balanced mosaic as the stable default, then offers layouts with
/// one larger slot for clients whose minimum size cannot fit the equal grid.
fn mosaic_region_variants(
    outer: Rectangle<i32, Logical>,
    count: usize,
    gap: i32,
) -> Option<Vec<Vec<Rectangle<i32, Logical>>>> {
    let balanced = mosaic_regions(outer, count, gap)?;
    let mut variants = vec![balanced];
    for (numerator, denominator) in [(3, 5), (2, 3), (3, 4)] {
        for vertical in [true, false] {
            for featured_at_start in [true, false] {
                if let Some(regions) = featured_mosaic(
                    outer,
                    count,
                    gap,
                    numerator,
                    denominator,
                    vertical,
                    featured_at_start,
                ) {
                    variants.push(regions);
                }
            }
        }
    }
    Some(variants)
}

fn featured_mosaic(
    outer: Rectangle<i32, Logical>,
    count: usize,
    gap: i32,
    numerator: i32,
    denominator: i32,
    vertical: bool,
    featured_at_start: bool,
) -> Option<Vec<Rectangle<i32, Logical>>> {
    if count < 2 || numerator <= 0 || numerator >= denominator {
        return None;
    }
    let gap = gap.max(0);
    let axis_length = if vertical { outer.size.w } else { outer.size.h };
    let available = axis_length.checked_sub(gap)?;
    let featured_length = available.checked_mul(numerator)?.div_euclid(denominator);
    let remainder_length = available.checked_sub(featured_length)?;
    if featured_length < 1 || remainder_length < 1 {
        return None;
    }
    let (featured_axis, remainder_axis) = if featured_at_start {
        (
            if vertical { outer.loc.x } else { outer.loc.y },
            (if vertical { outer.loc.x } else { outer.loc.y })
                .checked_add(featured_length)?
                .checked_add(gap)?,
        )
    } else {
        (
            (if vertical { outer.loc.x } else { outer.loc.y })
                .checked_add(remainder_length)?
                .checked_add(gap)?,
            if vertical { outer.loc.x } else { outer.loc.y },
        )
    };
    let featured = if vertical {
        Rectangle::new(
            (featured_axis, outer.loc.y).into(),
            (featured_length, outer.size.h).into(),
        )
    } else {
        Rectangle::new(
            (outer.loc.x, featured_axis).into(),
            (outer.size.w, featured_length).into(),
        )
    };
    let remainder = if vertical {
        Rectangle::new(
            (remainder_axis, outer.loc.y).into(),
            (remainder_length, outer.size.h).into(),
        )
    } else {
        Rectangle::new(
            (outer.loc.x, remainder_axis).into(),
            (outer.size.w, remainder_length).into(),
        )
    };
    let mut regions = vec![featured];
    if count == 2 {
        regions.push(remainder);
    } else {
        regions.extend(mosaic_regions(remainder, count - 1, gap)?);
    }
    Some(regions)
}

fn split_axis(start: i32, length: i32, count: usize, gap: i32) -> Option<Vec<(i32, i32)>> {
    let count = i32::try_from(count).ok()?;
    let gap = gap.max(0);
    let available = length.checked_sub(gap.checked_mul(count.checked_sub(1)?)?)?;
    if available < count {
        return None;
    }
    let base = available / count;
    let remainder = available % count;
    let mut cursor = start;
    let mut segments = Vec::with_capacity(count as usize);
    for index in 0..count {
        let size = base + i32::from(index < remainder);
        segments.push((cursor, size));
        cursor = cursor.checked_add(size)?.checked_add(gap)?;
    }
    Some(segments)
}

fn ceil_sqrt(value: usize) -> usize {
    let mut root = 1usize;
    while root.saturating_mul(root) < value {
        root += 1;
    }
    root
}

fn rect_center(rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2).into()
}

fn squared_distance(a: Point<i32, Logical>, b: Point<i32, Logical>) -> i64 {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dy = i64::from(a.y) - i64::from(b.y);
    dx.saturating_mul(dx)
        .saturating_add(dy.saturating_mul(dy))
        .min(INFEASIBLE_COST - 1)
}

fn feasible_assignment(costs: &[Vec<i64>]) -> Option<Vec<usize>> {
    if costs.len() < 2 || costs.iter().any(|row| row.len() != costs.len()) {
        return None;
    }
    let assignment = minimum_cost_assignment(costs);
    assignment
        .iter()
        .enumerate()
        .all(|(candidate, region)| costs[candidate][*region] < INFEASIBLE_COST)
        .then_some(assignment)
}

fn minimum_cost_assignment(costs: &[Vec<i64>]) -> Vec<usize> {
    let count = costs.len();
    debug_assert!(costs.iter().all(|row| row.len() == count));
    let mut row_potential = vec![0i64; count + 1];
    let mut column_potential = vec![0i64; count + 1];
    let mut matched_row = vec![0usize; count + 1];
    let mut previous_column = vec![0usize; count + 1];

    for row in 1..=count {
        matched_row[0] = row;
        let mut column = 0usize;
        let mut minimum = vec![UNREACHED_COST; count + 1];
        let mut used = vec![false; count + 1];
        loop {
            used[column] = true;
            let active_row = matched_row[column];
            let mut delta = UNREACHED_COST;
            let mut next_column = 0usize;
            for candidate_column in 1..=count {
                if used[candidate_column] {
                    continue;
                }
                let reduced = costs[active_row - 1][candidate_column - 1]
                    .saturating_sub(row_potential[active_row])
                    .saturating_sub(column_potential[candidate_column]);
                if reduced < minimum[candidate_column] {
                    minimum[candidate_column] = reduced;
                    previous_column[candidate_column] = column;
                }
                if minimum[candidate_column] < delta {
                    delta = minimum[candidate_column];
                    next_column = candidate_column;
                }
            }
            for candidate_column in 0..=count {
                if used[candidate_column] {
                    row_potential[matched_row[candidate_column]] =
                        row_potential[matched_row[candidate_column]].saturating_add(delta);
                    column_potential[candidate_column] =
                        column_potential[candidate_column].saturating_sub(delta);
                } else {
                    minimum[candidate_column] = minimum[candidate_column].saturating_sub(delta);
                }
            }
            column = next_column;
            if matched_row[column] == 0 {
                break;
            }
        }
        loop {
            let prior = previous_column[column];
            matched_row[column] = matched_row[prior];
            column = prior;
            if column == 0 {
                break;
            }
        }
    }

    let mut assignment = vec![0usize; count];
    for column in 1..=count {
        assignment[matched_row[column] - 1] = column - 1;
    }
    assignment
}

#[cfg(test)]
mod tests {
    use super::{
        ArrangeTransactions, feasible_assignment, minimum_cost_assignment, mosaic_region_variants,
        mosaic_regions, visible_work_area_outer,
    };
    use halley_core::camera::Camera;
    use halley_core::field::Vec2;
    use smithay::utils::{Logical, Rectangle};

    #[test]
    fn arrange_transactions_toggle_once_per_output() {
        let mut transactions = ArrangeTransactions::default();
        transactions.insert("DP-1".to_string(), Vec::new());
        transactions.insert("DP-2".to_string(), Vec::new());

        assert!(transactions.active_on("DP-1"));
        assert!(transactions.active_on("DP-2"));
        assert!(transactions.take("DP-1").is_some());
        assert!(!transactions.active_on("DP-1"));
        assert!(transactions.take("DP-1").is_none());
        assert!(transactions.take("DP-2").is_some());
        assert!(!transactions.active_on("DP-2"));
    }

    #[test]
    fn editing_a_participant_forgets_its_whole_output_but_not_other_outputs() {
        use smithay::reexports::wayland_server::{
            Client, DataInit, Dispatch, Display, DisplayHandle,
            protocol::wl_surface::{self, WlSurface},
        };
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        struct State;
        impl Dispatch<WlSurface, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &WlSurface,
                _: wl_surface::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
        let display = Display::<State>::new().unwrap();
        let mut handle = display.handle();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let client = handle
            .insert_client(socket, Arc::new(crate::wayland::ClientState::default()))
            .unwrap();
        let surfaces = (0..4)
            .map(|_| {
                client
                    .create_resource::<WlSurface, (), State>(&handle, 1, ())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let restore =
            |surface: &WlSurface, output: &str| crate::presentation::maximize::FieldRestore {
                surface: surface.clone(),
                geometry: Rectangle::new((0, 0).into(), (400, 300).into()),
                output: output.into(),
            };
        let mut transactions = ArrangeTransactions::default();
        transactions.insert(
            "DP-1".into(),
            vec![restore(&surfaces[0], "DP-1"), restore(&surfaces[1], "DP-1")],
        );
        transactions.insert("DP-2".into(), vec![restore(&surfaces[2], "DP-2")]);

        transactions.invalidate_surface(&surfaces[3]);
        assert!(transactions.active_on("DP-1"));
        assert!(transactions.active_on("DP-2"));
        transactions.invalidate_surface(&surfaces[1]);
        assert!(!transactions.active_on("DP-1"));
        assert!(!transactions.contains_surface(&surfaces[0]));
        assert!(!transactions.contains_surface(&surfaces[1]));
        assert!(transactions.active_on("DP-2"));
        assert!(transactions.contains_surface(&surfaces[2]));
        assert!(transactions.take("DP-2").is_some());
    }

    #[test]
    fn eligibility_uses_full_work_area_while_layout_keeps_outer_gap() {
        let camera = Camera::new(
            Vec2 { x: 960.0, y: 540.0 },
            Vec2 {
                x: 1920.0,
                y: 1080.0,
            },
        );
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let work_area = Rectangle::<i32, Logical>::new((0, 30).into(), (1920, 1050).into());

        let (visible, _) = visible_work_area_outer(&camera, output, work_area, 0).unwrap();
        let (layout, gap) = visible_work_area_outer(&camera, output, work_area, 20).unwrap();

        assert_eq!(visible, Rectangle::new((0, 30).into(), (1920, 1050).into()));
        assert_eq!(layout, Rectangle::new((20, 50).into(), (1880, 1010).into()));
        assert_eq!(gap, 20);
        assert!(visible.contains((10, 40)));
        assert!(!layout.contains((10, 40)));
    }

    #[test]
    fn two_windows_fill_halves_with_gap() {
        let regions = mosaic_regions(
            Rectangle::<i32, Logical>::new((0, 0).into(), (1000, 800).into()),
            2,
            20,
        )
        .unwrap();
        assert_eq!(regions[0], Rectangle::new((0, 0).into(), (490, 800).into()));
        assert_eq!(
            regions[1],
            Rectangle::new((510, 0).into(), (490, 800).into())
        );
    }

    #[test]
    fn three_windows_use_large_left_and_split_right() {
        let regions = mosaic_regions(
            Rectangle::<i32, Logical>::new((0, 0).into(), (1000, 800).into()),
            3,
            20,
        )
        .unwrap();
        assert_eq!(regions[0], Rectangle::new((0, 0).into(), (490, 800).into()));
        assert_eq!(
            regions[1],
            Rectangle::new((510, 0).into(), (490, 390).into())
        );
        assert_eq!(
            regions[2],
            Rectangle::new((510, 410).into(), (490, 390).into())
        );
    }

    #[test]
    fn five_windows_form_balanced_three_two_mosaic() {
        let regions = mosaic_regions(
            Rectangle::<i32, Logical>::new((0, 0).into(), (1000, 800).into()),
            5,
            20,
        )
        .unwrap();
        assert_eq!(regions.len(), 5);
        assert_eq!(regions[0].size.h, 390);
        assert_eq!(regions[2].loc.y, 0);
        assert_eq!(regions[3].loc.y, 410);
        assert_eq!(regions[3].size.w, 490);
        assert_eq!(regions[4].loc.x, 510);
    }

    #[test]
    fn asymmetric_variants_offer_a_large_slot_on_smaller_outputs() {
        let outer = Rectangle::<i32, Logical>::new((0, 0).into(), (1800, 1000).into());
        let variants = mosaic_region_variants(outer, 3, 20).unwrap();

        assert_eq!(variants[0], mosaic_regions(outer, 3, 20).unwrap());
        assert!(variants[0].iter().all(|region| region.size.w < 1000));
        assert!(variants.iter().skip(1).any(|regions| {
            regions.len() == 3 && regions.iter().any(|region| region.size.w >= 1068)
        }));
    }

    #[test]
    fn feasible_assignment_keeps_a_constrained_window_in_a_featured_slot() {
        let impossible = super::INFEASIBLE_COST;
        let balanced = vec![
            vec![impossible, impossible, impossible],
            vec![1, 2, 3],
            vec![3, 2, 1],
        ];
        assert!(feasible_assignment(&balanced).is_none());

        let featured = vec![
            vec![1, impossible, impossible],
            vec![impossible, 1, 2],
            vec![impossible, 2, 1],
        ];
        assert_eq!(feasible_assignment(&featured), Some(vec![0, 1, 2]));
    }

    #[test]
    fn assignment_minimizes_total_window_travel() {
        let assignment =
            minimum_cost_assignment(&[vec![100, 1, 50], vec![1, 100, 50], vec![50, 50, 1]]);
        assert_eq!(assignment, vec![1, 0, 2]);
    }

    #[test]
    fn assignment_can_avoid_infeasible_targets() {
        let impossible = super::INFEASIBLE_COST;
        let assignment = minimum_cost_assignment(&[vec![impossible, 1], vec![1, impossible]]);
        assert_eq!(assignment, vec![1, 0]);
    }

    #[test]
    fn assignment_terminates_when_no_target_is_feasible() {
        let impossible = super::INFEASIBLE_COST;
        let mut assignment =
            minimum_cost_assignment(&[vec![impossible, impossible], vec![impossible, impossible]]);
        assignment.sort_unstable();
        assert_eq!(assignment, vec![0, 1]);
    }
}
