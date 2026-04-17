pub(crate) mod app_icon;
mod bearings;
pub(crate) mod clipped_surface;
mod cluster_icon;
mod cursor;
mod cursor_theme;
pub(crate) mod draw_primitives;
mod frame;
mod icon_tint;
pub mod layer_shell;
mod node;
mod screenshot_icon;
pub(crate) mod state;
pub(crate) mod surface_capture;

pub(crate) fn log_rounded_shader_failure(
    shader_name: &str,
    role: &str,
    err: &dyn std::fmt::Display,
) {
    eventline::error!(
        "rounded rendering disabled: role={} shader={} renderer_info=unavailable error={}",
        role,
        shader_name,
        err
    );
    eventline::warn!(
        "rounded shader fallback active: role={} shader={} strict_env=HALLEY_WL_DEBUG_STRICT_ROUNDED",
        role,
        shader_name
    );
}

pub(crate) use bearings::bearing_hit_test;
pub(crate) use cluster_icon::cluster_core_icon_texture;
pub(crate) use cursor::cursor_surface_hotspot;
pub(crate) use cursor_theme::themed_cursor_sprite_with_fallback;
pub(crate) use frame::{
    draw_debug_frame, draw_debug_frame_to_target, draw_offscreen_textures, draw_window_borders,
    ensure_window_texture_program,
};
pub(crate) use node::{
    ensure_node_circle_resources, node_app_icon_fallback_glyph, node_app_icon_texture_allowed,
};
pub(crate) use screenshot_icon::{
    screenshot_menu_background_color, screenshot_menu_highlight_color,
    screenshot_menu_icon_texture, screenshot_menu_inactive_highlight_color,
    screenshot_menu_item_fill_color,
};

#[derive(Debug)]
pub struct GpuBootstrap {
    pub instance: wgpu::Instance,
}

impl GpuBootstrap {
    pub fn new() -> Self {
        Self {
            instance: wgpu::Instance::default(),
        }
    }
}

impl Default for GpuBootstrap {
    fn default() -> Self {
        Self::new()
    }
}
