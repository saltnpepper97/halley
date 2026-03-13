use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::renderer::{
        Color32F, Frame, Renderer,
        element::Kind,
        element::surface::render_elements_from_surface_tree,
        gles::{GlesFrame, GlesRenderer, GlesTarget},
        utils::draw_render_elements,
    },
    backend::winit::WinitGraphicsBackend,
    utils::{Physical, Rectangle, Transform},
};

use crate::interaction::types::ResizeCtx;
use crate::spatial::node_in_active_area;
use crate::state::HalleyWlState;

use super::cursor_render::{cursor_surface_hotspot, draw_cursor_sprite};
use super::cursor_theme::themed_cursor_sprite_with_fallback;
use super::dock_render::{draw_dock_preview, draw_docked_pairs};
use super::layer_render::collect_layer_surfaces;
use super::node_render::{
    ActiveBorderRect, NodeSnapshot, collect_active_surfaces, collect_hover_preview,
    draw_node_markers,
};
use super::render_utils::{draw_outline_rect, draw_rect, draw_ring};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;
type CroppedSurfaceElement =
    smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>;

struct PreparedFrameState {
    damage: Rectangle<i32, Physical>,
    now: Instant,
}

struct SceneCollections {
    layer_under_elements: Vec<SurfaceElement>,
    layer_over_elements: Vec<SurfaceElement>,
    active_elements: Vec<CroppedSurfaceElement>,
    resized_active_elements: Vec<CroppedSurfaceElement>,
    popup_elements: Vec<CroppedSurfaceElement>,
    border_rects: Vec<ActiveBorderRect>,
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

fn draw_clamped_border_rect<F: smithay::backend::renderer::Frame>(
    frame: &mut F,
    rect: (i32, i32, i32, i32),
    border_width: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
    framebuffer_size: smithay::utils::Size<i32, Physical>,
) -> Result<(), F::Error> {
    let bw = border_width.max(1);
    let inner_w = rect.2.max(1);
    let inner_h = rect.3.max(1);
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

    draw_intersection(rect.0 - bw, rect.1 - bw, inner_w + (bw * 2), bw)?;
    draw_intersection(rect.0 - bw, rect.1 + inner_h, inner_w + (bw * 2), bw)?;
    draw_intersection(rect.0 - bw, rect.1 - bw, bw, inner_h + (bw * 2))?;
    draw_intersection(rect.0 + inner_w, rect.1 - bw, bw, inner_h + (bw * 2))
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
            // Smithay's nested winit path expects a flipped output transform.
            // The shared world/screen math is already top-left oriented; this
            // compensates for the final EGL target orientation.
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
    let cursor = collect_cursor_scene(renderer, cursor_screen, cursor_image);

    let mut frame = renderer.render(framebuffer, size, frame_transform)?;
    frame.clear(Color32F::new(0.04, 0.05, 0.06, 1.0), &[prepared.damage])?;

    draw_debug_frame_scene(&mut frame, st, size, &prepared, &scene, hover_node)?;
    draw_cursor_layer(&mut frame, prepared.damage, cursor_screen, &cursor)?;

    let _ = frame.finish()?;
    Ok(())
}

fn prepare_debug_frame_state(
    st: &mut HalleyWlState,
    size: smithay::utils::Size<i32, Physical>,
) -> PreparedFrameState {
    let now = Instant::now();
    st.tick_animator_frame(now);
    st.begin_render_frame(now);
    st.configure_layer_shell_surfaces((size.w, size.h).into());

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
    let (layer_under_elements, layer_over_elements) =
        collect_layer_surfaces(renderer, st, size, now);

    let (
        active_elements,
        resized_active_elements,
        popup_elements,
        node_surface_map,
        border_rects,
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
            if !st.field.is_visible(id) {
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
        layer_under_elements,
        layer_over_elements,
        active_elements,
        resized_active_elements,
        popup_elements,
        border_rects,
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
) -> Result<(), Box<dyn Error>> {
    if !scene.layer_under_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_under_elements, &[prepared.damage]);
    }

    if !scene.active_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.active_elements, &[prepared.damage]);
    }

    draw_overlap_overlays(frame, prepared.damage, &scene.overlap_overlay_rects)?;

    if !scene.resized_active_elements.is_empty() {
        let _ = draw_render_elements(
            frame,
            1.0,
            &scene.resized_active_elements,
            &[prepared.damage],
        );
    }

    draw_window_borders(frame, size, prepared.damage, &scene.border_rects)?;

    if !scene.popup_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.popup_elements, &[prepared.damage]);
    }

    draw_geometry_overlays(frame, st, size, prepared.damage, scene)?;

    draw_node_markers(
        frame,
        st,
        size,
        &scene.render_nodes,
        hover_node,
        prepared.damage,
        prepared.now,
    )?;

    draw_hover_preview(frame, prepared.damage, scene)?;
    draw_docked_pairs(frame, st, size, prepared.damage, prepared.now)?;
    draw_dock_preview(frame, st, size, prepared.damage, prepared.now)?;

    if !scene.layer_over_elements.is_empty() {
        let _ = draw_render_elements(frame, 1.0, &scene.layer_over_elements, &[prepared.damage]);
    }

    let focus_ring = st.active_focus_ring();
    draw_ring(
        frame,
        st,
        size.w,
        size.h,
        focus_ring.radius_x,
        focus_ring.radius_y,
        focus_ring.offset_x,
        focus_ring.offset_y,
        Color32F::new(0.15, 0.85, 0.85, 0.9),
        prepared.damage,
    )?;

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

fn draw_window_borders<F>(
    frame: &mut F,
    size: smithay::utils::Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    border_rects: &[ActiveBorderRect],
) -> Result<(), F::Error>
where
    F: Frame,
{
    let bw = 2i32;
    for rect in border_rects {
        let color = if rect.focused {
            Color32F::new(0.22, 0.82, 0.92, 1.0)
        } else {
            Color32F::new(0.38, 0.42, 0.48, 0.90)
        };

        draw_clamped_border_rect(
            frame,
            (rect.x, rect.y, rect.w, rect.h),
            bw,
            color,
            damage,
            size,
        )?;
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
