//! Flat Apogee overview renderer.

use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame, gles::Uniform},
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::overview::{ApogeePhase, ApogeeTile, ApogeeTileKind, TileRect};
use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    OverlayView, OverlayVisuals, draw_overflow_member_chip, draw_overlay_chip,
    draw_overlay_chip_with_border_color, draw_overlay_chip_without_shadow, draw_overlay_ring,
    overlay_accent_fill, overlay_text_color_for_fill,
    preview_source::{preview_src_uv, window_preview_source_rect},
    resolve_overlay_visuals, truncate_overlay_text_to_width,
};

#[inline]
fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - (f * f * f) / 2.0
    }
}

fn tile_screen_rect(rect: TileRect) -> Rectangle<i32, Physical> {
    Rectangle::<i32, Physical>::new(
        (
            (rect.cx - rect.w * 0.5).round() as i32,
            (rect.cy - rect.h * 0.5).round() as i32,
        )
            .into(),
        (
            rect.w.round().max(1.0) as i32,
            rect.h.round().max(1.0) as i32,
        )
            .into(),
    )
}

fn fit_rect(outer: Rectangle<i32, Physical>, src_w: i32, src_h: i32) -> Rectangle<i32, Physical> {
    let src_w = src_w.max(1) as f32;
    let src_h = src_h.max(1) as f32;
    let scale = (outer.size.w as f32 / src_w)
        .min(outer.size.h as f32 / src_h)
        .max(0.0);
    let w = (src_w * scale).round().clamp(1.0, outer.size.w as f32) as i32;
    let h = (src_h * scale).round().clamp(1.0, outer.size.h as f32) as i32;
    Rectangle::<i32, Physical>::new(
        (
            outer.loc.x + (outer.size.w - w) / 2,
            outer.loc.y + (outer.size.h - h) / 2,
        )
            .into(),
        (w, h).into(),
    )
}

fn inset_rect(rect: Rectangle<i32, Physical>, pad: i32) -> Rectangle<i32, Physical> {
    Rectangle::<i32, Physical>::new(
        (rect.loc.x + pad, rect.loc.y + pad).into(),
        (
            (rect.size.w - pad * 2).max(1),
            (rect.size.h - pad * 2).max(1),
        )
            .into(),
    )
}

fn outset_rect(rect: Rectangle<i32, Physical>, pad: i32) -> Rectangle<i32, Physical> {
    Rectangle::<i32, Physical>::new(
        (rect.loc.x - pad, rect.loc.y - pad).into(),
        (rect.size.w + pad * 2, rect.size.h + pad * 2).into(),
    )
}

fn preview_body_rect(
    overlay: &OverlayView<'_>,
    tile: &ApogeeTile,
    slot: Rectangle<i32, Physical>,
    pad: i32,
) -> Rectangle<i32, Physical> {
    let available = inset_rect(slot, pad);
    overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&tile.node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| cache.bbox)
        .map(|bbox| {
            let source = window_preview_source_rect(overlay, tile.node_id, bbox);
            fit_rect(available, source.w.round() as i32, source.h.round() as i32)
        })
        .unwrap_or(available)
}

fn draw_window_preview(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    hovered: bool,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let pad = 4;
    let body = preview_body_rect(overlay, tile, rect, pad);
    let rect = outset_rect(body, pad);
    let radius = 16.0_f32
        .min(rect.size.w.min(rect.size.h) as f32 * 0.5)
        .max(0.0);
    draw_overlay_chip(
        frame,
        overlay.render_state,
        visuals,
        rect,
        radius,
        visuals.palette.fill.alpha(0.78 * alpha),
        true,
        damage,
        alpha,
    )?;
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        body,
        (radius - pad as f32).max(4.0),
        Color32F::new(0.02, 0.03, 0.05, 0.96 * alpha),
        false,
        damage,
        alpha,
    )?;

    // Fullscreen windows capture to a black/unusable texture (video, keybind
    // fullscreen), so skip the preview and show the app icon instead. Non-fullscreen
    // windows with no captured texture yet also fall back to the icon.
    let preview = (!overlay.node_is_fullscreen(tile.node_id))
        .then(|| {
            overlay
                .render_state
                .cache
                .window_offscreen_cache
                .get(&tile.node_id)
                .filter(|cache| cache.has_content)
                .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)))
        })
        .flatten();

    if let Some((texture, bbox)) = preview {
        let source = window_preview_source_rect(overlay, tile.node_id, bbox);
        let dst = fit_rect(body, source.w.round() as i32, source.h.round() as i32);
        let src = Rectangle::<f64, Buffer>::new(
            (source.x as f64, source.y as f64).into(),
            (source.w.max(1.0) as f64, source.h.max(1.0) as f64).into(),
        );
        let (src_uv_offset, src_uv_scale) = preview_src_uv(texture, source);
        let corner = (radius - pad as f32)
            .max(0.0)
            .min(dst.size.w.min(dst.size.h) as f32 * 0.5);
        let program = overlay.render_state.gpu.window_texture_program.as_ref();
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
            alpha,
            program,
            if program.is_some() { &uniforms } else { &[] },
        )?;
    } else {
        // Centred app-icon chip when there is no usable preview texture.
        let icon_size = body.size.w.min(body.size.h).clamp(40, 96);
        let icon_rect = Rectangle::<i32, Physical>::new(
            (
                body.loc.x + (body.size.w - icon_size) / 2,
                body.loc.y + (body.size.h - icon_size) / 2,
            )
                .into(),
            (icon_size, icon_size).into(),
        );
        let icon_fill = visuals.palette.key_fill.alpha(0.84 * alpha);
        draw_overflow_member_chip(
            frame,
            overlay,
            visuals,
            tile.node_id,
            icon_rect,
            icon_fill,
            alpha,
            damage,
        )?;
    }

    draw_tile_label(frame, overlay, visuals, tile, rect, alpha, hovered, damage)?;
    if tile.collapsed {
        draw_badge(frame, overlay, visuals, rect, alpha, damage)?;
    }
    if hovered {
        let gap = 5;
        let ring = outset_rect(rect, gap);
        let ring_radius = radius + gap as f32;
        let ring_px = visuals.border_px.max(2.0);
        draw_overlay_ring(
            frame,
            overlay.render_state,
            visuals,
            ring,
            ring_radius,
            visuals.palette.border.alpha(alpha),
            ring_px,
            damage,
            alpha,
        )?;
    }
    Ok(())
}

fn tile_label(overlay: &OverlayView<'_>, tile: &ApogeeTile) -> String {
    overlay
        .field
        .node(tile.node_id)
        .map(|node| node.label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .or_else(|| overlay.node_app_ids.get(&tile.node_id).cloned())
        .unwrap_or_else(|| format!("window {}", tile.node_id.as_u64()))
}

fn draw_tile_label(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    hovered: bool,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if rect.size.w < 96 || rect.size.h < 72 {
        return Ok(());
    }
    let label_h = ((rect.size.h as f32) * 0.13).round() as i32;
    let label_h = label_h.clamp(22, 34);
    let label_rect = Rectangle::<i32, Physical>::new(
        (rect.loc.x + 8, rect.loc.y + rect.size.h - 8 - label_h).into(),
        ((rect.size.w - 16).max(1), label_h).into(),
    );
    let fill = if hovered {
        overlay_accent_fill(visuals, 0.55, 0.88 * alpha)
    } else {
        Color32F::new(0.02, 0.025, 0.035, 0.72 * alpha)
    };
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        label_rect,
        8.0,
        fill,
        false,
        damage,
        alpha,
    )?;
    let label = truncate_overlay_text_to_width(
        overlay.render_state,
        &overlay.tuning.font,
        tile_label(overlay, tile).as_str(),
        1,
        label_rect.size.w - 16,
    );
    let (text_w, text_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        label.as_str(),
        1,
    );
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        label_rect.loc.x + ((label_rect.size.w - text_w).max(0) / 2),
        label_rect.loc.y + ((label_rect.size.h - text_h).max(0) / 2),
        label.as_str(),
        1,
        overlay_text_color_for_fill(fill, alpha),
        damage,
    )?;
    Ok(())
}

pub(super) fn draw_badge(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if rect.size.w < 104 || rect.size.h < 78 {
        return Ok(());
    }
    let badge =
        Rectangle::<i32, Physical>::new((rect.loc.x + 8, rect.loc.y + 8).into(), (58, 24).into());
    let fill = visuals.palette.key_fill.alpha(0.92 * alpha);
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        badge,
        8.0,
        fill,
        false,
        damage,
        alpha,
    )?;
    let (text_w, text_h) = ui_text_size_in(overlay.render_state, &overlay.tuning.font, "NODE", 1);
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        badge.loc.x + (badge.size.w - text_w) / 2,
        badge.loc.y + (badge.size.h - text_h) / 2,
        "NODE",
        1,
        overlay_text_color_for_fill(fill, alpha),
        damage,
    )?;
    Ok(())
}

fn draw_core_tile(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    hovered: bool,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let fill = crate::presentation::themed_node_fill_color(&st.runtime.tuning, hovered);
    let border_color =
        crate::presentation::themed_node_ring_color(&st.runtime.tuning, hovered, 1.0);
    let core_visuals = OverlayVisuals {
        border_px: 5.0,
        ..*visuals
    };
    draw_overlay_chip_with_border_color(
        frame,
        overlay.render_state,
        &core_visuals,
        rect,
        rect.size.w.min(rect.size.h) as f32 * 0.5,
        fill,
        border_color,
        true,
        damage,
        alpha,
    )?;
    if let Some(icon) = crate::render::cluster_core_icon_texture(st, false) {
        let side = (rect.size.w.min(rect.size.h) as f32 * 0.62).round() as i32;
        let side = side.clamp(20, rect.size.w.min(rect.size.h).max(1));
        let dest = Rectangle::<i32, Physical>::new(
            (
                rect.loc.x + (rect.size.w - side) / 2,
                rect.loc.y + (rect.size.h - side) / 2,
            )
                .into(),
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
            alpha,
            None,
            &[],
        )?;
        return Ok(());
    }
    let label = tile_label(overlay, tile);
    let label = truncate_overlay_text_to_width(
        overlay.render_state,
        &overlay.tuning.font,
        label.as_str(),
        1,
        rect.size.w - 10,
    );
    let (text_w, text_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        label.as_str(),
        1,
    );
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        rect.loc.x + ((rect.size.w - text_w).max(0) / 2),
        rect.loc.y + ((rect.size.h - text_h).max(0) / 2),
        label.as_str(),
        1,
        overlay_text_color_for_fill(fill, alpha),
        damage,
    )?;
    Ok(())
}

pub(crate) fn draw_observatory(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<bool, Box<dyn Error>> {
    let Some(session) = st.input.interaction_state.apogee_session.as_ref() else {
        return Ok(false);
    };
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let Some(monitor_session) = session.monitor_session(current_monitor.as_str()) else {
        return Ok(false);
    };

    let progress = ease_in_out_cubic(session.progress(now));
    let overlay_alpha = match session.phase {
        ApogeePhase::Opening => progress,
        ApogeePhase::Open => 1.0,
        ApogeePhase::Closing => 1.0 - progress,
    };
    let tile_alpha = match session.phase {
        ApogeePhase::Opening => (0.35 + 0.65 * progress).clamp(0.0, 1.0),
        ApogeePhase::Open => 1.0,
        ApogeePhase::Closing => 1.0,
    };
    let tiles = monitor_session.tiles.clone();
    let core_tiles = monitor_session.core_tiles.clone();
    let core_offset = monitor_session.core_scroll_offset;
    let phase_open = matches!(session.phase, ApogeePhase::Open);
    let hovered_node = st.input.interaction_state.apogee_live_preview_node;
    let hovered_overlay_node = st.input.interaction_state.apogee_hover_node;

    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    let overlay = OverlayView::from_halley(st);
    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(
            0.02,
            0.03,
            0.05,
            st.runtime.tuning.apogee.background_dim * overlay_alpha,
        ),
        damage,
    )?;

    for tile in &core_tiles {
        let mut target = tile.to;
        target.cx -= core_offset;
        let rect = tile_screen_rect(tile.from.lerp(target, progress));
        if rect.loc.x > screen_w || rect.loc.x + rect.size.w < 0 {
            continue;
        }
        let hovered = phase_open && hovered_overlay_node == Some(tile.node_id);
        draw_core_tile(
            frame, &*st, &overlay, &visuals, tile, rect, tile_alpha, hovered, damage,
        )?;
    }

    for tile in &tiles {
        if !matches!(tile.kind, ApogeeTileKind::Window) {
            continue;
        }
        let rect = tile_screen_rect(tile.from.lerp(tile.to, progress));
        if rect.loc.x > screen_w
            || rect.loc.x + rect.size.w < 0
            || rect.loc.y > screen_h
            || rect.loc.y + rect.size.h < 0
        {
            continue;
        }
        let hovered = phase_open && hovered_node == Some(tile.node_id);
        draw_window_preview(
            frame, &overlay, &visuals, tile, rect, tile_alpha, hovered, damage,
        )?;
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apogee_chrome_can_match_fitted_texture_inside_wide_slot() {
        let slot = Rectangle::<i32, Physical>::new((0, 0).into(), (600, 360).into());
        let available = inset_rect(slot, 4);
        let body = fit_rect(available, 320, 240);
        let chrome = outset_rect(body, 4);

        assert!(chrome.size.w < slot.size.w);
        assert!((body.size.w * 3 - body.size.h * 4).abs() <= 1);
        assert_eq!(chrome.size.w, body.size.w + 8);
        assert_eq!(chrome.size.h, body.size.h + 8);
    }
}
