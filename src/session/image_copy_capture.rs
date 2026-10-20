use super::{Session, SessionDriver};
use crate::wayland::ext_image_copy_capture::{
    CursorData, Descriptor, FrameData, GlobalData, Handler, SessionData, State,
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
    ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
    ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
};
use smithay::{
    output::Output,
    utils::Rectangle,
    wayland::image_capture_source::{
        ImageCaptureSource, ImageCaptureSourceHandler, OutputCaptureSourceHandler,
        OutputCaptureSourceState,
    },
};

impl<D: SessionDriver> Session<D> {
    pub(crate) fn sync_image_capture(&mut self) {
        let outputs = if self.session_lock.active() {
            Vec::new()
        } else {
            self.driver
                .output_states()
                .into_iter()
                .filter(|state| {
                    state.enabled
                        && self.driver.capture_output_available(&state.output)
                        && self.wayland.space.output_geometry(&state.output).is_some()
                })
                .filter_map(|state| {
                    let size = crate::render::output_physical_size(&state.output);
                    (size.w > 0 && size.h > 0).then(|| {
                        let descriptor = Descriptor {
                            size,
                            scale: state.output.current_scale().fractional_scale(),
                            transform: state.output.current_transform(),
                        };
                        (state.output, descriptor)
                    })
                })
                .collect()
        };
        self.wayland.ext_image_copy_capture_state.sync(outputs);
    }

    pub(crate) fn service_image_capture(&mut self, output: &Output) {
        if !self
            .wayland
            .ext_image_copy_capture_state
            .has_pending_frames()
        {
            return;
        }
        self.sync_image_capture();
        for frame in self
            .wayland
            .ext_image_copy_capture_state
            .take_frames(output)
        {
            let region = Rectangle::from_size(frame.descriptor.size);
            match crate::capture::capture_monitor_region_pixels(
                self,
                output,
                region,
                frame.paint_cursor,
            ) {
                Ok(pixels) => frame.complete(&pixels, crate::frame_clock::monotonic_now()),
                Err(error) => {
                    eventline::debug!("image copy capture failed: {error}");
                    frame.fail();
                }
            }
        }
    }
}
impl<D: SessionDriver> Handler for Session<D> {
    fn image_capture_state(&mut self) -> &mut State {
        &mut self.wayland.ext_image_copy_capture_state
    }
    fn refresh_image_capture(&mut self) {
        self.sync_image_capture();
    }
    fn request_image_capture(&mut self, output: &Output) {
        self.request_output_redraw(output);
    }
}
impl<D: SessionDriver> ImageCaptureSourceHandler for Session<D> {
    fn source_destroyed(&mut self, source: ImageCaptureSource) {
        self.wayland
            .ext_image_copy_capture_state
            .stop_source(&source);
    }
}
impl<D: SessionDriver> OutputCaptureSourceHandler for Session<D> {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.wayland.ext_image_copy_capture_state.output_sources
    }
    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}
smithay::reexports::wayland_server::delegate_global_dispatch!(@<D: SessionDriver> Session<D>: [ExtImageCopyCaptureManagerV1: GlobalData] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ExtImageCopyCaptureManagerV1: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ExtImageCopyCaptureSessionV1: std::sync::Arc<SessionData>] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ExtImageCopyCaptureFrameV1: std::sync::Arc<FrameData>] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ExtImageCopyCaptureCursorSessionV1: CursorData] => State);
