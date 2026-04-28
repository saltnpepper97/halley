use std::error::Error;

use halley_core::field::Vec2;
use halley_core::viewport::FocusRing;
use smithay::{
    backend::renderer::{
        Color32F, Frame, Texture,
        gles::{GlesFrame, GlesTexProgram, Uniform},
        utils::draw_render_elements,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use super::super::bearings::draw_bearings;
use super::super::cursor::draw_cursor_sprite;
use super::super::cursor_theme::themed_cursor_sprite_with_fallback;
use super::super::draw_primitives::{draw_outline_rect, draw_rect, draw_ring};
use super::super::node::{draw_closing_node_markers, draw_node_hover_labels, draw_node_markers};
use super::super::state::{ClosingWindowAnimationKind, ClosingWindowAnimationSnapshot};
use super::scene::{CursorScene, PreparedFrameState, SceneCollections};
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;
use crate::overlay::{
    OverlayView, draw_cluster_bloom, draw_cluster_overflow_promotion, draw_cluster_overflow_strip,
    draw_cluster_selection_markers, draw_monitor_hud, draw_overlay_hover_label,
};
use crate::presentation::world_to_screen;
use crate::render::shadow::draw_shadow_rect;
use crate::window::{
    ActiveBorderRect, OffscreenNodeTexture, StackWindowDrawUnit, WindowShadowRect,
};

fn focus_ring_screen_radii(
    view_size: Vec2,
    output_size: Size<i32, Physical>,
    focus_ring: FocusRing,
) -> (f32, f32) {
    let px_per_world_x = output_size.w as f32 / view_size.x.max(1.0);
    let px_per_world_y = output_size.h as f32 / view_size.y.max(1.0);
    (
        focus_ring.radius_x * px_per_world_x,
        focus_ring.radius_y * px_per_world_y,
    )
}

fn draw_clamped_outline_rect<F: smithay::backend::renderer::Frame>(
    frame: &mut F,
    rect: (i32, i32, i32, i32),
    line_width: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
    framebuffer_size: smithay::utils::Size<i32, Physical>,
) -> Result<(), F::Error> {
    let lw = line_width.max(1);
    let w = rect.2.max(1);
    let h = rect.3.max(1);
    let fb = Rectangle::<i32, Physical>::from_size(framebuffer_size);

    let mut draw_intersection = |x: i32, y: i32, w: i32, h: i32| -> Result<(), F::Error> {
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let edge = Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
        if let Some(visible) = edge.intersection(fb) {
            draw_rect(
                frame,
                visible.loc.x,
                visible.loc.y,
                visible.size.w,
                visible.size.h,
                color,
                damage,
            )?;
        }
        Ok(())
    };

    draw_intersection(rect.0, rect.1, w, lw)?;
    draw_intersection(rect.0, rect.1 + h - lw, w, lw)?;
    draw_intersection(rect.0, rect.1, lw, h)?;
    draw_intersection(rect.0 + w - lw, rect.1, lw, h)
}

pub(super) fn draw_debug_frame_scene(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    prepared: &PreparedFrameState,
    scene: &SceneCollections,
    hover_node: Option<halley_core::field::NodeId>,
) -> Result<(), Box<dyn Error>> {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        if !scene.session_lock_elements.is_empty() {
            let _ =
                draw_render_elements(frame, 1.0, &scene.session_lock_elements, &[prepared.damage]);
        }
        return Ok(());
    }

    if !scene.layer_background_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_background_elements,
            &[prepared.damage],
        );
    }

    if !scene.layer_bottom_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_bottom_elements, &[prepared.damage]);
    }

    draw_node_markers(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    draw_window_shadows(frame, size, prepared.damage, &scene.shadow_rects, st)?;
    if !scene.active_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.active_elements, &[prepared.damage]);
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    draw_window_borders(frame, size, prepared.damage, &scene.border_rects, st)?;
    draw_stack_window_units(frame, size, prepared.damage, &scene.stack_window_units, st)?;
    draw_overlap_overlays(frame, prepared.damage, &scene.overlap_overlay_rects)?;
    draw_window_shadows(
        frame,
        size,
        prepared.damage,
        &scene.resized_shadow_rects,
        st,
    )?;
    if !scene.resized_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.resized_active_elements,
            &[prepared.damage],
        );
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.resized_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    draw_window_borders(
        frame,
        size,
        prepared.damage,
        &scene.resized_border_rects,
        st,
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.popup_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;

    if !scene.popup_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.popup_elements, &[prepared.damage]);
    }

    draw_closing_window_animations(
        frame,
        size,
        prepared.damage,
        &scene.closing_window_animations,
        st,
    )?;

    draw_geometry_overlays(frame, st, size, prepared.damage, scene)?;

    if !scene.bearing_layouts.is_empty() {
        draw_bearings(frame, st, prepared.damage, &scene.bearing_layouts)?;
    }
    let bloom_monitor = st.model.monitor_state.current_monitor.clone();
    draw_cluster_bloom(
        frame,
        st,
        size.w,
        size.h,
        bloom_monitor.as_str(),
        prepared.damage,
    )?;
    let overlay = OverlayView::from_halley(st);
    draw_cluster_overflow_strip(
        frame,
        &overlay,
        bloom_monitor.as_str(),
        prepared.damage,
        st.now_ms(prepared.now),
    )?;
    draw_cluster_overflow_promotion(
        frame,
        &overlay,
        bloom_monitor.as_str(),
        prepared.damage,
        st.now_ms(prepared.now),
    )?;
    drop(overlay);
    draw_overlay_hover_label(frame, st, size.w, size.h, prepared.damage)?;

    if st.cluster_mode_active() {
        let overlay = OverlayView::from_halley(st);
        draw_cluster_selection_markers(frame, &overlay, size.w, size.h, prepared.damage)?;
    }

    draw_hover_preview(frame, prepared.damage, scene)?;
    draw_node_hover_labels(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    if !scene.layer_top_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_top_elements, &[prepared.damage]);
    }

    if !scene.fullscreen_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.fullscreen_active_elements,
            &[prepared.damage],
        );
    }
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.fullscreen_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.fullscreen_popup_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    if !scene.fullscreen_popup_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.fullscreen_popup_elements,
            &[prepared.damage],
        );
    }
    draw_window_shadows(
        frame,
        size,
        prepared.damage,
        &scene.above_fullscreen_shadow_rects,
        st,
    )?;
    if !scene.above_fullscreen_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.above_fullscreen_active_elements,
            &[prepared.damage],
        );
    }
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.above_fullscreen_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    draw_window_borders(
        frame,
        size,
        prepared.damage,
        &scene.above_fullscreen_border_rects,
        st,
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.above_fullscreen_popup_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
    )?;
    if !scene.above_fullscreen_popup_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.above_fullscreen_popup_elements,
            &[prepared.damage],
        );
    }

    if !scene.layer_overlay_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_overlay_elements,
            &[prepared.damage],
        );
    }

    if st.should_draw_focus_ring_preview(prepared.now) {
        let focus_ring = st.active_focus_ring();
        let ring_world_cx = st.model.viewport.center.x + focus_ring.offset_x;
        let ring_world_cy = st.model.viewport.center.y + focus_ring.offset_y;
        let (ring_sx, ring_sy) = world_to_screen(st, size.w, size.h, ring_world_cx, ring_world_cy);
        let (screen_rx, screen_ry) =
            focus_ring_screen_radii(camera_controller(&*st).view_size(), size, focus_ring);
        draw_ring(
            frame,
            ring_sx as f32,
            ring_sy as f32,
            screen_rx,
            screen_ry,
            Color32F::new(0.15, 0.85, 0.85, 0.9),
            prepared.damage,
        )?;
    }

    draw_monitor_hud(frame, st, size.w, size.h, prepared.damage, prepared.now)?;
    Ok(())
}

pub(crate) fn draw_offscreen_textures(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    offscreen_textures: &[OffscreenNodeTexture],
    window_texture_program: Option<&GlesTexProgram>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for tex in offscreen_textures {
        let tex_size = tex.texture.size();
        let max_src_w = (tex_size.w as f64 - tex.src_x).max(1.0);
        let max_src_h = (tex_size.h as f64 - tex.src_y).max(1.0);
        let src = Rectangle::<f64, Buffer>::new(
            (tex.src_x, tex.src_y).into(),
            (
                tex.src_w.min(max_src_w).max(1.0),
                tex.src_h.min(max_src_h).max(1.0),
            )
                .into(),
        );
        let dst = Rectangle::<i32, Physical>::new(
            (tex.dst_x, tex.dst_y).into(),
            (tex.dst_w.max(1), tex.dst_h.max(1)).into(),
        );
        let visible = Rectangle::<i32, Physical>::new(
            (tex.clip_x, tex.clip_y).into(),
            (tex.clip_w.max(1), tex.clip_h.max(1)).into(),
        )
        .intersection(damage)
        .unwrap_or_else(|| Rectangle::<i32, Physical>::new((0, 0).into(), (0, 0).into()));
        if visible.size.w <= 0 || visible.size.h <= 0 {
            continue;
        }
        let local_damage = Rectangle::<i32, Physical>::new(
            (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
            visible.size,
        );
        let uniforms = [
            Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
            Uniform::new("corner_radius", tex.corner_radius.max(0.0)),
            Uniform::new("border_px", 0.0f32),
            Uniform::new("border_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
            Uniform::new("fill_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
            Uniform::new("content_alpha_scale", 1.0f32),
            Uniform::new("geo_offset", (tex.geo_offset_x, tex.geo_offset_y)),
            Uniform::new("geo_size", (tex.geo_w, tex.geo_h)),
        ];

        frame.render_texture_from_to(
            &tex.texture,
            src,
            dst,
            &[local_damage],
            &[],
            Transform::Normal,
            tex.alpha,
            window_texture_program,
            if window_texture_program.is_some() {
                &uniforms
            } else {
                &[]
            },
        )?;
    }

    Ok(())
}

fn transform_rect_about_center(
    _x: i32,
    _y: i32,
    w: i32,
    h: i32,
    center: (f32, f32),
    scale: f32,
) -> (i32, i32, i32, i32) {
    let new_w = (w as f32 * scale).round().max(1.0) as i32;
    let new_h = (h as f32 * scale).round().max(1.0) as i32;
    (
        (center.0 - new_w as f32 * 0.5).round() as i32,
        (center.1 - new_h as f32 * 0.5).round() as i32,
        new_w,
        new_h,
    )
}

fn draw_closing_window_animations(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    animations: &[ClosingWindowAnimationSnapshot],
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    draw_closing_node_markers(frame, st, size, animations, damage)?;

    for animation in animations {
        let ClosingWindowAnimationKind::Window {
            style,
            border_rects,
            offscreen_textures,
        } = &animation.kind
        else {
            continue;
        };

        let scale = match style {
            halley_config::WindowCloseAnimationStyle::Shrink => {
                (1.0 - crate::animation::ease_in_out_cubic(animation.progress)).clamp(0.0, 1.0)
            }
        };
        if scale <= 0.001 {
            continue;
        }

        let scaled_textures = offscreen_textures
            .iter()
            .cloned()
            .map(|mut tex| {
                let center = (
                    tex.dst_x as f32 + tex.dst_w as f32 * 0.5,
                    tex.dst_y as f32 + tex.dst_h as f32 * 0.5,
                );
                let (dst_x, dst_y, dst_w, dst_h) = transform_rect_about_center(
                    tex.dst_x, tex.dst_y, tex.dst_w, tex.dst_h, center, scale,
                );
                tex.dst_x = dst_x;
                tex.dst_y = dst_y;
                tex.dst_w = dst_w;
                tex.dst_h = dst_h;
                tex.geo_offset_x *= scale;
                tex.geo_offset_y *= scale;
                tex.geo_w *= scale;
                tex.geo_h *= scale;
                tex.corner_radius *= scale;
                tex
            })
            .collect::<Vec<_>>();
        draw_offscreen_textures(
            frame,
            damage,
            &scaled_textures,
            st.ui.render_state.gpu.window_texture_program.as_ref(),
        )?;
        if !border_rects.is_empty() {
            let scaled_border_rects = border_rects
                .iter()
                .map(|border_rect| {
                    let center = (
                        border_rect.x as f32 + border_rect.w as f32 * 0.5,
                        border_rect.y as f32 + border_rect.h as f32 * 0.5,
                    );
                    let (x, y, w, h) = transform_rect_about_center(
                        border_rect.x,
                        border_rect.y,
                        border_rect.w,
                        border_rect.h,
                        center,
                        scale,
                    );
                    ActiveBorderRect {
                        x,
                        y,
                        w,
                        h,
                        inner_offset_x: border_rect.inner_offset_x * scale,
                        inner_offset_y: border_rect.inner_offset_y * scale,
                        inner_w: (border_rect.inner_w * scale).max(1.0),
                        inner_h: (border_rect.inner_h * scale).max(1.0),
                        alpha: border_rect.alpha,
                        border_px: border_rect.border_px * scale,
                        corner_radius: border_rect.corner_radius * scale,
                        inner_corner_radius: border_rect.inner_corner_radius * scale,
                        border_color: border_rect.border_color,
                    }
                })
                .collect::<Vec<_>>();
            draw_window_borders(frame, size, damage, &scaled_border_rects, st)?;
        }
    }
    Ok(())
}

fn draw_stack_window_units(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    stack_window_units: &[StackWindowDrawUnit],
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    for unit in stack_window_units {
        draw_window_shadows(frame, size, damage, &unit.shadow_rects, st)?;
        if !unit.active_elements.is_empty() {
            let _ = draw_render_elements(frame, 1.0, &unit.active_elements, &[damage]);
        }
        draw_offscreen_textures(
            frame,
            damage,
            &unit.offscreen_textures,
            st.ui.render_state.gpu.window_texture_program.as_ref(),
        )?;
        if !unit.border_rects.is_empty() {
            draw_window_borders(frame, size, damage, &unit.border_rects, st)?;
        }
    }
    Ok(())
}

pub(crate) fn draw_window_shadows(
    frame: &mut GlesFrame<'_, '_>,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    shadow_rects: &[WindowShadowRect],
    st: &Halley,
) -> Result<(), Box<dyn Error>> {
    if shadow_rects.is_empty() {
        return Ok(());
    }

    let config = st.runtime.tuning.decorations.shadows.window;
    if !config.enabled || config.color.a <= 0.0 || config.blur_radius <= 0.0 {
        return Ok(());
    }
    let clipped_damage = damage
        .intersection(Rectangle::<i32, Physical>::from_size(size))
        .unwrap_or(damage);

    for rect in shadow_rects {
        if rect.alpha <= 0.0 || rect.w <= 0 || rect.h <= 0 {
            continue;
        }
        draw_shadow_rect(
            frame,
            &st.ui.render_state,
            config,
            Rectangle::<i32, Physical>::new((rect.x, rect.y).into(), (rect.w, rect.h).into()),
            rect.corner_radius,
            rect.alpha,
            clipped_damage,
        )?;
    }

    Ok(())
}

pub(crate) fn draw_window_borders(
    frame: &mut GlesFrame<'_, '_>,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    border_rects: &[ActiveBorderRect],
    st: &Halley,
) -> Result<(), Box<dyn Error>> {
    let border_texture = st.ui.render_state.gpu.node_circle_texture.as_ref();
    let border_program = st.ui.render_state.gpu.ui_rect_rounded_program.as_ref();
    let window_program = st.ui.render_state.gpu.window_texture_program.as_ref();
    let framebuffer = Rectangle::<i32, Physical>::from_size(size);

    for rect in border_rects {
        let border_px = rect.border_px.max(0.0).round() as i32;
        if border_px <= 0 || rect.alpha <= 0.0 {
            continue;
        }

        let dst = Rectangle::<i32, Physical>::new(
            (rect.x - border_px, rect.y - border_px).into(),
            (
                (rect.w + border_px * 2).max(1),
                (rect.h + border_px * 2).max(1),
            )
                .into(),
        );
        let Some(visible) = dst
            .intersection(framebuffer)
            .and_then(|r| r.intersection(damage))
        else {
            continue;
        };
        let local_damage = Rectangle::<i32, Physical>::new(
            (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
            visible.size,
        );
        let fill_color = (0.0f32, 0.0f32, 0.0f32, 0.0f32);

        if rect.corner_radius > 0.0 {
            if let (Some(texture), Some(program)) = (border_texture, border_program) {
                let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (tex_size.w as f64, tex_size.h as f64).into(),
                );
                let uniforms = [
                    Uniform::new(
                        "node_color",
                        (
                            rect.border_color.r(),
                            rect.border_color.g(),
                            rect.border_color.b(),
                            rect.border_color.a(),
                        ),
                    ),
                    Uniform::new("fill_color", fill_color),
                    Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
                    Uniform::new("inner_rect_size", (rect.inner_w, rect.inner_h)),
                    Uniform::new(
                        "inner_rect_offset",
                        (rect.inner_offset_x, rect.inner_offset_y),
                    ),
                    Uniform::new("corner_radius", rect.corner_radius),
                    Uniform::new("inner_corner_radius", rect.inner_corner_radius),
                    Uniform::new("border_px", rect.border_px),
                ];

                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    &[local_damage],
                    &[],
                    Transform::Normal,
                    rect.alpha.clamp(0.0, 1.0),
                    Some(program),
                    &uniforms,
                )?;
                continue;
            }

            if let (Some(texture), Some(program)) = (border_texture, window_program) {
                let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (tex_size.w as f64, tex_size.h as f64).into(),
                );
                let uniforms = [
                    Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
                    Uniform::new("corner_radius", rect.corner_radius),
                    Uniform::new("border_px", rect.border_px),
                    Uniform::new(
                        "border_color",
                        (
                            rect.border_color.r(),
                            rect.border_color.g(),
                            rect.border_color.b(),
                            rect.border_color.a(),
                        ),
                    ),
                    Uniform::new("fill_color", fill_color),
                    Uniform::new("content_alpha_scale", 0.0f32),
                    Uniform::new("geo_offset", (0.0f32, 0.0f32)),
                    Uniform::new("geo_size", (0.0f32, 0.0f32)),
                ];

                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    &[local_damage],
                    &[],
                    Transform::Normal,
                    rect.alpha.clamp(0.0, 1.0),
                    Some(program),
                    &uniforms,
                )?;
                continue;
            }
        }

        draw_clamped_outline_rect(
            frame,
            (dst.loc.x, dst.loc.y, dst.size.w, dst.size.h),
            border_px,
            Color32F::new(
                rect.border_color.r(),
                rect.border_color.g(),
                rect.border_color.b(),
                rect.border_color.a() * rect.alpha.clamp(0.0, 1.0),
            ),
            damage,
            size,
        )?;
    }

    Ok(())
}

fn draw_overlap_overlays<F>(
    frame: &mut F,
    damage: Rectangle<i32, Physical>,
    overlap_overlay_rects: &[(i32, i32, i32, i32)],
) -> Result<(), F::Error>
where
    F: Frame,
{
    for &(x, y, w, h) in overlap_overlay_rects {
        draw_rect(
            frame,
            x,
            y,
            w,
            h,
            Color32F::new(0.45, 0.45, 0.45, 0.34),
            damage,
        )?;
        draw_outline_rect(
            frame,
            x,
            y,
            w,
            h,
            Color32F::new(0.72, 0.72, 0.72, 0.78),
            damage,
        )?;
    }

    Ok(())
}

fn draw_geometry_overlays<F>(
    _frame: &mut F,
    _st: &Halley,
    _size: smithay::utils::Size<i32, Physical>,
    _damage: Rectangle<i32, Physical>,
    _scene: &SceneCollections,
) -> Result<(), F::Error>
where
    F: Frame,
{
    Ok(())
}

fn draw_hover_preview(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    scene: &SceneCollections,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    if let Some((px, py, pw, ph)) = scene.hover_preview_rect
        && !scene.hover_preview_elements.is_empty()
    {
        let dl = damage.loc.x;
        let dt = damage.loc.y;
        let dr = damage.loc.x + damage.size.w;
        let db = damage.loc.y + damage.size.h;
        let l = px.max(dl);
        let t = py.max(dt);
        let r = (px + pw).min(dr);
        let b = (py + ph).min(db);
        if r > l && b > t {
            let clip = Rectangle::new((l, t).into(), ((r - l), (b - t)).into());
            let _ = draw_render_elements(frame, 1.0, &scene.hover_preview_elements, &[clip]);
        }
    }

    Ok(())
}

pub(super) fn draw_cursor_layer(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    cursor_screen: Option<(f32, f32)>,
    cursor: &CursorScene,
    cursor_config: &halley_config::CursorConfig,
) -> Result<(), Box<dyn Error>> {
    if let Some((sx, sy)) = cursor_screen {
        let draw_fallback_arrow = match &cursor.cursor_status {
            smithay::input::pointer::CursorImageStatus::Hidden => false,
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                if let Some(sprite) = themed_cursor_sprite_with_fallback(cursor_config, *icon) {
                    draw_cursor_sprite(frame, damage, (sx, sy), sprite.as_ref())?;
                    false
                } else {
                    true
                }
            }
            smithay::input::pointer::CursorImageStatus::Surface(_) => {
                if !cursor.cursor_surface_elements.is_empty() {
                    let _ = draw_render_elements(
                        frame,
                        1.0,
                        &cursor.cursor_surface_elements,
                        &[damage],
                    );
                }
                false
            }
        };

        if draw_fallback_arrow {
            draw_fallback_cursor_arrow(frame, sx, sy, damage)?;
        }
    }

    Ok(())
}

fn draw_fallback_cursor_arrow<F>(
    frame: &mut F,
    sx: f32,
    sy: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>>
where
    F: smithay::backend::renderer::Frame,
    F::Error: std::error::Error + 'static,
{
    let cx = sx.round() as i32;
    let cy = sy.round() as i32;
    let shadow = Color32F::new(0.0, 0.0, 0.0, 0.40);
    let outline = Color32F::new(0.0, 0.0, 0.0, 0.98);
    let fill = Color32F::new(1.0, 1.0, 1.0, 0.96);

    draw_rect(frame, cx + 2, cy + 2, 2, 14, shadow, damage)?;
    draw_rect(frame, cx + 2, cy + 2, 10, 2, shadow, damage)?;
    draw_rect(frame, cx + 4, cy + 8, 8, 2, shadow, damage)?;
    draw_rect(frame, cx, cy, 2, 14, outline, damage)?;
    draw_rect(frame, cx, cy, 10, 2, outline, damage)?;
    draw_rect(frame, cx + 3, cy + 7, 8, 2, outline, damage)?;
    draw_rect(frame, cx + 1, cy + 1, 1, 12, fill, damage)?;
    draw_rect(frame, cx + 1, cy + 1, 8, 1, fill, damage)?;
    draw_rect(frame, cx + 4, cy + 8, 6, 1, fill, damage)?;
    Ok(())
}
