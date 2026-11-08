//! Arrange: lay out the windows visible on one Field output without overlap,
//! and put them back with a second press.
//!
//! The layout is the mosaic packer that Apogee used, so it is predictable:
//! windows keep their shapes, larger windows get larger slots, nothing
//! overlaps, and Field reading order breaks ties. A window's minimum size is
//! part of the packing, so a window that cannot shrink (common for X11 apps
//! such as Steam) gets a slot at least that big and the others shrink instead.
//! A maximum size smaller than the slot leaves the window centred in it. No
//! eligible window is ever left out.

use halley_core::camera::Camera;
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{Session, SessionDriver};

/// How much more room the window you are working in asks for. The packer
/// turns weight into slot size through a square root, limited to 0.68 to
/// 1.45 times the average, so doubling it gives the focused window a clearly
/// larger slot without dwarfing everything else.
const FOCUSED_WEIGHT: f32 = 2.0;

/// Rows the packer may use. Matches Cluster Composer so both read alike.
const MAX_ROWS: usize = 3;

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

    /// A deliberate edit to one participant (moving, resizing, transferring
    /// or closing it) keeps that window where the user put it: only its own
    /// restore entry is forgotten. Untouched windows still go back on the
    /// next press, and an output with nothing left to restore arranges afresh.
    pub(crate) fn invalidate_surface(&mut self, surface: &WlSurface) {
        self.by_output.retain(|_, transaction| {
            transaction
                .restores
                .retain(|restore| restore.surface != *surface);
            !transaction.restores.is_empty()
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
    id: halley_core::field::NodeId,
    window: Window,
    surface: WlSurface,
    current: Rectangle<i32, Logical>,
    outer: Rectangle<i32, Logical>,
    /// The smallest outer size the window accepts, decorations included.
    min_outer: Size<i32, Logical>,
    center: Point<i32, Logical>,
}

pub(crate) fn arrange_visible<D: SessionDriver>(
    session: &mut Session<D>,
    output_name: &str,
) -> bool {
    if let Some(transaction) = session.interactions.field_arrange.take(output_name) {
        return restore_transaction(session, output_name, transaction);
    }
    if session.clusters.active_on(output_name).is_some() || session.interactions.has_grab() {
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
    // Which windows take part is decided on the whole visible work area; the
    // layout itself keeps one gap clear of bars and screen edges, since the
    // filled mosaic otherwise reaches them.
    let (layout_area, gap) = visible_work_area_outer(
        camera,
        output_geometry,
        work_area,
        session.settings.field.gap.ceil() as i32,
    )
    .unwrap_or((visible_area, 0));
    session.nodes.sync_from_space(&session.wayland.space);
    let candidates = session
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && !record.collapsed
                && record.output == output_name
                && session.nodes.field().is_visible(record.id)
                && !session.clusters.is_member(record.id)
                && !super::node_user_pinned(session, record.id)
                && !session.fullscreen.is_fullscreen_or_pending(&record.surface)
                && !session.maximize.contains(&record.surface)
                && !crate::input::grab::belongs_to_surface(
                    session.interactions.grab(),
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
            // Asking for a 1x1 window returns the client's minimum size, from
            // xdg min_size or X11 WM_NORMAL_HINTS alike.
            let min_client = accepted_size(&record.window, Size::from((1, 1)));
            let min_outer = crate::titlebar::outer_rect_for_client(
                &record.window,
                Rectangle::new(current.loc, min_client),
                &session.settings.decorations,
                &session.settings.font,
            )
            .size;
            visible_area.contains(center).then(|| Candidate {
                id: record.id,
                window: record.window.clone(),
                surface: record.surface.clone(),
                current,
                outer,
                min_outer,
                center,
            })
        })
        .collect::<Vec<_>>();

    if candidates.len() < 2 {
        return false;
    }
    let slots = arrange_slots(&candidates, session.nodes.focused(), layout_area, gap);
    let assignment = candidates
        .iter()
        .zip(slots)
        .enumerate()
        .map(|(index, (candidate, slot))| {
            let requested = crate::titlebar::client_rect_for_outer(
                &candidate.window,
                slot,
                &session.settings.decorations,
                &session.settings.font,
            );
            (index, fit_accepted_size(&candidate.window, requested))
        })
        .collect::<Vec<_>>();

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

/// One outer rectangle per candidate, in Field coordinates inside `area`.
fn arrange_slots(
    candidates: &[Candidate],
    focused: Option<halley_core::field::NodeId>,
    area: Rectangle<i32, Logical>,
    gap: i32,
) -> Vec<Rectangle<i32, Logical>> {
    let items = candidates
        .iter()
        .map(|candidate| crate::shell::mosaic::Item {
            x: candidate.center.x as f32,
            y: candidate.center.y as f32,
            aspect: (candidate.outer.size.w.max(1) as f32 / candidate.outer.size.h.max(1) as f32)
                .clamp(0.25, 4.5),
            stable_key: candidate.id.as_u64(),
            weight: candidate.outer.size.w.max(1) as f32
                * candidate.outer.size.h.max(1) as f32
                * if focused == Some(candidate.id) {
                    FOCUSED_WEIGHT
                } else {
                    1.0
                },
            min_w: candidate.min_outer.w as f32,
            min_h: candidate.min_outer.h as f32,
        })
        .collect::<Vec<_>>();
    slots_in(&items, area, gap)
}

fn slots_in(
    items: &[crate::shell::mosaic::Item],
    area: Rectangle<i32, Logical>,
    gap: i32,
) -> Vec<Rectangle<i32, Logical>> {
    let gap = gap.max(0) as f32;
    let mut slots = crate::shell::mosaic::mosaic(items, area.size.w, area.size.h, gap, MAX_ROWS);
    crate::shell::mosaic::fill(&mut slots, items, area.size.w, area.size.h, gap);
    slots
        .into_iter()
        .map(|slot| {
            // Round edges, not centre and size, so filled tiles that reach the
            // area edge cannot spill past it by a rounding pixel.
            let left = ((slot.cx - slot.w * 0.5).round() as i32).clamp(0, area.size.w - 1);
            let top = ((slot.cy - slot.h * 0.5).round() as i32).clamp(0, area.size.h - 1);
            let right = ((slot.cx + slot.w * 0.5).round() as i32).clamp(left + 1, area.size.w);
            let bottom = ((slot.cy + slot.h * 0.5).round() as i32).clamp(top + 1, area.size.h);
            Rectangle::new(
                (area.loc.x + left, area.loc.y + top).into(),
                (right - left, bottom - top).into(),
            )
        })
        .collect()
}

/// The size the window accepts nearest to `requested`, centred on it.
fn fit_accepted_size(
    window: &Window,
    requested: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    if window_size_is_accepted(window, requested.size) {
        return requested;
    }
    let size = accepted_size(window, requested.size);
    let center = rect_center(requested);
    Rectangle::new((center.x - size.w / 2, center.y - size.h / 2).into(), size)
}

fn accepted_size(window: &Window, requested: Size<i32, Logical>) -> Size<i32, Logical> {
    let requested = Size::from((requested.w.max(1), requested.h.max(1)));
    if crate::xwayland::is_x11(window) {
        return crate::xwayland::constrain_window_size(window, requested);
    }
    let Some(toplevel) = window.toplevel() else {
        return requested;
    };
    smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
        let mut cached = states
            .cached_state
            .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
        let state = cached.current();
        clamp_to_limits(requested, state.min_size, state.max_size)
    })
}

/// Clamps each axis to a client's limits, where 0 means "no limit".
fn clamp_to_limits(
    requested: Size<i32, Logical>,
    min: Size<i32, Logical>,
    max: Size<i32, Logical>,
) -> Size<i32, Logical> {
    let axis = |value: i32, min: i32, max: i32| {
        let value = if max > 0 { value.min(max) } else { value };
        if min > 0 { value.max(min) } else { value }
    };
    (
        axis(requested.w, min.w, max.w),
        axis(requested.h, min.h, max.h),
    )
        .into()
}

fn rect_center(rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2).into()
}

#[cfg(test)]
mod tests {
    use super::{ArrangeTransactions, clamp_to_limits, slots_in, visible_work_area_outer};
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
    fn editing_a_participant_forgets_only_that_window() {
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
        assert!(
            transactions.active_on("DP-1"),
            "the untouched window still goes back"
        );
        assert!(transactions.contains_surface(&surfaces[0]));
        assert!(!transactions.contains_surface(&surfaces[1]));
        assert!(transactions.active_on("DP-2"));

        transactions.invalidate_surface(&surfaces[0]);
        assert!(
            !transactions.active_on("DP-1"),
            "an output with nothing left to restore arranges afresh"
        );
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

    fn item(x: f32, y: f32, w: f32, h: f32, key: u64) -> crate::shell::mosaic::Item {
        crate::shell::mosaic::Item {
            x,
            y,
            aspect: w / h,
            stable_key: key,
            weight: w * h,
            min_w: 0.0,
            min_h: 0.0,
        }
    }

    fn overlaps(a: Rectangle<i32, Logical>, b: Rectangle<i32, Logical>) -> bool {
        a.loc.x < b.loc.x + b.size.w
            && b.loc.x < a.loc.x + a.size.w
            && a.loc.y < b.loc.y + b.size.h
            && b.loc.y < a.loc.y + a.size.h
    }

    #[test]
    fn arrange_places_every_window_inside_the_area_without_overlap() {
        let area = Rectangle::<i32, Logical>::new((-400, 200).into(), (2560, 1400).into());
        for count in 2..=12 {
            let items = (0..count)
                .map(|index| {
                    let wide = index % 3 == 0;
                    item(
                        (index * 137 % 2000) as f32,
                        (index * 89 % 1200) as f32,
                        if wide { 1600.0 } else { 900.0 },
                        if wide { 900.0 } else { 1000.0 },
                        index as u64,
                    )
                })
                .collect::<Vec<_>>();
            let slots = slots_in(&items, area, 20);
            assert_eq!(slots.len(), count, "no window is left out");
            for (index, slot) in slots.iter().enumerate() {
                assert!(
                    area.contains_rect(*slot),
                    "{count} windows: slot {index} {slot:?} leaves {area:?}"
                );
                for other in &slots[index + 1..] {
                    assert!(!overlaps(*slot, *other), "{count} windows overlap");
                }
            }
        }
    }

    #[test]
    fn arrange_keeps_each_window_shape_and_favours_bigger_windows() {
        let area = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let items = [
            item(100.0, 100.0, 1600.0, 900.0, 1),
            item(900.0, 100.0, 600.0, 900.0, 2),
            item(400.0, 700.0, 640.0, 480.0, 3),
        ];
        let slots = slots_in(&items, area, 20);
        for (item, slot) in items.iter().zip(&slots) {
            // Filling may stretch a tile up to a quarter beyond its shape,
            // plus a rounding pixel, but never flips wide into tall.
            let stretch = (slot.size.w as f32 / slot.size.h as f32) / item.aspect;
            assert!(
                (0.79..=1.26).contains(&stretch),
                "shape drifted too far: {stretch} for aspect {}",
                item.aspect
            );
        }
        let area_of = |slot: &Rectangle<i32, Logical>| slot.size.w * slot.size.h;
        assert!(area_of(&slots[0]) > area_of(&slots[2]));
    }

    #[test]
    fn arrange_uses_the_screen_instead_of_leaving_a_border() {
        let area = Rectangle::<i32, Logical>::new((0, 0).into(), (2560, 1400).into());
        for count in 2..=6_u64 {
            let items = (0..count)
                .map(|index| item(index as f32 * 400.0, 0.0, 800.0, 600.0, index))
                .collect::<Vec<_>>();
            let slots = slots_in(&items, area, 20);
            let left = slots.iter().map(|slot| slot.loc.x).min().unwrap();
            let top = slots.iter().map(|slot| slot.loc.y).min().unwrap();
            let right = slots
                .iter()
                .map(|slot| slot.loc.x + slot.size.w)
                .max()
                .unwrap();
            let bottom = slots
                .iter()
                .map(|slot| slot.loc.y + slot.size.h)
                .max()
                .unwrap();
            // The block reaches both edges of at least one axis.
            assert!(
                (left <= 1 && right >= area.size.w - 1) || (top <= 1 && bottom >= area.size.h - 1),
                "{count} windows leave a border: {left},{top} to {right},{bottom}"
            );
            let covered: i32 = slots.iter().map(|slot| slot.size.w * slot.size.h).sum();
            let fraction = covered as f32 / (area.size.w * area.size.h) as f32;
            // Three equal windows leave the lone bottom tile at its shape
            // limit, so it covers least; every other count is far above.
            assert!(fraction > 0.65, "{count} windows cover only {fraction:.2}");
        }
    }

    #[test]
    fn arrange_is_the_same_every_time_for_the_same_windows() {
        let area = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let items = (0..6)
            .map(|index| item(index as f32 * 300.0, 0.0, 800.0, 600.0, index))
            .collect::<Vec<_>>();
        assert_eq!(slots_in(&items, area, 20), slots_in(&items, area, 20));
    }

    #[test]
    fn a_window_with_a_large_minimum_size_gets_room_instead_of_overlapping() {
        let area = Rectangle::<i32, Logical>::new((0, 0).into(), (2560, 1400).into());
        // A tall X11 tool window that refuses to be smaller than 700x1100,
        // next to three ordinary windows.
        let mut tower = item(1800.0, 300.0, 700.0, 1100.0, 1);
        tower.min_w = 700.0;
        tower.min_h = 1100.0;
        let items = [
            item(100.0, 100.0, 1600.0, 900.0, 2),
            tower,
            item(400.0, 900.0, 1200.0, 800.0, 3),
            item(1200.0, 900.0, 900.0, 700.0, 4),
        ];
        let slots = slots_in(&items, area, 20);
        assert!(
            slots[1].size.w >= 700 && slots[1].size.h >= 1100,
            "{:?}",
            slots[1]
        );
        for (index, slot) in slots.iter().enumerate() {
            assert!(area.contains_rect(*slot));
            for other in &slots[index + 1..] {
                assert!(!overlaps(*slot, *other), "{slot:?} overlaps {other:?}");
            }
        }
    }

    #[test]
    fn the_focused_window_gets_a_larger_slot() {
        let area = Rectangle::<i32, Logical>::new((0, 0).into(), (2560, 1400).into());
        let mut items = (0..4)
            .map(|index| item(index as f32 * 400.0, 0.0, 800.0, 600.0, index))
            .collect::<Vec<_>>();
        let even = slots_in(&items, area, 20);
        items[2].weight *= super::FOCUSED_WEIGHT;
        let focused = slots_in(&items, area, 20);
        let area_of = |slot: &Rectangle<i32, Logical>| slot.size.w * slot.size.h;
        assert!(area_of(&focused[2]) > area_of(&even[2]));
        for other in [0, 1, 3] {
            assert!(area_of(&focused[2]) > area_of(&focused[other]));
        }
    }

    #[test]
    fn size_limits_clamp_each_axis_and_zero_means_unlimited() {
        let clamp = |requested: (i32, i32), min: (i32, i32), max: (i32, i32)| {
            clamp_to_limits(requested.into(), min.into(), max.into())
        };
        assert_eq!(clamp((400, 300), (0, 0), (0, 0)), (400, 300).into());
        assert_eq!(clamp((400, 300), (1000, 600), (0, 0)), (1000, 600).into());
        assert_eq!(clamp((2000, 300), (0, 0), (1200, 0)), (1200, 300).into());
        assert_eq!(clamp((400, 900), (500, 0), (0, 700)), (500, 700).into());
    }
}
