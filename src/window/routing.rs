use halley_config::WindowSpawnPlacement;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;

use crate::presentation::camera::OutputCameras;
use crate::wayland::WaylandState;

pub(crate) struct InitialWindowPlacement {
    pub output: Output,
    pub location: Point<i32, Logical>,
}

pub(crate) struct InitialWindowPlacementRequest<'a> {
    pub wayland: &'a WaylandState,
    pub cameras: &'a OutputCameras,
    pub primary_output: &'a Output,
    pub window: Option<&'a Window>,
    pub window_size: Size<i32, Logical>,
    pub placement: WindowSpawnPlacement,
    pub cursor_position: Point<f64, Logical>,
    pub gap: f32,
}

/// Resolves initial placement without moving any already-mapped window.
pub(crate) fn initial_window_placement(
    request: InitialWindowPlacementRequest<'_>,
) -> InitialWindowPlacement {
    let parent = request
        .window
        .and_then(|window| super::stacking::parent_window(&request.wayland.space, window));
    let focused = focused_window(request.wayland);
    let adjacent_anchor = parent.as_ref().or(focused.as_ref());
    let preferred_anchor = parent.as_ref();

    let mut output = match request.placement {
        WindowSpawnPlacement::Cursor => request
            .wayland
            .space
            .output_under(request.cursor_position)
            .next()
            .cloned(),
        WindowSpawnPlacement::Center
        | WindowSpawnPlacement::Adjacent
        | WindowSpawnPlacement::App => preferred_anchor
            .as_ref()
            .or(adjacent_anchor.as_ref())
            .and_then(|window| output_for_window(request.wayland, window)),
        WindowSpawnPlacement::Default | WindowSpawnPlacement::ViewportCenter => None,
    }
    .unwrap_or_else(|| {
        crate::wayland::focus::output_for_new_surface(request.wayland, None, request.primary_output)
    });

    let camera_centered = || {
        centered_location_for_size(
            request.wayland,
            request.cameras,
            &output,
            request.window_size,
        )
    };
    let location = match request.placement {
        WindowSpawnPlacement::Default | WindowSpawnPlacement::ViewportCenter => camera_centered(),
        WindowSpawnPlacement::Cursor => center_window(
            Point::<f32, Physical>::from((
                request.cursor_position.x as f32,
                request.cursor_position.y as f32,
            )),
            request.window_size,
        ),
        WindowSpawnPlacement::Center => preferred_anchor
            .as_ref()
            .and_then(|window| request.wayland.space.element_geometry(window))
            .map(|geometry| center_in_rect(geometry, request.window_size))
            .unwrap_or_else(camera_centered),
        WindowSpawnPlacement::Adjacent => adjacent_anchor
            .as_ref()
            .and_then(|anchor| {
                adjacent_location(
                    request.wayland,
                    &output,
                    anchor,
                    request.window_size,
                    request.gap,
                )
            })
            .unwrap_or_else(camera_centered),
        WindowSpawnPlacement::App => {
            if let Some(parent) = preferred_anchor.as_ref()
                && let Some(geometry) = request.wayland.space.element_geometry(parent)
            {
                center_in_rect(geometry, request.window_size)
            } else {
                adjacent_anchor
                    .as_ref()
                    .and_then(|anchor| {
                        adjacent_location(
                            request.wayland,
                            &output,
                            anchor,
                            request.window_size,
                            request.gap,
                        )
                    })
                    .unwrap_or_else(camera_centered)
            }
        }
    };

    // A parent or focused anchor can resolve an output after the initial
    // selection above. Keep the returned ownership and location coherent.
    if let Some(anchor_output) = match request.placement {
        WindowSpawnPlacement::Center | WindowSpawnPlacement::App => preferred_anchor.as_ref(),
        WindowSpawnPlacement::Adjacent => adjacent_anchor.as_ref(),
        _ => None,
    }
    .and_then(|window| output_for_window(request.wayland, window))
    {
        output = anchor_output;
    }

    InitialWindowPlacement { output, location }
}

pub(crate) fn centered_location_for_size(
    wayland: &WaylandState,
    cameras: &OutputCameras,
    output: &Output,
    window_size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    let Some(output_geometry) = wayland.space.output_geometry(output) else {
        return (0, 0).into();
    };
    let local_camera_center = cameras
        .view(&output.name())
        .map(|view| view.center)
        .unwrap_or_else(|| {
            Point::<f32, Physical>::from((
                output_geometry.size.w as f32 / 2.0,
                output_geometry.size.h as f32 / 2.0,
            ))
        });
    let center = crate::presentation::camera::global_center(local_camera_center, output_geometry);
    center_window(center, window_size)
}

fn focused_window(wayland: &WaylandState) -> Option<Window> {
    let focused = wayland.focused_window.as_ref()?;
    wayland
        .space
        .elements()
        .find(|window| {
            window
                .wl_surface()
                .is_some_and(|surface| surface.as_ref() == focused)
        })
        .cloned()
}

fn output_for_window(wayland: &WaylandState, window: &Window) -> Option<Output> {
    let name = crate::wayland::window_output_name(window)?;
    wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .cloned()
}

fn adjacent_location(
    wayland: &WaylandState,
    output: &Output,
    anchor: &Window,
    size: Size<i32, Logical>,
    gap: f32,
) -> Option<Point<i32, Logical>> {
    let anchor = wayland.space.element_geometry(anchor)?;
    let gap = gap.ceil().max(0.0) as i32;
    let centered_y = anchor.loc.y + (anchor.size.h - size.h) / 2;
    let centered_x = anchor.loc.x + (anchor.size.w - size.w) / 2;
    let candidates = [
        (anchor.loc.x + anchor.size.w + gap, centered_y).into(),
        (anchor.loc.x - size.w - gap, centered_y).into(),
        (centered_x, anchor.loc.y + anchor.size.h + gap).into(),
        (centered_x, anchor.loc.y - size.h - gap).into(),
    ];
    candidates.into_iter().find(|location| {
        let candidate = Rectangle::new(*location, size);
        wayland
            .space
            .elements()
            .filter(|window| {
                crate::wayland::window_output_name(window).is_some_and(|name| name == output.name())
            })
            .filter_map(|window| wayland.space.element_geometry(window))
            .all(|geometry| geometry.intersection(candidate).is_none())
    })
}

fn center_in_rect(rect: Rectangle<i32, Logical>, size: Size<i32, Logical>) -> Point<i32, Logical> {
    (
        rect.loc.x + (rect.size.w - size.w) / 2,
        rect.loc.y + (rect.size.h - size.h) / 2,
    )
        .into()
}

fn center_window(
    center: Point<f32, Physical>,
    window_size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    Point::from((
        (center.x - window_size.w as f32 / 2.0).round() as i32,
        (center.y - window_size.h as f32 / 2.0).round() as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::{center_in_rect, center_window};
    use smithay::utils::{Logical, Physical, Point, Rectangle, Size};

    #[test]
    fn secondary_output_camera_center_produces_global_window_location() {
        assert_eq!(
            center_window(Point::from((3620.0, 550.0)), Size::from((1000, 700))),
            Point::from((3120, 200))
        );
    }

    #[test]
    fn parent_center_uses_the_parent_geometry() {
        let parent = Rectangle::<i32, Logical>::new((100, 200).into(), (800, 600).into());
        assert_eq!(
            center_in_rect(parent, Size::from((400, 300))),
            Point::from((300, 350))
        );
    }

    #[test]
    fn cursor_centers_independently_of_the_buffer() {
        assert_eq!(
            center_window(
                Point::<f32, Physical>::from((1280.0, 720.0)),
                Size::from((640, 480))
            ),
            Point::from((960, 480))
        );
    }
}
