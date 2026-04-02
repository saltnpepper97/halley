use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::backend::{allocator::dmabuf::Dmabuf, allocator::Format};

use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::render::draw_debug_frame;
use std::cell::Cell;

pub(crate) trait BackendView {
    fn window_size_i32(&self) -> (i32, i32);
    fn request_redraw(&self);
}

pub(crate) trait RenderBackend: BackendView {
    fn draw_frame(
        &self,
        st: &mut Halley,
        resize_preview: Option<ResizeCtx>,
        hover_node: Option<halley_core::field::NodeId>,
        preview_hover_node: Option<halley_core::field::NodeId>,
    ) -> Result<(), Box<dyn Error>>;
}

pub(crate) trait DmabufImportBackend {
    fn dmabuf_formats(&self) -> Vec<Format>;
    fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<(), Box<dyn Error>>;
}

pub(crate) struct CaptureDmabufResult {
    pub(crate) captured_at: std::time::SystemTime,
}

#[derive(Clone)]
pub(crate) struct TtyBackendHandle {
    size: Rc<Cell<(i32, i32)>>,
}

impl TtyBackendHandle {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            size: Rc::new(Cell::new((width, height))),
        }
    }

    pub(crate) fn set_size(&self, width: i32, height: i32) {
        self.size.set((width, height));
    }
}

impl BackendView for TtyBackendHandle {
    fn window_size_i32(&self) -> (i32, i32) {
        self.size.get()
    }

    fn request_redraw(&self) {}
}

pub(crate) struct TtyDmabufImportBackend {
    inner: Rc<RefCell<GlesRenderer>>,
}

impl TtyDmabufImportBackend {
    pub(crate) fn new(inner: Rc<RefCell<GlesRenderer>>) -> Self {
        Self { inner }
    }
}

impl DmabufImportBackend for TtyDmabufImportBackend {
    fn dmabuf_formats(&self) -> Vec<Format> {
        self.inner
            .borrow()
            .dmabuf_formats()
            .iter()
            .copied()
            .collect()
    }

    fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<(), Box<dyn Error>> {
        let mut renderer = self.inner.borrow_mut();
        renderer.import_dmabuf(dmabuf, None)?;
        Ok(())
    }
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

impl DmabufImportBackend for WinitBackendHandle {
    fn dmabuf_formats(&self) -> Vec<Format> {
        self.inner
            .borrow_mut()
            .renderer()
            .dmabuf_formats()
            .iter()
            .copied()
            .collect()
    }

    fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<(), Box<dyn Error>> {
        self.inner
            .borrow_mut()
            .renderer()
            .import_dmabuf(dmabuf, None)?;
        Ok(())
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
        st: &mut Halley,
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
