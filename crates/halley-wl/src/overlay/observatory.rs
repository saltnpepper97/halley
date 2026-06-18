//! Flat Apogee overview renderer.

use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame, gles::Uniform},
    utils::{Buffer, Logical, Physical, Rectangle, Transform},
};

use crate::compositor::overview::{ApogeePhase, ApogeeTile, ApogeeTileKind, TileRect};
use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    OverlayView, OverlayVisuals, draw_overlay_chip, draw_overlay_chip_without_shadow,
    overlay_text_color_for_fill, resolve_overlay_visuals, truncate_overlay_text_to_width,
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

fn preview_source_rect(
    overlay: &OverlayView<'_>,
    tile: &ApogeeTile,
    bbox: Rectangle<i32, Logical>,
) -> (f32, f32, f32, f32) {
    let bbox_w = bbox.size.w.max(1) as f32;
    let bbox_h = bbox.size.h.max(1) as f32;
    let Some((geo_x, geo_y, geo_w, geo_h)) = overlay
        .render_state
        .cache
        .window_geometry
        .get(&tile.node_id)
        .copied()
    else {
        return (0.0, 0.0, bbox_w, bbox_h);
    };
    if geo_w <= 0.0 || geo_h <= 0.0 {
        return (0.0, 0.0, bbox_w, bbox_h);
    }
    let left = (geo_x - bbox.loc.x as f32).clamp(0.0, bbox_w);
    let top = (geo_y - bbox.loc.y as f32).clamp(0.0, bbox_h);
    let right = (geo_x + geo_w - bbox.loc.x as f32).clamp(0.0, bbox_w);
    let bottom = (geo_y + geo_h - bbox.loc.y as f32).clamp(0.0, bbox_h);
    let w = right - left;
    let h = bottom - top;
    if w < 1.0 || h < 1.0 {
        (0.0, 0.0, bbox_w, bbox_h)
    } else {
        (left, top, w, h)
    }
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

fn draw_window_preview(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
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
    let pad = 4;
    let body = Rectangle::<i32, Physical>::new(
        (rect.loc.x + pad, rect.loc.y + pad).into(),
        (
            (rect.size.w - pad * 2).max(1),
            (rect.size.h - pad * 2).max(1),
        )
            .into(),
    );
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

    if let Some((texture, bbox)) = overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&tile.node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)))
    {
        let (src_x, src_y, src_w, src_h) = preview_source_rect(overlay, tile, bbox);
        let dst = fit_rect(body, src_w.round() as i32, src_h.round() as i32);
        let src = Rectangle::<f64, Buffer>::new(
            (src_x as f64, src_y as f64).into(),
            (src_w.max(1.0) as f64, src_h.max(1.0) as f64).into(),
        );
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
    }

    draw_tile_label(frame, overlay, visuals, tile, rect, alpha, damage)?;
    if tile.collapsed {
        draw_badge(frame, overlay, visuals, rect, alpha, damage)?;
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
    let fill = Color32F::new(0.02, 0.025, 0.035, 0.72 * alpha);
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

fn draw_badge(
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
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let fill = visuals.palette.key_fill.alpha(0.84 * alpha);
    draw_overlay_chip(
        frame,
        overlay.render_state,
        visuals,
        rect,
        rect.size.w.min(rect.size.h) as f32 * 0.5,
        fill,
        true,
        damage,
        alpha,
    )?;
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

pub(super) fn draw_observatory(
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
    if session.monitor != st.model.monitor_state.current_monitor {
        return Ok(false);
    }

    let progress = ease_in_out_cubic(session.progress(now));
    let overlay_alpha = match session.phase {
        ApogeePhase::Opening => progress,
        ApogeePhase::Open => 1.0,
        ApogeePhase::Closing => 1.0 - progress,
    };
    let tile_alpha = match session.phase {
        ApogeePhase::Opening => (0.35 + 0.65 * progress).clamp(0.0, 1.0),
        ApogeePhase::Open => 1.0,
        ApogeePhase::Closing => (1.0 - 0.65 * progress).clamp(0.0, 1.0),
    };
    let tiles = session.tiles.clone();
    let core_tiles = session.core_tiles.clone();
    let core_offset = session.core_scroll_offset;

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
        draw_core_tile(frame, &overlay, &visuals, tile, rect, tile_alpha, damage)?;
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
        draw_window_preview(frame, &overlay, &visuals, tile, rect, tile_alpha, damage)?;
    }

    Ok(true)
}
