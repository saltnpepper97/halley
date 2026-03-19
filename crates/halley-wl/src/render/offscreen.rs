use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, Offscreen, Renderer,
            element::{
                Kind,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::{GlesError, GlesRenderer, GlesTexture},
            utils::draw_render_elements,
        },
    },
    desktop::utils::bbox_from_surface_tree,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Physical, Rectangle, Size, Transform},
};

type SurfaceElement = WaylandSurfaceRenderElement<GlesRenderer>;

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
}

pub(crate) fn render_surface_tree_to_texture(
    renderer: &mut GlesRenderer,
    wl: &WlSurface,
    alpha: f32,
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

    let mut texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        renderer,
        Fourcc::Argb8888,
        (physical_size.w, physical_size.h).into(),
    )?;

    {
        let mut target = renderer.bind(&mut texture)?;
        let damage = Rectangle::<i32, Physical>::from_size(physical_size);

        let mut frame = renderer.render(&mut target, physical_size, Transform::Normal)?;
        frame.clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[damage])?;

        let _ = draw_render_elements(&mut frame, 1.0, &elements, &[damage]);

        let _ = frame.finish()?;
    }

    Ok(OffscreenSurfaceTexture { texture, bbox })
}
