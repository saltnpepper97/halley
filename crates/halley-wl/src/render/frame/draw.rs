use std::error::Error;

use halley_core::field::Vec2;
use halley_core::viewport::FocusRing;
use smithay::{
    backend::renderer::{
        Bind, Color32F, Frame, FrameContext, Renderer, Texture,
        element::utils::{Relocate, RelocateRenderElement},
        gles::{GlesFrame, GlesTexProgram, Uniform},
        utils::draw_render_elements,
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use super::super::bearings::draw_bearings;
use super::super::cursor::draw_cursor_sprite;
use super::super::cursor_theme::CursorManager;
use super::super::draw_primitives::{draw_rect, draw_ring};
use super::super::node::{draw_closing_node_markers, draw_node_hover_labels, draw_node_markers};
use super::super::pin_icon::draw_pin_badges;
use super::super::state::{ClosingWindowAnimationKind, ClosingWindowAnimationSnapshot};
use super::scene::{CursorScene, PreparedFrameState, SceneCollections};
use crate::compositor::root::Halley;
use crate::overlay::{
    OverlayView, draw_cluster_bloom, draw_cluster_overflow_promotion, draw_cluster_overflow_strip,
    draw_cluster_selection_markers, draw_monitor_hud, draw_overlay_hover_label,
    draw_overlay_hover_preview_card,
};
use crate::presentation::world_to_screen;
use crate::render::blur::{BlurTextures, capture_current_framebuffer_blur_patch};
use crate::render::layer_shell::LayerSurfaceRenderGroup;
use crate::render::shadow::draw_shadow_rect;
use crate::window::{
    ActiveBorderRect, OffscreenNodeTexture, StackWindowDrawUnit,
    WindowShadowRect,
};

pub(crate) struct FrameBlurContext<'a> {
    pub(super) textures: &'a mut BlurTextures,
    pub(super) down_program: &'a GlesTexProgram,
    pub(super) up_program: &'a GlesTexProgram,
    pub(super) composite_program: &'a GlesTexProgram,
    pub(super) masked_composite_program: &'a GlesTexProgram,
    pub(super) offset: f32,
    pub(super) saturation: f32,
    pub(super) noise: f32,
    pub(super) layer_mask_texture: Option<&'a mut smithay::backend::renderer::gles::GlesTexture>,
}

impl FrameBlurContext<'_> {
    pub(crate) fn draw_patch(
        &mut self,
        frame: &mut GlesFrame<'_, '_>,
        damage: Rectangle<i32, Physical>,
        dst: Rectangle<i32, Physical>,
        corner_radius: f32,
        alpha: f32,
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        capture_current_framebuffer_blur_patch(
            frame,
            self.textures,
            self.down_program,
            self.up_program,
            self.composite_program,
            dst,
            corner_radius,
            self.saturation,
            self.noise,
            alpha,
            damage,
            self.offset,
        )
    }

    fn draw_before_texture(
        &mut self,
        frame: &mut GlesFrame<'_, '_>,
        damage: Rectangle<i32, Physical>,
        tex: &OffscreenNodeTexture,
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        if !tex.blur {
            return Ok(());
        }
        let dst = Rectangle::<i32, Physical>::new(
            (tex.dst_x, tex.dst_y).into(),
            (tex.dst_w.max(1), tex.dst_h.max(1)).into(),
        );
        if let Err(err) = self.draw_patch(
            frame,
            damage,
            dst,
            tex.corner_radius,
            tex.blur_alpha.clamp(0.0, 1.0),
        ) {
            eventline::warn!("window blur skipped this frame: {err}");
        }
        Ok(())
    }

    fn draw_layer_group_blur(
        &mut self,
        frame: &mut GlesFrame<'_, '_>,
        damage: Rectangle<i32, Physical>,
        group: &LayerSurfaceRenderGroup,
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        let Some(mask) = self.layer_mask_texture.take() else {
            return Ok(());
        };

        let result = (|| {
            render_layer_group_mask(frame, group, mask)?;
            crate::render::blur::capture_current_framebuffer_blur_patch_masked(
                frame,
                self.textures,
                self.down_program,
                self.up_program,
                self.masked_composite_program,
                group.dst,
                mask,
                self.saturation,
                self.noise,
                1.0,
                damage,
                self.offset,
            )
        })();
        self.layer_mask_texture = Some(mask);
        result
    }
}

fn render_layer_group_mask(
    frame: &mut GlesFrame<'_, '_>,
    group: &LayerSurfaceRenderGroup,
    mask: &mut smithay::backend::renderer::gles::GlesTexture,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    if group.dst.size.w <= 0 || group.dst.size.h <= 0 || group.elements.is_empty() {
        return Ok(());
    }

    let offset = (-group.dst.loc.x, -group.dst.loc.y);
    let relocated = group
        .elements
        .iter()
        .map(|element| RelocateRenderElement::from_element(element, offset, Relocate::Relative))
        .collect::<Vec<_>>();
    let damage = Rectangle::<i32, Physical>::from_size(group.dst.size);

    let mut renderer_guard = frame.renderer();
    let renderer = renderer_guard.as_mut();
    let mut bound = renderer.bind(mask)?;
    let mut mask_frame = renderer.render(&mut bound, group.dst.size, Transform::Normal)?;
    mask_frame.clear(Color32F::TRANSPARENT, &[damage])?;
    let _ = draw_render_elements(&mut mask_frame, 1.0, &relocated, &[damage])?;
    let _ = mask_frame.finish()?;
    Ok(())
}

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

    draw_scene_below_windows(frame, st, size, prepared, scene, hover_node, None)?;
    draw_scene_windows_and_hud(frame, st, size, prepared, scene, hover_node, None)?;
    Ok(())
}

/// Everything drawn behind client windows: background/bottom layer surfaces, node
/// markers, and window shadows. Blur captures start after this content is on the
/// real framebuffer, immediately before each blur-enabled window is drawn.
pub(super) fn draw_scene_below_windows(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    prepared: &PreparedFrameState,
    scene: &SceneCollections,
    _hover_node: Option<halley_core::field::NodeId>,
    mut blur_ctx: Option<&mut FrameBlurContext<'_>>,
) -> Result<(), Box<dyn Error>> {
    draw_layer_groups(
        frame,
        prepared.damage,
        &scene.layer_background_elements,
        blur_ctx.as_deref_mut(),
    )?;
    draw_layer_groups(
        frame,
        prepared.damage,
        &scene.layer_bottom_elements,
        blur_ctx.as_deref_mut(),
    )?;

    // Node/core markers used to be drawn here, beneath windows, which let a window
    // grown over a marker hide it. They're now drawn in `draw_scene_windows_and_hud`
    // (above window bodies, below popups/HUD and their own hover labels) so a landmark
    // is never occluded. See that pass for the marker draw.
    draw_window_shadows(frame, size, prepared.damage, &scene.shadow_rects, st)?;
    Ok(())
}

/// Client windows (all routes), popups, fullscreen, top/overlay layers, focus
/// ring, and the monitor HUD. Drawn on top of the (optionally blur-patched)
/// below-windows content.
pub(super) fn draw_scene_windows_and_hud(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    prepared: &PreparedFrameState,
    scene: &SceneCollections,
    hover_node: Option<halley_core::field::NodeId>,
    mut blur_ctx: Option<&mut FrameBlurContext<'_>>,
) -> Result<(), Box<dyn Error>> {
    // Minimize/collapse-to-node shrinks draw beneath live windows so a minimizing
    // window drops behind the windows it was stacked under instead of flashing to
    // the front. Real closes stay in the on-top pass below.
    draw_closing_window_shrink(
        frame,
        size,
        prepared.damage,
        &scene.closing_window_animations,
        st,
        true,
    )?;

    // Node/core markers (landmarks: standalone nodes + cluster cores) draw beneath the
    // live windows so a window collapsing into its node drops behind the windows it was
    // stacked under instead of the node marker flashing to the front. They sit above the
    // wallpaper/below-windows backdrop and below popups, overlay HUD, and the node hover
    // labels that follow. Markers are screen-space constant, so this keeps them sized the
    // same at every zoom. (Per the node/window invariant, settled markers don't overlap
    // windows, so a window can't normally hide one once the collapse slide finishes.)
    draw_node_markers(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    if !scene.active_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.active_elements, &[prepared.damage]);
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
        blur_ctx.as_deref_mut(),
    )?;
    draw_window_borders(frame, size, prepared.damage, &scene.border_rects, st)?;
    draw_stack_window_units(
        frame,
        size,
        prepared.damage,
        &scene.stack_window_units,
        st,
        blur_ctx.as_deref_mut(),
    )?;
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
        blur_ctx.as_deref_mut(),
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
        blur_ctx.as_deref_mut(),
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

    let overlay_blur_enabled = halley_config::overlay_blur_enabled(
        &st.runtime.tuning.effects.blur,
        &st.runtime.tuning.overlay_style,
    );
    let overlay_blur_ctx = if overlay_blur_enabled {
        blur_ctx.as_deref_mut()
    } else {
        None
    };
    crate::overlay::with_overlay_blur_context(
        overlay_blur_ctx,
        || -> Result<(), Box<dyn Error>> {
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

            draw_hover_preview(frame, st, prepared.damage, scene)?;
            draw_node_hover_labels(
                frame,
                st,
                size,
                &scene.render_nodes,
                hover_node,
                prepared.damage,
                prepared.now,
            )?;
            Ok(())
        },
    )?;

    draw_layer_groups(
        frame,
        prepared.damage,
        &scene.layer_top_elements,
        blur_ctx.as_deref_mut(),
    )?;

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
        blur_ctx.as_deref_mut(),
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.fullscreen_popup_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
        blur_ctx.as_deref_mut(),
    )?;
    if !scene.fullscreen_popup_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.fullscreen_popup_elements,
            &[prepared.damage],
        );
    }
    // Above-fullscreen windows all render as atomic stack units (content + border
    // drawn together, sorted by draw_order) so a back window's border can't bleed
    // over a front window the way a flat batched border pass would.
    draw_stack_window_units(
        frame,
        size,
        prepared.damage,
        &scene.above_fullscreen_stack_window_units,
        st,
        blur_ctx.as_deref_mut(),
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.above_fullscreen_popup_offscreen_textures,
        st.ui.render_state.gpu.window_texture_program.as_ref(),
        blur_ctx.as_deref_mut(),
    )?;
    if !scene.above_fullscreen_popup_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.above_fullscreen_popup_elements,
            &[prepared.damage],
        );
    }

    draw_pin_badges(frame, st, &scene.pin_badges, prepared.damage)?;

    draw_layer_groups(
        frame,
        prepared.damage,
        &scene.layer_overlay_elements,
        blur_ctx.as_deref_mut(),
    )?;

    if st.should_draw_focus_ring_preview(prepared.now) {
        let focus_ring = st.active_focus_ring();
        let ring_world_cx = st.model.viewport.center.x + focus_ring.offset_x;
        let ring_world_cy = st.model.viewport.center.y + focus_ring.offset_y;
        let (ring_sx, ring_sy) = world_to_screen(st, size.w, size.h, ring_world_cx, ring_world_cy);
        let (screen_rx, screen_ry) = focus_ring_screen_radii(
            crate::compositor::monitor::camera::camera_view_size(&*st),
            size,
            focus_ring,
        );
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

    let overlay_blur_ctx = if overlay_blur_enabled {
        blur_ctx.as_deref_mut()
    } else {
        None
    };
    crate::overlay::with_overlay_blur_context(overlay_blur_ctx, || {
        draw_monitor_hud(frame, st, size.w, size.h, prepared.damage, prepared.now)
    })?;
    Ok(())
}

pub(crate) fn draw_offscreen_textures(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    offscreen_textures: &[OffscreenNodeTexture],
    window_texture_program: Option<&GlesTexProgram>,
    mut blur_ctx: Option<&mut FrameBlurContext<'_>>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for tex in offscreen_textures {
        if let Some(ctx) = blur_ctx.as_deref_mut() {
            ctx.draw_before_texture(frame, damage, tex)?;
        }
        draw_single_offscreen_texture(frame, damage, tex, window_texture_program)?;
    }
    Ok(())
}

pub(super) fn draw_layer_groups(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    groups: &[LayerSurfaceRenderGroup],
    mut blur_ctx: Option<&mut FrameBlurContext<'_>>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for group in groups {
        if group.blur
            && let Some(ctx) = blur_ctx.as_deref_mut()
        {
            if let Err(err) = ctx.draw_layer_group_blur(frame, damage, group) {
                eventline::warn!("layer-shell blur skipped this frame: {err}");
            }
        }
        let _ = draw_render_elements(frame, 1.0, &group.elements, &[damage])?;
    }
    Ok(())
}

pub(super) fn draw_apogee_background_layers(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    background: &[LayerSurfaceRenderGroup],
    bottom: &[LayerSurfaceRenderGroup],
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    draw_non_aperture_layer_groups(frame, damage, background)?;
    draw_non_aperture_layer_groups(frame, damage, bottom)
}

pub(super) fn draw_apogee_aperture_layers(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    groups: &[LayerSurfaceRenderGroup],
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for group in groups.iter().filter(|group| group.is_aperture) {
        let _ = draw_render_elements(frame, 1.0, &group.elements, &[damage])?;
    }
    Ok(())
}

fn draw_non_aperture_layer_groups(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    groups: &[LayerSurfaceRenderGroup],
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for group in groups.iter().filter(|group| !group.is_aperture) {
        let _ = draw_render_elements(frame, 1.0, &group.elements, &[damage])?;
    }
    Ok(())
}

pub(crate) fn draw_single_offscreen_texture(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    tex: &OffscreenNodeTexture,
    window_texture_program: Option<&GlesTexProgram>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    {
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
            return Ok(());
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
            Uniform::new("src_uv_offset", (0.0f32, 0.0f32)),
            Uniform::new("src_uv_scale", (0.0f32, 0.0f32)),
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

/// On-top closing pass: node markers (the landmark a window collapses toward)
/// plus the shrink for real window closes (`behind == false`).
fn draw_closing_window_animations(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    animations: &[ClosingWindowAnimationSnapshot],
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    draw_closing_node_markers(frame, st, size, animations, damage)?;
    draw_closing_window_shrink(frame, size, damage, animations, st, false)
}

/// Window shrink/fade tween. `behind` selects which animations to draw: minimize
/// (collapse-to-node) animations are drawn beneath live windows, real closes on
/// top. Called once per z-phase from `draw_scene_windows_and_hud`.
fn draw_closing_window_shrink(
    frame: &mut GlesFrame<'_, '_>,
    size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    animations: &[ClosingWindowAnimationSnapshot],
    st: &mut Halley,
    behind: bool,
) -> Result<(), Box<dyn Error>> {
    for animation in animations {
        let ClosingWindowAnimationKind::Window {
            style,
            border_rects,
            offscreen_textures,
            start_scale,
            start_alpha,
            behind: anim_behind,
            pull_to,
        } = &animation.kind
        else {
            continue;
        };
        if *anim_behind != behind {
            continue;
        }

        let p = animation.progress.clamp(0.0, 1.0);
        // Cluster close ("suck into core") drives scale + travel with back-loaded
        // ease-in curves so each ghost holds near full-size while it starts to
        // drift, then accelerates and collapses tightly onto the core node in the
        // final stretch. Plain window closes keep the symmetric ease-in-out.
        let pulling = pull_to.is_some();
        let shrink_curve = if pulling { p * p } else { crate::animation::ease_in_out_cubic(p) };
        // Travel is even more back-loaded (t^3) than the shrink, so the window
        // rushes into the node right as it vanishes — selling the "sucked in" feel.
        let pull_t = pull_to.map(|_| p * p * p);
        let pull_offset = |cx: f32, cy: f32| -> (f32, f32) {
            match (pull_to, pull_t) {
                (Some((tx, ty)), Some(t)) => ((tx - cx) * t, (ty - cy) * t),
                _ => (0.0, 0.0),
            }
        };
        // Fold in the window's live scale/alpha at close time so the tween continues seamlessly
        // from the open animation instead of snapping to full size.
        let (scale, alpha) = match style {
            halley_config::WindowCloseAnimationStyle::Shrink => {
                (start_scale * (1.0 - shrink_curve).clamp(0.0, 1.0), *start_alpha)
            }
            halley_config::WindowCloseAnimationStyle::Fade => {
                (*start_scale, start_alpha * (1.0 - shrink_curve).clamp(0.0, 1.0))
            }
        };
        if scale <= 0.001 || alpha <= 0.001 {
            continue;
        }

        let scaled_textures = offscreen_textures
            .iter()
            .cloned()
            .map(|mut tex| {
                let cx = tex.dst_x as f32 + tex.dst_w as f32 * 0.5;
                let cy = tex.dst_y as f32 + tex.dst_h as f32 * 0.5;
                let (ox, oy) = pull_offset(cx, cy);
                let center = (cx + ox, cy + oy);
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
                tex.alpha *= alpha;
                tex
            })
            .collect::<Vec<_>>();
        draw_offscreen_textures(
            frame,
            damage,
            &scaled_textures,
            st.ui.render_state.gpu.window_texture_program.as_ref(),
            None,
        )?;
        if !border_rects.is_empty() {
            let scaled_border_rects = border_rects
                .iter()
                .map(|border_rect| {
                    let bcx = border_rect.x as f32 + border_rect.w as f32 * 0.5;
                    let bcy = border_rect.y as f32 + border_rect.h as f32 * 0.5;
                    let (ox, oy) = pull_offset(bcx, bcy);
                    let center = (bcx + ox, bcy + oy);
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
                        alpha: border_rect.alpha * alpha,
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
    mut blur_ctx: Option<&mut FrameBlurContext<'_>>,
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
            blur_ctx.as_deref_mut(),
        )?;
        if !unit.border_rects.is_empty() {
            draw_window_borders(frame, size, damage, &unit.border_rects, st)?;
        }
        if !unit.pin_badges.is_empty() {
            draw_pin_badges(frame, st, &unit.pin_badges, damage)?;
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

    let config = st.runtime.tuning.effects.shadows.window;
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
                    Uniform::new("src_uv_offset", (0.0f32, 0.0f32)),
                    Uniform::new("src_uv_scale", (0.0f32, 0.0f32)),
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
    st: &Halley,
    damage: Rectangle<i32, Physical>,
    scene: &SceneCollections,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    if let Some(card) = scene.hover_preview_card {
        if let Err(err) =
            draw_overlay_hover_preview_card(frame, st, card.rect, card.node_id, card.alpha, damage)
        {
            eventline::warn!("hover preview card skipped this frame: {err}");
        }
    }

    Ok(())
}

pub(super) fn draw_cursor_layer(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    cursor_screen: Option<(f32, f32)>,
    cursor: &CursorScene,
    cursor_manager: &mut CursorManager,
    cursor_config: &halley_config::CursorConfig,
) -> Result<(), Box<dyn Error>> {
    if let Some((sx, sy)) = cursor_screen {
        let draw_fallback_arrow = match &cursor.cursor_status {
            smithay::input::pointer::CursorImageStatus::Hidden => false,
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                if let Some(sprite) = cursor_manager.sprite_with_fallback(cursor_config, *icon) {
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
