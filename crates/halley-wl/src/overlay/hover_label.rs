use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame, gles::Uniform},
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::presentation::themed_node_label_colors;
use crate::text::{draw_ui_text, ui_text_size};

use super::{
    BANNER_EDGE_PAD, draw_overlay_chip, draw_overlay_chip_without_shadow,
    preview_source::{preview_src_uv, window_preview_source_rect},
    resolve_overlay_visuals,
    view::OverlayView,
};

pub(crate) fn draw_overlay_hover_label(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if st
        .input
        .interaction_state
        .bloom_pull_preview
        .as_ref()
        .is_some_and(|preview| preview.monitor == st.model.monitor_state.current_monitor)
    {
        return Ok(());
    }
    let Some(target) = st
        .input
        .interaction_state
        .overlay_hover_target
        .clone()
        .filter(|target| target.monitor == st.model.monitor_state.current_monitor)
    else {
        return Ok(());
    };
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let bloom_core = st
        .cluster_bloom_for_monitor(current_monitor.as_str())
        .and_then(|cid| st.model.field.cluster(cid).and_then(|cluster| cluster.core));
    if bloom_core == Some(target.node_id) {
        return Ok(());
    }
    let preview_active = st
        .ui
        .render_state
        .view
        .node_preview_hover
        .get(&target.monitor)
        .is_some_and(|state| state.node == Some(target.node_id) && state.mix > 0.0);
    if preview_active {
        return Ok(());
    }
    let Some(label) = st
        .model
        .field
        .node(target.node_id)
        .map(|node| node.label.clone())
    else {
        return Ok(());
    };
    let hover_mix = st
        .ui
        .render_state
        .node_label_hover_mix(target.node_id, true);
    let reveal_mix = crate::animation::ease_in_out_cubic(hover_mix * hover_mix * hover_mix);
    let label_fade = ((reveal_mix - 0.30) / 0.55).clamp(0.0, 1.0);
    if label_fade <= 0.01 {
        return Ok(());
    }

    let text_scale = 2;
    let mut text = label;
    let max_chars = 18usize;
    if text.chars().count() > max_chars {
        let keep = max_chars.saturating_sub(3);
        text = text.chars().take(keep).collect::<String>();
        text.push_str("...");
    }
    let (text_w, text_h) = ui_text_size(st, &text, text_scale);
    let label_w = (text_w + 24).clamp(96, 240);
    let label_h = (text_h + 18).clamp(28, 44);
    let side_gap = 18;
    let prefer_left = target.prefer_left
        || target.screen_anchor.0 + side_gap + label_w + BANNER_EDGE_PAD > screen_w;
    let label_x = if prefer_left {
        target.screen_anchor.0 - side_gap - label_w
    } else {
        target.screen_anchor.0 + side_gap
    }
    .clamp(
        BANNER_EDGE_PAD,
        (screen_w - label_w - BANNER_EDGE_PAD).max(BANNER_EDGE_PAD),
    );
    let label_y = (target.screen_anchor.1 - label_h / 2).clamp(
        BANNER_EDGE_PAD,
        (screen_h - label_h - BANNER_EDGE_PAD).max(BANNER_EDGE_PAD),
    );
    let rect =
        Rectangle::<i32, Physical>::new((label_x, label_y).into(), (label_w, label_h).into());
    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    let (label_fill, label_text) = themed_node_label_colors(
        &st.runtime.tuning,
        true,
        0.96 * label_fade,
        0.94 * label_fade,
    );

    draw_overlay_chip(
        frame,
        &st.ui.render_state,
        &visuals,
        rect,
        (label_h as f32) * 0.32,
        label_fill,
        false,
        damage,
        label_fade,
    )?;
    draw_ui_text(
        frame,
        st,
        rect.loc.x + ((rect.size.w - text_w).max(0) / 2),
        rect.loc.y + ((rect.size.h - text_h).max(0) / 2),
        &text,
        text_scale,
        label_text,
        damage,
    )?;
    Ok(())
}

/// Draws the frosted backdrop card that sits behind the live hover-preview
/// thumbnail: a Dual-Kawase backdrop blur plus a subtle translucent fill,
/// matching the rest of the overlay chips. Must be called while the overlay
/// blur context is active (it is, around `draw_hover_preview`); the blur is a
/// no-op when overlay blur is disabled, leaving just the translucent fill.
pub(crate) fn draw_overlay_hover_preview_card(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    rect: Rectangle<i32, Physical>,
    node_id: halley_core::field::NodeId,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if alpha <= 0.01 {
        return Ok(());
    }
    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    let corner_radius = (rect.size.w.min(rect.size.h) as f32 * 0.075).clamp(14.0, 22.0);
    let (fill_color, text_color) =
        themed_node_label_colors(&st.runtime.tuning, true, 0.86 * alpha, 0.94 * alpha);
    draw_overlay_chip(
        frame,
        &st.ui.render_state,
        &visuals,
        rect,
        corner_radius,
        fill_color,
        true,
        damage,
        alpha,
    )?;

    let pad = ((rect.size.w.min(rect.size.h) as f32) * 0.045).round() as i32;
    let label_h = 30.min((rect.size.h / 5).max(22));
    let body = Rectangle::<i32, Physical>::new(
        (rect.loc.x + pad, rect.loc.y + pad).into(),
        (
            (rect.size.w - pad * 2).max(1),
            (rect.size.h - pad * 2 - label_h).max(1),
        )
            .into(),
    );
    let preview_radius = (corner_radius - pad as f32).max(8.0);
    draw_cached_hover_thumbnail(frame, st, node_id, body, preview_radius, damage, alpha)?;

    if let Some(label) = st
        .model
        .field
        .node(node_id)
        .map(|node| node.label.trim().to_string())
        && !label.is_empty()
    {
        let text_scale = 2;
        let max_chars = 24usize;
        let text = if label.chars().count() > max_chars {
            let mut truncated = label
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>();
            truncated.push_str("...");
            truncated
        } else {
            label
        };
        let (text_w, text_h) = ui_text_size(st, &text, text_scale);
        let label_y = rect.loc.y + rect.size.h - pad - label_h + ((label_h - text_h).max(0) / 2);
        draw_ui_text(
            frame,
            st,
            rect.loc.x + ((rect.size.w - text_w).max(0) / 2),
            label_y,
            &text,
            text_scale,
            text_color,
            damage,
        )?;
    }

    Ok(())
}

fn draw_cached_hover_thumbnail(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    node_id: halley_core::field::NodeId,
    body: Rectangle<i32, Physical>,
    radius: f32,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let preview = st
        .ui
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)));
    let Some((texture, bbox)) = preview else {
        let visuals = resolve_overlay_visuals(&st.runtime.tuning);
        draw_overlay_chip_without_shadow(
            frame,
            &st.ui.render_state,
            &visuals,
            body,
            radius,
            Color32F::new(0.02, 0.03, 0.05, 0.76 * alpha),
            false,
            damage,
            alpha,
        )?;
        return Ok(());
    };

    let overlay = OverlayView::from_halley(st);
    let source = window_preview_source_rect(&overlay, node_id, bbox);
    let dst = aspect_fit_rect(body, source.w.round() as i32, source.h.round() as i32);
    let src = Rectangle::<f64, Buffer>::new(
        (source.x as f64, source.y as f64).into(),
        (source.w.max(1.0) as f64, source.h.max(1.0) as f64).into(),
    );
    let (src_uv_offset, src_uv_scale) = preview_src_uv(texture, source);
    let corner = radius.min(dst.size.w.min(dst.size.h) as f32 / 2.0).max(0.0);
    let program = st.ui.render_state.gpu.window_texture_program.as_ref();
    let uniforms = [
        Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
        Uniform::new("corner_radius", corner),
        Uniform::new("border_px", 0.0f32),
        Uniform::new("border_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
        Uniform::new("fill_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
        Uniform::new("content_alpha_scale", 1.0f32),
        Uniform::new("geo_offset", (0.0f32, 0.0f32)),
        Uniform::new("geo_size", (dst.size.w as f32, dst.size.h as f32)),
        Uniform::new("src_uv_offset", src_uv_offset),
        Uniform::new("src_uv_scale", src_uv_scale),
    ];
    frame.render_texture_from_to(
        texture,
        src,
        dst,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        program,
        if program.is_some() { &uniforms } else { &[] },
    )?;
    Ok(())
}

fn aspect_fit_rect(
    body: Rectangle<i32, Physical>,
    src_w: i32,
    src_h: i32,
) -> Rectangle<i32, Physical> {
    let src_w = src_w.max(1) as f32;
    let src_h = src_h.max(1) as f32;
    let scale = (body.size.w as f32 / src_w).min(body.size.h as f32 / src_h);
    let w = (src_w * scale).round().max(1.0) as i32;
    let h = (src_h * scale).round().max(1.0) as i32;
    Rectangle::<i32, Physical>::new(
        (
            body.loc.x + (body.size.w - w) / 2,
            body.loc.y + (body.size.h - h) / 2,
        )
            .into(),
        (w, h).into(),
    )
}
