use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::WinitGraphicsBackend;

use crate::interaction::types::ResizeCtx;
use crate::runtime_render::draw_debug_frame;
use crate::state::HalleyWlState;

pub(crate) trait BackendView {
    fn window_size_i32(&self) -> (i32, i32);
    fn request_redraw(&self);
}

pub(crate) trait RenderBackend: BackendView {
    fn draw_frame(
        &self,
        st: &mut HalleyWlState,
        resize_preview: Option<ResizeCtx>,
        hover_node: Option<halley_core::field::NodeId>,
        preview_hover_node: Option<halley_core::field::NodeId>,
    ) -> Result<(), Box<dyn Error>>;
}

#[derive(Clone)]
pub(crate) struct WinitBackendHandle {
    inner: Rc<RefCell<WinitGraphicsBackend<GlesRenderer>>>,
}

impl WinitBackendHandle {
    pub(crate) fn new(inner: Rc<RefCell<WinitGraphicsBackend<GlesRenderer>>>) -> Self {
        Self { inner }
    }
}

impl BackendView for WinitBackendHandle {
    fn window_size_i32(&self) -> (i32, i32) {
        let size = self.inner.borrow().window_size();
        (size.w, size.h)
    }

    fn request_redraw(&self) {
        self.inner.borrow().window().request_redraw();
    }
}

impl RenderBackend for WinitBackendHandle {
    fn draw_frame(
        &self,
        st: &mut HalleyWlState,
        resize_preview: Option<ResizeCtx>,
        hover_node: Option<halley_core::field::NodeId>,
        preview_hover_node: Option<halley_core::field::NodeId>,
    ) -> Result<(), Box<dyn Error>> {
        let mut backend = self.inner.borrow_mut();
        draw_debug_frame(
            &mut backend,
            st,
            resize_preview,
            hover_node,
            preview_hover_node,
        )
    }
}
