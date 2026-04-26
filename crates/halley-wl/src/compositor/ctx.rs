use crate::compositor::root::Halley;
use smithay::reexports::wayland_server::DisplayHandle;

pub(crate) struct FocusCtx<'a> {
    pub(crate) display_handle: &'a DisplayHandle,
}

pub(crate) struct SpawnCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct SurfaceLifecycleCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct LayerShellCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct PointerCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct FullscreenCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) fn focus_ctx(st: &Halley) -> FocusCtx<'_> {
    FocusCtx {
        display_handle: &st.platform.display_handle,
    }
}

pub(crate) fn spawn_ctx(st: &mut Halley) -> SpawnCtx<'_> {
    SpawnCtx { st }
}

pub(crate) fn surface_lifecycle_ctx(st: &mut Halley) -> SurfaceLifecycleCtx<'_> {
    SurfaceLifecycleCtx { st }
}

pub(crate) fn layer_shell_ctx(st: &mut Halley) -> LayerShellCtx<'_> {
    LayerShellCtx { st }
}

pub(crate) fn pointer_ctx(st: &mut Halley) -> PointerCtx<'_> {
    PointerCtx { st }
}

pub(crate) fn fullscreen_ctx(st: &mut Halley) -> FullscreenCtx<'_> {
    FullscreenCtx { st }
}
