use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::draw_render_elements;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer, Texture};
use smithay::output::Output;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform};
use smithay::wayland::seat::WaylandFocus;

use crate::render::{
    self, CursorContext, DesktopContext, FrameContext, OverlayContext, RenderRequest, VisualContext,
};
use crate::session::{Session, SessionDriver};

struct OutputImage {
    geometry: Rectangle<i32, Logical>,
    pixels: Vec<u8>,
}

pub fn save_region<D: SessionDriver>(
    session: &mut Session<D>,
    region: Rectangle<i32, Logical>,
) -> Result<CapturedImage, Box<dyn Error>> {
    save_region_inner(session, region, false, None)
}

pub fn save_area<D: SessionDriver>(
    session: &mut Session<D>,
    region: Rectangle<i32, Logical>,
) -> Result<CapturedImage, Box<dyn Error>> {
    save_region_inner(session, region, true, None)
}

/// An explicit output target must not composite overlapping/mirrored outputs.
pub(super) fn save_output_region<D: SessionDriver>(
    session: &mut Session<D>,
    output: &str,
    region: Rectangle<i32, Logical>,
) -> Result<CapturedImage, Box<dyn Error>> {
    save_region_inner(session, region, false, Some(output))
}

pub(super) fn screenshot_outputs(
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    name: Option<&str>,
) -> Vec<(Output, Rectangle<i32, Logical>)> {
    space
        .outputs()
        .filter(|output| name.is_none_or(|name| output.name() == name))
        .filter_map(|output| Some((output.clone(), space.output_geometry(output)?)))
        .collect()
}

fn save_region_inner<D: SessionDriver>(
    session: &mut Session<D>,
    mut region: Rectangle<i32, Logical>,
    trim_outer_gaps: bool,
    output_name: Option<&str>,
) -> Result<CapturedImage, Box<dyn Error>> {
    if session.session_lock.active() {
        return Err(io::Error::other("session is locked").into());
    }
    if region.size.w <= 0 || region.size.h <= 0 {
        return Err(io::Error::other("selected screenshot region is empty").into());
    }
    let all_outputs = screenshot_outputs(&session.wayland.space, output_name);
    if trim_outer_gaps {
        region =
            visible_region_bounds(region, all_outputs.iter().map(|(_, geometry)| *geometry))
                .ok_or_else(|| io::Error::other("selected screenshot region contains no output"))?;
    }
    // Only outputs the region actually covers need a scene built and read
    // back. A region on one monitor used to render and download every screen.
    let outputs = all_outputs
        .into_iter()
        .filter(|(_, geometry)| geometry.intersection(region).is_some())
        .collect::<Vec<_>>();
    if outputs.is_empty() {
        return Err(io::Error::other("selected screenshot region contains no output").into());
    }
    // A multi-monitor image uses a common pixel density, retaining native
    // detail on the densest selected monitor and keeping the layout contiguous.
    let image_scale = outputs
        .iter()
        .map(|(output, _)| output.current_scale().fractional_scale())
        .fold(0.0_f64, f64::max);
    let full_monitor_size = (outputs.len() == 1 && region == outputs[0].1)
        .then(|| render::output_physical_size(&outputs[0].0));
    let mut image_region = region.to_f64().upscale(image_scale).to_i32_round();
    if let Some(size) = full_monitor_size {
        image_region.size = size.to_logical(1);
    }
    let primary = session.driver.primary_output().clone();
    let target_time = crate::frame_clock::monotonic_now();
    let node_grab_active = matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::PendingNode { .. } | crate::input::grab::Grab::MoveNode { .. }
    );

    let driver = &mut session.driver;
    let cursor = &session.cursor;
    let pointer_position = session.pointer.position();
    let wayland = &session.wayland;
    let decorations = &session.settings.decorations;
    let blur = session.settings.effects.blur;
    let shadows = session.settings.effects.shadows;
    let cameras = &session.cameras;
    let window_animations = &session.window_animations;
    let fullscreen = &session.fullscreen;
    let maximize = &session.maximize;
    let nodes = &session.nodes;
    let clusters = &session.clusters;
    let window_rules = &session.window_rules;
    let bearings = &session.shell.bearings;
    let overlays = &session.shell.overlays;
    let overlay_config = &session.settings.overlays;
    let resources = &mut session.render;
    let session_lock = &session.session_lock;
    let images = driver.with_renderer(|renderer| -> Result<Vec<OutputImage>, Box<dyn Error>> {
        outputs
            .iter()
            .map(|(output, geometry)| {
                let pixels = capture_output(
                    renderer,
                    output,
                    &primary,
                    *geometry,
                    RenderRequest {
                        frame: FrameContext {
                            target_presentation_time: target_time,
                            vrr_auto_eligible: false,
                            force_full_repaint: false,
                            clear: render::CLEAR_COLOR,
                        },
                        desktop: DesktopContext {
                            fifo: &wayland.fifo,
                            session_lock,
                            space: &wayland.space,
                            focused: wayland.focused_window.as_ref(),
                            cameras,
                            window_animations,
                            fullscreen,
                            maximize,
                            nodes,
                            clusters,
                            window_rules,
                            screencast: &session.screencast,
                            layer_rules: &session.settings.layer_rules,
                            node_grab_active,
                            titlebar_hovered: session.interactions.titlebar_hovered.as_ref(),
                            titlebar_pressed: session.interactions.titlebar_pressed.as_ref(),
                        },
                        cursor: CursorContext {
                            cursor,
                            dnd_icon: None,
                            cursor_position: pointer_position,
                            show_cursor: false,
                            cursor_override: None,
                        },
                        overlays: OverlayContext {
                            capture_overlay: crate::capture::CaptureOverlay::None,
                            bearings,
                            focus_cycle: &session.shell.focus_cycle,
                            apogee: &session.shell.apogee,
                            cluster_composer: &session.shell.cluster_composer,
                            apogee_config: session.settings.apogee,
                            overlays,
                            overlay_config,
                        },
                        visuals: VisualContext {
                            decorations,
                            pins: &session.settings.field.pins,
                            font: &session.settings.font,
                            debug: session.settings.debug,
                            blur,
                            shadows,
                            background: &session.settings.background,
                            background_base: session.config_path.as_deref().and_then(Path::parent),
                        },
                        resources: crate::render::resources::RenderResources::from(&mut *resources),
                    },
                )?;
                let physical = render::output_physical_size(output);
                let mut image_geometry = geometry.to_f64().upscale(image_scale).to_i32_round();
                if full_monitor_size.is_some() {
                    image_geometry.size = physical.to_logical(1);
                }
                let pixels =
                    resize_output_pixels(pixels, physical, image_geometry.size.to_physical(1))?;
                Ok(OutputImage {
                    geometry: image_geometry,
                    pixels,
                })
            })
            .collect()
    })?;

    let pixels = composite_region(image_region, &images)?;
    Ok(CapturedImage {
        width: image_region.size.w as u32,
        height: image_region.size.h as u32,
        pixels,
    })
}

fn visible_region_bounds(
    region: Rectangle<i32, Logical>,
    outputs: impl IntoIterator<Item = Rectangle<i32, Logical>>,
) -> Option<Rectangle<i32, Logical>> {
    outputs
        .into_iter()
        .filter_map(|output| region.intersection(output))
        .reduce(Rectangle::merge)
}

pub fn save_window<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Result<CapturedImage, Box<dyn Error>> {
    if session.session_lock.active() {
        return Err(io::Error::other("session is locked").into());
    }
    if session.window_rules.applied(surface).block_screenshot {
        return Err(io::Error::other("window screenshot blocked by rule").into());
    }
    let window = session
        .wayland
        .space
        .elements()
        .find(|window| {
            window
                .wl_surface()
                .is_some_and(|candidate| candidate.as_ref() == surface)
        })
        .cloned()
        .ok_or_else(|| io::Error::other("selected window is not mapped"))?;
    let CapturedWindowPixels { pixels, size } = capture_decorated_window_pixels(session, &window)?;
    Ok(CapturedImage {
        width: size.w as u32,
        height: size.h as u32,
        pixels,
    })
}

pub(crate) fn capture_source_pixels<D: SessionDriver>(
    session: &mut Session<D>,
    source: &halley_ipc::CaptureSource,
    show_cursor: bool,
) -> Result<Vec<u8>, Box<dyn Error>> {
    if session.session_lock.active() {
        return Err(io::Error::other("session is locked").into());
    }
    match source {
        halley_ipc::CaptureSource::Monitor {
            name,
            width,
            height,
            ..
        } => with_monitor_scene(
            session,
            name,
            *width,
            *height,
            show_cursor,
            true,
            |renderer, elements, size| {
                capture_elements(
                    renderer,
                    Fourcc::Abgr8888,
                    size,
                    elements,
                    render::CLEAR_COLOR,
                )
            },
        ),
        halley_ipc::CaptureSource::Window {
            window_id,
            width,
            height,
        } => {
            let window = resolve_source_window(&session.wayland.space, &session.nodes, *window_id)?;
            validate_source_window_size(session, &window, *width, *height)?;
            if window.wl_surface().is_some_and(|surface| {
                session
                    .window_rules
                    .applied(surface.as_ref())
                    .block_screencast
            }) {
                let len = (*width as usize)
                    .checked_mul(*height as usize)
                    .and_then(|v| v.checked_mul(4))
                    .ok_or_else(|| io::Error::other("window frame dimensions overflow"))?;
                return Ok(vec![0; len]);
            }
            capture_decorated_window_pixels(session, &window).map(|capture| capture.pixels)
        }
    }
}

pub(crate) fn render_source_dmabuf<D: SessionDriver>(
    session: &mut Session<D>,
    source: &halley_ipc::CaptureSource,
    show_cursor: bool,
    dmabuf: &mut Dmabuf,
) -> Result<SyncPoint, Box<dyn Error>> {
    if session.session_lock.active() {
        return Err(io::Error::other("session is locked").into());
    }
    if !session.driver.import_dmabuf(dmabuf) {
        return Err(io::Error::other("renderer rejected capture DMA-BUF").into());
    }
    match source {
        halley_ipc::CaptureSource::Monitor {
            name,
            width,
            height,
            ..
        } => with_monitor_scene(
            session,
            name,
            *width,
            *height,
            show_cursor,
            true,
            |renderer, elements, size| {
                render_elements_to_dmabuf(renderer, dmabuf, size, elements, render::CLEAR_COLOR)
            },
        ),
        halley_ipc::CaptureSource::Window {
            window_id,
            width,
            height,
        } => {
            let window = resolve_source_window(&session.wayland.space, &session.nodes, *window_id)?;
            validate_source_window_size(session, &window, *width, *height)?;
            if window.wl_surface().is_some_and(|surface| {
                session
                    .window_rules
                    .applied(surface.as_ref())
                    .block_screencast
            }) {
                return session.driver.with_renderer(|renderer| {
                    render_elements_to_dmabuf::<render::scene::SceneElement>(
                        renderer,
                        dmabuf,
                        (*width, *height).into(),
                        &[],
                        Color32F::BLACK,
                    )
                });
            }
            with_decorated_window(session, &window, |renderer, element, size| {
                render_elements_to_dmabuf(renderer, dmabuf, size, &[element], Color32F::TRANSPARENT)
            })
        }
    }
}

pub(crate) fn with_monitor_scene<D, T>(
    session: &mut Session<D>,
    name: &str,
    width: i32,
    height: i32,
    show_cursor: bool,
    block_screencast: bool,
    consume: impl FnOnce(
        &mut GlesRenderer,
        &[render::scene::SceneElement],
        smithay::utils::Size<i32, Physical>,
    ) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>>
where
    D: SessionDriver,
{
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .cloned()
        .ok_or_else(|| io::Error::other(format!("unknown output {name}")))?;
    let geometry = session
        .wayland
        .space
        .output_geometry(&output)
        .ok_or_else(|| io::Error::other(format!("output {name} has no geometry")))?;
    if render::output_physical_size(&output) != (width, height).into() {
        return Err(io::Error::other("selected output size changed").into());
    }
    let primary = session.driver.primary_output().clone();
    let target_time = crate::frame_clock::monotonic_now();
    let node_grab_active = matches!(
        &session.interactions.grab,
        crate::input::grab::Grab::PendingNode { .. } | crate::input::grab::Grab::MoveNode { .. }
    );
    let driver = &mut session.driver;
    let cursor = &session.cursor;
    let pointer_position = session.pointer.position();
    let wayland = &session.wayland;
    let decorations = &session.settings.decorations;
    let blur = session.settings.effects.blur;
    let shadows = session.settings.effects.shadows;
    let cameras = &session.cameras;
    let window_animations = &session.window_animations;
    let fullscreen = &session.fullscreen;
    let maximize = &session.maximize;
    let nodes = &session.nodes;
    let clusters = &session.clusters;
    let window_rules = &session.window_rules;
    let bearings = &session.shell.bearings;
    let overlays = &session.shell.overlays;
    let overlay_config = &session.settings.overlays;
    let resources = &mut session.render;
    let session_lock = &session.session_lock;
    driver.with_renderer(|renderer| {
        let elements = render::scene::build_with_privacy(
            renderer,
            &output,
            &primary,
            geometry,
            RenderRequest {
                frame: FrameContext {
                    target_presentation_time: target_time,
                    vrr_auto_eligible: false,
                    force_full_repaint: false,
                    clear: render::CLEAR_COLOR,
                },
                desktop: DesktopContext {
                    fifo: &wayland.fifo,
                    session_lock,
                    space: &wayland.space,
                    focused: wayland.focused_window.as_ref(),
                    cameras,
                    window_animations,
                    fullscreen,
                    maximize,
                    nodes,
                    clusters,
                    window_rules,
                    screencast: &session.screencast,
                    layer_rules: &session.settings.layer_rules,
                    node_grab_active,
                    titlebar_hovered: session.interactions.titlebar_hovered.as_ref(),
                    titlebar_pressed: session.interactions.titlebar_pressed.as_ref(),
                },
                cursor: CursorContext {
                    cursor,
                    dnd_icon: show_cursor.then_some(wayland.dnd_icon.as_ref()).flatten(),
                    cursor_position: pointer_position,
                    show_cursor,
                    cursor_override: None,
                },
                overlays: OverlayContext {
                    capture_overlay: crate::capture::CaptureOverlay::None,
                    bearings,
                    focus_cycle: &session.shell.focus_cycle,
                    apogee: &session.shell.apogee,
                    cluster_composer: &session.shell.cluster_composer,
                    apogee_config: session.settings.apogee,
                    overlays,
                    overlay_config,
                },
                visuals: VisualContext {
                    decorations,
                    pins: &session.settings.field.pins,
                    font: &session.settings.font,
                    debug: session.settings.debug,
                    blur,
                    shadows,
                    background: &session.settings.background,
                    background_base: session.config_path.as_deref().and_then(Path::parent),
                },
                resources: crate::render::resources::RenderResources::from(resources),
            },
            block_screencast,
        )?;
        consume(renderer, &elements, render::output_physical_size(&output))
    })
}

pub(crate) fn capture_monitor_region_pixels<D: SessionDriver>(
    session: &mut Session<D>,
    output: &Output,
    region: Rectangle<i32, Physical>,
    show_cursor: bool,
) -> Result<Vec<u8>, Box<dyn Error>> {
    if session.wayland.space.output_geometry(output).is_none() {
        return Err(io::Error::other("screencopy output is not mapped").into());
    }
    with_monitor_scene(
        session,
        &output.name(),
        render::output_physical_size(output).w,
        render::output_physical_size(output).h,
        show_cursor,
        false,
        |renderer, elements, _| {
            let offset = region.loc.upscale(-1);
            let relocated = elements
                .iter()
                .map(|element| {
                    RelocateRenderElement::from_element(element, offset, Relocate::Relative)
                })
                .collect::<Vec<_>>();
            capture_elements(
                renderer,
                Fourcc::Xrgb8888,
                region.size,
                &relocated,
                render::CLEAR_COLOR,
            )
        },
    )
}

pub(crate) fn render_monitor_region_dmabuf<D: SessionDriver>(
    session: &mut Session<D>,
    output: &Output,
    region: Rectangle<i32, Physical>,
    show_cursor: bool,
    dmabuf: &mut Dmabuf,
) -> Result<SyncPoint, Box<dyn Error>> {
    if session.wayland.space.output_geometry(output).is_none() {
        return Err(io::Error::other("screencopy output is not mapped").into());
    }
    session.driver.import_dmabuf(dmabuf);
    with_monitor_scene(
        session,
        &output.name(),
        render::output_physical_size(output).w,
        render::output_physical_size(output).h,
        show_cursor,
        false,
        |renderer, elements, _| {
            let offset = region.loc.upscale(-1);
            let relocated = elements
                .iter()
                .map(|element| {
                    RelocateRenderElement::from_element(element, offset, Relocate::Relative)
                })
                .collect::<Vec<_>>();
            render_elements_to_dmabuf(
                renderer,
                dmabuf,
                region.size,
                &relocated,
                render::CLEAR_COLOR,
            )
        },
    )
}

pub(crate) fn resolve_source_window(
    space: &smithay::desktop::Space<smithay::desktop::Window>,
    nodes: &crate::nodes::NodesState,
    window_id: u64,
) -> Result<smithay::desktop::Window, Box<dyn Error>> {
    let window = space
        .elements()
        .find(|window| {
            window.wl_surface().is_some_and(|surface| {
                nodes
                    .id_for_surface(surface.as_ref())
                    .is_some_and(|id| id.as_u64() == window_id)
            })
        })
        .cloned()
        .ok_or_else(|| io::Error::other("selected window is no longer mapped"))?;
    Ok(window)
}

fn validate_source_window_size<D: SessionDriver>(
    session: &Session<D>,
    window: &smithay::desktop::Window,
    width: i32,
    height: i32,
) -> Result<(), Box<dyn Error>> {
    if crate::capture::window_capture_size(session, window) != (width, height).into() {
        return Err(io::Error::other("selected window size changed").into());
    }
    Ok(())
}

struct CapturedWindowPixels {
    pixels: Vec<u8>,
    size: smithay::utils::Size<i32, Physical>,
}

fn capture_decorated_window_pixels<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
) -> Result<CapturedWindowPixels, Box<dyn Error>> {
    with_decorated_window(session, window, |renderer, element, size| {
        let pixels = capture_elements(
            renderer,
            Fourcc::Abgr8888,
            size,
            &[element],
            Color32F::TRANSPARENT,
        )?;
        Ok(CapturedWindowPixels { pixels, size })
    })
}

fn with_decorated_window<D, T>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    consume: impl FnOnce(
        &mut GlesRenderer,
        smithay::backend::renderer::element::texture::TextureRenderElement<GlesTexture>,
        smithay::utils::Size<i32, Physical>,
    ) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>>
where
    D: SessionDriver,
{
    let chrome_visible = crate::capture::window_chrome_visible(session, window);
    let focused = window
        .wl_surface()
        .is_some_and(|surface| session.wayland.focused_window.as_ref() == Some(surface.as_ref()));
    let draw_border = window
        .wl_surface()
        .is_none_or(|surface| session.window_rules.applied(surface.as_ref()).border != Some(false));
    let maximized = window
        .wl_surface()
        .is_some_and(|surface| session.maximize.contains(surface.as_ref()));
    let scale = crate::capture::window_display_scale(session, window);
    let mut decorations = session.settings.decorations;
    if let Some(radius) = window
        .wl_surface()
        .and_then(|surface| session.window_rules.applied(surface.as_ref()).corner_radius)
    {
        decorations.border_radius_px = radius;
    }
    let font = &session.settings.font;
    let resources = &mut session.render;
    resources.ui_text.set_display_scale(scale);
    session.driver.with_renderer(|renderer| {
        let texture = crate::render::window_texture::capture_decorated_at_scale(
            renderer,
            window,
            None,
            scale,
            &decorations,
            font,
            focused,
            chrome_visible,
            maximized,
            draw_border,
            &mut resources.titlebar_renderer,
            &mut resources.window_decoration_renderer,
            &mut resources.node_renderer,
            &mut resources.ui_text,
        )?;
        let size = texture
            .texture
            .size()
            .to_logical(1, Transform::Normal)
            .to_physical(1);
        let element = texture.render_element(Id::new(), Rectangle::from_size(size), 1.0);
        consume(renderer, element, size)
    })
}

pub(crate) fn render_elements_to_dmabuf<E>(
    renderer: &mut GlesRenderer,
    dmabuf: &mut Dmabuf,
    size: smithay::utils::Size<i32, Physical>,
    elements: &[E],
    clear: Color32F,
) -> Result<SyncPoint, Box<dyn Error>>
where
    E: smithay::backend::renderer::element::RenderElement<GlesRenderer>,
{
    let damage = Rectangle::<i32, Physical>::from_size(size);
    let mut target = renderer.bind(dmabuf)?;
    let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
    frame.clear(clear, &[damage])?;
    draw_render_elements(&mut frame, 1.0, elements, &[damage])?;
    Ok(frame.finish()?)
}

/// A finished capture, still in memory. Encoding happens on the worker in
/// [`crate::capture::encoder`] so the compositor loop never blocks on zlib.
pub(crate) struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub(crate) fn screenshot_directory(configured_directory: &str) -> PathBuf {
    expand_directory(configured_directory)
}

/// Worker-side half of saving: allocate a filename and write the PNG.
pub(crate) fn write_capture(
    directory: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<PathBuf, Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let (path, file) = create_unique_file(directory)?;
    if let Err(err) = write_png(file, width, height, pixels) {
        let _ = fs::remove_file(&path);
        return Err(err.into());
    }
    Ok(path)
}

pub(crate) fn capture_cursor_surface_tree(
    renderer: &mut GlesRenderer,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    snapshot: Option<&crate::cursor::CursorSurfaceSnapshot>,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let size = geometry.size.to_f64().to_physical(scale).to_i32_round();
    let location = smithay::utils::Point::from((-geometry.loc.x, -geometry.loc.y))
        .to_f64()
        .to_physical(scale)
        .to_i32_round();
    let elements = crate::cursor::render::surface_elements(
        renderer,
        surface,
        snapshot,
        location,
        Scale::from(scale),
        Kind::Cursor,
    )?;
    if elements.is_empty() {
        return Err(io::Error::other("cursor surface tree is empty").into());
    }
    let elements = elements
        .into_iter()
        .map(|element| {
            crate::render::display_scale::element(
                crate::render::scene::SceneElement::Cursor(element.into()),
                scale,
                true,
            )
        })
        .collect::<Vec<_>>();
    capture_elements(
        renderer,
        Fourcc::Abgr8888,
        size,
        &elements,
        Color32F::TRANSPARENT,
    )
}

fn capture_elements<E>(
    renderer: &mut GlesRenderer,
    format: Fourcc,
    size: smithay::utils::Size<i32, Physical>,
    elements: &[E],
    clear: Color32F,
) -> Result<Vec<u8>, Box<dyn Error>>
where
    E: smithay::backend::renderer::element::RenderElement<GlesRenderer>,
{
    let buffer_size = smithay::utils::Size::<i32, Buffer>::from((size.w, size.h));
    let mut texture =
        <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(renderer, format, buffer_size)?;
    let damage = Rectangle::<i32, Physical>::from_size(size);
    {
        let mut target = renderer.bind(&mut texture)?;
        let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
        frame.clear(clear, &[damage])?;
        draw_render_elements(&mut frame, 1.0, elements, &[damage])?;
        let _ = frame.finish()?;
    }
    let mapping = renderer.copy_texture(
        &texture,
        Rectangle::<i32, Buffer>::from_size(buffer_size),
        format,
    )?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

fn capture_output(
    renderer: &mut GlesRenderer,
    output: &Output,
    primary: &Output,
    geometry: Rectangle<i32, Logical>,
    request: RenderRequest<'_>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let elements =
        render::scene::build_with_screenshot_privacy(renderer, output, primary, geometry, request)?;
    let size = render::output_physical_size(output);
    capture_elements(
        renderer,
        Fourcc::Abgr8888,
        size,
        &elements,
        render::CLEAR_COLOR,
    )
}

fn resize_output_pixels(
    pixels: Vec<u8>,
    source: smithay::utils::Size<i32, Physical>,
    destination: smithay::utils::Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    if source == destination {
        return Ok(pixels);
    }
    let image = image::RgbaImage::from_raw(source.w as u32, source.h as u32, pixels)
        .ok_or_else(|| io::Error::other("captured output size does not match pixels"))?;
    Ok(image::imageops::resize(
        &image,
        destination.w as u32,
        destination.h as u32,
        image::imageops::FilterType::Triangle,
    )
    .into_raw())
}

fn composite_region(
    region: Rectangle<i32, Logical>,
    outputs: &[OutputImage],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let width = region.size.w as usize;
    let height = region.size.h as usize;
    let mut composite = vec![0u8; width * height * 4];

    for output in outputs {
        let expected = output.geometry.size.w as usize * output.geometry.size.h as usize * 4;
        if output.pixels.len() != expected {
            return Err(io::Error::other("renderer returned an unexpected pixel count").into());
        }
        let Some(intersection) = region.intersection(output.geometry) else {
            continue;
        };
        let copy_width = intersection.size.w as usize;
        for row in 0..intersection.size.h as usize {
            let source_x = (intersection.loc.x - output.geometry.loc.x) as usize;
            let source_y = (intersection.loc.y - output.geometry.loc.y) as usize + row;
            let destination_x = (intersection.loc.x - region.loc.x) as usize;
            let destination_y = (intersection.loc.y - region.loc.y) as usize + row;
            let source_start = (source_y * output.geometry.size.w as usize + source_x) * 4;
            let destination_start = (destination_y * width + destination_x) * 4;
            let byte_count = copy_width * 4;
            composite[destination_start..destination_start + byte_count]
                .copy_from_slice(&output.pixels[source_start..source_start + byte_count]);
        }
    }
    Ok(composite)
}

fn expand_directory(raw: &str) -> PathBuf {
    for prefix in ["$env.HOME/", "$HOME/", "~/"] {
        if let Some(rest) = raw.strip_prefix(prefix)
            && let Some(home) = std::env::var_os("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}

fn create_unique_file(directory: &Path) -> io::Result<(PathBuf, File)> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    for suffix in 0..1000u16 {
        let path = directory.join(format!(
            "halley-screenshot-{}-{:03}-{suffix}.png",
            now.as_secs(),
            now.subsec_millis()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique screenshot filename",
    ))
}

fn write_png(file: File, width: u32, height: u32, pixels: &[u8]) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(geometry: Rectangle<i32, Logical>, color: [u8; 4]) -> OutputImage {
        let pixel_count = geometry.size.w as usize * geometry.size.h as usize;
        OutputImage {
            geometry,
            pixels: color.repeat(pixel_count),
        }
    }

    #[test]
    fn desktop_composite_keeps_output_positions_and_transparent_gaps() {
        let outputs = [
            image(Rectangle::new((0, 0).into(), (2, 2).into()), [1, 2, 3, 255]),
            image(Rectangle::new((3, 0).into(), (1, 2).into()), [4, 5, 6, 255]),
        ];
        let pixels =
            composite_region(Rectangle::new((0, 0).into(), (4, 2).into()), &outputs).unwrap();

        assert_eq!(&pixels[0..4], &[1, 2, 3, 255]);
        assert_eq!(&pixels[8..12], &[0, 0, 0, 0]);
        assert_eq!(&pixels[12..16], &[4, 5, 6, 255]);
    }

    #[test]
    fn crop_can_cross_output_boundaries() {
        let outputs = [
            image(Rectangle::new((0, 0).into(), (2, 1).into()), [1, 0, 0, 255]),
            image(Rectangle::new((2, 0).into(), (2, 1).into()), [2, 0, 0, 255]),
        ];
        let pixels =
            composite_region(Rectangle::new((1, 0).into(), (2, 1).into()), &outputs).unwrap();
        assert_eq!(pixels, vec![1, 0, 0, 255, 2, 0, 0, 255]);
    }

    #[test]
    fn area_crop_discards_the_void_below_a_shorter_output() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = Rectangle::new((2560, 0).into(), (1920, 1440).into());

        assert_eq!(
            visible_region_bounds(selected, outputs),
            Some(Rectangle::new((2560, 0).into(), (1920, 1200).into()))
        );
    }

    #[test]
    fn area_crop_keeps_a_full_main_output_touching_the_secondary_boundary() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = outputs[0];

        assert_eq!(visible_region_bounds(selected, outputs), Some(selected));
    }

    #[test]
    fn area_crop_preserves_internal_voids_for_a_cross_output_selection() {
        let outputs = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ];
        let selected = Rectangle::new((2000, 0).into(), (1000, 1440).into());

        assert_eq!(visible_region_bounds(selected, outputs), Some(selected));
    }

    #[test]
    fn area_crop_rejects_a_selection_entirely_in_a_layout_void() {
        let outputs = [Rectangle::new((0, 0).into(), (2560, 1440).into())];
        let selected = Rectangle::new((3000, 0).into(), (200, 200).into());

        assert_eq!(visible_region_bounds(selected, outputs), None);
    }

    #[test]
    fn home_expansion_accepts_config_and_shell_spellings() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        for raw in ["$env.HOME/Pictures", "$HOME/Pictures", "~/Pictures"] {
            assert_eq!(expand_directory(raw), PathBuf::from(&home).join("Pictures"));
        }
    }
}
