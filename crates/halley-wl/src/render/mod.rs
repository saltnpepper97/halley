use glam::{Vec2, Vec4};
use halley_config::RuntimeTuning;
use halley_core::field::Field;
use halley_core::viewport::{FocusRing, FocusZone, Viewport};

pub(crate) mod app_icon;
mod bearings;
mod clipped_surface;
mod cursor;
mod cursor_theme;
mod frame;
pub mod layer_shell;
mod node;
mod offscreen;
pub(crate) mod utils;
mod window;

pub(crate) fn active_window_frame_pad_px(tuning: &RuntimeTuning) -> i32 {
    tuning.border_size_px.max(0)
}

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
pub(crate) use frame::{draw_debug_frame, draw_debug_frame_to_target};
pub(crate) use utils::preview_proxy_size;
pub(crate) use utils::{node_marker_metrics, world_to_screen};

#[derive(Clone, Copy, Debug)]
pub struct DebugPalette {
    pub clear: Vec4,
    pub focus_ring: Vec4,
    pub node_active: Vec4,
    pub node_preview: Vec4,
    pub node_cold: Vec4,
}

impl Default for DebugPalette {
    fn default() -> Self {
        Self {
            clear: Vec4::new(0.04, 0.05, 0.06, 1.0),
            focus_ring: Vec4::new(0.15, 0.85, 0.85, 0.75),
            node_active: Vec4::new(0.25, 0.95, 0.35, 1.0),
            node_preview: Vec4::new(0.95, 0.85, 0.25, 0.95),
            node_cold: Vec4::new(0.65, 0.70, 0.78, 0.9),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RingDebugGeom {
    pub center: Vec2,
    pub radius_x: f32,
    pub radius_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ZoneCounts {
    pub inside: usize,
    pub outside: usize,
}

#[derive(Clone, Debug)]
pub struct DebugScene {
    pub viewport_center: Vec2,
    pub viewport_size: Vec2,
    pub focus_ring: RingDebugGeom,
    pub node_count_visible: usize,
    pub zones: ZoneCounts,
}

pub fn build_debug_scene(field: &Field, vp: &Viewport, focus_ring: FocusRing) -> DebugScene {
    let mut zones = ZoneCounts::default();
    let mut visible = 0usize;

    for (&id, node) in field.nodes() {
        if !field.is_visible(id) {
            continue;
        }
        visible += 1;
        match focus_ring.zone(vp.center, node.pos) {
            FocusZone::Inside => zones.inside += 1,
            FocusZone::Outside => zones.outside += 1,
        }
    }

    DebugScene {
        viewport_center: Vec2::new(vp.center.x, vp.center.y),
        viewport_size: Vec2::new(vp.size.x, vp.size.y),
        focus_ring: RingDebugGeom {
            center: Vec2::new(
                vp.center.x + focus_ring.offset_x,
                vp.center.y + focus_ring.offset_y,
            ),
            radius_x: focus_ring.radius_x,
            radius_y: focus_ring.radius_y,
            offset_x: focus_ring.offset_x,
            offset_y: focus_ring.offset_y,
        },
        node_count_visible: visible,
        zones,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::{Field, Vec2};
    use halley_core::viewport::FocusRing;

    #[test]
    fn scene_counts_visible_nodes_by_focus_zone() {
        let mut f = Field::new();
        let _a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let _b = f.spawn_surface("B", Vec2 { x: 500.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let c = f.spawn_surface("C", Vec2 { x: 2000.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let _ = f.set_hidden(c, true);

        let vp = Viewport::new(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1920.0,
                y: 1080.0,
            },
        );
        let focus_ring = FocusRing::new(200.0, 120.0, 0.0, 0.0);

        let s = build_debug_scene(&f, &vp, focus_ring);
        assert_eq!(s.node_count_visible, 2);
        assert_eq!(s.zones.inside, 1);
        assert_eq!(s.zones.outside, 1);
    }
}
