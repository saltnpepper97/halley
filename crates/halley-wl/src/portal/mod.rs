use std::io;
use std::ptr;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen, TextureMapping,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    output::Output,
    reexports::wayland_server::protocol::{wl_buffer, wl_shm},
    utils::{Buffer, Logical, Physical, Rectangle, Size, Transform},
    wayland::shm::{BufferAccessError, BufferData, with_buffer_contents_mut},
};

use crate::{
    compositor::{interaction::ResizeCtx, root::Halley},
    render::draw_debug_frame_to_target,
};

#[derive(Default)]
pub(crate) struct PortalState {
    pub(crate) capture_backend: Option<Rc<dyn OutputCaptureBackend>>,
}

pub(crate) trait OutputCaptureBackend {
    fn capture_output_shm(
        &self,
        st: &mut Halley,
        output_name: &str,
        overlay_cursor: bool,
        logical_region: Option<Rectangle<i32, Logical>>,
    ) -> Result<ShmCaptureFrame, Box<dyn std::error::Error>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreencopyBufferSpec {
    pub(crate) format: wl_shm::Format,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) stride: i32,
    pub(crate) logical_region: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug)]
pub(crate) struct ShmCaptureFrame {
    pub(crate) spec: ScreencopyBufferSpec,
    pub(crate) bytes: Vec<u8>,
    pub(crate) y_invert: bool,
    pub(crate) captured_at: SystemTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReadyTimestamp {
    pub(crate) tv_sec_hi: u32,
    pub(crate) tv_sec_lo: u32,
    pub(crate) tv_nsec: u32,
}

pub(crate) fn configure_output_capture_backend(
    st: &mut Halley,
    backend: Rc<dyn OutputCaptureBackend>,
) {
    st.portal.capture_backend = Some(backend);
}

pub(crate) fn screencopy_spec_for_output(
    output: &Output,
    logical_region: Option<Rectangle<i32, Logical>>,
) -> Option<ScreencopyBufferSpec> {
    let mode = output.current_mode()?;
    let full = Rectangle::<i32, Logical>::from_size((mode.size.w, mode.size.h).into());
    let logical_region = logical_region
        .map(|region| clip_capture_region(full, region))
        .unwrap_or(Some(full))?;
    let format = wl_shm::Format::Xrgb8888;
    Some(ScreencopyBufferSpec {
        format,
        width: logical_region.size.w.max(0),
        height: logical_region.size.h.max(0),
        stride: logical_region.size.w.max(0).saturating_mul(4),
        logical_region,
    })
}

pub(crate) fn capture_output_shm(
    st: &mut Halley,
    output: &Output,
    overlay_cursor: bool,
    logical_region: Option<Rectangle<i32, Logical>>,
) -> Result<ShmCaptureFrame, Box<dyn std::error::Error>> {
    let output_name = output.name();
    let backend = st
        .portal
        .capture_backend
        .clone()
        .ok_or_else(|| io::Error::other("no capture backend configured"))?;
    backend.capture_output_shm(st, output_name.as_str(), overlay_cursor, logical_region)
}

pub(crate) fn write_capture_to_shm_buffer(
    buffer: &wl_buffer::WlBuffer,
    frame: &ShmCaptureFrame,
) -> Result<(), String> {
    with_buffer_contents_mut(buffer, |ptr, len, metadata| {
        validate_shm_buffer(metadata, frame.spec, len)?;
        let expected_len = frame.spec.stride.saturating_mul(frame.spec.height) as usize;
        if frame.bytes.len() < expected_len {
            return Err("capture buffer shorter than advertised metadata".to_string());
        }
        // The readback data is already in the advertised format; keep row order unchanged and
        // report y_invert through protocol flags when needed.
        unsafe {
            ptr::copy_nonoverlapping(frame.bytes.as_ptr(), ptr, expected_len);
        }
        Ok(())
    })
    .map_err(buffer_access_error)
    .and_then(|result| result)
}

pub(crate) fn ready_timestamp(time: SystemTime) -> ReadyTimestamp {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    ReadyTimestamp {
        tv_sec_hi: (secs >> 32) as u32,
        tv_sec_lo: secs as u32,
        tv_nsec: duration.subsec_nanos(),
    }
}

pub(crate) fn capture_output_via_renderer(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    output_name: &str,
    output_size: Size<i32, Physical>,
    frame_transform: Transform,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    overlay_cursor: bool,
    logical_region: Option<Rectangle<i32, Logical>>,
) -> Result<ShmCaptureFrame, Box<dyn std::error::Error>> {
    let spec = screencopy_spec_for_output_name(st, output_name, logical_region)
        .ok_or_else(|| io::Error::other(format!("output {output_name} has no active mode")))?;
    let capture_region = Rectangle::<i32, Buffer>::new(
        (spec.logical_region.loc.x, spec.logical_region.loc.y).into(),
        (spec.width, spec.height).into(),
    );

    let previous_monitor = st.begin_temporary_render_monitor(output_name);
    let previous_layer_configure = st.input.interaction_state.suppress_layer_shell_configure;
    let result = (|| {
        let mut texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            renderer,
            Fourcc::Xrgb8888,
            (output_size.w, output_size.h).into(),
        )?;

        let cursor_status =
            overlay_cursor.then(|| crate::compositor::platform::effective_cursor_image_status(st));
        let local_cursor = overlay_cursor
            .then(|| {
                cursor_screen.and_then(|(sx, sy)| {
                    let target_monitor = st.monitor_for_screen(sx, sy)?;
                    if target_monitor != output_name {
                        return None;
                    }
                    let (_, _, local_sx, local_sy) =
                        st.local_screen_in_monitor(output_name, sx, sy);
                    Some((local_sx, local_sy))
                })
            })
            .flatten();

        st.input.interaction_state.suppress_layer_shell_configure = previous_monitor.is_some();
        {
            let mut target = renderer.bind(&mut texture)?;
            draw_debug_frame_to_target(
                renderer,
                &mut target,
                output_size,
                st,
                resize_preview,
                hover_node,
                preview_hover_node,
                local_cursor,
                cursor_status.as_ref(),
                frame_transform,
            )?;
        }

        let mapping = renderer.copy_texture(&texture, capture_region, Fourcc::Xrgb8888)?;
        let bytes = renderer.map_texture(&mapping)?.to_vec();
        Ok(ShmCaptureFrame {
            spec,
            bytes,
            y_invert: mapping.flipped(),
            captured_at: SystemTime::now(),
        })
    })();
    st.input.interaction_state.suppress_layer_shell_configure = previous_layer_configure;
    st.end_temporary_render_monitor(previous_monitor);
    result
}

fn screencopy_spec_for_output_name(
    st: &Halley,
    output_name: &str,
    logical_region: Option<Rectangle<i32, Logical>>,
) -> Option<ScreencopyBufferSpec> {
    let output = st.model.monitor_state.outputs.get(output_name)?;
    let mode = output.current_mode()?;
    let full = Rectangle::<i32, Logical>::from_size((mode.size.w, mode.size.h).into());
    let logical_region = logical_region
        .map(|region| clip_capture_region(full, region))
        .unwrap_or(Some(full))?;
    let format = wl_shm::Format::Xrgb8888;
    Some(ScreencopyBufferSpec {
        format,
        width: logical_region.size.w,
        height: logical_region.size.h,
        stride: logical_region.size.w.saturating_mul(4),
        logical_region,
    })
}

fn clip_capture_region(
    full: Rectangle<i32, Logical>,
    region: Rectangle<i32, Logical>,
) -> Option<Rectangle<i32, Logical>> {
    let x1 = region.loc.x.max(full.loc.x);
    let y1 = region.loc.y.max(full.loc.y);
    let x2 = (region.loc.x + region.size.w).min(full.loc.x + full.size.w);
    let y2 = (region.loc.y + region.size.h).min(full.loc.y + full.size.h);
    let width = x2 - x1;
    let height = y2 - y1;
    (width > 0 && height > 0).then(|| Rectangle::new((x1, y1).into(), (width, height).into()))
}

fn validate_shm_buffer(
    metadata: BufferData,
    spec: ScreencopyBufferSpec,
    len: usize,
) -> Result<(), String> {
    if metadata.width != spec.width
        || metadata.height != spec.height
        || metadata.stride != spec.stride
        || metadata.format != spec.format
    {
        return Err(format!(
            "buffer attributes mismatch: got {:?} {}x{} stride {}, expected {:?} {}x{} stride {}",
            metadata.format,
            metadata.width,
            metadata.height,
            metadata.stride,
            spec.format,
            spec.width,
            spec.height,
            spec.stride,
        ));
    }

    let expected_len = spec.stride.saturating_mul(spec.height).max(0) as usize;
    if len < expected_len {
        return Err(format!(
            "buffer too small: got {len} bytes, expected at least {expected_len}"
        ));
    }
    Ok(())
}

fn buffer_access_error(err: BufferAccessError) -> String {
    match err {
        BufferAccessError::NotManaged => "buffer is not a managed wl_shm buffer".to_string(),
        BufferAccessError::BadMap => "failed to map wl_shm buffer".to_string(),
        BufferAccessError::NotReadable => "wl_shm buffer is not readable".to_string(),
        BufferAccessError::NotWritable => "wl_shm buffer is not writable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_capture_region_clamps_to_output_bounds() {
        let full = Rectangle::<i32, Logical>::new((0, 0).into(), (1920, 1080).into());
        let clipped =
            clip_capture_region(full, Rectangle::new((-20, 10).into(), (200, 120).into()))
                .expect("clipped region");

        assert_eq!(clipped.loc.x, 0);
        assert_eq!(clipped.loc.y, 10);
        assert_eq!(clipped.size.w, 180);
        assert_eq!(clipped.size.h, 120);
    }

    #[test]
    fn ready_timestamp_splits_unix_time() {
        let stamp = ready_timestamp(UNIX_EPOCH + std::time::Duration::new((1u64 << 33) + 7, 9));

        assert_eq!(stamp.tv_sec_hi, 2);
        assert_eq!(stamp.tv_sec_lo, 7);
        assert_eq!(stamp.tv_nsec, 9);
    }

    #[test]
    fn validate_shm_buffer_rejects_format_mismatch() {
        let spec = ScreencopyBufferSpec {
            format: wl_shm::Format::Xrgb8888,
            width: 10,
            height: 10,
            stride: 40,
            logical_region: Rectangle::new((0, 0).into(), (10, 10).into()),
        };

        let err = validate_shm_buffer(
            BufferData {
                offset: 0,
                width: 10,
                height: 10,
                stride: 40,
                format: wl_shm::Format::Argb8888,
            },
            spec,
            400,
        )
        .expect_err("buffer should be rejected");

        assert!(err.contains("mismatch"));
    }
}
