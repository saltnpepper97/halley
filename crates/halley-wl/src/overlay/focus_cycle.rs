use std::{collections::HashMap, error::Error};

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame, gles::Uniform},
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::animation::ease_in_out_cubic;
use crate::compositor::interaction::state::FocusCycleSession;
use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    BANNER_EDGE_PAD, FOCUS_CYCLE_BACKDROP_ALPHA, FOCUS_CYCLE_GAP, FOCUS_CYCLE_LABEL_SCALE,
    FOCUS_CYCLE_META_SCALE, FOCUS_CYCLE_MONITOR_SCALE, FOCUS_CYCLE_VISIBLE_RADIUS, OverlayView,
    OverlayVisuals, draw_overflow_member_chip, draw_overlay_action_row, draw_overlay_chip,
    draw_overlay_chip_without_shadow, overlay_accent_fill, overlay_action_row_size,
    overlay_text_color_for_fill,
    preview_source::{preview_src_uv, window_preview_source_rect},
    resolve_overlay_visuals, truncate_overlay_text, truncate_overlay_text_to_width,
};

const FOCUS_CYCLE_OPEN_MS: u64 = 140;
const FOCUS_CYCLE_STEP_MS: u64 = 130;
const FOCUS_CYCLE_CLOSE_MS: u64 = 120;

#[derive(Clone, Copy)]
struct FocusCycleCardPose {
    slot_index: usize,
    visual_offset: f32,
    distance: f32,
    rect: Rectangle<i32, Physical>,
}

fn focus_cycle_open_progress(session: &FocusCycleSession, now: std::time::Instant) -> f32 {
    let elapsed_ms = now.saturating_duration_since(session.opened_at).as_millis() as f32;
    let t = (elapsed_ms / FOCUS_CYCLE_OPEN_MS as f32).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn focus_cycle_close_progress(session: &FocusCycleSession, now: std::time::Instant) -> f32 {
    let Some(started_at) = session.closing_started_at else {
        return 0.0;
    };
    let elapsed_ms = now.saturating_duration_since(started_at).as_millis() as f32;
    ease_in_out_cubic((elapsed_ms / FOCUS_CYCLE_CLOSE_MS as f32).clamp(0.0, 1.0))
}

fn focus_cycle_visual_index(session: &FocusCycleSession, now: std::time::Instant) -> f32 {
    let elapsed_ms = now
        .saturating_duration_since(session.step_started_at)
        .as_millis() as f32;
    let t = ease_in_out_cubic((elapsed_ms / FOCUS_CYCLE_STEP_MS as f32).clamp(0.0, 1.0));
    let visual = session.step_from_visual_index
        + (session.step_to_visual_index - session.step_from_visual_index) * t;
    if t >= 1.0 {
        session.preview_index as f32
    } else {
        visual
    }
}

fn focus_cycle_visual_offset(
    session: &FocusCycleSession,
    node_id: halley_core::field::NodeId,
    visual_index: f32,
) -> f32 {
    let Some(candidate_index) = session
        .candidates
        .iter()
        .position(|candidate| *candidate == node_id)
    else {
        return 0.0;
    };
    focus_cycle_visual_offset_for_index(session, candidate_index, visual_index)
}

fn focus_cycle_visual_offset_for_index(
    session: &FocusCycleSession,
    candidate_index: usize,
    visual_index: f32,
) -> f32 {
    let len = session.candidates.len() as f32;
    let mut index = candidate_index as f32;
    while index - visual_index > len * 0.5 {
        index -= len;
    }
    while index - visual_index < -len * 0.5 {
        index += len;
    }
    index - visual_index
}

fn focus_cycle_card_height_f(distance: f32, screen_h: i32) -> i32 {
    let center = (screen_h as f32 * 0.46).clamp(240.0, 480.0);
    let d = distance.clamp(0.0, 2.0);
    let scale = if d <= 1.0 {
        1.0 + (0.82 - 1.0) * d
    } else {
        0.82 + (0.64 - 0.82) * (d - 1.0)
    };
    (center * scale).round().max(1.0) as i32
}

/// Aspect ratio (w/h) of the node's captured preview, clamped to a sane range, or
/// a default when nothing has been captured yet (first frame / live-surface
/// fallback). Driving card width off this makes the thumbnail fill the card height
/// edge-to-edge instead of letterboxing.
fn focus_cycle_node_aspect(overlay: &OverlayView<'_>, node_id: halley_core::field::NodeId) -> f32 {
    overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| cache.bbox)
        .map(|bbox| window_preview_source_rect(overlay, node_id, bbox).aspect())
        .unwrap_or(1.6)
        .clamp(0.7, 2.0)
}

fn focus_cycle_card_size_f(
    overlay: &OverlayView<'_>,
    node_id: halley_core::field::NodeId,
    distance: f32,
    screen_h: i32,
) -> (i32, i32) {
    let h = focus_cycle_card_height_f(distance, screen_h);
    let w = (h as f32 * focus_cycle_node_aspect(overlay, node_id)).round() as i32;
    (w.max(1), h)
}

fn scale_rect_about_center(rect: Rectangle<i32, Physical>, scale: f32) -> Rectangle<i32, Physical> {
    let scale = scale.max(0.01);
    let cx = rect.loc.x as f32 + rect.size.w as f32 * 0.5;
    let cy = rect.loc.y as f32 + rect.size.h as f32 * 0.5;
    let w = (rect.size.w as f32 * scale).round().max(1.0) as i32;
    let h = (rect.size.h as f32 * scale).round().max(1.0) as i32;
    Rectangle::<i32, Physical>::new(
        (
            (cx - w as f32 * 0.5).round() as i32,
            (cy - h as f32 * 0.5).round() as i32,
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

/// Aspect-fit a `tex_w × tex_h` texture centred inside `outer`, letterboxing on
/// the card's chip fill. Returns the destination rect for `render_texture_from_to`.
fn focus_cycle_fit_rect(
    outer: Rectangle<i32, Physical>,
    tex_w: i32,
    tex_h: i32,
) -> Rectangle<i32, Physical> {
    let tex_w = tex_w.max(1) as f32;
    let tex_h = tex_h.max(1) as f32;
    let scale = (outer.size.w as f32 / tex_w)
        .min(outer.size.h as f32 / tex_h)
        .max(0.0);
    let w = (tex_w * scale).round().clamp(1.0, outer.size.w as f32) as i32;
    let h = (tex_h * scale).round().clamp(1.0, outer.size.h as f32) as i32;
    Rectangle::<i32, Physical>::new(
        (
            outer.loc.x + (outer.size.w - w) / 2,
            outer.loc.y + (outer.size.h - h) / 2,
        )
            .into(),
        (w, h).into(),
    )
}

fn focus_cycle_label(overlay: &OverlayView<'_>, node_id: halley_core::field::NodeId) -> String {
    overlay
        .field
        .node(node_id)
        .map(|node| node.label.trim())
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .or_else(|| overlay.node_app_ids.get(&node_id).cloned())
        .unwrap_or_else(|| format!("window {}", node_id.as_u64()))
}

/// Draw the window preview into `body`: the live/still offscreen texture when one
/// has been captured, otherwise the app icon (live-surface windows and the first
/// frame before capture). `radius` rounds both the dark backing and the thumbnail.
fn draw_focus_cycle_preview(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    node_id: halley_core::field::NodeId,
    body: Rectangle<i32, Physical>,
    radius: f32,
    selected: bool,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    // Opaque dark backing so aspect-fit letterbox bars read black (video-friendly)
    // rather than tinting with the card's accent fill.
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        body,
        radius,
        Color32F::new(0.02, 0.03, 0.05, 0.96 * alpha),
        false,
        damage,
        alpha,
    )?;

    let preview = overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)));

    if let Some((texture, bbox)) = preview {
        let source = window_preview_source_rect(overlay, node_id, bbox);
        let dst = focus_cycle_fit_rect(body, source.w.round() as i32, source.h.round() as i32);
        let src = Rectangle::<f64, Buffer>::new(
            (source.x as f64, source.y as f64).into(),
            (source.w.max(1.0) as f64, source.h.max(1.0) as f64).into(),
        );
        let (src_uv_offset, src_uv_scale) = preview_src_uv(texture, source);
        // Round the thumbnail corners to nest inside the chip, reusing the same
        // shader the main pass uses for offscreen window textures.
        let corner = radius.min(dst.size.w.min(dst.size.h) as f32 / 2.0).max(0.0);
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
        return Ok(());
    }

    // Fallback: a centred app-icon chip when no preview texture is available.
    let icon_size = body.size.w.min(body.size.h).clamp(40, 96);
    let icon_rect = Rectangle::<i32, Physical>::new(
        (
            body.loc.x + (body.size.w - icon_size) / 2,
            body.loc.y + (body.size.h - icon_size) / 2,
        )
            .into(),
        (icon_size, icon_size).into(),
    );
    let icon_fill = if selected {
        visuals.palette.key_fill.alpha(0.94 * alpha)
    } else {
        visuals.palette.key_fill.alpha(0.84 * alpha)
    };
    draw_overflow_member_chip(
        frame, overlay, visuals, node_id, icon_rect, icon_fill, alpha, damage,
    )
}

fn focus_cycle_preview_body_rect(
    overlay: &OverlayView<'_>,
    node_id: halley_core::field::NodeId,
    slot: Rectangle<i32, Physical>,
    pad: i32,
) -> Rectangle<i32, Physical> {
    let available = inset_rect(slot, pad);
    overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| cache.bbox)
        .map(|bbox| {
            let source = window_preview_source_rect(overlay, node_id, bbox);
            focus_cycle_fit_rect(available, source.w.round() as i32, source.h.round() as i32)
        })
        .unwrap_or(available)
}

fn draw_focus_cycle_card(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    node_id: halley_core::field::NodeId,
    monitor: &str,
    selected: bool,
    distance: i32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.01 {
        return Ok(());
    }
    let pad = if distance >= 2 { 4 } else { 6 };
    let body = focus_cycle_preview_body_rect(overlay, node_id, rect, pad);
    let rect = outset_rect(body, pad);
    let fill = if selected {
        overlay_accent_fill(visuals, 0.46, 0.99 * alpha)
    } else {
        visuals
            .palette
            .fill
            .mix(visuals.palette.border, 0.04 * (distance as f32 * 0.5))
            .alpha((0.82 - distance as f32 * 0.12).clamp(0.5, 0.82) * alpha)
    };
    let chip_radius = if selected { 20.0 } else { 18.0 };
    draw_overlay_chip(
        frame,
        overlay.render_state,
        visuals,
        rect,
        chip_radius,
        fill,
        true,
        damage,
        alpha,
    )?;

    // Rounding the preview to the chip radius minus the pad gap turns that gap into
    // a clean frame (accent on the selected card).
    let preview_radius = (chip_radius - pad as f32).max(4.0);
    draw_focus_cycle_preview(
        frame,
        overlay,
        visuals,
        node_id,
        body,
        preview_radius,
        selected,
        alpha,
        damage,
    )?;

    // Monitor badge, top-right over the preview.
    let monitor_label = truncate_overlay_text(monitor, 10);
    let (monitor_w, monitor_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
    );
    let badge_w = monitor_w + 14;
    let badge_fill =
        visuals
            .palette
            .border
            .alpha(if selected { 0.95 * alpha } else { 0.78 * alpha });
    let badge_rect = Rectangle::<i32, Physical>::new(
        (body.loc.x + body.size.w - badge_w - 8, body.loc.y + 8).into(),
        (badge_w, monitor_h + 8).into(),
    );
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        badge_rect,
        10.0,
        badge_fill,
        false,
        damage,
        alpha,
    )?;
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        badge_rect.loc.x + 7,
        badge_rect.loc.y + 4,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
        overlay_text_color_for_fill(badge_fill, alpha),
        damage,
    )?;

    // Overlaid caption band: a small app icon + title floating over the bottom of
    // the thumbnail, so the texture keeps the full card height.
    let title_scale = match distance {
        0 => FOCUS_CYCLE_LABEL_SCALE + 1,
        1 => FOCUS_CYCLE_LABEL_SCALE,
        _ => FOCUS_CYCLE_META_SCALE,
    };
    let (_, title_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        "Mg",
        title_scale,
    );
    let band_margin = 6;
    let band_h = (title_h + 10).min((body.size.h - band_margin * 2).max(1));
    let band_rect = Rectangle::<i32, Physical>::new(
        (
            body.loc.x + band_margin,
            body.loc.y + body.size.h - band_h - band_margin,
        )
            .into(),
        ((body.size.w - band_margin * 2).max(1), band_h).into(),
    );
    let band_fill = if selected {
        overlay_accent_fill(visuals, 0.30, 0.66 * alpha)
    } else {
        Color32F::new(0.0, 0.0, 0.0, 0.55 * alpha)
    };
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        band_rect,
        (band_h as f32 / 2.0).min(12.0),
        band_fill,
        false,
        damage,
        alpha,
    )?;

    let band_text_color = overlay_text_color_for_fill(band_fill, alpha);
    let mut text_x = band_rect.loc.x + 10;
    if distance < 2 {
        let icon_size = (band_h - 6).clamp(14, 30);
        let icon_rect = Rectangle::<i32, Physical>::new(
            (
                band_rect.loc.x + 5,
                band_rect.loc.y + (band_h - icon_size) / 2,
            )
                .into(),
            (icon_size, icon_size).into(),
        );
        let icon_fill =
            visuals
                .palette
                .key_fill
                .alpha(if selected { 0.94 * alpha } else { 0.84 * alpha });
        draw_overflow_member_chip(
            frame, overlay, visuals, node_id, icon_rect, icon_fill, alpha, damage,
        )?;
        text_x = icon_rect.loc.x + icon_rect.size.w + 8;
    }

    let raw_label = focus_cycle_label(overlay, node_id);
    let text_max_w = (band_rect.loc.x + band_rect.size.w - 8 - text_x).max(16);
    let label = truncate_overlay_text_to_width(
        overlay.render_state,
        &overlay.tuning.font,
        raw_label.as_str(),
        title_scale,
        text_max_w,
    );
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        text_x,
        band_rect.loc.y + (band_h - title_h) / 2,
        label.as_str(),
        title_scale,
        band_text_color,
        damage,
    )?;

    Ok(())
}

pub(super) fn draw_focus_cycle_switcher(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<bool, Box<dyn Error>> {
    let Some(session) = st.input.interaction_state.focus_cycle_session.as_ref() else {
        return Ok(false);
    };
    if session.candidates.len() < 2 {
        return Ok(false);
    }

    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    let slots = session.visible_slots(FOCUS_CYCLE_VISIBLE_RADIUS);
    let overlay = OverlayView::from_halley(st);
    let open_t = focus_cycle_open_progress(session, now);
    let close_t = focus_cycle_close_progress(session, now);
    let overlay_alpha = (open_t * (1.0 - close_t)).clamp(0.0, 1.0);
    let visual_index = focus_cycle_visual_index(session, now);
    let candidate_indices = session
        .candidates
        .iter()
        .enumerate()
        .map(|(index, node_id)| (*node_id, index))
        .collect::<HashMap<_, _>>();

    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(0.02, 0.03, 0.05, FOCUS_CYCLE_BACKDROP_ALPHA * overlay_alpha),
        damage,
    )?;

    let center_y = (screen_h as f32 * 0.5).round() as i32;
    let rail_step = (screen_w as f32 * 0.28).clamp(260.0, 440.0) + FOCUS_CYCLE_GAP as f32 * 0.5;
    let mut poses = slots
        .iter()
        .enumerate()
        .map(|(slot_index, (_, node_id))| {
            let visual_offset = candidate_indices
                .get(node_id)
                .copied()
                .map(|index| focus_cycle_visual_offset_for_index(session, index, visual_index))
                .unwrap_or_else(|| focus_cycle_visual_offset(session, *node_id, visual_index));
            let distance = visual_offset.abs().min(2.0);
            let (w, h) = focus_cycle_card_size_f(&overlay, *node_id, distance, screen_h);
            let cx = screen_w as f32 * 0.5 + visual_offset * rail_step;
            let cy = center_y as f32 + distance * 22.0 + (1.0 - open_t) * 22.0 + close_t * 18.0;
            let rect = Rectangle::<i32, Physical>::new(
                (
                    (cx - w as f32 * 0.5).round() as i32,
                    (cy - h as f32 * 0.5).round() as i32,
                )
                    .into(),
                (w, h).into(),
            );
            FocusCycleCardPose {
                slot_index,
                visual_offset,
                distance,
                rect: scale_rect_about_center(rect, 0.88 + 0.12 * open_t - 0.08 * close_t),
            }
        })
        .collect::<Vec<_>>();
    let base_h = focus_cycle_card_height_f(0.0, screen_h);

    poses.sort_by(|a, b| {
        b.distance
            .partial_cmp(&a.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.slot_index.cmp(&b.slot_index))
    });
    for pose in poses {
        let (_, node_id) = slots[pose.slot_index];
        let monitor = overlay
            .monitor_state
            .node_monitor
            .get(&node_id)
            .map(String::as_str)
            .unwrap_or("?");
        let selected = pose.visual_offset.abs() < 0.45;
        let distance = pose.distance.round().clamp(0.0, 2.0) as i32;
        draw_focus_cycle_card(
            frame,
            &overlay,
            &visuals,
            pose.rect,
            node_id,
            monitor,
            selected,
            distance,
            overlay_alpha,
            damage,
        )?;
    }

    let actions = [
        ("Tab", "next"),
        ("Shift+Tab", "previous"),
        ("Esc", "cancel"),
    ];
    let (actions_w, _actions_h) =
        overlay_action_row_size(overlay.render_state, &overlay.tuning.font, &actions);
    let actions_x = ((screen_w - actions_w) / 2).max(BANNER_EDGE_PAD);
    let actions_y = center_y + base_h / 2 + 20;
    draw_overlay_action_row(
        frame,
        overlay.render_state,
        &visuals,
        &overlay.tuning.font,
        actions_x,
        actions_y,
        &actions,
        damage,
        0.96 * overlay_alpha,
    )?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycle_chrome_can_match_fitted_texture_inside_wide_slot() {
        let slot = Rectangle::<i32, Physical>::new((0, 0).into(), (600, 360).into());
        let available = inset_rect(slot, 6);
        let body = focus_cycle_fit_rect(available, 320, 240);
        let chrome = outset_rect(body, 6);

        assert!(chrome.size.w < slot.size.w);
        assert_eq!(body.size.w * 3, body.size.h * 4);
        assert_eq!(chrome.size.w, body.size.w + 12);
        assert_eq!(chrome.size.h, body.size.h + 12);
    }
}
