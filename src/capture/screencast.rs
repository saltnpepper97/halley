use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use smithay::backend::allocator::{
    Fourcc, Modifier,
    dmabuf::{Dmabuf, DmabufFlags},
};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use crate::cursor::RenderCursor;
use crate::session::{Session, SessionDriver};

#[derive(Default)]
pub struct ScreencastState {
    buffers: HashMap<(u64, String, u64), Dmabuf>,
    window_casts: HashMap<u64, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
}

impl ScreencastState {
    pub fn register(
        &mut self,
        client_id: u64,
        request: halley_ipc::RegisterDmabufRequest,
        fds: Vec<OwnedFd>,
    ) -> Result<(), String> {
        let key = (client_id, request.stream_handle.clone(), request.buffer_id);
        if request.stream_handle.len() > 256
            || request.planes.len() > 4
            || fds.len() > 4
            || request.width > 16384
            || request.height > 16384
        {
            return Err("DMA-BUF registration exceeds limits".into());
        }
        if !self.buffers.contains_key(&key)
            && (self.buffers.len() >= 128
                || self.buffers.keys().filter(|key| key.0 == client_id).count() >= 16)
        {
            return Err("DMA-BUF registration quota exceeded".into());
        }
        if request.width <= 0 || request.height <= 0 || request.planes.is_empty() {
            return Err("invalid DMA-BUF dimensions or planes".to_string());
        }
        let format = Fourcc::try_from(request.format)
            .map_err(|_| format!("unsupported DMA-BUF format 0x{:08x}", request.format))?;
        let mut descriptors = fds.into_iter().map(Some).collect::<Vec<_>>();
        let mut builder = Dmabuf::builder(
            (request.width, request.height),
            format,
            Modifier::from(request.modifier),
            DmabufFlags::from_bits_retain(request.flags),
        );
        let mut planes = request.planes;
        planes.sort_by_key(|plane| plane.plane_index);
        for (index, plane) in planes.into_iter().enumerate() {
            if plane.plane_index as usize != index {
                return Err("DMA-BUF plane indices must be contiguous and unique".into());
            }
            let fd = descriptors
                .get_mut(plane.fd_index as usize)
                .and_then(Option::take)
                .ok_or_else(|| format!("missing DMA-BUF descriptor {}", plane.fd_index))?;
            if !builder.add_plane(fd, plane.offset, plane.stride) {
                return Err("too many DMA-BUF planes".to_string());
            }
        }
        let dmabuf = builder
            .build()
            .ok_or_else(|| "could not build DMA-BUF".to_string())?;
        self.buffers.insert(key, dmabuf);
        Ok(())
    }

    pub fn disconnect(&mut self, client_id: u64) -> bool {
        self.buffers.retain(|key, _| key.0 != client_id);
        self.window_casts.remove(&client_id).is_some()
    }

    pub fn begin_window_cast(
        &mut self,
        client_id: u64,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        self.window_casts.insert(client_id, surface);
    }

    // Wayland object equality and hashing use stable object identity, not liveness.
    #[allow(clippy::mutable_key_type)]
    pub fn window_cast_targets(
        &self,
    ) -> std::collections::HashSet<
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    > {
        self.window_casts.values().cloned().collect()
    }

    pub fn is_window_cast(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> bool {
        self.window_casts.values().any(|target| target == surface)
    }

    pub fn remove(&mut self, client_id: u64, stream_handle: &str, buffer_id: u64) {
        self.buffers
            .remove(&(client_id, stream_handle.to_string(), buffer_id));
    }
}

/// Result of filling one PipeWire buffer.
///
/// CPU-backed buffers are complete when this function returns. DMA-BUFs are
/// only publishable after their renderer fence signals; keeping that
/// distinction explicit prevents the portal from ever queueing a stale GPU
/// buffer again.
pub enum CaptureFrameResult {
    Immediate(halley_ipc::CaptureFrameResponse),
    Submitted {
        response: halley_ipc::CaptureFrameResponse,
        sync: SyncPoint,
    },
}

pub fn capture_frame<D: SessionDriver>(
    session: &mut Session<D>,
    client_id: u64,
    request: halley_ipc::CaptureFrameRequest,
    fds: Vec<OwnedFd>,
) -> Result<CaptureFrameResult, String> {
    if session.session_lock.active() {
        return Err("session is locked".to_string());
    }
    let blocked_window = match &request.source {
        halley_ipc::CaptureSource::Window { window_id, .. } => {
            crate::capture::resolve_source_window(
                &session.wayland.space,
                &session.nodes,
                *window_id,
            )
            .ok()
            .and_then(|window| {
                window.wl_surface().map(|surface| {
                    session
                        .window_rules
                        .applied(surface.as_ref())
                        .block_screencast
                })
            })
            .unwrap_or(false)
        }
        _ => false,
    };
    let embedded = !blocked_window
        && request.cursor_mode == halley_ipc::CursorMode::Embedded
        && crate::session::cursor_visible(session);
    let width = request.source.width();
    let height = request.source.height();
    let expected = frame_len(width, height)?;

    let sync = match request.buffer {
        halley_ipc::CaptureBuffer::MemFd {
            fd_index,
            offset,
            size,
            stride,
        } => {
            if stride != width as u32 * 4 {
                return Err(format!(
                    "unsupported MemFd stride {stride}, expected {}",
                    width * 4
                ));
            }
            if size < expected as u64 {
                return Err(format!(
                    "MemFd buffer has {size} bytes, expected at least {expected}"
                ));
            }
            let fd = fds
                .get(fd_index as usize)
                .ok_or_else(|| format!("missing MemFd descriptor {fd_index}"))?;
            let mut pixels =
                crate::capture::capture_source_pixels(session, &request.source, embedded)
                    .map_err(|err| err.to_string())?;
            if pixels.len() != expected {
                return Err(format!(
                    "captured frame has {} bytes, expected {expected}",
                    pixels.len()
                ));
            }
            if embedded
                && matches!(request.source, halley_ipc::CaptureSource::Window { .. })
                && let Some(cursor) = cursor_metadata(session, &request.source)
            {
                blend_cursor_rgba(&mut pixels, width, height, &cursor);
            }
            rgba_to_bgrx(&mut pixels);
            write_all_at(fd, offset, &pixels).map_err(|err| err.to_string())?;
            None
        }
        halley_ipc::CaptureBuffer::Dmabuf { buffer_id } => {
            if !fds.is_empty() {
                return Err("DMA-BUF frame request included descriptors".to_string());
            }
            let key = (client_id, request.stream_handle.clone(), buffer_id);
            let mut dmabuf = session
                .capture
                .screencast
                .buffers
                .remove(&key)
                .ok_or_else(|| format!("unknown DMA-BUF {buffer_id}"))?;
            let result = crate::capture::render_source_dmabuf(
                session,
                &request.source,
                embedded,
                &mut dmabuf,
            )
            .map_err(|err| err.to_string());
            session.capture.screencast.buffers.insert(key, dmabuf);
            Some(result?)
        }
    };

    let cursor = (!blocked_window && request.cursor_mode == halley_ipc::CursorMode::Metadata)
        .then(|| cursor_metadata(session, &request.source))
        .flatten();
    let response = halley_ipc::CaptureFrameResponse { cursor };
    Ok(match sync {
        Some(sync) => CaptureFrameResult::Submitted { response, sync },
        None => CaptureFrameResult::Immediate(response),
    })
}

fn rgba_to_bgrx(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
}

fn frame_len(width: i32, height: i32) -> Result<usize, String> {
    if width <= 0 || height <= 0 {
        return Err(format!("invalid capture dimensions {width}x{height}"));
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "capture dimensions overflow".to_string())
}

fn write_all_at(fd: &OwnedFd, offset: u64, bytes: &[u8]) -> io::Result<()> {
    let mut written = 0usize;
    while written < bytes.len() {
        let position = offset
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("MemFd offset overflow"))?;
        let count = unsafe {
            libc::pwrite(
                fd.as_raw_fd(),
                bytes[written..].as_ptr().cast(),
                bytes.len() - written,
                position
                    .try_into()
                    .map_err(|_| io::Error::other("MemFd offset is too large"))?,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "could not write screencast frame",
            ));
        }
        written += count as usize;
    }
    Ok(())
}

fn cursor_metadata<D: SessionDriver>(
    session: &mut Session<D>,
    source: &halley_ipc::CaptureSource,
) -> Option<halley_ipc::CursorMetadata> {
    if !crate::session::cursor_visible(session) {
        return None;
    }
    let (x, y) = cursor_position(session, source)?;
    let scale = match source {
        halley_ipc::CaptureSource::Monitor { name, .. } => session
            .wayland
            .space
            .outputs()
            .find(|output| output.name() == *name)?
            .current_scale()
            .fractional_scale(),
        halley_ipc::CaptureSource::Window { window_id, .. } => {
            let window = crate::capture::resolve_source_window(
                &session.wayland.space,
                &session.nodes,
                *window_id,
            )
            .ok()?;
            crate::capture::window_capture_context(session).display_scale(&window)
        }
    };
    let presentation_override = crate::session::cursor_override(session);
    match session.cursor.render_cursor_with_override(
        scale.ceil() as i32,
        crate::frame_clock::monotonic_now(),
        presentation_override,
    ) {
        RenderCursor::Hidden => None,
        RenderCursor::Named(frame) => {
            let factor = scale / f64::from(frame.scale.max(1));
            let width = (f64::from(frame.width) * factor).round().max(1.0) as u32;
            let height = (f64::from(frame.height) * factor).round().max(1.0) as u32;
            let image = image::RgbaImage::from_raw(
                frame.width,
                frame.height,
                frame.metadata_bgra.to_vec(),
            )?;
            let bgra = image::imageops::resize(
                &image,
                width,
                height,
                image::imageops::FilterType::Triangle,
            )
            .into_raw();
            Some(halley_ipc::CursorMetadata {
                x,
                y,
                hotspot_x: (f64::from(frame.hotspot_x) * factor).round() as i32,
                hotspot_y: (f64::from(frame.hotspot_y) * factor).round() as i32,
                width,
                height,
                bgra,
            })
        }
        RenderCursor::Surface { surface, snapshot } => {
            let bounds = smithay::desktop::utils::bbox_from_surface_tree(
                &surface,
                Point::<i32, Logical>::from((0, 0)),
            );
            let (hotspot_x, hotspot_y, width, height) =
                cursor_surface_layout(crate::cursor::surface::hotspot(&surface), bounds)?;
            let mut pixels = session
                .driver
                .with_renderer(|renderer| {
                    crate::capture::capture_cursor_surface_tree(
                        renderer,
                        &surface,
                        snapshot.as_deref(),
                        bounds,
                        scale,
                    )
                })
                .ok()?;
            rgba_to_bgra(&mut pixels);
            Some(halley_ipc::CursorMetadata {
                x,
                y,
                hotspot_x: (f64::from(hotspot_x) * scale).round() as i32,
                hotspot_y: (f64::from(hotspot_y) * scale).round() as i32,
                width: (f64::from(width) * scale).round().max(1.0) as u32,
                height: (f64::from(height) * scale).round().max(1.0) as u32,
                bgra: pixels,
            })
        }
    }
}

fn cursor_surface_layout(
    hotspot: Point<i32, Logical>,
    bounds: Rectangle<i32, Logical>,
) -> Option<(i32, i32, u32, u32)> {
    Some((
        hotspot.x - bounds.loc.x,
        hotspot.y - bounds.loc.y,
        bounds.size.w.try_into().ok()?,
        bounds.size.h.try_into().ok()?,
    ))
}

fn cursor_position<D: SessionDriver>(
    session: &Session<D>,
    source: &halley_ipc::CaptureSource,
) -> Option<(i32, i32)> {
    match source {
        halley_ipc::CaptureSource::Monitor {
            name,
            x,
            y,
            width,
            height,
        } => {
            let position = session.pointer.position();
            let scale = session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == *name)?
                .current_scale()
                .fractional_scale();
            let local = (
                ((position.0 - f64::from(*x)) * scale).round() as i32,
                ((position.1 - f64::from(*y)) * scale).round() as i32,
            );
            (session
                .wayland
                .space
                .outputs()
                .any(|output| output.name() == *name)
                && local.0 >= 0
                && local.1 >= 0
                && local.0 < *width
                && local.1 < *height)
                .then_some(local)
        }
        halley_ipc::CaptureSource::Window { window_id, .. } => {
            let route = crate::input::pointer::route_to_client(
                crate::input::pointer::PointerRoutingContext {
                    space: &session.wayland.space,
                    cameras: &session.cameras,
                    clusters: &session.clusters,
                    nodes: &session.nodes,
                    window_animations: &session.window_animations,
                    primary: session.driver.primary_output(),
                    fullscreen: &session.fullscreen,
                    maximize: &session.maximize,
                    decorations: &session.settings.decorations,
                    font: &session.settings.font,
                    focused: session.wayland.focused_window.as_ref(),
                    now: crate::frame_clock::monotonic_now(),
                },
                session.pointer.position(),
            )?;
            let window = match route.target {
                crate::input::pointer::PointerTarget::Window(window)
                | crate::input::pointer::PointerTarget::Decoration { window, .. } => window,
                _ => return None,
            };
            let surface = window.wl_surface()?;
            if session
                .nodes
                .id_for_surface(surface.as_ref())
                .is_none_or(|id| id.as_u64() != *window_id)
            {
                return None;
            }
            let location = session.wayland.space.element_location(&window)?;
            let client_offset =
                crate::capture::window_capture_context(session).client_offset(&window);
            let scale = crate::capture::window_capture_context(session).display_scale(&window);
            Some((
                ((route.location.x - f64::from(location.x) + f64::from(client_offset.x)) * scale)
                    .round() as i32,
                ((route.location.y - f64::from(location.y) + f64::from(client_offset.y)) * scale)
                    .round() as i32,
            ))
        }
    }
}

fn blend_cursor_rgba(
    frame: &mut [u8],
    width: i32,
    height: i32,
    cursor: &halley_ipc::CursorMetadata,
) {
    let left = cursor.x - cursor.hotspot_x;
    let top = cursor.y - cursor.hotspot_y;
    for row in 0..cursor.height as i32 {
        let y = top + row;
        if y < 0 || y >= height {
            continue;
        }
        for column in 0..cursor.width as i32 {
            let x = left + column;
            if x < 0 || x >= width {
                continue;
            }
            let source = ((row as u32 * cursor.width + column as u32) * 4) as usize;
            let destination = ((y * width + x) * 4) as usize;
            let alpha = u16::from(cursor.bgra[source + 3]);
            for (frame_channel, cursor_channel) in
                [(0usize, 2usize), (1usize, 1usize), (2usize, 0usize)]
            {
                let foreground = u16::from(cursor.bgra[source + cursor_channel]);
                let background = u16::from(frame[destination + frame_channel]);
                frame[destination + frame_channel] =
                    ((foreground * alpha + background * (255 - alpha)) / 255) as u8;
            }
            frame[destination + 3] = 255;
        }
    }
}

fn rgba_to_bgra(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_frames_are_converted_to_pipewire_bgrx() {
        let mut pixels = vec![10, 20, 30, 40, 50, 60, 70, 80];
        rgba_to_bgrx(&mut pixels);
        assert_eq!(pixels, vec![30, 20, 10, 255, 70, 60, 50, 255]);
    }

    #[test]
    fn cursor_pixels_are_converted_to_bgra_without_losing_alpha() {
        let mut pixels = vec![10, 20, 30, 40, 50, 60, 70, 80];
        rgba_to_bgra(&mut pixels);
        assert_eq!(pixels, vec![30, 20, 10, 40, 70, 60, 50, 80]);
    }

    #[test]
    fn cursor_surface_hotspot_is_relative_to_the_complete_surface_tree() {
        let bounds = Rectangle::new((-3, -4).into(), (20, 24).into());
        assert_eq!(
            cursor_surface_layout((2, 3).into(), bounds),
            Some((5, 7, 20, 24))
        );
    }

    #[test]
    fn cursor_blending_clips_at_stream_edges() {
        let mut frame = vec![0; 2 * 2 * 4];
        let cursor = halley_ipc::CursorMetadata {
            x: 0,
            y: 0,
            hotspot_x: 1,
            hotspot_y: 1,
            width: 2,
            height: 2,
            bgra: [0, 0, 255, 255].repeat(4),
        };
        blend_cursor_rgba(&mut frame, 2, 2, &cursor);
        assert_eq!(&frame[0..4], &[255, 0, 0, 255]);
        assert!(frame[4..].iter().all(|byte| *byte == 0));
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    fn register(state: &mut ScreencastState, client: u64, id: u64) -> Result<(), String> {
        state.register(
            client,
            halley_ipc::RegisterDmabufRequest {
                stream_handle: "stream".into(),
                buffer_id: id,
                width: 1,
                height: 1,
                format: Fourcc::Xrgb8888 as u32,
                modifier: 0,
                flags: 0,
                planes: vec![halley_ipc::DmabufPlane {
                    fd_index: 0,
                    plane_index: 0,
                    offset: 0,
                    stride: 4,
                }],
            },
            vec![std::fs::File::open("/dev/null").unwrap().into()],
        )
    }
    #[test]
    fn buffers_are_owned_quota_limited_and_reclaimed_on_disconnect() {
        let mut state = ScreencastState::default();
        for id in 0..16 {
            register(&mut state, 1, id).unwrap();
        }
        assert!(register(&mut state, 1, 16).is_err());
        register(&mut state, 1, 0).unwrap(); // replacement consumes no extra slot
        register(&mut state, 2, 0).unwrap();
        state.remove(2, "stream", 1); // cannot remove client's 1 buffer
        assert!(state.buffers.contains_key(&(1, "stream".into(), 1)));
        state.disconnect(1);
        assert_eq!(state.buffers.len(), 1);
        assert!(state.buffers.contains_key(&(2, "stream".into(), 0)));
        register(&mut state, 1, 16).unwrap();
    }
    #[test]
    fn total_registration_count_is_bounded_across_clients() {
        let mut state = ScreencastState::default();
        for client in 0..128 {
            register(&mut state, client, 0).unwrap();
        }
        assert!(register(&mut state, 129, 0).is_err());
        state.disconnect(0);
        register(&mut state, 129, 0).unwrap();
    }
}
