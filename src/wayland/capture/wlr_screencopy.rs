//! `wlr-screencopy-unstable-v1` capture protocol.
//!
//! Protocol validation and damage-wait queues live here. Scene composition
//! remains in `capture`, while fence waiting remains a session-driver job.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Physical, Rectangle, Size};
use smithay::wayland::{dmabuf, shm};
use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};

use crate::session::{Session, SessionDriver};

const VERSION: u32 = 3;

pub struct State {
    _global: GlobalId,
    queues: HashMap<ZwlrScreencopyManagerV1, Queue>,
}

#[derive(Default)]
struct Queue {
    pending_frames: HashSet<ZwlrScreencopyFrameV1>,
    copies: Vec<Screencopy>,
}

impl Queue {
    fn is_empty(&self) -> bool {
        self.pending_frames.is_empty() && self.copies.is_empty()
    }
}

impl State {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrScreencopyManagerV1, ()> + 'static,
    {
        Self {
            _global: display.create_global::<D, ZwlrScreencopyManagerV1, _>(VERSION, ()),
            queues: HashMap::new(),
        }
    }

    fn insert_manager(&mut self, manager: ZwlrScreencopyManagerV1) {
        self.queues.insert(manager, Queue::default());
    }

    fn queue(&mut self, manager: &ZwlrScreencopyManagerV1, copy: Screencopy) {
        if let Some(queue) = self.queues.get_mut(manager) {
            queue.copies.push(copy);
        }
    }

    pub fn take_for_output(&mut self, output: &Output) -> Vec<Screencopy> {
        let mut ready = Vec::new();
        for queue in self.queues.values_mut() {
            let mut index = 0;
            while index < queue.copies.len() {
                if queue.copies[index].output() == output {
                    ready.push(queue.copies.remove(index));
                } else {
                    index += 1;
                }
            }
        }
        self.cleanup();
        ready
    }

    pub fn output_disabled(&mut self, output: &Output) {
        for queue in self.queues.values_mut() {
            queue.copies.retain(|copy| copy.output() != output);
        }
        self.cleanup();
    }

    fn remove_frame(&mut self, frame: &ZwlrScreencopyFrameV1) {
        for queue in self.queues.values_mut() {
            queue.pending_frames.remove(frame);
            queue.copies.retain(|copy| copy.frame != *frame);
        }
        self.cleanup();
    }

    fn cleanup(&mut self) {
        self.queues
            .retain(|manager, queue| manager.is_alive() || !queue.is_empty());
    }
}

#[derive(Clone)]
pub struct FrameInfo {
    output: Output,
    region: Rectangle<i32, Physical>,
    overlay_cursor: bool,
}

pub enum FrameData {
    Failed,
    Pending {
        manager: ZwlrScreencopyManagerV1,
        info: FrameInfo,
        copied: Arc<AtomicBool>,
    },
}

#[derive(Clone)]
enum TargetBuffer {
    Dmabuf(Dmabuf),
    Shm(WlBuffer),
}

pub struct Screencopy {
    info: FrameInfo,
    frame: ZwlrScreencopyFrameV1,
    buffer: TargetBuffer,
    with_damage: bool,
    submitted: bool,
}

impl Screencopy {
    fn output(&self) -> &Output {
        &self.info.output
    }

    fn send_full_damage(&self) {
        let Size { w, h, .. } = self.info.region.size;
        self.frame.damage(0, 0, w as u32, h as u32);
    }

    fn submit(mut self, timestamp: Duration) {
        self.frame.flags(Flags::empty());
        self.frame.ready(
            (timestamp.as_secs() >> 32) as u32,
            timestamp.as_secs() as u32,
            timestamp.subsec_nanos(),
        );
        self.submitted = true;
    }
}

impl Drop for Screencopy {
    fn drop(&mut self) {
        if !self.submitted {
            self.frame.failed();
        }
    }
}

impl<D: SessionDriver> GlobalDispatch<ZwlrScreencopyManagerV1, (), Session<D>> for Session<D> {
    fn can_view(client: Client, _: &()) -> bool {
        super::super::permissions::allowed(&client, super::super::permissions::Capability::Capture)
    }

    fn bind(
        session: &mut Session<D>,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        let manager = data_init.init(resource, ());
        session.wayland.wlr_screencopy_state.insert_manager(manager);
    }
}

impl<D: SessionDriver> Dispatch<ZwlrScreencopyManagerV1, (), Session<D>> for Session<D> {
    fn request(
        session: &mut Session<D>,
        _client: &Client,
        manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        let (frame, overlay_cursor, output, requested_region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, overlay_cursor != 0, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (
                frame,
                overlay_cursor != 0,
                output,
                Some(Rectangle::new((x, y).into(), (width, height).into())),
            ),
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => unreachable!(),
        };

        let Some(output) = Output::from_resource(&output) else {
            fail_new_frame(frame, data_init);
            return;
        };
        let Some(output_geometry) = session.wayland.space.output_geometry(&output) else {
            fail_new_frame(frame, data_init);
            return;
        };
        let output_size = output_geometry
            .size
            .to_physical(output.current_scale().integer_scale());
        let Some(region) = capture_region(
            requested_region,
            output_size,
            output.current_scale().fractional_scale(),
        ) else {
            fail_new_frame(frame, data_init);
            return;
        };

        let info = FrameInfo {
            output,
            region,
            overlay_cursor,
        };
        let frame = data_init.init(
            frame,
            FrameData::Pending {
                manager: manager.clone(),
                info,
                copied: Arc::new(AtomicBool::new(false)),
            },
        );
        frame.buffer(
            Format::Xrgb8888,
            region.size.w as u32,
            region.size.h as u32,
            region.size.w as u32 * 4,
        );
        if frame.version() >= 3 {
            frame.linux_dmabuf(
                Fourcc::Xrgb8888 as u32,
                region.size.w as u32,
                region.size.h as u32,
            );
            frame.buffer_done();
        }
        if let Some(queue) = session.wayland.wlr_screencopy_state.queues.get_mut(manager) {
            queue.pending_frames.insert(frame);
        }
    }

    fn destroyed(
        session: &mut Session<D>,
        _client: ClientId,
        _manager: &ZwlrScreencopyManagerV1,
        _data: &(),
    ) {
        session.wayland.wlr_screencopy_state.cleanup();
    }
}

impl<D: SessionDriver> Dispatch<ZwlrScreencopyFrameV1, FrameData, Session<D>> for Session<D> {
    fn request(
        session: &mut Session<D>,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Session<D>>,
    ) {
        if matches!(request, zwlr_screencopy_frame_v1::Request::Destroy) {
            return;
        }
        let FrameData::Pending {
            manager,
            info,
            copied,
        } = data
        else {
            return;
        };
        if copied.swap(true, Ordering::SeqCst) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "copy was already requested",
            );
            return;
        }

        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            _ => unreachable!(),
        };
        let Some(buffer) = validate_buffer(&buffer, info.region.size) else {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "buffer does not match the advertised capture format",
            );
            return;
        };
        let output = info.output.clone();
        let copy = Screencopy {
            info: info.clone(),
            frame: frame.clone(),
            buffer,
            with_damage,
            submitted: false,
        };
        if let Some(queue) = session.wayland.wlr_screencopy_state.queues.get_mut(manager) {
            queue.pending_frames.remove(frame);
        }

        if with_damage {
            session.wayland.wlr_screencopy_state.queue(manager, copy);
            session.request_output_redraw(&output);
        } else {
            session.fulfill_screencopy(copy);
        }
    }

    fn destroyed(
        session: &mut Session<D>,
        _client: ClientId,
        frame: &ZwlrScreencopyFrameV1,
        _data: &FrameData,
    ) {
        session.wayland.wlr_screencopy_state.remove_frame(frame);
    }
}

impl<D: SessionDriver> Session<D> {
    pub fn service_screencopy(&mut self, output: &Output) {
        let copies = self.wayland.wlr_screencopy_state.take_for_output(output);
        for copy in copies {
            self.fulfill_screencopy(copy);
        }
    }

    fn fulfill_screencopy(&mut self, mut copy: Screencopy) {
        if self.session_lock.active() {
            return;
        }
        let result = match &mut copy.buffer {
            TargetBuffer::Shm(buffer) => {
                let pixels = crate::capture::capture_monitor_region_pixels(
                    self,
                    &copy.info.output,
                    copy.info.region,
                    copy.info.overlay_cursor,
                );
                pixels
                    .and_then(|pixels| {
                        write_shm_buffer(buffer, &pixels)
                            .map_err(|err| -> Box<dyn std::error::Error> { err.into() })
                    })
                    .map(|_| None)
            }
            TargetBuffer::Dmabuf(dmabuf) => crate::capture::render_monitor_region_dmabuf(
                self,
                &copy.info.output,
                copy.info.region,
                copy.info.overlay_cursor,
                dmabuf,
            )
            .map(Some),
        };
        let Ok(sync) = result else {
            return;
        };
        if copy.with_damage {
            copy.send_full_damage();
        }
        let timestamp = crate::frame_clock::monotonic_now();
        if let Some(sync) = sync {
            let _ = self
                .driver
                .schedule_render_completion(sync, Box::new(move || copy.submit(timestamp)));
        } else {
            copy.submit(timestamp);
        }
    }
}

fn fail_new_frame<D>(frame: New<ZwlrScreencopyFrameV1>, data_init: &mut DataInit<'_, D>)
where
    D: Dispatch<ZwlrScreencopyFrameV1, FrameData> + 'static,
{
    data_init.init(frame, FrameData::Failed).failed();
}

fn capture_region(
    requested: Option<Rectangle<i32, smithay::utils::Logical>>,
    output_size: Size<i32, Physical>,
    scale: f64,
) -> Option<Rectangle<i32, Physical>> {
    let output = Rectangle::from_size(output_size);
    match requested {
        None => Some(output),
        Some(requested) if requested.size.w > 0 && requested.size.h > 0 => requested
            .to_physical_precise_round(scale)
            .intersection(output),
        Some(_) => None,
    }
}

fn validate_buffer(buffer: &WlBuffer, size: Size<i32, Physical>) -> Option<TargetBuffer> {
    if let Ok(dmabuf) = dmabuf::get_dmabuf(buffer) {
        return (dmabuf.format().code == Fourcc::Xrgb8888
            && dmabuf.width() == size.w as u32
            && dmabuf.height() == size.h as u32)
            .then(|| TargetBuffer::Dmabuf(dmabuf.clone()));
    }
    shm::with_buffer_contents(buffer, |_, pool_len, data| {
        shm_layout_is_valid(data, pool_len, size)
    })
    .ok()
    .filter(|valid| *valid)
    .map(|_| TargetBuffer::Shm(buffer.clone()))
}

fn shm_layout_is_valid(data: shm::BufferData, pool_len: usize, size: Size<i32, Physical>) -> bool {
    let Some(byte_len) = (size.w as usize)
        .checked_mul(size.h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return false;
    };
    let Some(end) = (data.offset as usize).checked_add(byte_len) else {
        return false;
    };
    data.offset >= 0
        && data.format == Format::Xrgb8888
        && data.width == size.w
        && data.height == size.h
        && data.stride == size.w * 4
        && end <= pool_len
}

fn write_shm_buffer(buffer: &WlBuffer, pixels: &[u8]) -> Result<(), shm::BufferAccessError> {
    shm::with_buffer_contents_mut(buffer, |ptr, _pool_len, data| {
        // Smithay explicitly exposes the pool as a raw pointer because the
        // client can mutate shared memory concurrently. We only perform a
        // direct byte copy while the mapping callback is active.
        unsafe {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr(),
                ptr.add(data.offset as usize),
                pixels.len(),
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_region_is_scaled_and_clamped() {
        let region = capture_region(
            Some(Rectangle::new((80, 25).into(), (40, 20).into())),
            (200, 100).into(),
            2.0,
        )
        .unwrap();
        assert_eq!(region, Rectangle::new((160, 50).into(), (40, 40).into()));
    }

    #[test]
    fn rejects_non_positive_and_off_output_regions() {
        let size = Size::from((200, 100));
        assert!(
            capture_region(
                Some(Rectangle::new((0, 0).into(), (0, 5).into())),
                size,
                1.0
            )
            .is_none()
        );
        assert!(
            capture_region(
                Some(Rectangle::new((300, 0).into(), (5, 5).into())),
                size,
                1.0
            )
            .is_none()
        );
    }

    #[test]
    fn shm_validation_allows_buffers_inside_larger_pools() {
        let data = shm::BufferData {
            offset: 64,
            width: 10,
            height: 4,
            stride: 40,
            format: Format::Xrgb8888,
        };
        assert!(shm_layout_is_valid(data, 4096, (10, 4).into()));
        assert!(!shm_layout_is_valid(data, 200, (10, 4).into()));
    }
}
