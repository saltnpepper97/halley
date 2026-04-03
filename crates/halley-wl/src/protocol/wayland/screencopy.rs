use std::sync::atomic::{AtomicBool, Ordering};

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, Resource,
            protocol::{wl_output::WlOutput, wl_shm},
        },
    },
    utils::{Logical, Rectangle},
};

use crate::compositor::root::Halley;

use super::portal;

pub(crate) struct ScreencopyFrameData {
    pub(crate) output: Output,
    pub(crate) overlay_cursor: bool,
    pub(crate) logical_region: Option<Rectangle<i32, Logical>>,
    pub(crate) copied: AtomicBool,
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, (), Halley> for Halley {
    fn bind(
        _state: &mut Halley,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Halley>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, (), Halley> for Halley {
    fn request(
        state: &mut Halley,
        _client: &Client,
        manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Halley>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => init_screencopy_frame(
                state,
                manager,
                data_init,
                frame,
                overlay_cursor != 0,
                output,
                None,
            ),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => init_screencopy_frame(
                state,
                manager,
                data_init,
                frame,
                overlay_cursor != 0,
                output,
                Some(Rectangle::new((x, y).into(), (width, height).into())),
            ),
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameData, Halley> for Halley {
    fn request(
        state: &mut Halley,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Halley>,
    ) {
        match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => {
                perform_copy(state, frame, data, &buffer, false);
            }
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => {
                perform_copy(state, frame, data, &buffer, true);
            }
            zwlr_screencopy_frame_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

fn init_screencopy_frame(
    state: &Halley,
    manager: &ZwlrScreencopyManagerV1,
    data_init: &mut DataInit<'_, Halley>,
    frame: smithay::reexports::wayland_server::New<ZwlrScreencopyFrameV1>,
    overlay_cursor: bool,
    output: WlOutput,
    logical_region: Option<Rectangle<i32, Logical>>,
) {
    let Some(output) = Output::from_resource(&output) else {
        manager.post_error(0u32, "capture requested for unmanaged output");
        return;
    };

    let spec = portal::screencopy_spec_for_output(&output, logical_region);
    let resource = data_init.init(
        frame,
        ScreencopyFrameData {
            output,
            overlay_cursor,
            logical_region,
            copied: AtomicBool::new(false),
        },
    );
    if let Some(spec) = spec {
        resource.buffer(
            spec.format.into(),
            spec.width as u32,
            spec.height as u32,
            spec.stride as u32,
        );
        if resource.version() >= 3 {
            resource.buffer_done();
        }
    }
    if resource.version() >= 3 {
        resource.buffer_done();
    }
}

fn perform_copy(
    state: &mut Halley,
    frame: &ZwlrScreencopyFrameV1,
    data: &ScreencopyFrameData,
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    with_damage: bool,
) {
    if data.copied.swap(true, Ordering::SeqCst) {
        frame.post_error(
            zwlr_screencopy_frame_v1::Error::AlreadyUsed,
            "zwlr_screencopy_frame_v1 can only be copied once",
        );
        return;
    }

    match portal::screencopy_buffer_type(buffer) {
        Some(smithay::backend::renderer::BufferType::Dma) => {
            perform_copy_dmabuf(state, frame, data, buffer, with_damage);
        }
        Some(smithay::backend::renderer::BufferType::Shm) => {
            perform_copy_shm(state, frame, data, buffer, with_damage);
        }
        _ => {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "unsupported screencopy buffer type",
            );
        }
    }
}

fn perform_copy_shm(
    state: &mut Halley,
    frame: &ZwlrScreencopyFrameV1,
    data: &ScreencopyFrameData,
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    with_damage: bool,
) {
    let capture = match portal::capture_output_shm(
        state,
        &data.output,
        data.overlay_cursor,
        data.logical_region,
    ) {
        Ok(capture) => capture,
        Err(_) => {
            frame.failed();
            return;
        }
    };

    if let Err(err) = portal::write_capture_to_shm_buffer(buffer, &capture) {
        frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, err);
        return;
    }

    finish_frame(
        frame,
        with_damage,
        capture.spec.width as u32,
        capture.spec.height as u32,
        capture.captured_at,
    );
}

fn perform_copy_dmabuf(
    state: &mut Halley,
    frame: &ZwlrScreencopyFrameV1,
    data: &ScreencopyFrameData,
    buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    with_damage: bool,
) {
    let mut dmabuf = match portal::clone_dmabuf_buffer(buffer) {
        Ok(dmabuf) => dmabuf,
        Err(err) => {
            frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, err);
            return;
        }
    };

    let capture = match portal::capture_output_dmabuf(
        state,
        &data.output,
        data.overlay_cursor,
        data.logical_region,
        &mut dmabuf,
    ) {
        Ok(capture) => capture,
        Err(err) => {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                err.to_string(),
            );
            return;
        }
    };

    let mode = match data.output.current_mode() {
        Some(mode) => mode,
        None => {
            frame.failed();
            return;
        }
    };
    finish_frame(
        frame,
        with_damage,
        mode.size.w as u32,
        mode.size.h as u32,
        capture.captured_at,
    );
}

fn finish_frame(
    frame: &ZwlrScreencopyFrameV1,
    with_damage: bool,
    width: u32,
    height: u32,
    captured_at: std::time::SystemTime,
) {
    if with_damage {
        frame.damage(0, 0, width, height);
    }
    frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
    let stamp = portal::ready_timestamp(captured_at);
    frame.ready(stamp.tv_sec_hi, stamp.tv_sec_lo, stamp.tv_nsec);
}
