use std::error::Error;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::{CommitCounter, draw_render_elements};
use smithay::backend::renderer::{Bind, Color32F, ContextId, Frame, Offscreen, Renderer, Texture};
use smithay::desktop::Window;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::seat::WaylandFocus;

#[derive(Clone, Debug)]
pub struct WindowTexture {
    pub(crate) texture: GlesTexture,
    pub(crate) context: ContextId<GlesTexture>,
}

/// A window surface tree captured like Niri's resize snapshots: the texture
/// contains the encompassing render-element rectangle rather than a cleared
/// buffer forced to the configured window size. `surface_geometry` records
/// where that texture belongs relative to `window_size`, allowing each side of
/// a resize crossfade to be mapped independently in the shader.
#[derive(Clone, Debug)]
pub struct ResizeWindowTexture {
    pub(crate) texture: GlesTexture,
    pub(crate) context: ContextId<GlesTexture>,
    pub(crate) surface_geometry: Rectangle<i32, Physical>,
    pub(crate) window_size: Size<i32, Physical>,
    pub(crate) client_opaque: bool,
    /// Approximate opaque pixel count at capture. Field fullscreen uses this
    /// to hold the outgoing snapshot until Firefox has painted the larger
    /// allocation instead of blending in a black buffer.
    pub(crate) opaque_area: i64,
    /// Stable surface identities and their client-local bounds at capture
    /// time. Resize transactions use this to reject a mixed surface tree in
    /// which (for example) Firefox has resized its CSD surface but is still
    /// presenting the outgoing content surface.
    pub(crate) surface_layers: Vec<(Id, Rectangle<i32, Physical>)>,
}

/// Everything that affects the pixels or readiness metadata of a resize
/// capture. Root damage alone misses child commits, viewport changes and
/// subsurface removal/reordering, which can happen without a root repaint.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResizeCaptureState {
    window_geometry: Rectangle<i32, Logical>,
    layers: Vec<ResizeCaptureLayer>,
}

#[derive(Clone, Debug, PartialEq)]
struct ResizeCaptureLayer {
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Physical>,
    src: Rectangle<f64, Buffer>,
    transform: Transform,
    opaque_regions: Vec<Rectangle<i32, Physical>>,
}

pub(crate) struct ResizeCapture {
    pub(crate) state: ResizeCaptureState,
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
}

impl WindowTexture {
    pub fn render_element(
        &self,
        id: Id,
        destination: Rectangle<i32, Physical>,
        alpha: f32,
    ) -> TextureRenderElement<GlesTexture> {
        let source = Rectangle::from_size(
            self.texture
                .size()
                .to_logical(1, Transform::Normal)
                .to_f64(),
        );
        TextureRenderElement::from_static_texture(
            id,
            self.context.clone(),
            destination.loc.to_f64(),
            self.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha.clamp(0.0, 1.0)),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        )
    }
}

pub fn capture(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
) -> Result<WindowTexture, Box<dyn Error>> {
    capture_at_scale(renderer, window, reusable, 1.0)
}

fn capture_at_scale(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
    scale: f64,
) -> Result<WindowTexture, Box<dyn Error>> {
    let surface = window
        .wl_surface()
        .ok_or("window snapshot has no surface")?;
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Err("window snapshot has empty geometry".into());
    }

    let location = smithay::utils::Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical(1);
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            surface.as_ref(),
            location,
            1.0,
            1.0,
            Kind::Unspecified,
        );
    let elements = elements
        .into_iter()
        .map(|element| {
            crate::render::display_scale::element(
                crate::render::scene::SceneElement::Layer(element),
                scale,
                false,
            )
        })
        .collect::<Vec<_>>();
    capture_surface_elements(renderer, geometry, &elements, reusable, scale)
}

pub fn capture_for_resize(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
) -> Result<ResizeWindowTexture, Box<dyn Error>> {
    prepare_resize_capture(renderer, window)?.capture(renderer, reusable)
}

/// Collect the actual rendered tree before deciding whether GPU capture is
/// needed. In particular, destroying a subsurface does not require a commit
/// on its parent, so commit callbacks alone cannot invalidate this cache.
pub(crate) fn prepare_resize_capture(
    renderer: &mut GlesRenderer,
    window: &Window,
) -> Result<ResizeCapture, Box<dyn Error>> {
    let surface = window
        .wl_surface()
        .ok_or("window resize snapshot has no surface")?;
    let geometry = window.geometry();
    if geometry.size.w <= 0 || geometry.size.h <= 0 {
        return Err("window resize snapshot has empty geometry".into());
    }

    let location = smithay::utils::Point::from((-geometry.loc.x, -geometry.loc.y)).to_physical(1);
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(
            renderer,
            surface.as_ref(),
            location,
            1.0,
            1.0,
            Kind::Unspecified,
        );
    if elements.is_empty() {
        return Err("window resize snapshot surface tree is empty".into());
    }

    let state = ResizeCaptureState {
        window_geometry: geometry,
        layers: elements
            .iter()
            .map(|element| ResizeCaptureLayer {
                id: element.id().clone(),
                commit: element.current_commit(),
                geometry: element.geometry(Scale::from(1.0)),
                src: element.src(),
                transform: element.transform(),
                opaque_regions: element
                    .opaque_regions(Scale::from(1.0))
                    .into_iter()
                    .collect(),
            })
            .collect(),
    };
    Ok(ResizeCapture { state, elements })
}

impl ResizeCapture {
    pub(crate) fn capture(
        self,
        renderer: &mut GlesRenderer,
        reusable: Option<GlesTexture>,
    ) -> Result<ResizeWindowTexture, Box<dyn Error>> {
        let geometry = self.state.window_geometry;
        let elements = self.elements;

        let surface_geometry = elements
            .iter()
            .map(|element| element.geometry(Scale::from(1.0)))
            .reduce(|bounds, geometry| bounds.merge(geometry))
            .ok_or("window resize snapshot surface tree is empty")?;
        if surface_geometry.size.w <= 0 || surface_geometry.size.h <= 0 {
            return Err("window resize snapshot surface tree has empty geometry".into());
        }
        let surface_layers = elements
            .iter()
            .map(|element| (element.id().clone(), element.geometry(Scale::from(1.0))))
            .collect::<Vec<_>>();
        let client_rect = Rectangle::<i32, Physical>::from_size(geometry.size.to_physical(1));
        let opaque_regions = elements
            .iter()
            .flat_map(|element| {
                let element_geometry = element.geometry(Scale::from(1.0));
                element
                    .opaque_regions(Scale::from(1.0))
                    .into_iter()
                    .map(move |opaque| {
                        Rectangle::new(element_geometry.loc + opaque.loc, opaque.size)
                    })
            })
            .collect::<Vec<_>>();
        let client_opaque = rectangle_is_covered(client_rect, &opaque_regions);
        let opaque_area = opaque_regions
            .iter()
            .map(|region| i64::from(region.size.w.max(0)) * i64::from(region.size.h.max(0)))
            .sum();
        let offset = surface_geometry.loc.upscale(-1);
        let relocated = elements
            .iter()
            .map(|element| RelocateRenderElement::from_element(element, offset, Relocate::Relative))
            .collect::<Vec<_>>();

        let size = surface_geometry.size;
        let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
        let context = renderer.context_id();
        let mut reusable = reusable;
        let can_reuse = reusable
            .as_mut()
            .is_some_and(|texture| texture.size() == buffer_size && texture.is_unique_reference());
        let mut texture = if can_reuse {
            reusable.expect("reusable texture checked above")
        } else {
            <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
                renderer,
                Fourcc::Abgr8888,
                buffer_size,
            )?
        };
        let damage = Rectangle::<i32, Physical>::from_size(size);
        {
            let mut target = renderer.bind(&mut texture)?;
            let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
            frame.clear(Color32F::TRANSPARENT, &[damage])?;
            draw_render_elements(&mut frame, 1.0, &relocated, &[damage])?;
            let _ = frame.finish()?;
        }

        Ok(ResizeWindowTexture {
            texture,
            context,
            surface_geometry,
            window_size: geometry.size.to_physical(1),
            client_opaque,
            opaque_area,
            surface_layers,
        })
    }
}

fn rectangle_is_covered(
    target: Rectangle<i32, Physical>,
    regions: &[Rectangle<i32, Physical>],
) -> bool {
    if target.size.w <= 0 || target.size.h <= 0 {
        return false;
    }
    let normalized = regions
        .iter()
        .map(|region| Rectangle::new(region.loc - target.loc, region.size))
        .collect::<Vec<_>>();
    rectangles_cover_size(target.size, &normalized)
}

pub(crate) fn rectangles_cover_size<Kind>(
    size: Size<i32, Kind>,
    rectangles: &[Rectangle<i32, Kind>],
) -> bool {
    if size.w <= 0 || size.h <= 0 {
        return false;
    }

    let clipped = rectangles
        .iter()
        .filter_map(|rect| {
            let x1 = rect.loc.x.max(0).min(size.w);
            let y1 = rect.loc.y.max(0).min(size.h);
            let x2 = rect.loc.x.saturating_add(rect.size.w).max(0).min(size.w);
            let y2 = rect.loc.y.saturating_add(rect.size.h).max(0).min(size.h);
            (x2 > x1 && y2 > y1).then_some((x1, y1, x2, y2))
        })
        .collect::<Vec<_>>();
    if clipped.is_empty() {
        return false;
    }

    let mut x_edges = vec![0, size.w];
    for &(x1, _, x2, _) in &clipped {
        x_edges.extend([x1, x2]);
    }
    x_edges.sort_unstable();
    x_edges.dedup();

    x_edges.windows(2).all(|slab| {
        let (left, right) = (slab[0], slab[1]);
        if right <= left {
            return true;
        }
        let mut intervals = clipped
            .iter()
            .filter_map(|&(x1, y1, x2, y2)| (x1 <= left && x2 >= right).then_some((y1, y2)))
            .collect::<Vec<_>>();
        intervals.sort_unstable();
        let mut covered_to = 0;
        for (start, end) in intervals {
            if start > covered_to {
                return false;
            }
            covered_to = covered_to.max(end);
            if covered_to >= size.h {
                return true;
            }
        }
        false
    })
}

fn capture_surface_elements<E: smithay::backend::renderer::element::RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    geometry: Rectangle<i32, Logical>,
    elements: &[E],
    reusable: Option<GlesTexture>,
    scale: f64,
) -> Result<WindowTexture, Box<dyn Error>> {
    if elements.is_empty() {
        return Err("window snapshot surface tree is empty".into());
    }

    let size = geometry.size.to_f64().to_physical(scale).to_i32_round();
    let context = renderer.context_id();
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
    let mut reusable = reusable;
    let can_reuse = reusable
        .as_mut()
        .is_some_and(|texture| texture.size() == buffer_size && texture.is_unique_reference());
    let mut texture = if can_reuse {
        reusable.expect("reusable texture checked above")
    } else {
        <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            buffer_size,
        )?
    };
    let damage = Rectangle::<i32, Physical>::from_size(size);
    {
        let mut target = renderer.bind(&mut texture)?;
        let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
        frame.clear(Color32F::TRANSPARENT, &[damage])?;
        draw_render_elements(&mut frame, 1.0, elements, &[damage])?;
        let _ = frame.finish()?;
    }

    Ok(WindowTexture { texture, context })
}

#[allow(clippy::too_many_arguments)]
pub fn capture_decorated(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    focused: bool,
    chrome_visible: bool,
    maximized: bool,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
) -> Result<WindowTexture, Box<dyn Error>> {
    capture_decorated_inner(
        renderer,
        window,
        reusable,
        decorations,
        font,
        focused,
        chrome_visible,
        maximized,
        titlebar_renderer,
        window_decoration_renderer,
        node_renderer,
        ui_text,
        1.0,
        |renderer, reusable| capture(renderer, window, reusable),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn capture_decorated_at_scale(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
    scale: f64,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    focused: bool,
    chrome_visible: bool,
    maximized: bool,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
) -> Result<WindowTexture, Box<dyn Error>> {
    capture_decorated_inner(
        renderer,
        window,
        reusable,
        decorations,
        font,
        focused,
        chrome_visible,
        maximized,
        titlebar_renderer,
        window_decoration_renderer,
        node_renderer,
        ui_text,
        scale,
        |renderer, reusable| capture_at_scale(renderer, window, reusable, scale),
    )
}

#[allow(clippy::too_many_arguments)]
fn capture_decorated_inner(
    renderer: &mut GlesRenderer,
    window: &Window,
    reusable: Option<GlesTexture>,
    decorations: &halley_config::Decorations,
    font: &halley_config::Font,
    focused: bool,
    chrome_visible: bool,
    maximized: bool,
    titlebar_renderer: &mut crate::render::titlebar::TitlebarRenderer,
    window_decoration_renderer: &mut crate::render::window_decoration::WindowDecorationRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    scale: f64,
    capture_client: impl FnOnce(
        &mut GlesRenderer,
        Option<GlesTexture>,
    ) -> Result<WindowTexture, Box<dyn Error>>,
) -> Result<WindowTexture, Box<dyn Error>> {
    if !chrome_visible || crate::xwayland::is_override_redirect(window) {
        return capture_client(renderer, reusable);
    }
    let client = capture_client(renderer, None)?;

    let client_size = window.geometry().size;
    let native_client = Rectangle::<i32, Logical>::from_size(client_size);
    let chrome = crate::titlebar::WindowChrome::for_window(window, decorations, font);
    let native_outer = chrome.outer_rect(native_client);
    let content = Rectangle::<i32, Physical>::new(
        (-native_outer.loc.x, -native_outer.loc.y).into(),
        client_size.to_physical(1),
    );
    let output_size = native_outer.size.to_f64().to_physical(scale).to_i32_round();
    let damage = Rectangle::<i32, Physical>::from_size(output_size);
    let server_titlebar = chrome.has_server_titlebar();
    let border_width = chrome.border_width;
    let content_radius = decorations.border_radius_px.max(0) as f32;
    let rounded = content_radius > 0.0 && window_decoration_renderer.available(renderer);
    let source = Rectangle::from_size(
        client
            .texture
            .size()
            .to_logical(1, Transform::Normal)
            .to_f64(),
    );
    let base = TextureRenderElement::from_static_texture(
        Id::new(),
        client.context.clone(),
        content.loc.to_f64(),
        client.texture.clone(),
        1,
        Transform::Normal,
        Some(1.0),
        Some(source),
        Some(content.size.to_logical(1)),
        None,
        Kind::Unspecified,
    );
    let mut elements = Vec::new();
    if server_titlebar {
        crate::render::scene::append_titlebar_elements(
            renderer,
            window,
            Some("snapshot"),
            content,
            crate::titlebar::effective_height(&decorations.titlebars, font.size),
            crate::titlebar::glyph_size(crate::titlebar::effective_height(
                &decorations.titlebars,
                font.size,
            )),
            1.0,
            maximized,
            border_width,
            decorations.titlebars.radius_px.max(0) as f32,
            focused,
            1.0,
            decorations,
            None,
            None,
            false,
            titlebar_renderer,
            window_decoration_renderer,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if rounded {
        let radii = if server_titlebar {
            crate::render::window_decoration::CornerRadii::bottom(content_radius)
        } else {
            crate::render::window_decoration::CornerRadii::all(content_radius)
        };
        let element = window_decoration_renderer
            .texture_element_with_radii(
                renderer,
                base,
                client.texture.clone(),
                content,
                radii,
                (1.0, 1.0, 1.0, 1.0),
            )
            .expect("rounded resources were checked above");
        elements.push(crate::render::scene::SceneElement::RoundedTexture(element));
    } else {
        elements.push(crate::render::scene::SceneElement::Closing(base));
    }
    let border_color = crate::render::window_border_color(decorations, focused);
    if border_width > 0 {
        if rounded
            && let Some(border) = if server_titlebar {
                window_decoration_renderer.body_border_element(
                    renderer,
                    Id::new(),
                    content,
                    border_width,
                    content_radius,
                    border_color,
                    1.0,
                )
            } else {
                window_decoration_renderer.border_element(
                    renderer,
                    Id::new(),
                    content,
                    border_width,
                    content_radius,
                    border_color,
                    1.0,
                )
            }
        {
            elements.push(crate::render::scene::SceneElement::WindowBorder(border));
        } else {
            let strips: Vec<_> = if server_titlebar {
                crate::render::body_border_strips(
                    std::array::from_fn(|_| Id::new()),
                    content,
                    border_width,
                    border_color,
                )
                .into_iter()
                .collect()
            } else {
                crate::render::border_strips(
                    std::array::from_fn(|_| Id::new()),
                    content,
                    border_width,
                    border_color,
                )
                .into_iter()
                .collect()
            };
            elements.extend(
                strips
                    .into_iter()
                    .map(crate::render::scene::SceneElement::Border),
            );
        }
    }

    let elements = elements
        .into_iter()
        .map(|element| crate::render::display_scale::element(element, scale, false))
        .collect::<Vec<_>>();
    let context = renderer.context_id();
    let buffer_size = output_size.to_logical(1).to_buffer(1, Transform::Normal);
    let mut reusable = reusable;
    let can_reuse = reusable
        .as_mut()
        .is_some_and(|texture| texture.size() == buffer_size && texture.is_unique_reference());
    let mut texture = if can_reuse {
        reusable.expect("reusable texture checked above")
    } else {
        <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            buffer_size,
        )?
    };
    {
        let mut target = renderer.bind(&mut texture)?;
        let mut frame = renderer.render(&mut target, output_size, Transform::Normal)?;
        frame.clear(Color32F::TRANSPARENT, &[damage])?;
        draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;
        let _ = frame.finish()?;
    }
    Ok(WindowTexture { texture, context })
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Buffer, Logical};

    #[test]
    fn texture_mapping_preserves_the_full_source() {
        let size = smithay::utils::Size::<i32, Buffer>::from((800, 600));
        let source = Rectangle::from_size(size.to_logical(1, Transform::Normal).to_f64());

        assert_eq!(
            source,
            Rectangle::<f64, Logical>::from_size((800.0, 600.0).into())
        );
    }
}
