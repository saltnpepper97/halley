use std::collections::{HashMap, HashSet};
use std::time::Instant;

use halley_core::cluster_layout::ClusterCycleDirection;
use halley_core::field::{NodeId, Vec2};
use halley_core::tiling::Rect;
use smithay::{
    backend::renderer::{
        Color32F, Texture,
        element::{
            Kind, render_elements, surface::render_elements_from_surface_tree,
            utils::CropRenderElement,
        },
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{PopupManager, utils::bbox_from_surface_tree},
    reexports::wayland_server::Resource,
    utils::{Logical, Physical, Rectangle, Size},
};

use crate::animation::{active_surface_render_scale, ease_in_out_cubic, ease_out_back};
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::monitor::layer_shell::layer_output_size_for_monitor;
use crate::compositor::root::Halley;
use crate::compositor::spawn::state::is_persistent_rule_top;
use crate::compositor::surface_ops::{
    active_stacking_visible_members_for_monitor, is_active_cluster_workspace_member,
    window_geometry_for_node,
};
use crate::input::active_resize_geometry_screen;

use super::clipped_surface::ClippedSurfaceRenderElement;
use super::offscreen::render_surface_tree_to_texture;
use super::utils::{sync_node_size_from_surface, world_to_screen};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
render_elements! {
    pub(crate) DirectSurfaceElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Clipped=ClippedSurfaceRenderElement,
}
pub(crate) type CroppedClippedSurfaceElement = CropRenderElement<DirectSurfaceElement>;
type CroppedSurfaceElement = CropRenderElement<SurfaceElement>;

const CSD_SOFT_CLIP_MARGIN_PX: f32 = 1.5;

fn should_draw_resize_overlap_overlay(
    resize_rect_px: Option<(i32, i32, i32, i32, NodeId)>,
    node_id: NodeId,
    geometry_rect: (i32, i32, i32, i32),
    resizing_node_has_overlap_policy: bool,
) -> bool {
    let Some((rl, rt, rr, rb, rid)) = resize_rect_px else {
        return false;
    };
    if resizing_node_has_overlap_policy || node_id == rid {
        return false;
    }
    let (gx, gy, gw, gh) = geometry_rect;
    let wl = gx;
    let wt = gy;
    let wr = gx + gw.max(1);
    let wb = gy + gh.max(1);
    wl < rr && rl < wr && wt < rb && rt < wb
}

fn log_window_render_path(
    _st: &Halley,
    _node_id: halley_core::field::NodeId,
    _path: &str,
    _detail: &str,
) {
}

fn rect4_str(x: i32, y: i32, w: i32, h: i32) -> String {
    format!("({},{} {}x{})", x, y, w, h)
}

fn rect4f_str(x: f32, y: f32, w: f32, h: f32) -> String {
    format!("({:.1},{:.1} {:.1}x{:.1})", x, y, w, h)
}

#[derive(Clone)]
pub(crate) struct ActiveBorderRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub inner_offset_x: f32,
    pub inner_offset_y: f32,
    pub inner_w: f32,
    pub inner_h: f32,
    pub alpha: f32,
    pub border_px: f32,
    pub corner_radius: f32,
    pub inner_corner_radius: f32,
    pub border_color: Color32F,
}

#[derive(Clone)]
pub(crate) struct OffscreenNodeTexture {
    pub texture: GlesTexture,
    pub alpha: f32,
    pub corner_radius: f32,
    pub src_x: f64,
    pub src_y: f64,
    pub src_w: f64,
    pub src_h: f64,
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: i32,
    pub dst_h: i32,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: i32,
    pub clip_h: i32,
    /// Geometry rect within the dst rect (in dst-local pixels).
    /// Used by the shader to clip content to the window geometry rather than
    /// the full bbox, so CSD-shadow pixels don't poke past the rounded border.
    pub geo_offset_x: f32,
    pub geo_offset_y: f32,
    pub geo_w: f32,
    pub geo_h: f32,
}

pub(crate) struct StackWindowDrawUnit {
    pub node_id: NodeId,
    pub draw_order: i32,
    pub border_rect: Option<ActiveBorderRect>,
    pub active_elements: Vec<CroppedClippedSurfaceElement>,
    pub offscreen_textures: Vec<OffscreenNodeTexture>,
}

impl StackWindowDrawUnit {
    fn new(node_id: NodeId, draw_order: i32) -> Self {
        Self {
            node_id,
            draw_order,
            border_rect: None,
            active_elements: Vec::new(),
            offscreen_textures: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct StackTransitionPose {
    center: Vec2,
    size: Vec2,
    alpha: f32,
    draw_order: i32,
}

struct StackTransitionExtraInstance {
    node_id: NodeId,
    pose: StackTransitionPose,
}

struct StackTransitionPlan {
    poses: HashMap<NodeId, StackTransitionPose>,
    extra_instances: Vec<StackTransitionExtraInstance>,
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_vec2(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    Vec2 {
        x: lerp_f32(a.x, b.x, t),
        y: lerp_f32(a.y, b.y, t),
    }
}

fn rect_center(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

fn rect_size(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.w.max(1.0),
        y: rect.h.max(1.0),
    }
}

fn stack_draw_order_map(front_to_back: &[NodeId]) -> HashMap<NodeId, i32> {
    let len = front_to_back.len() as i32;
    front_to_back
        .iter()
        .enumerate()
        .map(|(index, &node_id)| (node_id, len - index as i32 - 1))
        .collect()
}

fn build_stack_transition_plan(
    st: &Halley,
    monitor: &str,
    transition: &crate::render::state::StackCycleTransitionSnapshot,
) -> Option<StackTransitionPlan> {
    let custom_source_rects = transition.source_rects.is_some();
    let old_rects = transition
        .source_rects
        .clone()
        .or_else(|| st.stack_layout_rects_for_members(monitor, &transition.old_visible))?;
    let new_rects = st.stack_layout_rects_for_members(monitor, &transition.new_visible)?;
    let t = ease_in_out_cubic(transition.progress);
    let draw_orders = stack_draw_order_map(&transition.new_visible);
    let topmost_order = transition.new_visible.len() as i32 + 1;
    let old_top = transition.old_visible.first().copied();
    let old_bottom = transition.old_visible.last().copied();
    let new_top = transition.new_visible.first().copied();
    let wrapped_same_set = !custom_source_rects
        && transition.old_visible.len() == transition.new_visible.len()
        && transition
            .old_visible
            .iter()
            .all(|id| transition.new_visible.contains(id));
    let wrapped_node = match transition.direction {
        ClusterCycleDirection::Next
            if wrapped_same_set && old_top == transition.new_visible.last().copied() =>
        {
            old_top
        }
        ClusterCycleDirection::Prev if wrapped_same_set && old_bottom == new_top => old_bottom,
        _ => None,
    };

    let mut ids = transition.old_visible.clone();
    for &node_id in &transition.new_visible {
        if !ids.contains(&node_id) {
            ids.push(node_id);
        }
    }

    let mut poses = HashMap::new();
    let mut extra_instances = Vec::new();
    for node_id in ids {
        if wrapped_node == Some(node_id)
            && let (Some(old_rect), Some(new_rect)) = (
                old_rects.get(&node_id).copied(),
                new_rects.get(&node_id).copied(),
            )
        {
            let canonical_pose = match transition.direction {
                ClusterCycleDirection::Next => StackTransitionPose {
                    center: rect_center(new_rect),
                    size: rect_size(new_rect),
                    alpha: t,
                    draw_order: draw_orders.get(&node_id).copied().unwrap_or_default(),
                },
                ClusterCycleDirection::Prev => {
                    let end_center = rect_center(new_rect);
                    let mut start_center = end_center;
                    start_center.x -= new_rect.w * 0.55;
                    StackTransitionPose {
                        center: lerp_vec2(start_center, end_center, t),
                        size: rect_size(new_rect),
                        alpha: t,
                        draw_order: topmost_order,
                    }
                }
            };
            poses.insert(node_id, canonical_pose);

            if matches!(transition.direction, ClusterCycleDirection::Next) {
                let mut end_center = rect_center(old_rect);
                end_center.x -= old_rect.w * 0.55;
                extra_instances.push(StackTransitionExtraInstance {
                    node_id,
                    pose: StackTransitionPose {
                        center: lerp_vec2(rect_center(old_rect), end_center, t),
                        size: rect_size(old_rect),
                        alpha: 1.0 - t,
                        draw_order: topmost_order,
                    },
                });
            }
            continue;
        }

        let pose = match (
            old_rects.get(&node_id).copied(),
            new_rects.get(&node_id).copied(),
        ) {
            (Some(old_rect), Some(new_rect)) => StackTransitionPose {
                center: lerp_vec2(rect_center(old_rect), rect_center(new_rect), t),
                size: lerp_vec2(rect_size(old_rect), rect_size(new_rect), t),
                alpha: 1.0,
                draw_order: draw_orders.get(&node_id).copied().unwrap_or_default(),
            },
            (Some(old_rect), None) => {
                let mut end_center = rect_center(old_rect);
                let draw_order = match transition.direction {
                    ClusterCycleDirection::Next if Some(node_id) == old_top => {
                        end_center.x -= old_rect.w * 0.55;
                        topmost_order
                    }
                    ClusterCycleDirection::Prev if Some(node_id) == old_bottom => {
                        end_center.x += old_rect.w * 0.08;
                        end_center.y += old_rect.h * 0.04;
                        0
                    }
                    _ => 0,
                };
                StackTransitionPose {
                    center: lerp_vec2(rect_center(old_rect), end_center, t),
                    size: rect_size(old_rect),
                    alpha: 1.0 - t,
                    draw_order,
                }
            }
            (None, Some(new_rect)) => {
                let end_center = rect_center(new_rect);
                let mut start_center = end_center;
                let draw_order = match transition.direction {
                    ClusterCycleDirection::Prev if Some(node_id) == new_top => {
                        start_center.x -= new_rect.w * 0.55;
                        topmost_order
                    }
                    _ => draw_orders.get(&node_id).copied().unwrap_or_default(),
                };
                StackTransitionPose {
                    center: lerp_vec2(start_center, end_center, t),
                    size: rect_size(new_rect),
                    alpha: t,
                    draw_order,
                }
            }
            (None, None) => continue,
        };
        poses.insert(node_id, pose);
    }

    Some(StackTransitionPlan {
        poses,
        extra_instances,
    })
}

fn transform_rect_about_center(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    from_center: (f32, f32),
    to_center: (f32, f32),
    scale_x: f32,
    scale_y: f32,
) -> (i32, i32, i32, i32) {
    let rect_center_x = x as f32 + w as f32 * 0.5;
    let rect_center_y = y as f32 + h as f32 * 0.5;
    let new_center_x = to_center.0 + (rect_center_x - from_center.0) * scale_x;
    let new_center_y = to_center.1 + (rect_center_y - from_center.1) * scale_y;
    let new_w = (w as f32 * scale_x).round().max(1.0) as i32;
    let new_h = (h as f32 * scale_y).round().max(1.0) as i32;
    (
        (new_center_x - new_w as f32 * 0.5).round() as i32,
        (new_center_y - new_h as f32 * 0.5).round() as i32,
        new_w,
        new_h,
    )
}

fn clone_stack_window_unit_for_pose(
    st: &Halley,
    size: Size<i32, Physical>,
    unit: &StackWindowDrawUnit,
    from_pose: StackTransitionPose,
    to_pose: StackTransitionPose,
) -> Option<StackWindowDrawUnit> {
    let (from_cx, from_cy) =
        world_to_screen(st, size.w, size.h, from_pose.center.x, from_pose.center.y);
    let (to_cx, to_cy) = world_to_screen(st, size.w, size.h, to_pose.center.x, to_pose.center.y);
    let scale_x = (to_pose.size.x / from_pose.size.x.max(1.0)).max(0.01);
    let scale_y = (to_pose.size.y / from_pose.size.y.max(1.0)).max(0.01);

    let border_rect = unit.border_rect.as_ref().cloned().map(|mut rect| {
        let (x, y, w, h) = transform_rect_about_center(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            (from_cx as f32, from_cy as f32),
            (to_cx as f32, to_cy as f32),
            scale_x,
            scale_y,
        );
        rect.x = x;
        rect.y = y;
        rect.w = w;
        rect.h = h;
        rect.inner_w = w as f32;
        rect.inner_h = h as f32;
        rect.alpha *= to_pose.alpha.clamp(0.0, 1.0);
        rect
    });

    let offscreen_textures = unit
        .offscreen_textures
        .iter()
        .cloned()
        .map(|mut tex| {
            let (dst_x, dst_y, dst_w, dst_h) = transform_rect_about_center(
                tex.dst_x,
                tex.dst_y,
                tex.dst_w,
                tex.dst_h,
                (from_cx as f32, from_cy as f32),
                (to_cx as f32, to_cy as f32),
                scale_x,
                scale_y,
            );
            let (clip_x, clip_y, clip_w, clip_h) = transform_rect_about_center(
                tex.clip_x,
                tex.clip_y,
                tex.clip_w,
                tex.clip_h,
                (from_cx as f32, from_cy as f32),
                (to_cx as f32, to_cy as f32),
                scale_x,
                scale_y,
            );
            tex.dst_x = dst_x;
            tex.dst_y = dst_y;
            tex.dst_w = dst_w;
            tex.dst_h = dst_h;
            tex.clip_x = clip_x;
            tex.clip_y = clip_y;
            tex.clip_w = clip_w;
            tex.clip_h = clip_h;
            tex.geo_offset_x *= scale_x;
            tex.geo_offset_y *= scale_y;
            tex.geo_w *= scale_x;
            tex.geo_h *= scale_y;
            tex.alpha *= to_pose.alpha.clamp(0.0, 1.0);
            tex
        })
        .collect::<Vec<_>>();

    if border_rect.is_none() && offscreen_textures.is_empty() {
        return None;
    }

    Some(StackWindowDrawUnit {
        node_id: unit.node_id,
        draw_order: to_pose.draw_order,
        border_rect,
        active_elements: Vec::new(),
        offscreen_textures,
    })
}

fn rect_from_local_geometry(
    origin_x: i32,
    origin_y: i32,
    scale: f32,
    local_rect: (f32, f32, f32, f32),
) -> (i32, i32, i32, i32) {
    let (local_x, local_y, local_w, local_h) = local_rect;
    (
        origin_x + (local_x * scale).round() as i32,
        origin_y + (local_y * scale).round() as i32,
        (local_w * scale).round().max(1.0) as i32,
        (local_h * scale).round().max(1.0) as i32,
    )
}

fn expanded_csd_clip_rect(
    local_bbox: (f32, f32, f32, f32),
    local_geo: (f32, f32, f32, f32),
    margin: f32,
) -> (f32, f32, f32, f32) {
    let bbox_left = local_bbox.0;
    let bbox_top = local_bbox.1;
    let bbox_right = local_bbox.0 + local_bbox.2.max(1.0);
    let bbox_bottom = local_bbox.1 + local_bbox.3.max(1.0);

    let left = (local_geo.0 - margin).max(bbox_left);
    let top = (local_geo.1 - margin).max(bbox_top);
    let right = (local_geo.0 + local_geo.2 + margin).min(bbox_right);
    let bottom = (local_geo.1 + local_geo.3 + margin).min(bbox_bottom);

    (left, top, (right - left).max(1.0), (bottom - top).max(1.0))
}

fn strict_square_csd_transition_mode(
    no_csd: bool,
    effective_corner_radius_px: i32,
    transition_active: bool,
) -> bool {
    !no_csd && effective_corner_radius_px == 0 && transition_active
}

fn wrap_direct_surface_elements(
    elems: Vec<SurfaceElement>,
    display_clip: Rectangle<i32, Physical>,
    surface_clip_program: Option<&smithay::backend::renderer::gles::GlesTexProgram>,
    geo_rect: Rectangle<i32, Physical>,
    corner_radius: f32,
) -> Vec<CroppedClippedSurfaceElement> {
    elems
        .into_iter()
        .filter_map(|e| {
            let wrapped: DirectSurfaceElement = if let Some(program) = surface_clip_program
                && ClippedSurfaceRenderElement::will_clip(&e, geo_rect, corner_radius)
            {
                ClippedSurfaceRenderElement::new(e, program.clone(), geo_rect, corner_radius).into()
            } else {
                e.into()
            };
            CropRenderElement::from_element(wrapped, 1.0, display_clip)
        })
        .collect()
}

fn offscreen_visual_crop_and_dst(
    bbox_loc_x: i32,
    bbox_loc_y: i32,
    bbox_w: i32,
    bbox_h: i32,
    geo_lx: f32,
    geo_ly: f32,
    geo_w: f32,
    geo_h: f32,
    dst_x: i32,
    dst_y: i32,
    dst_w: i32,
    dst_h: i32,
    scale: f32,
    clip: Rectangle<i32, Physical>,
    preserve_visual_margin: bool,
    lock_dst_to_geometry: bool,
) -> (f64, f64, f64, f64, i32, i32, i32, i32, i32, i32, i32, i32) {
    const VISUAL_MARGIN_CAP: f32 = 4.0;

    let geo_x = geo_lx - bbox_loc_x as f32;
    let geo_y = geo_ly - bbox_loc_y as f32;
    let geo_w_f = geo_w.max(1.0);
    let geo_h_f = geo_h.max(1.0);

    let bbox_right = (bbox_loc_x + bbox_w) as f32;
    let bbox_bottom = (bbox_loc_y + bbox_h) as f32;
    let geo_right_abs = geo_lx + geo_w;
    let geo_bottom_abs = geo_ly + geo_h;

    let (left_extra, top_extra, right_extra, bottom_extra) = if preserve_visual_margin {
        (
            geo_x.clamp(0.0, VISUAL_MARGIN_CAP),
            geo_y.clamp(0.0, VISUAL_MARGIN_CAP),
            (bbox_right - geo_right_abs).clamp(0.0, VISUAL_MARGIN_CAP),
            (bbox_bottom - geo_bottom_abs).clamp(0.0, VISUAL_MARGIN_CAP),
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    let src_x = (geo_x - left_extra).max(0.0) as f64;
    let src_y = (geo_y - top_extra).max(0.0) as f64;
    let src_w = (geo_w_f + left_extra + right_extra) as f64;
    let src_w = src_w.min(bbox_w as f64 - src_x).max(1.0);
    let src_h = (geo_h_f + top_extra + bottom_extra) as f64;
    let src_h = src_h.min(bbox_h as f64 - src_y).max(1.0);

    let dst_expand_l = (left_extra * scale).round() as i32;
    let dst_expand_t = (top_extra * scale).round() as i32;
    let dst_expand_r = (right_extra * scale).round() as i32;
    let dst_expand_b = (bottom_extra * scale).round() as i32;

    let (final_dst_x, final_dst_y, final_dst_w, final_dst_h) = if lock_dst_to_geometry {
        (dst_x, dst_y, dst_w.max(1), dst_h.max(1))
    } else {
        (
            dst_x - dst_expand_l,
            dst_y - dst_expand_t,
            dst_w.max(1) + dst_expand_l + dst_expand_r,
            dst_h.max(1) + dst_expand_t + dst_expand_b,
        )
    };

    (
        src_x,
        src_y,
        src_w,
        src_h,
        final_dst_x,
        final_dst_y,
        final_dst_w,
        final_dst_h,
        clip.loc.x,
        clip.loc.y,
        clip.size.w,
        clip.size.h,
    )
}

fn render_view_for_monitor(st: &Halley, monitor: &str) -> (Vec2, Vec2, Vec2) {
    if st.model.monitor_state.current_monitor == monitor {
        return (
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        );
    }

    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| {
            (
                space.viewport.center,
                space.viewport.size,
                space.zoom_ref_size,
            )
        })
        .unwrap_or((
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        ))
}

fn world_to_screen_for_view(
    view_center: Vec2,
    view_size: Vec2,
    output_w: i32,
    output_h: i32,
    x: f32,
    y: f32,
) -> (i32, i32) {
    let vw = view_size.x.max(1.0);
    let vh = view_size.y.max(1.0);
    let nx = ((x - view_center.x) / vw) + 0.5;
    let ny = ((y - view_center.y) / vh) + 0.5;

    (
        (nx * output_w as f32).round() as i32,
        (ny * output_h as f32).round() as i32,
    )
}

pub(crate) fn capture_closing_window_animation(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
) -> Option<(Option<ActiveBorderRect>, Vec<OffscreenNodeTexture>)> {
    let node = st.model.field.node(node_id)?;
    let cache = st.ui.render_state.window_offscreen_cache.get(&node_id)?;
    let texture = cache.texture.clone()?;
    let ob = cache.bbox?;
    if !cache.has_content {
        return None;
    }

    let output_size = layer_output_size_for_monitor(st, monitor);
    if output_size.w <= 0 || output_size.h <= 0 {
        return None;
    }
    let output_clip = Rectangle::<i32, Physical>::new(
        (0, 0).into(),
        (output_size.w.max(1), output_size.h.max(1)).into(),
    );

    let (view_center, viewport_size, view_size) = render_view_for_monitor(st, monitor);
    let render_scale = (viewport_size.x.max(1.0) / view_size.x.max(1.0)).max(0.01);
    let local_geo = window_geometry_for_node(st, node_id).unwrap_or((
        ob.loc.x as f32,
        ob.loc.y as f32,
        ob.size.w.max(1) as f32,
        ob.size.h.max(1) as f32,
    ));
    let (cx, cy) = world_to_screen_for_view(
        view_center,
        view_size,
        output_size.w,
        output_size.h,
        node.pos.x,
        node.pos.y,
    );
    let gw = (local_geo.2 * render_scale).round().max(1.0) as i32;
    let gh = (local_geo.3 * render_scale).round().max(1.0) as i32;
    let gx = cx - (gw / 2);
    let gy = cy - (gh / 2);
    let fullscreen_on_monitor = st
        .fullscreen_monitor_for_node(node_id)
        .is_some_and(|fullscreen_monitor| fullscreen_monitor == monitor);
    let effective_border_px = if fullscreen_on_monitor {
        0
    } else {
        let base = st.runtime.tuning.border_size_px.max(0) as f32;
        let scaled = (base * render_scale).round();
        if base > 0.0 {
            scaled.max(1.0) as i32
        } else {
            0
        }
    };
    let effective_corner_radius_px = if fullscreen_on_monitor {
        0
    } else {
        let base = st.runtime.tuning.border_radius_px.max(0) as f32;
        let scaled = (base * render_scale).round();
        if base > 0.0 {
            scaled.max(1.0) as i32
        } else {
            0
        }
    };
    let strict_square_csd_transition = strict_square_csd_transition_mode(
        st.runtime.tuning.no_csd,
        effective_corner_radius_px,
        false,
    );
    let preserve_visual_margin = !strict_square_csd_transition
        && !st.runtime.tuning.no_csd
        && effective_corner_radius_px == 0;
    let lock_dst_to_geometry = effective_corner_radius_px > 0;
    let (src_x, src_y, src_w, src_h, dst_x, dst_y, dst_w, dst_h, clip_x, clip_y, clip_w, clip_h) =
        offscreen_visual_crop_and_dst(
            ob.loc.x,
            ob.loc.y,
            ob.size.w.max(1),
            ob.size.h.max(1),
            local_geo.0,
            local_geo.1,
            local_geo.2,
            local_geo.3,
            gx,
            gy,
            gw.max(1),
            gh.max(1),
            render_scale,
            output_clip,
            preserve_visual_margin,
            lock_dst_to_geometry,
        );
    let src_scale_x = if src_w > 0.0 {
        dst_w as f32 / src_w as f32
    } else {
        1.0
    };
    let src_scale_y = if src_h > 0.0 {
        dst_h as f32 / src_h as f32
    } else {
        1.0
    };
    let disable_geo_clip = !strict_square_csd_transition
        && !st.runtime.tuning.no_csd
        && effective_corner_radius_px == 0;
    let geo_local_x = local_geo.0 - ob.loc.x as f32;
    let geo_local_y = local_geo.1 - ob.loc.y as f32;
    let geo_src_x = (geo_local_x - src_x as f32).max(0.0);
    let geo_src_y = (geo_local_y - src_y as f32).max(0.0);
    let geo_offset_x = if disable_geo_clip {
        0.0
    } else {
        (geo_src_x * src_scale_x).max(0.0)
    };
    let geo_offset_y = if disable_geo_clip {
        0.0
    } else {
        (geo_src_y * src_scale_y).max(0.0)
    };
    let geo_w_px = if disable_geo_clip {
        0.0
    } else {
        (local_geo.2 * src_scale_x).min(dst_w as f32).max(1.0)
    };
    let geo_h_px = if disable_geo_clip {
        0.0
    } else {
        (local_geo.3 * src_scale_y).min(dst_h as f32).max(1.0)
    };
    let offscreen = OffscreenNodeTexture {
        texture,
        alpha: 1.0,
        corner_radius: (effective_corner_radius_px - effective_border_px).max(0) as f32,
        src_x,
        src_y,
        src_w,
        src_h,
        dst_x,
        dst_y,
        dst_w,
        dst_h,
        clip_x,
        clip_y,
        clip_w,
        clip_h,
        geo_offset_x,
        geo_offset_y,
        geo_w: geo_w_px,
        geo_h: geo_h_px,
    };
    let border_rect = if effective_border_px > 0 {
        let border_color = if st.model.focus_state.primary_interaction_focus == Some(node_id) {
            let color = st.runtime.tuning.border_color_focused;
            Color32F::new(color.r, color.g, color.b, 1.0)
        } else {
            let color = st.runtime.tuning.border_color_unfocused;
            Color32F::new(color.r, color.g, color.b, 1.0)
        };
        Some(ActiveBorderRect {
            x: gx,
            y: gy,
            w: gw.max(1),
            h: gh.max(1),
            inner_offset_x: effective_border_px as f32,
            inner_offset_y: effective_border_px as f32,
            inner_w: gw.max(1) as f32,
            inner_h: gh.max(1) as f32,
            alpha: 1.0,
            border_px: effective_border_px as f32,
            corner_radius: effective_corner_radius_px as f32,
            inner_corner_radius: (effective_corner_radius_px - effective_border_px).max(0) as f32,
            border_color,
        })
    } else {
        None
    };

    Some((border_rect, vec![offscreen]))
}

#[allow(clippy::type_complexity)]
pub(crate) fn collect_active_surfaces(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    size: Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    now: Instant,
) -> (
    Vec<CroppedClippedSurfaceElement>,
    Vec<CroppedClippedSurfaceElement>,
    Vec<CroppedClippedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    Vec<OffscreenNodeTexture>,
    Vec<CroppedSurfaceElement>,
    HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    Vec<StackWindowDrawUnit>,
    Vec<ActiveBorderRect>,
    Vec<ActiveBorderRect>,
    Vec<(i32, i32, i32, i32)>,
) {
    let mut active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let mut resized_active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let mut fullscreen_active_elements: Vec<CroppedClippedSurfaceElement> = Vec::new();
    let mut offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut resized_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut fullscreen_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture> = Vec::new();
    let mut popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut fullscreen_popup_elements: Vec<CroppedSurfaceElement> = Vec::new();
    let mut node_surface_map = HashMap::new();
    crate::animation::retain_live_cluster_tile_tracks(
        &mut st.ui.render_state.cluster_tile_tracks,
        &st.model.field,
        now,
    );
    if st.runtime.tuning.tile_animation_enabled()
        && crate::animation::cluster_tile_tracks_animating(
            &st.ui.render_state.cluster_tile_tracks,
            now,
        )
    {
        st.request_maintenance();
    }
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let stack_visible_front_to_back =
        active_stacking_visible_members_for_monitor(st, current_monitor.as_str());
    let stack_cycle_transition = st
        .runtime
        .tuning
        .stack_animation_enabled()
        .then(|| {
            st.ui
                .render_state
                .stack_cycle_transition_for_monitor(current_monitor.as_str(), now)
        })
        .flatten();
    if stack_cycle_transition.is_some() {
        st.request_maintenance();
    }
    let stack_transition_plan = stack_cycle_transition.as_ref().and_then(|transition| {
        build_stack_transition_plan(st, current_monitor.as_str(), transition)
    });
    let stack_render_front_to_back = if let Some(transition) = stack_cycle_transition.as_ref() {
        let mut ids = transition.old_visible.clone();
        for &node_id in &transition.new_visible {
            if !ids.contains(&node_id) {
                ids.push(node_id);
            }
        }
        ids
    } else {
        stack_visible_front_to_back.clone()
    };
    let stack_render_set: HashSet<_> = stack_render_front_to_back.iter().copied().collect();
    let stack_draw_orders = if let Some(plan) = stack_transition_plan.as_ref() {
        plan.poses
            .iter()
            .map(|(&node_id, pose)| (node_id, pose.draw_order))
            .collect::<HashMap<_, _>>()
    } else {
        stack_draw_order_map(&stack_visible_front_to_back)
    };
    let mut stack_window_units: HashMap<NodeId, StackWindowDrawUnit> = HashMap::new();
    let mut border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut resized_border_rects: Vec<ActiveBorderRect> = Vec::new();
    let mut overlap_overlay_rects: Vec<(i32, i32, i32, i32)> = Vec::new();

    let recent_top_node = st.recent_top_node_active(now);
    let has_persistent_rule_top = st
        .model
        .spawn_state
        .applied_window_rules
        .keys()
        .any(|id| st.model.field.is_visible(*id));
    let output_clip = Rectangle::<i32, Physical>::new((0, 0).into(), size);
    let resize_rect_px = resize_preview.and_then(|rz| {
        if !st.node_visible_on_current_monitor(rz.node_id) {
            return None;
        }
        Some((
            rz.preview_left_px.min(rz.preview_right_px).round() as i32,
            rz.preview_top_px.min(rz.preview_bottom_px).round() as i32,
            rz.preview_left_px.max(rz.preview_right_px).round() as i32,
            rz.preview_top_px.max(rz.preview_bottom_px).round() as i32,
            rz.node_id,
        ))
    });
    let resize_preview_has_overlap_policy = resize_rect_px
        .map(|(_, _, _, _, rid)| st.node_has_overlap_policy(rid))
        .unwrap_or(false);

    let mut wl_surfaces: Vec<_> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|t| {
            let wl = t.wl_surface().clone();
            let key = wl.id();
            let node_id = st.model.surface_to_node.get(&key).copied()?;
            node_surface_map.insert(node_id, wl.clone());
            Some((node_id, wl))
        })
        .collect();

    wl_surfaces.sort_by_key(|(id, _)| std::cmp::Reverse(id.as_u64()));

    for (node_id, wl) in wl_surfaces {
        let bbox = if resize_preview.is_some_and(|rz| rz.node_id == node_id) {
            bbox_from_surface_tree(&wl, (0, 0))
        } else {
            sync_node_size_from_surface(st, node_id, &wl)
        };

        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        let stack_transition_pose = stack_transition_plan
            .as_ref()
            .and_then(|plan| plan.poses.get(&node_id).copied());
        let stack_member_rendered = stack_render_set.contains(&node_id);
        if node.state != halley_core::field::NodeState::Active
            || (!stack_member_rendered
                && (!st.model.field.is_visible(node_id)
                    || !st.node_visible_on_current_monitor(node_id)))
        {
            continue;
        }

        let node_pos = node.pos;
        let node_state = node.state.clone();
        let node_intrinsic = node.intrinsic_size;
        let fullscreen_on_current_monitor = st
            .fullscreen_monitor_for_node(node_id)
            .is_some_and(|monitor| monitor == st.model.monitor_state.current_monitor);

        let active_cluster_member = is_active_cluster_workspace_member(st, node_id);
        let dragging_this_node = st.input.interaction_state.drag_authority_node == Some(node_id);
        let tiling_tile_transition = (active_cluster_member
            && !dragging_this_node
            && st.runtime.tuning.tile_animation_enabled()
            && matches!(
                st.runtime.tuning.cluster_layout_kind(),
                halley_core::cluster_layout::ClusterWorkspaceLayoutKind::Tiling
            ))
        .then(|| {
            crate::animation::cluster_tile_rect_for(
                &st.ui.render_state.cluster_tile_tracks,
                node_id,
                now,
            )
        })
        .flatten();
        let frozen_tiling_geometry = tiling_tile_transition.and_then(|_| {
            st.ui
                .render_state
                .cluster_tile_frozen_geometry
                .get(&node_id)
                .copied()
        });
        let transition_alpha = st.active_transition_alpha(node_id, now);
        let anim = crate::render::anim_style_for(st, node_id, node_state.clone(), now);
        let fullscreen_entry_scale = st.fullscreen_entry_scale(node_id, st.now_ms(now));
        let active_resize = active_resize_geometry_screen(st, node_id, resize_preview);
        let resizing_this_node = active_resize.is_some();
        let dragging_this_node = st.input.interaction_state.drag_authority_node == Some(node_id);
        let persistent_rule_top = is_persistent_rule_top(st, node_id);
        let draw_top_this_node = resizing_this_node
            || (recent_top_node == Some(node_id)
                && (!has_persistent_rule_top || persistent_rule_top))
            || dragging_this_node
            || persistent_rule_top;

        let force_live_surface_scale =
            resizing_this_node || dragging_this_node || active_cluster_member;
        let (scale, live_ramp) = if force_live_surface_scale {
            (1.0f32 * fullscreen_entry_scale, 1.0f32)
        } else {
            let s = active_surface_render_scale(
                anim.scale,
                st.active_zoom_lock_scale(),
                node_intrinsic.x,
                node_intrinsic.y,
                transition_alpha,
            );
            let live_t = ((anim.scale - 0.44) / (1.0 - 0.44)).clamp(0.0, 1.0);
            let live_ramp = if transition_alpha > 0.0 {
                ease_out_back((1.0 - transition_alpha).clamp(0.0, 1.0), 1.42).clamp(0.0, 1.08)
            } else {
                ease_in_out_cubic(live_t).clamp(0.0, 1.0)
            };
            (s * fullscreen_entry_scale, live_ramp)
        };

        // Fit scale for fullscreen windows that don't match the physical monitor resolution.
        let fit_scale = if fullscreen_on_current_monitor {
            let sw = (output_clip.size.w as f32) / node_intrinsic.x.max(1.0);
            let sh = (output_clip.size.h as f32) / node_intrinsic.y.max(1.0);
            sw.min(sh).max(0.1) // aspect-correct fit
        } else if let Some(monitor) = st.fullscreen_monitor_for_node(node_id) {
            let (target_w, target_h) = st.fullscreen_target_size_for(monitor);
            let sw = (target_w as f32) / node_intrinsic.x.max(1.0);
            let sh = (target_h as f32) / node_intrinsic.y.max(1.0);
            sw.min(sh).max(0.1) // aspect-correct fit
        } else {
            1.0
        };

        let cam_scale = st.camera_render_scale();
        let render_scale = scale * cam_scale * fit_scale;

        let p = stack_transition_pose
            .map(|pose| pose.center)
            .or_else(|| tiling_tile_transition.map(|rect| rect.center))
            .unwrap_or(node_pos);
        let local_bbox = (
            bbox.loc.x as f32,
            bbox.loc.y as f32,
            bbox.size.w.max(1) as f32,
            bbox.size.h.max(1) as f32,
        );

        let local_geo = if stack_member_rendered {
            let base_geo = window_geometry_for_node(st, node_id).unwrap_or(local_bbox);
            let target_size = stack_transition_pose
                .map(|pose| pose.size)
                .or_else(|| st.model.field.node(node_id).map(|node| node.intrinsic_size))
                .unwrap_or(Vec2 {
                    x: base_geo.2,
                    y: base_geo.3,
                });
            (
                base_geo.0,
                base_geo.1,
                target_size.x.max(1.0),
                target_size.y.max(1.0),
            )
        } else {
            frozen_tiling_geometry
                .unwrap_or_else(|| window_geometry_for_node(st, node_id).unwrap_or(local_bbox))
        };

        let (_cx, _cy, sx, sy, texture_rect, geometry_rect) =
            if let Some(active_resize) = active_resize {
                let (cx, cy) = active_resize.center_px();
                let (surface_origin_x, surface_origin_y) = active_resize.surface_origin_px();
                let frame = active_resize.frame_rect_px();
                (cx, cy, surface_origin_x, surface_origin_y, frame, frame)
            } else {
                let (cx, cy) = world_to_screen(st, size.w, size.h, p.x, p.y);

                let (render_geo_w, render_geo_h) = tiling_tile_transition
                    .map(|rect| (rect.size.x, rect.size.y))
                    .unwrap_or((local_geo.2, local_geo.3));
                let rw = (render_geo_w * render_scale).round().max(1.0) as i32;
                let rh = (render_geo_h * render_scale).round().max(1.0) as i32;

                let (rx, ry, rw, rh) = if fullscreen_on_current_monitor {
                    (
                        output_clip.loc.x,
                        output_clip.loc.y,
                        output_clip.size.w,
                        output_clip.size.h,
                    )
                } else {
                    let rx = cx - (rw / 2);
                    let ry = cy - (rh / 2);
                    (rx, ry, rw, rh)
                };

                let sx = rx - (local_geo.0 * render_scale).round() as i32;
                let sy = ry - (local_geo.1 * render_scale).round() as i32;

                let texture_rect = rect_from_local_geometry(sx, sy, render_scale, local_bbox);
                let geometry_rect = (rx, ry, rw, rh);

                (cx, cy, sx, sy, texture_rect, geometry_rect)
            };

        let element_scale = if active_resize.is_some() {
            scale
        } else {
            render_scale
        };

        let (gx, gy, gw, gh) = geometry_rect;

        if should_draw_resize_overlap_overlay(
            resize_rect_px,
            node_id,
            (gx, gy, gw, gh),
            resize_preview_has_overlap_policy,
        ) {
            overlap_overlay_rects.push((gx, gy, gw.max(1), gh.max(1)));
        }

        let alpha = (anim.alpha
            * live_ramp
            * stack_transition_pose.map(|pose| pose.alpha).unwrap_or(1.0)
            * tiling_tile_transition.map(|rect| rect.alpha).unwrap_or(1.0))
        .clamp(0.0, 1.0);
        let effective_border_px = if fullscreen_on_current_monitor {
            0
        } else {
            let base = st.runtime.tuning.border_size_px.max(0) as f32;
            let scaled = (base * render_scale).round();
            if base > 0.0 {
                scaled.max(1.0) as i32
            } else {
                0
            }
        };
        let effective_corner_radius_px = if fullscreen_on_current_monitor {
            0
        } else {
            let base = st.runtime.tuning.border_radius_px.max(0) as f32;
            let scaled = (base * render_scale).round();
            if base > 0.0 {
                scaled.max(1.0) as i32
            } else {
                0
            }
        };
        let transition_surface_active = stack_transition_pose.is_some()
            || tiling_tile_transition.is_some()
            || active_resize.is_some();
        let strict_square_csd_transition = strict_square_csd_transition_mode(
            st.runtime.tuning.no_csd,
            effective_corner_radius_px,
            transition_surface_active,
        );
        let lock_dst_to_geometry = effective_corner_radius_px > 0;
        let csd_soft_clip_margin = if !strict_square_csd_transition
            && !st.runtime.tuning.no_csd
            && effective_corner_radius_px == 0
        {
            CSD_SOFT_CLIP_MARGIN_PX
        } else {
            0.0
        };
        let clip_geo = if csd_soft_clip_margin > 0.0 {
            expanded_csd_clip_rect(local_bbox, local_geo, csd_soft_clip_margin)
        } else {
            local_geo
        };
        let offscreen_clip = if !st.runtime.tuning.no_csd {
            st.ui
                .render_state
                .surface_clip_program
                .as_ref()
                .map(|program| {
                    let geo_rect = Rectangle::<i32, Logical>::new(
                        (clip_geo.0.round() as i32, clip_geo.1.round() as i32).into(),
                        (
                            clip_geo.2.round().max(1.0) as i32,
                            clip_geo.3.round().max(1.0) as i32,
                        )
                            .into(),
                    );
                    (
                        geo_rect,
                        (effective_corner_radius_px - effective_border_px).max(0) as f32,
                        program.clone(),
                    )
                })
        } else {
            None
        };
        let preserve_visual_margin = !strict_square_csd_transition
            && !st.runtime.tuning.no_csd
            && effective_corner_radius_px == 0;
        let border_color = if st.model.focus_state.primary_interaction_focus == Some(node_id) {
            let color = st.runtime.tuning.border_color_focused;
            Color32F::new(color.r, color.g, color.b, 1.0)
        } else {
            let color = st.runtime.tuning.border_color_unfocused;
            Color32F::new(color.r, color.g, color.b, 1.0)
        };
        let border_rect = ActiveBorderRect {
            x: gx,
            y: gy,
            w: gw.max(1),
            h: gh.max(1),
            inner_offset_x: effective_border_px as f32,
            inner_offset_y: effective_border_px as f32,
            inner_w: gw.max(1) as f32,
            inner_h: gh.max(1) as f32,
            alpha,
            border_px: effective_border_px as f32,
            corner_radius: effective_corner_radius_px as f32,
            inner_corner_radius: (effective_corner_radius_px - effective_border_px).max(0) as f32,
            border_color,
        };
        if stack_render_set.contains(&node_id) {
            stack_window_units
                .entry(node_id)
                .or_insert_with(|| {
                    StackWindowDrawUnit::new(
                        node_id,
                        stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                    )
                })
                .border_rect = Some(border_rect);
        } else if draw_top_this_node {
            resized_border_rects.push(border_rect);
        } else {
            border_rects.push(border_rect);
        }
        // Games/fullscreen processes bypass offscreen zoom for performance and compatibility.
        let use_offscreen_zoom = !fullscreen_on_current_monitor;

        if use_offscreen_zoom {
            let defer_offscreen_rebuild =
                tiling_tile_transition.is_some() || stack_transition_pose.is_some();
            let stale_cache_available = st
                .ui
                .render_state
                .window_offscreen_cache
                .get(&node_id)
                .is_some_and(|cache| cache.texture.is_some() && cache.bbox.is_some());
            let cache_miss = if defer_offscreen_rebuild {
                !stale_cache_available
            } else {
                let cache = st.ui.render_state.ensure_window_offscreen_cache(
                    node_id,
                    bbox.size.w,
                    bbox.size.h,
                    now,
                );
                cache.dirty || cache.texture.is_none() || cache.bbox.is_none()
            };

            if cache_miss {
                if defer_offscreen_rebuild {
                    log_window_render_path(
                        st,
                        node_id,
                        "direct-surface-transition",
                        &format!(
                            "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                            texture_rect.0,
                            texture_rect.1,
                            texture_rect.2,
                            texture_rect.3,
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1)
                        ),
                    );
                    let elems = render_elements_from_surface_tree(
                        renderer,
                        &wl,
                        (sx, sy),
                        element_scale as f64,
                        alpha,
                        Kind::Unspecified,
                    );
                    let (tx, ty, tw, th) = if fullscreen_on_current_monitor {
                        (
                            output_clip.loc.x,
                            output_clip.loc.y,
                            output_clip.size.w,
                            output_clip.size.h,
                        )
                    } else {
                        texture_rect
                    };
                    let display_clip = Rectangle::<i32, Physical>::new(
                        (tx, ty).into(),
                        (tw.max(1), th.max(1)).into(),
                    );

                    let (clip_gx, clip_gy, clip_gw, clip_gh) =
                        rect_from_local_geometry(sx, sy, element_scale, clip_geo);
                    let geo_clip_rect = Rectangle::<i32, Physical>::new(
                        (clip_gx, clip_gy).into(),
                        (clip_gw.max(1), clip_gh.max(1)).into(),
                    );
                    let cropped = wrap_direct_surface_elements(
                        elems,
                        display_clip,
                        st.ui.render_state.surface_clip_program.as_ref(),
                        geo_clip_rect,
                        (effective_corner_radius_px - effective_border_px).max(0) as f32,
                    );

                    if stack_render_set.contains(&node_id) {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(
                                    node_id,
                                    stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                                )
                            })
                            .active_elements
                            .extend(cropped);
                    } else if fullscreen_on_current_monitor {
                        fullscreen_active_elements.extend(cropped);
                    } else if draw_top_this_node {
                        resized_active_elements.extend(cropped);
                    } else {
                        active_elements.extend(cropped);
                    }
                    continue;
                }

                match render_surface_tree_to_texture(renderer, &wl, 1.0, offscreen_clip.clone()) {
                    Ok(offscreen) => {
                        log_window_render_path(
                            st,
                            node_id,
                            "offscreen-rebuild-ok",
                            &format!(
                                "bbox=({},{} {}x{})",
                                offscreen.bbox.loc.x,
                                offscreen.bbox.loc.y,
                                offscreen.bbox.size.w,
                                offscreen.bbox.size.h
                            ),
                        );
                        let cache = st
                            .ui
                            .render_state
                            .window_offscreen_cache
                            .get_mut(&node_id)
                            .expect("offscreen cache should exist after ensure");
                        cache.texture = Some(offscreen.texture);
                        cache.bbox = Some(offscreen.bbox);
                        cache.has_content = offscreen.has_content;
                        cache.mark_clean(now);
                    }
                    Err(err) => {
                        let can_use_stale_cache = st
                            .ui
                            .render_state
                            .window_offscreen_cache
                            .get(&node_id)
                            .is_some_and(|cache| cache.texture.is_some() && cache.bbox.is_some());
                        log_window_render_path(
                            st,
                            node_id,
                            "offscreen-rebuild-failed",
                            &format!("stale_cache={} err={}", can_use_stale_cache, err),
                        );
                        if !can_use_stale_cache {
                            log_window_render_path(
                                st,
                                node_id,
                                "direct-surface-fallback",
                                &format!(
                                    "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                                    texture_rect.0,
                                    texture_rect.1,
                                    texture_rect.2,
                                    texture_rect.3,
                                    gx,
                                    gy,
                                    gw.max(1),
                                    gh.max(1)
                                ),
                            );
                            let elems = render_elements_from_surface_tree(
                                renderer,
                                &wl,
                                (sx, sy),
                                element_scale as f64,
                                alpha,
                                Kind::Unspecified,
                            );
                            let (tx, ty, tw, th) = if fullscreen_on_current_monitor {
                                (
                                    output_clip.loc.x,
                                    output_clip.loc.y,
                                    output_clip.size.w,
                                    output_clip.size.h,
                                )
                            } else {
                                texture_rect
                            };
                            let display_clip = Rectangle::<i32, Physical>::new(
                                (tx, ty).into(),
                                (tw.max(1), th.max(1)).into(),
                            );

                            let (clip_gx, clip_gy, clip_gw, clip_gh) =
                                rect_from_local_geometry(sx, sy, element_scale, clip_geo);
                            let geo_clip_rect = Rectangle::<i32, Physical>::new(
                                (clip_gx, clip_gy).into(),
                                (clip_gw.max(1), clip_gh.max(1)).into(),
                            );
                            let cropped = wrap_direct_surface_elements(
                                elems,
                                display_clip,
                                st.ui.render_state.surface_clip_program.as_ref(),
                                geo_clip_rect,
                                (effective_corner_radius_px - effective_border_px).max(0) as f32,
                            );

                            if stack_render_set.contains(&node_id) {
                                stack_window_units
                                    .entry(node_id)
                                    .or_insert_with(|| {
                                        StackWindowDrawUnit::new(
                                            node_id,
                                            stack_draw_orders
                                                .get(&node_id)
                                                .copied()
                                                .unwrap_or_default(),
                                        )
                                    })
                                    .active_elements
                                    .extend(cropped);
                            } else if fullscreen_on_current_monitor {
                                fullscreen_active_elements.extend(cropped);
                            } else if draw_top_this_node {
                                resized_active_elements.extend(cropped);
                            } else {
                                active_elements.extend(cropped);
                            }
                            continue;
                        }
                    }
                }
            }

            if let Some(cache) = st.ui.render_state.window_offscreen_cache.get_mut(&node_id) {
                cache.touch(now);
            }

            match st
                .ui
                .render_state
                .window_offscreen_cache
                .get(&node_id)
                .map(|cache| (cache.texture.clone(), cache.bbox, cache.has_content))
            {
                Some((texture, bbox, _has_content)) => {
                    let Some(texture) = texture else {
                        continue;
                    };
                    let Some(ob) = bbox else {
                        continue;
                    };
                    // src = full bbox, dst = bbox scaled to screen positioned so geo
                    // lands on frame, clip = frame rect to discard CSD shadow bleed.
                    let (
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                    ) = if let Some(active_resize) = active_resize {
                        // Use live committed geo (updated on every client commit)
                        // as the single source of truth. Falls back to frozen
                        // local_geo before the first commit after resize starts.
                        let (live_gw, live_gh): (f32, f32) = if active_resize.live_geo_w > 0.0 {
                            (active_resize.live_geo_w, active_resize.live_geo_h)
                        } else {
                            // Before first commit: keep the frozen start size.
                            (local_geo.2, local_geo.3)
                        };
                        let frozen_geo_lx =
                            (ob.loc.x + resize_preview.unwrap().start_geo_inset_x) as f32;
                        let frozen_geo_ly =
                            (ob.loc.y + resize_preview.unwrap().start_geo_inset_y) as f32;

                        // Match the normal offscreen path: anchor the destination from the
                        // live geometry rect itself, then let the visual crop helper expand it.
                        // Clipping only to the preview frame was shaving off the recovered edge
                        // margin during resize, which made the resize look slightly tighter than
                        // the steady-state path.
                        // Keep resize anchored to the same screen-space rect the frame/background
                        // uses. At non-1.0 zoom, deriving the destination from surface_origin_px
                        // plus live local geometry can drift by a pixel or two from the preview/frame
                        // rect because those values round in different spaces. Using the frame/top-left
                        // anchor directly keeps the texture locked to its background while still sizing
                        // from the live committed geometry.
                        let _live_gw_px = (live_gw * cam_scale).round().max(1.0) as i32;
                        let _live_gh_px = (live_gh * cam_scale).round().max(1.0) as i32;
                        let preview_gw_px = gw.max(1);
                        let preview_gh_px = gh.max(1);

                        offscreen_visual_crop_and_dst(
                            ob.loc.x,
                            ob.loc.y,
                            ob.size.w.max(1),
                            ob.size.h.max(1),
                            frozen_geo_lx,
                            frozen_geo_ly,
                            live_gw,
                            live_gh,
                            gx,
                            gy,
                            preview_gw_px,
                            preview_gh_px,
                            render_scale,
                            output_clip,
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                        )
                    } else {
                        offscreen_visual_crop_and_dst(
                            ob.loc.x,
                            ob.loc.y,
                            ob.size.w.max(1),
                            ob.size.h.max(1),
                            local_geo.0,
                            local_geo.1,
                            local_geo.2,
                            local_geo.3,
                            gx,
                            gy,
                            gw.max(1),
                            gh.max(1),
                            render_scale,
                            output_clip,
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                        )
                    };
                    log_window_render_path(
                        st,
                        node_id,
                        "offscreen-compose",
                        &format!(
                            "cache_bbox={} local_bbox={} local_geo={} texture_rect={} geometry_rect={} src={} dst={} clip={} preserve_visual_margin={} lock_dst_to_geometry={} tex_size={}x{} radius_px={} border_px={}",
                            rect4_str(ob.loc.x, ob.loc.y, ob.size.w, ob.size.h),
                            rect4f_str(local_bbox.0, local_bbox.1, local_bbox.2, local_bbox.3),
                            rect4f_str(local_geo.0, local_geo.1, local_geo.2, local_geo.3),
                            rect4_str(
                                texture_rect.0,
                                texture_rect.1,
                                texture_rect.2,
                                texture_rect.3
                            ),
                            rect4_str(gx, gy, gw.max(1), gh.max(1)),
                            rect4f_str(src_x as f32, src_y as f32, src_w as f32, src_h as f32),
                            rect4_str(dst_x, dst_y, dst_w, dst_h),
                            rect4_str(clip_x, clip_y, clip_w, clip_h),
                            preserve_visual_margin,
                            lock_dst_to_geometry,
                            texture.size().w,
                            texture.size().h,
                            effective_corner_radius_px,
                            effective_border_px,
                        ),
                    );

                    // Compute the geometry rect in dst-local pixel space so the
                    // shader can clip window content to it (fixes Firefox / CSD
                    // apps whose bbox is larger than their geometry rect).
                    // ob = cached bbox in logical surface space (origin at (0,0)).
                    // geo_lx/ly are the geometry origin inside that bbox.
                    // dst maps the bbox to screen: dst_x..dst_x+dst_w covers ob.
                    let src_scale_x = if src_w > 0.0 {
                        dst_w as f32 / src_w as f32
                    } else {
                        1.0
                    };
                    let src_scale_y = if src_h > 0.0 {
                        dst_h as f32 / src_h as f32
                    } else {
                        1.0
                    };
                    let disable_geo_clip = !strict_square_csd_transition
                        && !st.runtime.tuning.no_csd
                        && effective_corner_radius_px == 0;
                    // local_geo is (geo_lx, geo_ly, geo_w, geo_h) in surface-local logical px.
                    // In bbox-local space the geo origin is (geo_lx - ob.loc.x, geo_ly - ob.loc.y).
                    let geo_local_x = local_geo.0 - ob.loc.x as f32;
                    let geo_local_y = local_geo.1 - ob.loc.y as f32;
                    let geo_src_x = (geo_local_x - src_x as f32).max(0.0);
                    let geo_src_y = (geo_local_y - src_y as f32).max(0.0);
                    // Scale into dst-pixel space (relative to dst top-left).
                    let geo_offset_x = if disable_geo_clip {
                        0.0
                    } else {
                        (geo_src_x * src_scale_x).max(0.0)
                    };
                    let geo_offset_y = if disable_geo_clip {
                        0.0
                    } else {
                        (geo_src_y * src_scale_y).max(0.0)
                    };
                    let geo_w_px = if disable_geo_clip {
                        0.0
                    } else {
                        (local_geo.2 * src_scale_x).min(dst_w as f32).max(1.0)
                    };
                    let geo_h_px = if disable_geo_clip {
                        0.0
                    } else {
                        (local_geo.3 * src_scale_y).min(dst_h as f32).max(1.0)
                    };

                    let offscreen = OffscreenNodeTexture {
                        texture,
                        alpha,
                        corner_radius: (effective_corner_radius_px - effective_border_px).max(0)
                            as f32,
                        src_x,
                        src_y,
                        src_w,
                        src_h,
                        dst_x,
                        dst_y,
                        dst_w,
                        dst_h,
                        clip_x,
                        clip_y,
                        clip_w,
                        clip_h,
                        geo_offset_x,
                        geo_offset_y,
                        geo_w: geo_w_px,
                        geo_h: geo_h_px,
                    };
                    if stack_render_set.contains(&node_id) {
                        stack_window_units
                            .entry(node_id)
                            .or_insert_with(|| {
                                StackWindowDrawUnit::new(
                                    node_id,
                                    stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                                )
                            })
                            .offscreen_textures
                            .push(offscreen);
                    } else if fullscreen_on_current_monitor {
                        fullscreen_offscreen_textures.push(offscreen);
                    } else if draw_top_this_node {
                        resized_offscreen_textures.push(offscreen);
                    } else {
                        offscreen_textures.push(offscreen);
                    }
                }
                None => {
                    continue;
                }
            }
        } else {
            log_window_render_path(
                st,
                node_id,
                "direct-surface-no-offscreen",
                &format!(
                    "texture_rect=({},{} {}x{}) geo_rect=({},{} {}x{})",
                    texture_rect.0,
                    texture_rect.1,
                    texture_rect.2,
                    texture_rect.3,
                    gx,
                    gy,
                    gw.max(1),
                    gh.max(1)
                ),
            );
            let elems = render_elements_from_surface_tree(
                renderer,
                &wl,
                (sx, sy),
                element_scale as f64,
                alpha,
                Kind::Unspecified,
            );
            let (tx, ty, tw, th) = if fullscreen_on_current_monitor {
                (
                    output_clip.loc.x,
                    output_clip.loc.y,
                    output_clip.size.w,
                    output_clip.size.h,
                )
            } else {
                texture_rect
            };
            let display_clip =
                Rectangle::<i32, Physical>::new((tx, ty).into(), (tw.max(1), th.max(1)).into());
            let (clip_gx, clip_gy, clip_gw, clip_gh) =
                rect_from_local_geometry(sx, sy, element_scale, clip_geo);
            let geo_clip_rect = Rectangle::<i32, Physical>::new(
                (clip_gx, clip_gy).into(),
                (clip_gw.max(1), clip_gh.max(1)).into(),
            );
            let cropped = wrap_direct_surface_elements(
                elems,
                display_clip,
                st.ui.render_state.surface_clip_program.as_ref(),
                geo_clip_rect,
                (effective_corner_radius_px - effective_border_px).max(0) as f32,
            );

            if stack_render_set.contains(&node_id) {
                stack_window_units
                    .entry(node_id)
                    .or_insert_with(|| {
                        StackWindowDrawUnit::new(
                            node_id,
                            stack_draw_orders.get(&node_id).copied().unwrap_or_default(),
                        )
                    })
                    .active_elements
                    .extend(cropped);
            } else if fullscreen_on_current_monitor {
                fullscreen_active_elements.extend(cropped);
            } else if draw_top_this_node {
                resized_active_elements.extend(cropped);
            } else {
                active_elements.extend(cropped);
            }
        }

        let parent_geo = window_geometry_for_node(st, node_id).unwrap_or((
            0.0,
            0.0,
            node_intrinsic.x.max(1.0),
            node_intrinsic.y.max(1.0),
        ));
        let parent_geo_loc = (parent_geo.0.round() as i32, parent_geo.1.round() as i32);
        let mut popup_cropped = Vec::new();
        let mut popups: Vec<_> = PopupManager::popups_for_surface(&wl).collect();
        popups.reverse();
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let popup_sx = sx
                + ((parent_geo_loc.0 + popup_offset.x - popup_geo.loc.x) as f32 * element_scale)
                    .round() as i32;
            let popup_sy = sy
                + ((parent_geo_loc.1 + popup_offset.y - popup_geo.loc.y) as f32 * element_scale)
                    .round() as i32;
            if use_offscreen_zoom {
                match render_surface_tree_to_texture(renderer, popup.wl_surface(), alpha, None) {
                    Ok(offscreen) => {
                        let src_x = 0.0f64;
                        let src_y = 0.0f64;
                        let src_w = offscreen.bbox.size.w.max(1) as f64;
                        let src_h = offscreen.bbox.size.h.max(1) as f64;
                        let dst_x =
                            popup_sx + (offscreen.bbox.loc.x as f32 * element_scale).round() as i32;
                        let dst_y =
                            popup_sy + (offscreen.bbox.loc.y as f32 * element_scale).round() as i32;
                        let dst_w = (offscreen.bbox.size.w as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        let dst_h = (offscreen.bbox.size.h as f32 * element_scale)
                            .round()
                            .max(1.0) as i32;
                        let offscreen_texture = OffscreenNodeTexture {
                            texture: offscreen.texture,
                            alpha,
                            corner_radius: 0.0,
                            src_x,
                            src_y,
                            src_w,
                            src_h,
                            dst_x,
                            dst_y,
                            dst_w,
                            dst_h,
                            clip_x: output_clip.loc.x,
                            clip_y: output_clip.loc.y,
                            clip_w: output_clip.size.w,
                            clip_h: output_clip.size.h,
                            // Popups have no border rounding, geo == full dst.
                            geo_offset_x: 0.0,
                            geo_offset_y: 0.0,
                            geo_w: 0.0,
                            geo_h: 0.0,
                        };
                        if fullscreen_on_current_monitor {
                            fullscreen_popup_offscreen_textures.push(offscreen_texture);
                        } else {
                            popup_offscreen_textures.push(offscreen_texture);
                        }
                    }
                    Err(_) => {
                        let popup_elems: Vec<SurfaceElement> = render_elements_from_surface_tree(
                            renderer,
                            popup.wl_surface(),
                            (popup_sx, popup_sy),
                            element_scale as f64,
                            alpha,
                            Kind::Unspecified,
                        );
                        popup_cropped.extend(
                            popup_elems.into_iter().filter_map(|e| {
                                CropRenderElement::from_element(e, 1.0, output_clip)
                            }),
                        );
                    }
                }
            } else {
                let popup_elems: Vec<SurfaceElement> = render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    (popup_sx, popup_sy),
                    element_scale as f64,
                    alpha,
                    Kind::Unspecified,
                );
                popup_cropped.extend(
                    popup_elems
                        .into_iter()
                        .filter_map(|e| CropRenderElement::from_element(e, 1.0, output_clip)),
                );
            }
        }

        if fullscreen_on_current_monitor {
            fullscreen_popup_elements.extend(popup_cropped);
        } else {
            popup_elements.extend(popup_cropped);
        }
    }

    let mut stack_window_units = stack_window_units.into_values().collect::<Vec<_>>();
    if let Some(plan) = stack_transition_plan.as_ref() {
        for extra in &plan.extra_instances {
            let Some(from_pose) = plan.poses.get(&extra.node_id).copied() else {
                continue;
            };
            let Some(unit) = stack_window_units
                .iter()
                .find(|unit| unit.node_id == extra.node_id)
            else {
                continue;
            };
            if let Some(extra_unit) =
                clone_stack_window_unit_for_pose(st, size, unit, from_pose, extra.pose)
            {
                stack_window_units.push(extra_unit);
            }
        }
    }
    stack_window_units.sort_by_key(|unit| unit.draw_order);

    (
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        node_surface_map,
        stack_window_units,
        border_rects,
        resized_border_rects,
        overlap_overlay_rects,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        should_draw_resize_overlap_overlay, strict_square_csd_transition_mode,
        world_to_screen_for_view,
    };
    use crate::compositor::surface_ops::stacking_render_order_map;
    use halley_core::field::NodeId;
    use halley_core::field::Vec2;

    #[test]
    fn stacking_render_order_keeps_front_card_last() {
        let members = vec![
            NodeId::new(1),
            NodeId::new(2),
            NodeId::new(3),
            NodeId::new(4),
        ];
        let ranks = stacking_render_order_map(&members, 3);

        assert_eq!(ranks.get(&NodeId::new(1)), Some(&2));
        assert_eq!(ranks.get(&NodeId::new(2)), Some(&1));
        assert_eq!(ranks.get(&NodeId::new(3)), Some(&0));
        assert_eq!(ranks.get(&NodeId::new(4)), None);
    }

    #[test]
    fn resize_overlap_overlay_skips_underlay_for_overlap_policy_resize() {
        assert!(!should_draw_resize_overlap_overlay(
            Some((0, 0, 100, 100, NodeId::new(1))),
            NodeId::new(2),
            (20, 20, 40, 40),
            true,
        ));
    }

    #[test]
    fn resize_overlap_overlay_marks_intersecting_underlay_for_normal_resize() {
        assert!(should_draw_resize_overlap_overlay(
            Some((0, 0, 100, 100, NodeId::new(1))),
            NodeId::new(2),
            (20, 20, 40, 40),
            false,
        ));
    }

    #[test]
    fn square_csd_steady_state_keeps_legacy_visual_path() {
        assert!(!strict_square_csd_transition_mode(false, 0, false));
    }

    #[test]
    fn square_csd_transition_uses_strict_clip_path() {
        assert!(strict_square_csd_transition_mode(false, 0, true));
        assert!(!strict_square_csd_transition_mode(true, 0, true));
        assert!(!strict_square_csd_transition_mode(false, 8, true));
    }

    #[test]
    fn world_to_screen_for_view_uses_supplied_monitor_camera() {
        let center = Vec2 { x: 500.0, y: 200.0 };
        let view_size = Vec2 { x: 400.0, y: 200.0 };

        assert_eq!(
            world_to_screen_for_view(center, view_size, 1920, 1080, 500.0, 200.0),
            (960, 540)
        );
        assert_eq!(
            world_to_screen_for_view(center, view_size, 1920, 1080, 300.0, 100.0),
            (0, 0)
        );
    }
}
