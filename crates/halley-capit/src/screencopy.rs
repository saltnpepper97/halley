use std::fs::File;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage, imageops};
use memmap2::MmapMut;
use smithay_client_toolkit::{
    delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
};
use tempfile::tempfile;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_output, wl_shm, wl_shm_pool},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use crate::capture::temp_output_path;

#[derive(Clone, Debug)]
pub struct CaptureOutputInfo {
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: i32,
}

pub fn capture_desktop_to_temp_file(final_out_path: &Path) -> Result<PathBuf, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init(&conn).map_err(|e| format!("registry init: {e}"))?;
    let qh = queue.handle();
    let registry_state = RegistryState::new(&globals);
    let output_state = OutputState::new(&globals, &qh);
    let mut app = CaptureApp {
        registry_state,
        output_state,
        shm: globals.bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ()).ok(),
        screencopy: globals
            .bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(&qh, 1..=3, ())
            .ok(),
        active: None,
    };
    queue
        .roundtrip(&mut app)
        .map_err(|e| format!("roundtrip 1: {e}"))?;
    queue
        .roundtrip(&mut app)
        .map_err(|e| format!("roundtrip 2: {e}"))?;
    if app.shm.is_none() {
        return Err("wl_shm not available".into());
    }
    if app.screencopy.is_none() {
        return Err("zwlr_screencopy_manager_v1 not available".into());
    }
    let outputs = app.capture_outputs();
    if outputs.is_empty() {
        return Err("no outputs available for capture".into());
    }
    let mut captures = Vec::new();
    for (wl_output, info) in outputs {
        captures.push(capture_single_output(
            &conn, &mut queue, &qh, &mut app, &wl_output, info,
        )?);
    }
    let desktop = stitch_logical_desktop(&captures)?;
    let tmp_out = temp_output_path(final_out_path);
    desktop
        .save(&tmp_out)
        .map_err(|e| format!("save screenshot: {e}"))?;
    Ok(tmp_out)
}

struct CaptureApp {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    active: Option<ActiveCapture>,
}

struct ActiveCapture {
    frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    output: CaptureOutputInfo,
    buffer_spec: Option<BufferSpec>,
    buffer_done: bool,
    copied: bool,
    failed: bool,
    ready: bool,
    y_invert: bool,
    shm_buffer: Option<CaptureShmBuffer>,
}

#[derive(Clone, Copy)]
struct BufferSpec {
    format: wl_shm::Format,
    width: i32,
    height: i32,
    stride: i32,
}

struct CaptureShmBuffer {
    _file: File,
    mmap: MmapMut,
    _pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
}

struct CapturedOutput {
    info: CaptureOutputInfo,
    width: i32,
    height: i32,
    stride: i32,
    format: wl_shm::Format,
    y_invert: bool,
    bytes: Vec<u8>,
}

impl CaptureApp {
    fn capture_outputs(&self) -> Vec<(wl_output::WlOutput, CaptureOutputInfo)> {
        let mut out = self
            .output_state
            .outputs()
            .into_iter()
            .filter_map(|output| {
                let info = self.output_state.info(&output)?;
                Some((
                    output,
                    CaptureOutputInfo {
                        name: info.name.clone(),
                        x: info.logical_position.map(|(x, _)| x).unwrap_or(0),
                        y: info.logical_position.map(|(_, y)| y).unwrap_or(0),
                        width: info.logical_size.map(|(w, _)| w as i32).unwrap_or(0),
                        height: info.logical_size.map(|(_, h)| h as i32).unwrap_or(0),
                        scale: info.scale_factor.max(1),
                    },
                ))
            })
            .collect::<Vec<_>>();
        out.sort_by_key(|(_, info)| (info.y, info.x));
        out
    }

    fn maybe_issue_copy(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        if active.copied || active.failed || active.ready {
            return Ok(());
        }
        let Some(spec) = active.buffer_spec else {
            return Ok(());
        };
        if active.frame.version() >= 3 && !active.buffer_done {
            return Ok(());
        }
        let shm = self
            .shm
            .as_ref()
            .ok_or("wl_shm unavailable during capture")?;
        let buffer = CaptureShmBuffer::new(shm, qh, spec)?;
        active.frame.copy(&buffer.buffer);
        active.shm_buffer = Some(buffer);
        active.copied = true;
        Ok(())
    }
}

impl ActiveCapture {
    fn into_captured(self) -> Result<CapturedOutput, String> {
        let spec = self.buffer_spec.ok_or_else(|| {
            "screencopy frame never reported wl_shm buffer parameters".to_string()
        })?;
        let buffer = self
            .shm_buffer
            .ok_or_else(|| "screencopy frame never copied into a wl_shm buffer".to_string())?;
        Ok(CapturedOutput {
            info: self.output,
            width: spec.width,
            height: spec.height,
            stride: spec.stride,
            format: spec.format,
            y_invert: self.y_invert,
            bytes: buffer.mmap[..].to_vec(),
        })
    }
}

impl CaptureShmBuffer {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<CaptureApp>,
        spec: BufferSpec,
    ) -> Result<Self, String> {
        let width = spec.width.max(1);
        let height = spec.height.max(1);
        let stride = spec.stride.max(width.saturating_mul(4));
        let size = stride.saturating_mul(height) as u64;
        let file = tempfile().map_err(|e| format!("tempfile: {e}"))?;
        file.set_len(size).map_err(|e| format!("set_len: {e}"))?;
        let mmap = unsafe { MmapMut::map_mut(&file).map_err(|e| format!("mmap: {e}"))? };
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, spec.format, qh, ());
        Ok(Self {
            _file: file,
            mmap,
            _pool: pool,
            buffer,
        })
    }
}

fn capture_single_output(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<CaptureApp>,
    qh: &QueueHandle<CaptureApp>,
    app: &mut CaptureApp,
    wl_output: &wl_output::WlOutput,
    info: CaptureOutputInfo,
) -> Result<CapturedOutput, String> {
    let manager = app
        .screencopy
        .as_ref()
        .ok_or_else(|| "zwlr_screencopy_manager_v1 unavailable".to_string())?
        .clone();
    let frame = manager.capture_output(0, wl_output, qh, ());
    app.active = Some(ActiveCapture {
        frame,
        output: info,
        buffer_spec: None,
        buffer_done: false,
        copied: false,
        failed: false,
        ready: false,
        y_invert: false,
        shm_buffer: None,
    });
    let _ = conn.flush();
    loop {
        app.maybe_issue_copy(qh)?;
        queue
            .blocking_dispatch(app)
            .map_err(|e| format!("dispatch capture frame: {e}"))?;
        if app
            .active
            .as_ref()
            .is_some_and(|active| active.failed || active.ready)
        {
            break;
        }
    }
    let active = app
        .active
        .take()
        .ok_or("capture state missing after dispatch")?;
    if active.failed {
        return Err(format!(
            "screencopy failed for output {:?}",
            active.output.name
        ));
    }
    active.into_captured()
}

fn stitch_logical_desktop(captures: &[CapturedOutput]) -> Result<RgbaImage, String> {
    let min_x = captures.iter().map(|cap| cap.info.x).min().unwrap_or(0);
    let min_y = captures.iter().map(|cap| cap.info.y).min().unwrap_or(0);
    let max_x = captures
        .iter()
        .map(|cap| cap.info.x + cap.info.width)
        .max()
        .unwrap_or(0);
    let max_y = captures
        .iter()
        .map(|cap| cap.info.y + cap.info.height)
        .max()
        .unwrap_or(0);
    let desktop_w = (max_x - min_x).max(1) as u32;
    let desktop_h = (max_y - min_y).max(1) as u32;
    let mut desktop = RgbaImage::from_pixel(desktop_w, desktop_h, Rgba([0, 0, 0, 0]));
    for cap in captures {
        let image = captured_output_to_image(cap)?;
        imageops::overlay(
            &mut desktop,
            &image,
            (cap.info.x - min_x) as i64,
            (cap.info.y - min_y) as i64,
        );
    }
    Ok(desktop)
}

fn captured_output_to_image(cap: &CapturedOutput) -> Result<RgbaImage, String> {
    let mut output = RgbaImage::new(cap.width.max(1) as u32, cap.height.max(1) as u32);
    for y in 0..cap.height.max(0) {
        let src_y = if cap.y_invert { cap.height - 1 - y } else { y };
        for x in 0..cap.width.max(0) {
            let off = (src_y * cap.stride + x * 4) as usize;
            let b = cap.bytes.get(off).copied().unwrap_or(0);
            let g = cap.bytes.get(off + 1).copied().unwrap_or(0);
            let r = cap.bytes.get(off + 2).copied().unwrap_or(0);
            let a = match cap.format {
                wl_shm::Format::Argb8888 => cap.bytes.get(off + 3).copied().unwrap_or(255),
                wl_shm::Format::Xrgb8888 => 255,
                other => return Err(format!("unsupported screencopy wl_shm format {:?}", other)),
            };
            output.put_pixel(x as u32, y as u32, Rgba([r, g, b, a]));
        }
    }
    Ok(output)
}

impl ProvidesRegistryState for CaptureApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}
impl OutputHandler for CaptureApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}
delegate_registry!(CaptureApp);
delegate_output!(CaptureApp);

impl Dispatch<wl_shm::WlShm, ()> for CaptureApp {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureApp {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureApp {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CaptureApp {
    fn event(
        _: &mut Self,
        _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for CaptureApp {
    fn event(
        state: &mut Self,
        proxy: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(active) = state.active.as_mut() else {
            return;
        };
        if active.frame.id() != proxy.id() {
            return;
        }
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let WEnum::Value(format) = format else {
                    active.failed = true;
                    return;
                };
                active.buffer_spec = Some(BufferSpec {
                    format,
                    width: width as i32,
                    height: height as i32,
                    stride: stride as i32,
                });
                let _ = state.maybe_issue_copy(qh);
            }
            zwlr_screencopy_frame_v1::Event::LinuxDmabuf { .. } => {}
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                active.buffer_done = true;
                let _ = state.maybe_issue_copy(qh);
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                active.y_invert = matches!(flags, WEnum::Value(value) if value.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                active.ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                active.failed = true;
            }
            _ => {}
        }
    }
}
