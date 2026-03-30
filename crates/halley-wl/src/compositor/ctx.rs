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

impl Halley {
    pub(crate) fn focus_ctx(&mut self) -> FocusCtx<'_> {
        FocusCtx { st: self }
    }

    pub(crate) fn spawn_ctx(&mut self) -> SpawnCtx<'_> {
        SpawnCtx { st: self }
    }

    pub(crate) fn surface_lifecycle_ctx(&mut self) -> SurfaceLifecycleCtx<'_> {
        SurfaceLifecycleCtx { st: self }
    }

    pub(crate) fn layer_shell_ctx(&mut self) -> LayerShellCtx<'_> {
        LayerShellCtx { st: self }
    }

    pub(crate) fn pointer_ctx(&mut self) -> PointerCtx<'_> {
        PointerCtx { st: self }
    }

    pub(crate) fn fullscreen_ctx(&mut self) -> FullscreenCtx<'_> {
        FullscreenCtx { st: self }
    }

    pub(crate) fn monitor_ctx(&mut self) -> MonitorCtx<'_> {
        MonitorCtx { st: self }
    }

    pub(crate) fn cluster_ctx(&mut self) -> ClusterCtx<'_> {
        ClusterCtx { st: self }
    }

    pub(crate) fn carry_ctx(&mut self) -> CarryCtx<'_> {
        CarryCtx { st: self }
    }

    pub(crate) fn interaction_ctx(&mut self) -> InteractionCtx<'_> {
        InteractionCtx { st: self }
    }

    pub(crate) fn workspace_ctx(&mut self) -> WorkspaceCtx<'_> {
        WorkspaceCtx { st: self }
    }
}
