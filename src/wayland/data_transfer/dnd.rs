use std::time::Duration;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::Space;
use smithay::desktop::utils::{bbox_from_surface_tree, output_update, send_frames_surface_tree};
use smithay::output::{self, Output};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Transform};
use smithay::wayland::compositor::{
    SurfaceAttributes, SurfaceData, send_surface_state as send_compositor_surface_state,
    with_states,
};
use smithay::wayland::fractional_scale::with_fractional_scale;

use crate::session::{Session, SessionDriver};

#[derive(Debug)]
pub struct DndIcon {
    pub surface: WlSurface,
    offset: Point<i32, Logical>,
}

pub fn set_icon<D: SessionDriver>(session: &mut Session<D>, surface: Option<WlSurface>) {
    clear(session);
    session.wayland.dnd_icon = surface.map(|surface| DndIcon {
        surface,
        offset: Point::default(),
    });
    refresh_outputs(
        session.wayland.dnd_icon.as_ref(),
        &session.wayland.space,
        session.pointer.position(),
    );
    session.request_redraw();
}

pub fn clear<D: SessionDriver>(session: &mut Session<D>) {
    if let Some(icon) = session.wayland.dnd_icon.take() {
        for output in session.wayland.space.outputs() {
            output_update(output, None, &icon.surface);
        }
        session.request_redraw();
    }
}

pub fn handle_commit<D: SessionDriver>(
    session: &mut Session<D>,
    committed: &WlSurface,
    root: &WlSurface,
) -> bool {
    let Some(icon) = session
        .wayland
        .dnd_icon
        .as_mut()
        .filter(|icon| &icon.surface == root)
    else {
        return false;
    };
    if committed == &icon.surface {
        let delta = with_states(&icon.surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_delta
                .take()
                .unwrap_or_default()
        });
        icon.offset += delta;
    }
    refresh_outputs(
        session.wayland.dnd_icon.as_ref(),
        &session.wayland.space,
        session.pointer.position(),
    );
    true
}

pub fn destroyed<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) -> bool {
    let matches = session
        .wayland
        .dnd_icon
        .as_ref()
        .is_some_and(|icon| icon.surface == *surface);
    if matches {
        clear(session);
    }
    matches
}

pub fn elements(
    renderer: &mut GlesRenderer,
    icon: Option<&DndIcon>,
    _output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    pointer_position: (f64, f64),
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let Some(icon) = icon else {
        return Vec::new();
    };
    let origin = Point::<i32, Logical>::from((
        pointer_position.0.round() as i32,
        pointer_position.1.round() as i32,
    )) + icon.offset;
    if output_geometry
        .intersection(bbox_from_surface_tree(&icon.surface, origin))
        .is_none()
    {
        return Vec::new();
    }
    let scale = Scale::from(1.0);
    let local = (origin - output_geometry.loc)
        .to_f64()
        .to_physical(scale)
        .to_i32_round();
    render_elements_from_surface_tree(
        renderer,
        &icon.surface,
        local,
        scale,
        1.0,
        Kind::ScanoutCandidate,
    )
}

pub fn refresh_outputs(
    icon: Option<&DndIcon>,
    space: &Space<smithay::desktop::Window>,
    pointer_position: (f64, f64),
) {
    let Some(icon) = icon else {
        return;
    };
    let origin = Point::<i32, Logical>::from((
        pointer_position.0.round() as i32,
        pointer_position.1.round() as i32,
    )) + icon.offset;
    let bounds = bbox_from_surface_tree(&icon.surface, origin);
    let mut preferred_scale = 1.0_f64;
    let mut preferred_transform = Transform::Normal;
    for output in space.outputs() {
        let Some(geometry) = space.output_geometry(output) else {
            continue;
        };
        if let Some(mut overlap) = geometry.intersection(bounds) {
            overlap.loc -= origin;
            output_update(output, Some(overlap), &icon.surface);
            let scale = output.current_scale().fractional_scale();
            if scale >= preferred_scale {
                preferred_scale = scale;
                preferred_transform = output.current_transform();
            }
        } else {
            output_update(output, None, &icon.surface);
        }
    }
    with_states(&icon.surface, |data| {
        send_scale_transform(
            &icon.surface,
            data,
            output::Scale::Fractional(preferred_scale),
            preferred_transform,
        );
    });
}

pub fn send_frame(icon: Option<&DndIcon>, output: &Output, elapsed: Duration, sequence: u32) {
    let Some(icon) = icon else {
        return;
    };
    send_frames_surface_tree(
        &icon.surface,
        output,
        elapsed,
        super::super::frame_callbacks::FALLBACK_THROTTLE,
        |surface, states| {
            super::super::frame_callbacks::callback_output(surface, states, output, sequence, false)
        },
    );
}

fn send_scale_transform(
    surface: &WlSurface,
    data: &SurfaceData,
    scale: output::Scale,
    transform: Transform,
) {
    send_compositor_surface_state(surface, data, scale.integer_scale(), transform);
    with_fractional_scale(data, |fractional| {
        fractional.set_preferred_scale(scale.fractional_scale());
    });
}
