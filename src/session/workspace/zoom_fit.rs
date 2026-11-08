//! Zoom to fit: see every window and node on each monitor at once.
//!
//! This is the Field itself, zoomed out. Nothing moves and no separate scene
//! is drawn: each Field monitor's camera eases out until everything on it
//! fits, the way other infinite-canvas window managers show "everything".
//! Pressing the key again or Escape returns to the saved view. A plain click
//! on a window or node zooms back in on it and focuses it; a click on empty
//! Field returns to the saved view. Panning or zooming by hand simply keeps
//! the new view, and the saved one is forgotten.

use std::collections::HashMap;

use halley_core::camera::Camera;
use halley_core::field::{NodeId, Vec2};
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};

/// Zoom to fit may look further out than ordinary zoom (`zoom.min`), because
/// it is a temporary look rather than a place to work. Below this, windows are
/// too small to recognise.
const MIN_SCALE: f32 = 0.1;
/// Screen pixels kept clear around the fitted windows.
const MARGIN_PX: f32 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Fitted {
    saved_center: Vec2,
    saved_view_size: Vec2,
    fitted_center: Vec2,
    fitted_view_size: Vec2,
}

/// Saved views for monitors that are currently zoomed to fit.
#[derive(Clone, Debug, Default)]
pub(crate) struct ZoomFitState {
    by_output: HashMap<String, Fitted>,
}

impl ZoomFitState {
    /// Whether `output` is still showing the fitted view. Any manual pan or
    /// zoom moves the camera's targets away from it, which ends zoom to fit
    /// without needing every camera input to know about it.
    fn active_on(&self, output: &str, camera: &Camera) -> bool {
        self.by_output.get(output).is_some_and(|fitted| {
            camera.target_center == fitted.fitted_center
                && camera.target_view_size == fitted.fitted_view_size
        })
    }

    /// Forgets saved views whose cameras the user has since moved.
    fn prune(&mut self, cameras: &crate::presentation::camera::OutputCameras) {
        let stale = self
            .by_output
            .keys()
            .filter(|output| {
                cameras
                    .get(output)
                    .is_none_or(|camera| !self.active_on(output, camera))
            })
            .cloned()
            .collect::<Vec<_>>();
        for output in stale {
            self.by_output.remove(&output);
        }
    }
}

/// Field-unit bounds of everything that should be visible, as (min, max).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bounds {
    pub min: Vec2,
    pub max: Vec2,
}

impl Bounds {
    fn around(rect: Rectangle<i32, Logical>) -> Self {
        Self {
            min: Vec2 {
                x: rect.loc.x as f32,
                y: rect.loc.y as f32,
            },
            max: Vec2 {
                x: (rect.loc.x + rect.size.w) as f32,
                y: (rect.loc.y + rect.size.h) as f32,
            },
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: Vec2 {
                x: self.min.x.min(other.min.x),
                y: self.min.y.min(other.min.y),
            },
            max: Vec2 {
                x: self.max.x.max(other.max.x),
                y: self.max.y.max(other.max.y),
            },
        }
    }
}

/// The camera view that shows `bounds` on an output of `screen` logical
/// pixels: a centre in global Field coordinates and a render scale. The
/// scale never magnifies past 1.0 and never drops below [`MIN_SCALE`].
///
/// `node_margin_px` is extra screen room around the edges for nodes. Nodes are
/// drawn at a fixed screen size, and as the camera zooms out the zoom reflow
/// pushes them up to about one node away from neighbouring windows, so the
/// room they need is constant in screen pixels rather than in Field units.
pub(crate) fn fit(bounds: Bounds, screen: Vec2, node_margin_px: f32) -> (Vec2, f32) {
    let width = (bounds.max.x - bounds.min.x).max(1.0);
    let height = (bounds.max.y - bounds.min.y).max(1.0);
    let margin = MARGIN_PX + node_margin_px.max(0.0);
    let room_x = (screen.x - margin * 2.0).max(1.0);
    let room_y = (screen.y - margin * 2.0).max(1.0);
    let scale = (room_x / width).min(room_y / height).clamp(MIN_SCALE, 1.0);
    let center = Vec2 {
        x: (bounds.min.x + bounds.max.x) * 0.5,
        y: (bounds.min.y + bounds.max.y) * 0.5,
    };
    (center, scale)
}

/// Whether any monitor is currently zoomed to fit.
pub(crate) fn is_active<D: SessionDriver>(session: &mut Session<D>) -> bool {
    session.interactions.zoom_fit.prune(&session.cameras);
    !session.interactions.zoom_fit.by_output.is_empty()
}

/// Whether any monitor is showing its fitted view, without forgetting stale
/// entries. For read-only callers such as keyboard filtering.
pub(crate) fn showing<D: SessionDriver>(session: &Session<D>) -> bool {
    session
        .interactions
        .zoom_fit
        .by_output
        .keys()
        .any(|output| {
            session
                .cameras
                .get(output)
                .is_some_and(|camera| session.interactions.zoom_fit.active_on(output, camera))
        })
}

/// Zooms every Field monitor out to fit, or returns if already fitted.
pub(crate) fn toggle<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if is_active(session) {
        return leave(session);
    }
    enter(session)
}

/// Zooms every Field monitor out to fit everything on it.
pub(crate) fn enter<D: SessionDriver>(session: &mut Session<D>) -> bool {
    if !session.settings.zoom.enabled {
        eventline::info!("zoom to fit: zoom is disabled in the config");
        return false;
    }
    session.nodes.sync_from_space(&session.wayland.space);
    let outputs = session.wayland.space.outputs().cloned().collect::<Vec<_>>();
    let mut changed = false;
    for output in outputs {
        let name = output.name();
        if session.clusters.active_on(&name).is_some() {
            continue;
        }
        let Some(geometry) = session.wayland.space.output_geometry(&output) else {
            continue;
        };
        let Some(bounds) = content_bounds(session, &name) else {
            continue;
        };
        let Some(camera) = session.cameras.get_mut_for_pan(&name) else {
            continue;
        };
        // Room for a node drawn at the edge plus one reflow push away from a
        // neighbouring window: a node diameter and its gap on top of the
        // half diameter it already needs.
        let node_margin =
            crate::nodes::NODE_DIAMETER_PX * 1.5 + session.settings.field.gap.max(0.0);
        let (center, scale) = fit(
            bounds,
            Vec2 {
                x: geometry.size.w as f32,
                y: geometry.size.h as f32,
            },
            node_margin,
        );
        let fitted = Fitted {
            saved_center: camera.target_center,
            saved_view_size: camera.target_view_size,
            fitted_center: Vec2 {
                x: center.x - geometry.loc.x as f32,
                y: center.y - geometry.loc.y as f32,
            },
            fitted_view_size: Vec2 {
                x: camera.base_size.x / scale,
                y: camera.base_size.y / scale,
            },
        };
        camera.zoom_log_vel = 0.0;
        camera.pan_vel = Vec2 { x: 0.0, y: 0.0 };
        camera.target_center = fitted.fitted_center;
        camera.target_view_size = fitted.fitted_view_size;
        session.interactions.zoom_fit.by_output.insert(name, fitted);
        changed = true;
    }
    if changed {
        session.request_redraw();
    }
    changed
}

/// Returns every fitted monitor to the view it had before.
pub(crate) fn leave<D: SessionDriver>(session: &mut Session<D>) -> bool {
    session.interactions.zoom_fit.prune(&session.cameras);
    let fitted = std::mem::take(&mut session.interactions.zoom_fit.by_output);
    let changed = !fitted.is_empty();
    for (output, fitted) in fitted {
        if let Some(camera) = session.cameras.get_mut_for_pan(&output) {
            camera.target_center = fitted.saved_center;
            camera.target_view_size = fitted.saved_view_size;
        }
    }
    if changed {
        session.request_redraw();
    }
    changed
}

/// Everything zoom to fit shows on one output: its Field windows and its
/// collapsed nodes. Cluster members live in their cluster and are left out.
fn content_bounds<D: SessionDriver>(session: &Session<D>, output: &str) -> Option<Bounds> {
    session
        .nodes
        .records()
        .filter(|record| record.output == output && !session.clusters.is_member(record.id))
        .filter_map(|record| {
            if record.collapsed {
                // A node is drawn at a fixed screen size, so its room is
                // reserved in screen pixels by `node_margin_px`, not here.
                let node = session.nodes.field().node(record.id)?;
                return Some(Bounds {
                    min: node.pos,
                    max: node.pos,
                });
            }
            if !record.attached {
                return None;
            }
            let client = session.wayland.space.element_geometry(&record.window)?;
            Some(Bounds::around(crate::titlebar::outer_rect_for_client(
                &record.window,
                client,
                &session.settings.decorations,
                &session.settings.font,
            )))
        })
        .reduce(Bounds::union)
}

/// What a plain click at `position` picks while its monitor is fitted.
enum Pick {
    Node(NodeId),
    Window(NodeId),
    Field,
}

fn pick_at<D: SessionDriver>(
    session: &Session<D>,
    output: &smithay::output::Output,
    geometry: Rectangle<i32, Logical>,
    camera: &Camera,
    position: Point<f64, Logical>,
) -> Pick {
    if let Some(id) = session.nodes.hit_test(output, geometry, camera, position) {
        return Pick::Node(id);
    }
    let world =
        crate::input::grab::screen_to_world_on_output((position.x, position.y), camera, geometry);
    let world = Point::<i32, Logical>::from((world.x.round() as i32, world.y.round() as i32));
    session
        .wayland
        .space
        .elements()
        .rev()
        .find_map(|window| {
            let id = session
                .nodes
                .id_for_surface(window.wl_surface()?.as_ref())?;
            let record = session.nodes.record(id)?;
            if record.output != output.name() || record.collapsed || !record.attached {
                return None;
            }
            let client = session.wayland.space.element_geometry(window)?;
            crate::titlebar::outer_rect_for_client(
                window,
                client,
                &session.settings.decorations,
                &session.settings.font,
            )
            .contains(world)
            .then_some(Pick::Window(id))
        })
        .unwrap_or(Pick::Field)
}

/// Zooms back in on the picked window or node and focuses it. A collapsed
/// node is restored. An empty-Field pick just returns to the saved views.
fn open<D: SessionDriver>(session: &mut Session<D>, output: &str, pick: Pick) {
    let target = match pick {
        Pick::Node(id) | Pick::Window(id) => Some(id),
        Pick::Field => None,
    };
    let saved = session.interactions.zoom_fit.by_output.get(output).copied();
    leave(session);
    let (Some(id), Some(saved)) = (target, saved) else {
        return;
    };
    // Zoom back in around the picked item, not around the old view.
    let center = session.nodes.field().node(id).map(|node| node.pos);
    let geometry = session
        .wayland
        .space
        .outputs()
        .find(|candidate| candidate.name() == output)
        .and_then(|candidate| session.wayland.space.output_geometry(candidate));
    if let (Some(center), Some(geometry), Some(camera)) =
        (center, geometry, session.cameras.get_mut_for_pan(output))
    {
        camera.target_view_size = saved.saved_view_size;
        camera.target_center = Vec2 {
            x: center.x - geometry.loc.x as f32,
            y: center.y - geometry.loc.y as f32,
        };
    }
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    crate::nodes::focus_or_reveal_node(session, id, serial, false);
    session.request_redraw();
}

/// Claims a plain left click on a fitted monitor. `Mod`-clicks and drags
/// still reach the ordinary Field handlers, so windows stay movable.
pub(crate) fn handle_pointer<D, B>(
    session: &mut Session<D>,
    event: &smithay::backend::input::InputEvent<B>,
) -> bool
where
    D: SessionDriver,
    B: smithay::backend::input::InputBackend,
{
    use smithay::backend::input::{ButtonState, InputEvent, PointerButtonEvent};
    let InputEvent::PointerButton { event } = event else {
        return false;
    };
    if event.state() != ButtonState::Pressed
        || event.button_code() != crate::input::buttons::BTN_LEFT
        || session
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().logo)
    {
        return false;
    }
    if !is_active(session) {
        return false;
    }
    let position = Point::<f64, Logical>::from(session.pointer.position());
    let Some(output) = session
        .wayland
        .space
        .output_under(session.pointer.position())
        .next()
        .cloned()
    else {
        return false;
    };
    let name = output.name();
    let Some(camera) = session.cameras.get(&name).copied() else {
        return false;
    };
    if !session.interactions.zoom_fit.active_on(&name, &camera) {
        return false;
    }
    let Some(geometry) = session.wayland.space.output_geometry(&output) else {
        return false;
    };
    let pick = pick_at(session, &output, geometry, &camera, position);
    session
        .interactions
        .screenshot_buttons
        .suppress(event.button_code());
    open(session, &name, pick);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Vec2 = Vec2 {
        x: 1920.0,
        y: 1080.0,
    };

    fn bounds(min: (f32, f32), max: (f32, f32)) -> Bounds {
        Bounds {
            min: Vec2 { x: min.0, y: min.1 },
            max: Vec2 { x: max.0, y: max.1 },
        }
    }

    #[test]
    fn fits_spread_out_windows_and_centres_on_them() {
        let (center, scale) = fit(bounds((-2000.0, -500.0), (2000.0, 1500.0)), SCREEN, 0.0);
        assert_eq!(center, Vec2 { x: 0.0, y: 500.0 });
        let visible_w = SCREEN.x / scale;
        let visible_h = SCREEN.y / scale;
        assert!(visible_w >= 4000.0 && visible_h >= 2000.0);
        assert!(scale < 0.5);
    }

    #[test]
    fn never_magnifies_and_never_zooms_past_the_floor() {
        let (_, small) = fit(bounds((0.0, 0.0), (300.0, 200.0)), SCREEN, 0.0);
        assert_eq!(small, 1.0, "a few small windows are shown at full size");
        let (_, huge) = fit(bounds((0.0, 0.0), (100_000.0, 100_000.0)), SCREEN, 0.0);
        assert_eq!(huge, MIN_SCALE);
    }

    #[test]
    fn nodes_at_the_edge_keep_screen_room_at_any_zoom() {
        // A node far to the left of a window: after fitting, the node must
        // sit at least `node_margin` screen pixels inside the screen edge.
        let node_margin = 100.0;
        let content = bounds((-6000.0, 0.0), (2000.0, 900.0));
        let (center, scale) = fit(content, SCREEN, node_margin);
        let left_edge_world = center.x - SCREEN.x * 0.5 / scale;
        let node_screen_x = (content.min.x - left_edge_world) * scale;
        assert!(
            node_screen_x >= node_margin,
            "node is {node_screen_x}px from the edge"
        );
    }

    #[test]
    fn manual_camera_movement_ends_zoom_to_fit() {
        let mut camera = Camera::new(Vec2 { x: 960.0, y: 540.0 }, SCREEN);
        let fitted = Fitted {
            saved_center: camera.target_center,
            saved_view_size: camera.target_view_size,
            fitted_center: Vec2 { x: 100.0, y: 100.0 },
            fitted_view_size: Vec2 {
                x: 3840.0,
                y: 2160.0,
            },
        };
        let mut state = ZoomFitState::default();
        state.by_output.insert("DP-1".into(), fitted);
        camera.target_center = fitted.fitted_center;
        camera.target_view_size = fitted.fitted_view_size;
        assert!(state.active_on("DP-1", &camera));
        camera.pan_target(Vec2 { x: 10.0, y: 0.0 });
        assert!(!state.active_on("DP-1", &camera));
    }
}
