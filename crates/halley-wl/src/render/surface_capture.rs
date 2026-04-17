use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, Offscreen, Renderer,
            element::{
                Kind, render_elements,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::{GlesError, GlesRenderer, GlesTexProgram, GlesTexture},
            utils::draw_render_elements,
        },
    },
    desktop::utils::bbox_from_surface_tree,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Rectangle, Size, Transform},
};

use super::clipped_surface::ClippedSurfaceRenderElement;

type SurfaceElement = WaylandSurfaceRenderElement<GlesRenderer>;
render_elements! {
    OffscreenElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Clipped=ClippedSurfaceRenderElement,
}

#[derive(Debug)]
pub(crate) enum OffscreenSurfaceError {
    EmptySurfaceTree,
    Render(GlesError),
}

impl std::fmt::Display for OffscreenSurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySurfaceTree => write!(f, "surface tree has no renderable size"),
            Self::Render(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for OffscreenSurfaceError {}

impl From<GlesError> for OffscreenSurfaceError {
    fn from(value: GlesError) -> Self {
        Self::Render(value)
    }
}

#[derive(Debug)]
pub(crate) struct OffscreenSurfaceTexture {
    pub texture: GlesTexture,
    pub bbox: Rectangle<i32, Logical>,
    pub has_content: bool,
}

pub(crate) fn render_surface_tree_to_texture(
    renderer: &mut GlesRenderer,
    wl: &WlSurface,
    alpha: f32,
    clip_to_geometry: Option<(Rectangle<i32, Logical>, f32, GlesTexProgram)>,
) -> Result<OffscreenSurfaceTexture, OffscreenSurfaceError> {
    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w <= 0 || bbox.size.h <= 0 {
        return Err(OffscreenSurfaceError::EmptySurfaceTree);
    }

    let logical_size = bbox.size;
    let physical_size: Size<i32, Physical> = (logical_size.w.max(1), logical_size.h.max(1)).into();

    let elements: Vec<SurfaceElement> = render_elements_from_surface_tree(
        renderer,
        wl,
        (-bbox.loc.x, -bbox.loc.y),
        1.0,
        alpha.clamp(0.0, 1.0),
        Kind::Unspecified,
    );
    let geo_rect = clip_to_geometry.as_ref().map(|(geo_rect, _, _)| {
        Rectangle::<i32, Physical>::new(
            ((geo_rect.loc.x - bbox.loc.x), (geo_rect.loc.y - bbox.loc.y)).into(),
            (geo_rect.size.w, geo_rect.size.h).into(),
        )
    });
    let has_content = !elements.is_empty();
    let elements: Vec<OffscreenElement> = elements
        .into_iter()
        .map(|elem| {
            if let (Some((_, radius, program)), Some(geo_rect)) = (&clip_to_geometry, geo_rect) {
                ClippedSurfaceRenderElement::new(elem, program.clone(), geo_rect, *radius).into()
            } else {
                elem.into()
            }
        })
        .collect();

    let mut texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        renderer,
        Fourcc::Argb8888,
        (physical_size.w, physical_size.h).into(),
    )?;

    {
        let mut target = renderer.bind(&mut texture)?;
        let damage = Rectangle::<i32, Physical>::from_size(physical_size);

        let mut frame = renderer.render(&mut target, physical_size, Transform::Normal)?;
        frame.clear(Color32F::TRANSPARENT, &[damage])?;

        let _ = draw_render_elements(&mut frame, 1.0, &elements, &[damage]);

        let _ = frame.finish()?;
    }

    Ok(OffscreenSurfaceTexture {
        texture,
        bbox,
        has_content,
    })
}
