use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};

use super::RenderState;

#[derive(Default)]
pub(crate) struct RenderGpuState {
    pub(crate) node_circle_texture: Option<GlesTexture>,
    pub(crate) node_circle_program: Option<GlesTexProgram>,
    pub(crate) node_square_program: Option<GlesTexProgram>,
    pub(crate) node_squircle_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_rounded_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_rounded_program_failed: bool,
    pub(crate) ui_rect_square_program: Option<GlesTexProgram>,
    pub(crate) ui_rect_square_program_failed: bool,
    pub(crate) window_texture_program: Option<GlesTexProgram>,
    pub(crate) window_texture_program_failed: bool,
    pub(crate) window_shadow_program: Option<GlesTexProgram>,
    pub(crate) window_shadow_program_failed: bool,
    pub(crate) surface_clip_program: Option<GlesTexProgram>,
    pub(crate) surface_clip_program_failed: bool,
}

impl RenderGpuState {
    pub(crate) fn ui_rect_program(&self, rounded: bool) -> Option<&GlesTexProgram> {
        if rounded {
            self.ui_rect_rounded_program.as_ref()
        } else {
            self.ui_rect_square_program.as_ref()
        }
    }
}

impl RenderState {
    pub(crate) fn ui_rect_program(&self, rounded: bool) -> Option<&GlesTexProgram> {
        self.gpu.ui_rect_program(rounded)
    }
}
