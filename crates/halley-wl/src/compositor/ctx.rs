#![allow(dead_code)]

use crate::compositor::root::Halley;

pub(crate) struct FocusCtx<'a> {
    pub(crate) st: &'a mut Halley,
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

pub(crate) struct MonitorCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct ClusterCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct CarryCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct InteractionCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) struct WorkspaceCtx<'a> {
    pub(crate) st: &'a mut Halley,
}

pub(crate) fn focus_ctx(st: &mut Halley) -> FocusCtx<'_> {
    FocusCtx { st }
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

pub(crate) fn monitor_ctx(st: &mut Halley) -> MonitorCtx<'_> {
    MonitorCtx { st }
}

pub(crate) fn cluster_ctx(st: &mut Halley) -> ClusterCtx<'_> {
    ClusterCtx { st }
}

pub(crate) fn carry_ctx(st: &mut Halley) -> CarryCtx<'_> {
    CarryCtx { st }
}

pub(crate) fn interaction_ctx(st: &mut Halley) -> InteractionCtx<'_> {
    InteractionCtx { st }
}

pub(crate) fn workspace_ctx(st: &mut Halley) -> WorkspaceCtx<'_> {
    WorkspaceCtx { st }
}
