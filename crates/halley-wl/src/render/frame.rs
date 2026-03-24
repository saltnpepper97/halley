use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        Color32F, Frame, Renderer, Texture,
        element::Kind,
        element::surface::render_elements_from_surface_tree,
        gles::{GlesFrame, GlesRenderer, GlesTarget, GlesTexProgram},
        utils::draw_render_elements,
    },
    backend::winit::WinitGraphicsBackend,
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::interaction::types::ResizeCtx;
use crate::spatial::node_in_active_area;
use crate::state::HalleyWlState;

use super::ACTIVE_WINDOW_FRAME_PAD_PX;
use super::app_icon::ensure_node_app_icon_resources;
use super::cursor::{cursor_surface_hotspot, draw_cursor_sprite};
use super::cursor_theme::themed_cursor_sprite_with_fallback;
use super::layer_shell::collect_layer_surfaces;
use super::node::{
    NodeSnapshot, collect_hover_preview, draw_node_hover_labels, draw_node_markers,
    ensure_node_circle_resources,
};
use super::utils::{draw_outline_rect, draw_rect, draw_ring, world_to_screen};
use super::window::{ActiveBorderRect, OffscreenNodeTexture, collect_active_surfaces};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement =
    smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>;

struct PreparedFrameState {
    damage: Rectangle<i32, Physical>,
    now: Instant,
}

struct SceneCollections {
    layer_background_elements: Vec<SurfaceElement>,
    layer_bottom_elements: Vec<SurfaceElement>,
    layer_top_elements: Vec<SurfaceElement>,
    layer_overlay_elements: Vec<SurfaceElement>,
    active_elements: Vec<CroppedSurfaceElement>,
    resized_active_elements: Vec<CroppedSurfaceElement>,
    offscreen_textures: Vec<OffscreenNodeTexture>,
    resized_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    popup_elements: Vec<CroppedSurfaceElement>,
    border_rects: Vec<ActiveBorderRect>,
    resized_border_rects: Vec<ActiveBorderRect>,
    overlay_rects: Vec<(i32, i32, i32, i32, Color32F)>,
    overlay_points: Vec<(i32, i32, Color32F)>,
    overlap_overlay_rects: Vec<(i32, i32, i32, i32)>,
    hover_preview_rect: Option<(i32, i32, i32, i32)>,
    hover_preview_elements: Vec<SurfaceElement>,
    render_nodes: Vec<NodeSnapshot>,
}

struct CursorScene {
    cursor_status: smithay::input::pointer::CursorImageStatus,
    cursor_surface_elements: Vec<
        smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>,
    >,
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

pub(crate) fn draw_debug_frame(
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    st: &mut HalleyWlState,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
) -> Result<(), Box<dyn Error>> {
    let size = backend.window_size();
    let damage = Rectangle::<i32, Physical>::from_size(size);
    {
        let (renderer, mut framebuffer) = backend.bind()?;
        draw_debug_frame_to_target(
            renderer,
            &mut framebuffer,
            size,
            st,
            resize_preview,
            hover_node,
            preview_hover_node,
            None,
            None,
            Transform::Flipped180,
        )?;
    }
    backend.submit(Some(&[damage]))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_debug_frame_to_target(
    renderer: &mut GlesRenderer,
    framebuffer: &mut GlesTarget<'_>,
    size: smithay::utils::Size<i32, Physical>,
    st: &mut HalleyWlState,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
    frame_transform: Transform,
) -> Result<(), Box<dyn Error>> {
    ensure_node_circle_resources(renderer, st)?;

    let prepared = prepare_debug_frame_state(st, size);
    let scene = collect_debug_frame_scene(
        renderer,
        st,
        size,
        resize_preview,
        hover_node,
        preview_hover_node,
        prepared.now,
    );
    ensure_node_app_icon_resources(renderer, st, &scene.render_nodes)?;
    let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);
    let offscreen_cleanup_program = renderer.compile_custom_texture_shader(
        include_str!("shaders/offscreen_cleanup.frag"),
        &[],
    )?;

    let mut frame = renderer.render(framebuffer, size, frame_transform)?;
    frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;

    draw_debug_frame_scene(
        &mut frame,
        st,
        size,
        &prepared,
        &scene,
        hover_node,
        &offscreen_cleanup_program,
    )?;
    draw_cursor_layer(&mut frame, prepared.damage, cursor_screen, &cursor)?;

    let _ = frame.finish()?;
    Ok(())
}

fn prepare_debug_frame_state(
    st: &mut HalleyWlState,
    size: smithay::utils::Size<i32, Physical>,
) -> PreparedFrameState {
    let now = Instant::now();
    if !st.interaction_state.suppress_layer_shell_configure {
        st.configure_layer_shell_surfaces((size.w, size.h).into());
    }

    PreparedFrameState {
        damage: Rectangle::<i32, Physical>::from_size(size),
        now,
    }
}

fn collect_debug_frame_scene(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    size: smithay::utils::Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    now: Instant,
) -> SceneCollections {
    let (
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
    ) = collect_layer_surfaces(renderer, st, size, now);

    let (
        active_elements,
        resized_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        node_surface_map,
        border_rects,
        resized_border_rects,
        overlay_rects,
        overlay_points,
        overlap_overlay_rects,
    ) = collect_active_surfaces(renderer, st, size, resize_preview, now);

    let hovered_preview_id = preview_hover_node.and_then(|id| {
        st.field.node(id).and_then(|n| {
            (node_in_active_area(st, id)
                && matches!(
                    n.state,
                    halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
                ))
            .then_some(id)
        })
    });
    let (hover_preview_rect, hover_preview_elements) = collect_hover_preview(
        renderer,
        st,
        size,
        &node_surface_map,
        hovered_preview_id,
        hover_node,
        now,
    );

    let render_nodes = st
        .field
        .nodes()
        .iter()
        .filter_map(|(&id, node)| {
            if !st.field.is_visible(id) || !st.node_visible_on_current_monitor(id) {
                return None;
            }
            Some(NodeSnapshot {
                id,
                state: node.state.clone(),
                pos: node.pos,
                intrinsic_size: node.intrinsic_size,
                label: node.label.clone(),
            })
        })
        .collect();

    SceneCollections {
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
        active_elements,
        resized_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        border_rects,
        resized_border_rects,
        overlay_rects,
        overlay_points,
        overlap_overlay_rects,
        hover_preview_rect,
        hover_preview_elements,
        render_nodes,
    }
}

fn collect_cursor_scene(
    renderer: &mut GlesRenderer,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> CursorScene {
    let cursor_status = cursor_image
        .cloned()
        .unwrap_or_else(smithay::input::pointer::CursorImageStatus::default_named);

    let mut cursor_surface_elements = Vec::new();
    if let (Some((sx, sy)), smithay::input::pointer::CursorImageStatus::Surface(surface)) =
        (cursor_screen, cursor_status.clone())
    {
        let (hotspot_x, hotspot_y) = cursor_surface_hotspot(&surface);
        let loc = (sx.round() as i32 - hotspot_x, sy.round() as i32 - hotspot_y);
        cursor_surface_elements =
            render_elements_from_surface_tree(renderer, &surface, loc, 1.0, 1.0, Kind::Unspecified);
    }

    CursorScene {
        cursor_status,
        cursor_surface_elements,
    }
}

fn draw_debug_frame_scene(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut HalleyWlState,
    size: smithay::utils::Size<i32, Physical>,
    prepared: &PreparedFrameState,
    scene: &SceneCollections,
    hover_node: Option<halley_core::field::NodeId>,
    offscreen_cleanup_program: &GlesTexProgram,
) -> Result<(), Box<dyn Error>> {
    if !scene.layer_background_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_background_elements,
            &[prepared.damage],
        );
    }

    if !scene.layer_bottom_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_bottom_elements,
            &[prepared.damage],
        );
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

    draw_window_backgrounds(frame, size, prepared.damage, &scene.border_rects)?;

    if !scene.active_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.active_elements, &[prepared.damage]);
    }

    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.offscreen_textures,
        offscreen_cleanup_program,
    )?;
    draw_overlap_overlays(frame, prepared.damage, &scene.overlap_overlay_rects)?;
    draw_window_backgrounds(frame, size, prepared.damage, &scene.resized_border_rects)?;

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
        offscreen_cleanup_program,
    )?;
    draw_offscreen_textures(
        frame,
        prepared.damage,
        &scene.popup_offscreen_textures,
        offscreen_cleanup_program,
    )?;

    if !scene.popup_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.popup_elements, &[prepared.damage]);
    }

    draw_geometry_overlays(frame, st, size, prepared.damage, scene)?;
    draw_hover_preview(frame, prepared.damage, scene)?;

    if !scene.layer_top_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_top_elements, &[prepared.damage]);
    }

    if !scene.layer_overlay_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.layer_overlay_elements,
            &[prepared.damage],
        );
    }

    draw_node_hover_labels(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    if st.should_draw_focus_ring_preview(prepared.now) {
        let focus_ring = st.active_focus_ring();
        let ring_world_cx = st.viewport.center.x + focus_ring.offset_x;
        let ring_world_cy = st.viewport.center.y + focus_ring.offset_y;
        let (ring_sx, ring_sy) = world_to_screen(st, size.w, size.h, ring_world_cx, ring_world_cy);
        let base_px_per_world_x = size.w as f32 / st.viewport.size.x.max(1.0);
        let base_px_per_world_y = size.h as f32 / st.viewport.size.y.max(1.0);
        let screen_rx = focus_ring.radius_x * base_px_per_world_x;
        let screen_ry = focus_ring.radius_y * base_px_per_world_y;
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

    Ok(())
}

fn draw_offscreen_textures(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    offscreen_textures: &[OffscreenNodeTexture],
    offscreen_cleanup_program: &GlesTexProgram,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    for tex in offscreen_textures {
        let tex_size = tex.texture.size();
        let max_src_w = (tex_size.w - tex.src_x).max(1);
        let max_src_h = (tex_size.h - tex.src_y).max(1);

        let src = Rectangle::<f64, Buffer>::new(
            (tex.src_x as f64, tex.src_y as f64).into(),
            (
                tex.src_w.min(max_src_w).max(1) as f64,
                tex.src_h.min(max_src_h).max(1) as f64,
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

        frame.render_texture_from_to(
            &tex.texture,
            src,
            dst,
            &[local_damage],
            &[],
            Transform::Normal,
            tex.alpha,
            Some(offscreen_cleanup_program),
            &[],
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

fn draw_window_backgrounds<F>(
    frame: &mut F,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    border_rects: &[ActiveBorderRect],
) -> Result<(), F::Error>
where
    F: Frame,
    F::Error: std::error::Error + 'static,
{
    let bw = ACTIVE_WINDOW_FRAME_PAD_PX;
    let fb = Rectangle::<i32, Physical>::from_size(size);
    for rect in border_rects {
        let color = if rect.focused {
            Color32F::new(0.22, 0.82, 0.92, 1.0)
        } else {
            Color32F::new(0.28, 0.30, 0.35, 1.0)
        };
        let bg = Rectangle::<i32, Physical>::new(
            (rect.x - bw, rect.y - bw).into(),
            ((rect.w + bw * 2).max(1), (rect.h + bw * 2).max(1)).into(),
        );
        if let Some(visible) = bg.intersection(fb) {
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
    }
    Ok(())
}

fn draw_geometry_overlays<F>(
    frame: &mut F,
    st: &HalleyWlState,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    scene: &SceneCollections,
) -> Result<(), F::Error>
where
    F: Frame,
{
    if st.tuning.dev_enabled && st.tuning.dev_show_geometry_overlay {
        for &(x, y, w, h, color) in &scene.overlay_rects {
            draw_clamped_outline_rect(frame, (x, y, w, h), 2, color, damage, size)?;
        }
        for &(x, y, color) in &scene.overlay_points {
            draw_rect(frame, x - 2, y - 2, 5, 5, color, damage)?;
        }
    }

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

fn draw_cursor_layer(
    frame: &mut GlesFrame<'_, '_>,
    damage: Rectangle<i32, Physical>,
    cursor_screen: Option<(f32, f32)>,
    cursor: &CursorScene,
) -> Result<(), Box<dyn Error>> {
    if let Some((sx, sy)) = cursor_screen {
        let draw_fallback_arrow = match &cursor.cursor_status {
            smithay::input::pointer::CursorImageStatus::Hidden => false,
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                if let Some(sprite) = themed_cursor_sprite_with_fallback(*icon) {
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
                    false
                } else {
                    true
                }
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

