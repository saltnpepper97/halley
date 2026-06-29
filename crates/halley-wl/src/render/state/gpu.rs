use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};

use super::RenderState;
use crate::render::blur::BlurTextures;

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
    pub(crate) blur_down_program: Option<GlesTexProgram>,
    pub(crate) blur_up_program: Option<GlesTexProgram>,
    pub(crate) blur_composite_program: Option<GlesTexProgram>,
    pub(crate) blur_composite_masked_program: Option<GlesTexProgram>,
    pub(crate) blur_programs_failed: bool,
    pub(crate) blur_textures: Option<BlurTextures>,
    pub(crate) layer_mask_texture: Option<GlesTexture>,
    pub(crate) background_unit_texture: Option<GlesTexture>,
    pub(crate) background_shader_program: Option<GlesTexProgram>,
    pub(crate) background_shader_key: Option<String>,
    pub(crate) background_shader_failed_key: Option<String>,
    pub(crate) background_image_texture: Option<GlesTexture>,
    pub(crate) background_image_size: (i32, i32),
    pub(crate) background_image_key: Option<String>,
    pub(crate) background_image_failed_key: Option<String>,
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
