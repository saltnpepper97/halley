use std::collections::HashMap;
use std::error::Error;
use std::time::Instant;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ImportMem, Texture,
            element::{Kind, surface::render_elements_from_surface_tree},
            gles::{GlesFrame, GlesRenderer, Uniform, UniformName, UniformType},
        },
    },
    desktop::utils::bbox_from_surface_tree,
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use crate::state::HalleyWlState;
use halley_config::{NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy};

use super::utils::{
    bitmap_text_size, draw_bitmap_text, node_marker_bounds, node_marker_metrics,
    node_render_diameter_px, world_to_screen,
};
use crate::animation::ease_in_out_cubic;

const NODE_SQUIRCLE_SHADER: &str =
    include_str!("shaders/node_squircle_shader.frag");

const NODE_LABEL_SHADER: &str =
    include_str!("shaders/node_label_rounded_shader.frag");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Snapshot of per-node data captured before any mutable frame calls so that
/// node iteration and drawing stay in separate, borrow-clean passes.
pub(crate) struct NodeSnapshot {
    pub id: halley_core::field::NodeId,
    pub state: halley_core::field::NodeState,
    pub pos: halley_core::field::Vec2,
    pub intrinsic_size: halley_core::field::Vec2,
    pub label: String,
}

pub(crate) fn ensure_node_circle_resources(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
) -> Result<(), Box<dyn Error>> {
    if st.render_state.node_circle_texture.is_none() {
        const TEX_SIZE: usize = 4;
        let pixel = vec![255u8; TEX_SIZE * TEX_SIZE * 4];
        st.render_state.node_circle_texture = Some(renderer.import_memory(
            &pixel,
            Fourcc::Abgr8888,
            (TEX_SIZE as i32, TEX_SIZE as i32).into(),
            false,
        )?);
    }

    if st.render_state.node_squircle_program.is_none() {
        st.render_state.node_squircle_program = Some(renderer.compile_custom_texture_shader(
            NODE_SQUIRCLE_SHADER,
            &[
                UniformName::new("node_color", UniformType::_4f),
                UniformName::new("fill_color", UniformType::_4f),
            ],
        )?);
    }

    if st.render_state.node_label_program.is_none() {
        st.render_state.node_label_program = Some(renderer.compile_custom_texture_shader(
            NODE_LABEL_SHADER,
            &[
                UniformName::new("node_color", UniformType::_4f),
                UniformName::new("fill_color", UniformType::_4f),
                UniformName::new("rect_size", UniformType::_2f),
                UniformName::new("corner_radius", UniformType::_1f),
                UniformName::new("border_px", UniformType::_1f),
            ],
        )?);
    }

    Ok(())
}

fn draw_shader_circle(
    frame: &mut GlesFrame<'_, '_>,
    st: &HalleyWlState,
    cx: i32,
    cy: i32,
    radius: i32,
    alpha: f32,
    border_color: Color32F,
    fill_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = st.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.render_state.node_squircle_program.as_ref() else {
        return Ok(());
    };

    let radius = radius.max(1);
    let diameter = (radius * 2).max(1);
    let dest = Rectangle::<i32, Physical>::new(
        (cx - radius, cy - radius).into(),
        (diameter, diameter).into(),
    );
    let tex_size = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new(
            "node_color",
            (
                border_color.r(),
                border_color.g(),
                border_color.b(),
                border_color.a(),
            ),
        ),
        Uniform::new(
            "fill_color",
            (
                fill_color.r(),
                fill_color.g(),
                fill_color.b(),
                fill_color.a(),
            ),
        ),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        dest,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )?;

    Ok(())
}

fn draw_shader_label(
    frame: &mut GlesFrame<'_, '_>,
    st: &HalleyWlState,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    corner_radius: f32,
    border_px: f32,
    alpha: f32,
    border_color: Color32F,
    fill_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = st.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = st.render_state.node_label_program.as_ref() else {
        return Ok(());
    };

    let w = w.max(1);
    let h = h.max(1);
    let dest = Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
    let tex_size = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new(
            "node_color",
            (
                border_color.r(),
                border_color.g(),
                border_color.b(),
                border_color.a(),
            ),
        ),
        Uniform::new(
            "fill_color",
            (
                fill_color.r(),
                fill_color.g(),
                fill_color.b(),
                fill_color.a(),
            ),
        ),
        Uniform::new("rect_size", (w as f32, h as f32)),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("border_px", border_px),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        dest,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )?;

    Ok(())
}

fn window_active_border_color() -> Color32F {
    Color32F::new(0.22, 0.82, 0.92, 1.0)
}

fn window_inactive_border_color() -> Color32F {
    Color32F::new(0.28, 0.30, 0.35, 1.0)
}

fn node_ring_color(st: &HalleyWlState, hovered: bool, alpha: f32) -> Color32F {
    let mode = if hovered {
        st.tuning.node_border_color_hover
    } else {
        st.tuning.node_border_color_inactive
    };
    let base = match mode {
        NodeBorderColorMode::UseWindowActive => window_active_border_color(),
        NodeBorderColorMode::UseWindowInactive => window_inactive_border_color(),
    };
    Color32F::new(base.r(), base.g(), base.b(), alpha)
}

fn node_fill_color(st: &HalleyWlState, hovered: bool) -> Color32F {
    match st.tuning.node_background_color {
        NodeBackgroundColorMode::Auto | NodeBackgroundColorMode::Theme => {
            let ring = node_ring_color(st, hovered, 1.0);
            let base = (0.94, 0.96, 0.985);
            Color32F::new(
                base.0 * 0.86 + ring.r() * 0.14,
                base.1 * 0.86 + ring.g() * 0.14,
                base.2 * 0.86 + ring.b() * 0.14,
                1.0,
            )
        }
        NodeBackgroundColorMode::Fixed { r, g, b } => Color32F::new(r, g, b, 1.0),
    }
}

fn node_icon_glyph(
    st: &HalleyWlState,
    id: halley_core::field::NodeId,
    fallback: &str,
) -> Option<char> {
    st.node_app_ids
        .get(&id)
        .map(String::as_str)
        .unwrap_or(fallback)
        .chars()
        .find(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Active surface collection
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
pub(crate) fn collect_hover_preview(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    node_surface_map: &HashMap<
        halley_core::field::NodeId,
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    >,
    hovered_preview_id: Option<halley_core::field::NodeId>,
    hover_node: Option<halley_core::field::NodeId>,
    now: Instant,
) -> (
    Option<(i32, i32, i32, i32)>,
    Vec<smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>>,
) {
    let _ = hover_node;

    let Some((preview_id, preview_mix_raw)) = st.node_preview_hover_anim(hovered_preview_id) else {
        return (None, Vec::new());
    };
    let Some(wl) = node_surface_map.get(&preview_id) else {
        return (None, Vec::new());
    };
    let Some((node_state, node_pos, label_len)) = st
        .field
        .node(preview_id)
        .map(|n| (n.state.clone(), n.pos, n.label.len()))
    else {
        return (None, Vec::new());
    };

    if !matches!(
        node_state,
        halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
    ) {
        return (None, Vec::new());
    }

    let bbox = bbox_from_surface_tree(wl, (0, 0));
    if bbox.size.w <= 0 || bbox.size.h <= 0 {
        return (None, Vec::new());
    }

    let preview_mix = ease_in_out_cubic(preview_mix_raw.clamp(0.0, 1.0));
    let anim = st.anim_style_for(preview_id, node_state.clone(), now);

    const PROXY_TO_MARKER_START: f32 = 0.50;
    const PROXY_TO_MARKER_END: f32 = 0.20;
    let marker_mix_lin = ((PROXY_TO_MARKER_START - anim.scale)
        / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
        .clamp(0.0, 1.0);
    let marker_mix = ease_in_out_cubic(marker_mix_lin);

    let p = node_pos;
    let _ = marker_mix;
    let (cx, cy) = world_to_screen(st, size.w, size.h, p.x, p.y);

    let (dot_half, _, _, _) = node_marker_metrics(st, label_len, anim.scale);
    let render_pad = 8;
    let (bx, by, bw, bh) = node_marker_bounds(cx, cy, dot_half, 0, 0, dot_half * 2, render_pad);

    let mut preview_size_base = ((size.w.min(size.h) as f32) * 0.30).round() as i32;
    preview_size_base = preview_size_base.clamp(220, 360);
    let inset = 10i32;
    let source_side = bbox.size.w.max(bbox.size.h).max(1);
    let base_side = (source_side + inset * 2).clamp(120, preview_size_base);
    let preview_size = ((base_side as f32) * (0.94 + 0.06 * preview_mix))
        .round()
        .max(120.0) as i32;

    let anchor_cx = bx + (bw / 2);
    let anchor_cy = by + (bh / 2);
    let mut preview_x = anchor_cx - (preview_size / 2);
    let mut preview_y = anchor_cy - (preview_size / 2);
    preview_x = preview_x.clamp(10, (size.w - preview_size - 10).max(10));
    preview_y = preview_y.clamp(10, (size.h - preview_size - 10).max(10));

    let sx = preview_x + inset - bbox.loc.x;
    let sy = preview_y + inset - bbox.loc.y;
    let alpha = (preview_mix * preview_mix).clamp(0.0, 1.0);

    let elements =
        render_elements_from_surface_tree(renderer, wl, (sx, sy), 1.0f64, alpha, Kind::Unspecified);

    (
        Some((preview_x, preview_y, preview_size, preview_size)),
        elements,
    )
}

// ---------------------------------------------------------------------------
// Node marker drawing
// ---------------------------------------------------------------------------

pub(crate) fn draw_node_markers(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    render_nodes: &[NodeSnapshot],
    hover_node: Option<halley_core::field::NodeId>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>> {
    const NODE_ICON_FADE_DELAY_MS: u64 = 1000;
    const NODE_ICON_FADE_MS: u64 = 220;

    for NodeSnapshot {
        id,
        state: node_state,
        pos: node_pos,
        intrinsic_size,
        label: node_label,
    } in render_nodes
    {
        let id = *id;
        let node_pos = *node_pos;
        let intrinsic_size = *intrinsic_size;

        let anim = st.anim_style_for(id, node_state.clone(), now);

        if !matches!(
            node_state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) {
            continue;
        }

        let p_smooth = node_pos;

        const PROXY_TO_MARKER_START: f32 = 0.50;
        const PROXY_TO_MARKER_END: f32 = 0.20;
        let marker_mix_lin = ((PROXY_TO_MARKER_START - anim.scale)
            / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
            .clamp(0.0, 1.0);
        let marker_mix = ease_in_out_cubic(marker_mix_lin);
        let proxy_mix = 1.0 - marker_mix;

        let p = halley_core::field::Vec2 {
            x: p_smooth.x + (node_pos.x - p_smooth.x) * marker_mix,
            y: p_smooth.y + (node_pos.y - p_smooth.y) * marker_mix,
        };
        let (sx, sy) = world_to_screen(st, size.w, size.h, p.x, p.y);
        let hovered = hover_node == Some(id);
        let hover_mix = ease_in_out_cubic(st.node_label_hover_mix(id, hovered));
        let border_mix = ease_in_out_cubic(((0.304 - anim.scale) / 0.004).clamp(0.0, 1.0));
        let icon_mix = st
            .anim_track_elapsed_for(id, node_state.clone(), now)
            .map(|elapsed| {
                let elapsed_ms = elapsed.as_millis() as u64;
                let fade_t = elapsed_ms.saturating_sub(NODE_ICON_FADE_DELAY_MS) as f32
                    / NODE_ICON_FADE_MS as f32;
                ease_in_out_cubic(fade_t.clamp(0.0, 1.0))
            })
            .unwrap_or(0.0);

        let (dot_half, _, _, _) = node_marker_metrics(st, node_label.len(), anim.scale);
        let render_radius = (dot_half as f32 * 1.5).round() as i32;

        if proxy_mix > 0.01 && border_mix < 0.99 {
            let diameter = node_render_diameter_px(st, intrinsic_size, node_label.len(), anim.scale)
                .round() as i32;
            let proxy_radius = (diameter / 2).max(dot_half);
            let proxy_col = Color32F::new(0.84, 0.89, 0.95, 0.0);
            draw_shader_circle(
                frame,
                st,
                sx,
                sy,
                proxy_radius,
                1.0 - border_mix,
                proxy_col,
                proxy_col,
                damage,
            )?;
        }

        let dot_alpha = (anim.alpha * marker_mix).clamp(0.0, 1.0);
        if dot_alpha <= 0.01 {
            continue;
        }

        if border_mix > 0.01 {
            // border_frac = 3px border expressed as a fraction of the radius
            let border_frac = (3.0 / render_radius as f32).clamp(0.01, 0.5);
            let nc = node_ring_color(st, hover_mix > 0.02, 1.0);
            // node_color.rgb = border ring colour; .a = border_px / radius
            // fill_color.rgb  = node fill colour (inner fill + outer halo)
            let node_color = Color32F::new(nc.r(), nc.g(), nc.b(), border_frac);
            let fill_color = node_fill_color(st, hovered);
            draw_shader_circle(
                frame,
                st,
                sx,
                sy,
                render_radius,
                border_mix,
                node_color,
                fill_color,
                damage,
            )?;
        }

        let show_icon = match st.tuning.node_show_app_icons {
            NodeDisplayPolicy::Off => false,
            NodeDisplayPolicy::Hover => hovered,
            NodeDisplayPolicy::Always => true,
        };
        if show_icon {
            let icon_alpha = (dot_alpha * icon_mix).clamp(0.0, 1.0);
            let mut drew_real_icon = false;
            if icon_alpha > 0.01
                && let Some(app_id) = st.node_app_ids.get(&id)
                && let Some(crate::state::NodeAppIconCacheEntry::Ready(icon)) =
                    st.render_state.node_app_icon_cache.get(app_id)
            {
                let side = ((render_radius * 2) as f32 * st.tuning.node_icon_size).round() as i32;
                let side = side.clamp(16, 42);
                let dest = Rectangle::<i32, Physical>::new(
                    (sx - side / 2, sy - side / 2).into(),
                    (side, side).into(),
                );
                let src = Rectangle::<f64, Buffer>::new(
                    (0.0, 0.0).into(),
                    (icon.width as f64, icon.height as f64).into(),
                );
                frame.render_texture_from_to(
                    &icon.texture,
                    src,
                    dest,
                    &[damage],
                    &[],
                    Transform::Normal,
                    icon_alpha,
                    None,
                    &[],
                )?;
                drew_real_icon = true;
            }

            if !drew_real_icon
                && icon_alpha > 0.01
                && let Some(icon) = node_icon_glyph(st, id, node_label)
            {
                let scale = if render_radius >= 24 { 3 } else { 2 };
                let icon_text = icon.to_string();
                let (tw, th) = bitmap_text_size(&icon_text, scale);
                let text_x = sx - (tw / 2);
                let text_y = sy - (th / 2);
                draw_bitmap_text(
                    frame,
                    text_x,
                    text_y,
                    &icon_text,
                    scale,
                    Color32F::new(0.18, 0.21, 0.26, 0.92 * icon_alpha),
                    damage,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn draw_node_hover_labels(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut HalleyWlState,
    size: Size<i32, Physical>,
    render_nodes: &[NodeSnapshot],
    hover_node: Option<halley_core::field::NodeId>,
    damage: Rectangle<i32, Physical>,
    now: Instant,
) -> Result<(), Box<dyn Error>> {
    if st.tuning.node_show_labels == NodeDisplayPolicy::Off {
        return Ok(());
    }

    for node in render_nodes {
        if !matches!(
            node.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) {
            continue;
        }

        let anim = st.anim_style_for(node.id, node.state.clone(), now);
        let dot_alpha = (anim.alpha
            * ease_in_out_cubic(((0.50 - anim.scale) / (0.50 - 0.20)).clamp(0.0, 1.0)))
        .clamp(0.0, 1.0);
        if dot_alpha <= 0.01 {
            continue;
        }

        let hover_mix = match st.tuning.node_show_labels {
            NodeDisplayPolicy::Off => 0.0,
            NodeDisplayPolicy::Hover => {
                st.node_label_hover_mix(node.id, hover_node == Some(node.id))
            }
            NodeDisplayPolicy::Always => 1.0,
        };
        // cube the hover_mix so the whole animation is back-loaded — nothing
        // happens until well into the hover, then it rushes in
        let reveal_mix = ease_in_out_cubic(hover_mix * hover_mix * hover_mix);
        let label_fade = ((reveal_mix - 0.30) / 0.55).clamp(0.0, 1.0);
        if label_fade <= 0.01 {
            continue;
        }
        let label_slide = ((reveal_mix - 0.15) / 0.65).clamp(0.0, 1.0);
        let label_grow = ((reveal_mix - 0.40) / 0.55).clamp(0.0, 1.0);

        let (sx, sy) = world_to_screen(st, size.w, size.h, node.pos.x, node.pos.y);
        let (dot_half, base_label_gap, base_label_w, base_label_h) =
            node_marker_metrics(st, node.label.len(), anim.scale);
        let label_gap = ((base_label_gap as f32) * (1.0 + 0.45 * label_grow)).round() as i32;
        let label_w_target =
            ((((base_label_w as f32) * 1.80).round() as i32 + 1) & !1).clamp(72, 240);
        // Round to even so label_h / 2 centering is always exact — odd dims cause
        // a 0.5px vertical drift that steps jaggedly each animation frame.
        let label_w = ((((base_label_w as f32) * (1.0 + 0.80 * label_grow)).round() as i32 + 1)
            & !1)
            .clamp(72, 240);
        let label_h = (((base_label_h as f32) * (1.0 + 0.55 * label_grow)).round() as i32 + 1) & !1;

        let margin = 12;
        let side_gap = dot_half + label_gap.max(10);
        let prefer_left = sx + side_gap + label_w_target + margin > size.w;
        let label_x_target = if prefer_left {
            sx - side_gap - label_w
        } else {
            sx + side_gap
        };
        let label_x_start = if prefer_left {
            label_x_target + 44
        } else {
            label_x_target - 44
        };
        let label_x = ((label_x_start as f32)
            + ((label_x_target - label_x_start) as f32) * label_slide)
            .round() as i32;
        let label_y_target = sy - (label_h / 2);
        let label_y = (label_y_target as f32 + (1.0 - label_slide) * 10.0).round() as i32;

        let final_x = label_x.clamp(margin, (size.w - label_w - margin).max(margin));
        let final_y = label_y.clamp(margin, (size.h - label_h - margin).max(margin));

        let fill_color = Color32F::new(0.96, 0.98, 1.0, 1.0);
        draw_shader_label(
            frame,
            st,
            final_x,
            final_y,
            label_w.max(1),
            label_h.max(1),
            (label_h as f32) * 0.32,
            0.0,
            1.0,
            fill_color,
            fill_color,
            damage,
        )?;

        let text_scale = 2;
        let char_advance = 5 * text_scale + text_scale;
        let max_chars = ((label_w - 20).max(0) / char_advance).max(1) as usize;
        let mut text = node.label.to_ascii_uppercase();
        if text.chars().count() > max_chars {
            let keep = max_chars.saturating_sub(3);
            text = text.chars().take(keep).collect::<String>();
            text.push_str("...");
        }
        let (text_w, text_h) = bitmap_text_size(&text, text_scale);
        let text_x = final_x + ((label_w - text_w).max(0) / 2);
        let text_y = final_y + ((label_h - text_h).max(0) / 2);
        draw_bitmap_text(
            frame,
            text_x,
            text_y,
            &text,
            text_scale,
            Color32F::new(0.16, 0.18, 0.22, 0.94 * dot_alpha * label_fade),
            damage,
        )?;
    }

    Ok(())
}
