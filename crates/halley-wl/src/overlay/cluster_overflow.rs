use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame},
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::render::{node_app_icon_fallback_glyph, node_app_icon_texture_allowed};
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    OVERFLOW_ICON_GAP, OVERFLOW_ICON_PAD, OVERFLOW_ICON_SIZE, OVERFLOW_REVEAL_ANIM_MS,
    OVERFLOW_REVEAL_SLIDE_PX, OVERFLOW_SCROLLBAR_PAD, OVERFLOW_SCROLLBAR_W, OVERFLOW_VISIBLE_SLOTS,
    OverlayView, OverlayVisuals, draw_overlay_chip, draw_overlay_chip_without_shadow,
    overlay_text_color_for_fill, resolve_overlay_visuals,
};

fn cluster_overflow_visibility_mix(overlay: &OverlayView<'_>, monitor: &str, now_ms: u64) -> f32 {
    let started_at_ms = overlay
        .cluster_state
        .cluster_overflow_reveal_started_at_ms
        .get(monitor)
        .copied()
        .unwrap_or(now_ms.saturating_sub(OVERFLOW_REVEAL_ANIM_MS));
    let visible_until_ms = overlay
        .cluster_state
        .cluster_overflow_visible_until_ms
        .get(monitor)
        .copied()
        .unwrap_or(now_ms);
    let intro_t = (now_ms.saturating_sub(started_at_ms) as f32
        / OVERFLOW_REVEAL_ANIM_MS.max(1) as f32)
        .clamp(0.0, 1.0);
    let outro_t = (visible_until_ms.saturating_sub(now_ms) as f32
        / OVERFLOW_REVEAL_ANIM_MS.max(1) as f32)
        .clamp(0.0, 1.0);
    let intro = intro_t * intro_t * (3.0 - 2.0 * intro_t);
    let outro = outro_t * outro_t * (3.0 - 2.0 * outro_t);
    intro.min(outro)
}

fn cluster_overflow_strip_rect(
    overlay: &OverlayView<'_>,
    monitor: &str,
    now_ms: u64,
) -> Option<Rectangle<i32, Physical>> {
    if !overlay.cluster_overflow_visible_for_monitor(monitor, now_ms) {
        return None;
    }
    let rect = overlay.cluster_overflow_rect_for_monitor(monitor)?;
    let visibility_mix = cluster_overflow_visibility_mix(overlay, monitor, now_ms);
    let slide_x = ((1.0 - visibility_mix) * OVERFLOW_REVEAL_SLIDE_PX as f32).round() as i32;
    Some(Rectangle::<i32, Physical>::new(
        (rect.x.round() as i32 + slide_x, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(48),
            (rect.h.round() as i32).max(1),
        )
            .into(),
    ))
}

fn cluster_overflow_scrollbar_metrics(
    overlay: &OverlayView<'_>,
    monitor: &str,
    strip: Rectangle<i32, Physical>,
) -> Option<(Rectangle<i32, Physical>, Rectangle<i32, Physical>, usize)> {
    let overflow_len = overlay
        .cluster_overflow_member_ids_for_monitor(monitor)
        .len();
    if overflow_len <= OVERFLOW_VISIBLE_SLOTS {
        return None;
    }
    let max_offset = overflow_len.saturating_sub(OVERFLOW_VISIBLE_SLOTS);
    let scroll_offset = overlay
        .cluster_overflow_scroll_offset_for_monitor(monitor)
        .min(max_offset);
    let track_x = strip.loc.x + strip.size.w - OVERFLOW_SCROLLBAR_PAD - OVERFLOW_SCROLLBAR_W;
    let track_y = strip.loc.y + OVERFLOW_ICON_PAD;
    let track_h = strip.size.h - OVERFLOW_ICON_PAD * 2;
    let track = Rectangle::<i32, Physical>::new(
        (track_x, track_y).into(),
        (OVERFLOW_SCROLLBAR_W, track_h.max(8)).into(),
    );
    let thumb_h = ((OVERFLOW_VISIBLE_SLOTS as f32 / overflow_len as f32) * track.size.h as f32)
        .round() as i32;
    let thumb_h = thumb_h.clamp(18, track.size.h.max(18));
    let thumb_travel = (track.size.h - thumb_h).max(0);
    let thumb_y = if max_offset == 0 {
        track.loc.y
    } else {
        track.loc.y
            + ((scroll_offset as f32 / max_offset as f32) * thumb_travel as f32).round() as i32
    };
    let thumb = Rectangle::<i32, Physical>::new(
        (track.loc.x, thumb_y).into(),
        (track.size.w, thumb_h).into(),
    );
    Some((track, thumb, scroll_offset))
}

pub(super) fn draw_overflow_member_chip(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    node_id: halley_core::field::NodeId,
    icon_rect: Rectangle<i32, Physical>,
    chip_fill: Color32F,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        icon_rect,
        12.0,
        chip_fill,
        false,
        damage,
        alpha,
    )?;
    if overlay.tuning.tile_queue_show_icons
        && node_app_icon_texture_allowed(overlay.tuning.node_show_app_icons, false)
        && let Some(crate::render::state::NodeAppIconCacheEntry::Ready(icon)) =
            overlay.node_app_icon_entry(node_id)
    {
        let icon_dest = Rectangle::<i32, Physical>::new(
            (icon_rect.loc.x + 4, icon_rect.loc.y + 4).into(),
            (icon_rect.size.w - 8, icon_rect.size.h - 8).into(),
        );
        let icon_src = Rectangle::<f64, Buffer>::new(
            (0.0, 0.0).into(),
            (icon.width as f64, icon.height as f64).into(),
        );
        frame.render_texture_from_to(
            &icon.texture,
            icon_src,
            icon_dest,
            &[damage],
            &[],
            Transform::Normal,
            alpha,
            None,
            &[],
        )?;
        return Ok(());
    }
    let glyph = node_app_icon_fallback_glyph(
        overlay.node_app_ids.get(&node_id).map(String::as_str),
        overlay
            .field
            .node(node_id)
            .map(|n| n.label.as_str())
            .unwrap_or("?"),
    )
    .to_string();
    let (text_w, text_h) = ui_text_size_in(overlay.render_state, &overlay.tuning.font, &glyph, 2);
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        icon_rect.loc.x + (icon_rect.size.w - text_w) / 2,
        icon_rect.loc.y + (icon_rect.size.h - text_h) / 2,
        &glyph,
        2,
        overlay_text_color_for_fill(chip_fill, alpha),
        damage,
    )?;
    Ok(())
}

pub(crate) fn draw_cluster_overflow_promotion(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    monitor: &str,
    damage: Rectangle<i32, Physical>,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let Some(anim) = overlay.cluster_overflow_promotion_anim_for_monitor(monitor) else {
        return Ok(());
    };
    if now_ms >= anim.reveal_at_ms {
        return Ok(());
    }
    let visuals = resolve_overlay_visuals(overlay.tuning);
    let duration_ms = anim.reveal_at_ms.saturating_sub(anim.started_at_ms).max(1);
    let t =
        ((now_ms.saturating_sub(anim.started_at_ms)) as f32 / duration_ms as f32).clamp(0.0, 1.0);
    let e = t * t * (3.0 - 2.0 * t);
    let center_x = anim.source_center.x + (anim.target_center.x - anim.source_center.x) * e;
    let center_y = anim.source_center.y + (anim.target_center.y - anim.source_center.y) * e;
    let icon_size = (OVERFLOW_ICON_SIZE as f32 * (1.04 - 0.04 * e)).round() as i32;
    let icon_rect = Rectangle::<i32, Physical>::new(
        (
            center_x.round() as i32 - icon_size / 2,
            center_y.round() as i32 - icon_size / 2,
        )
            .into(),
        (icon_size.max(1), icon_size.max(1)).into(),
    );
    let queue_visible = overlay.cluster_overflow_visible_for_monitor(monitor, now_ms)
        && !overlay
            .cluster_overflow_member_ids_for_monitor(monitor)
            .is_empty();
    if !queue_visible {
        let strip = Rectangle::<i32, Physical>::new(
            (
                anim.source_strip_rect.x.round() as i32,
                anim.source_strip_rect.y.round() as i32,
            )
                .into(),
            (
                anim.source_strip_rect.w.round() as i32,
                anim.source_strip_rect.h.round() as i32,
            )
                .into(),
        );
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            strip,
            18.0,
            visuals.palette.fill.alpha(0.90 * (1.0 - e)),
            true,
            damage,
            (1.0 - e * 0.65).clamp(0.0, 1.0),
        )?;
    }
    draw_overflow_member_chip(
        frame,
        overlay,
        &visuals,
        anim.member_id,
        icon_rect,
        visuals.palette.border.alpha(0.97),
        1.0,
        damage,
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct OverflowStripHit {
    pub(crate) member_id: halley_core::field::NodeId,
}

pub(crate) fn cluster_overflow_icon_hit_test(
    overlay: &OverlayView<'_>,
    monitor: &str,
    sx: f32,
    sy: f32,
    now_ms: u64,
) -> Option<OverflowStripHit> {
    let strip = cluster_overflow_strip_rect(overlay, monitor, now_ms)?;
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return None;
    }
    let scroll_offset = overlay.cluster_overflow_scroll_offset_for_monitor(monitor);
    let visible_slots = ((strip.size.h - OVERFLOW_ICON_PAD * 2 + OVERFLOW_ICON_GAP)
        / (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP))
        .max(1) as usize;

    overflow
        .iter()
        .copied()
        .skip(scroll_offset)
        .take(visible_slots)
        .enumerate()
        .find_map(|(index, node_id)| {
            let icon_rect = Rectangle::<i32, Physical>::new(
                (
                    strip.loc.x + (strip.size.w - OVERFLOW_ICON_SIZE) / 2,
                    strip.loc.y
                        + OVERFLOW_ICON_PAD
                        + index as i32 * (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP),
                )
                    .into(),
                (OVERFLOW_ICON_SIZE, OVERFLOW_ICON_SIZE).into(),
            );
            ((sx.round() as i32) >= icon_rect.loc.x
                && (sx.round() as i32) <= icon_rect.loc.x + icon_rect.size.w
                && (sy.round() as i32) >= icon_rect.loc.y
                && (sy.round() as i32) <= icon_rect.loc.y + icon_rect.size.h)
                .then_some(OverflowStripHit { member_id: node_id })
        })
}

pub(crate) fn cluster_overflow_strip_slot_at(
    overlay: &OverlayView<'_>,
    monitor: &str,
    sx: f32,
    sy: f32,
    now_ms: u64,
) -> Option<usize> {
    let strip = cluster_overflow_strip_rect(overlay, monitor, now_ms)?;
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return None;
    }
    let scroll_offset = overlay.cluster_overflow_scroll_offset_for_monitor(monitor);
    if sx < strip.loc.x as f32
        || sx > (strip.loc.x + strip.size.w) as f32
        || sy < strip.loc.y as f32
        || sy > (strip.loc.y + strip.size.h) as f32
    {
        return None;
    }
    let relative_y = (sy.round() as i32 - strip.loc.y - OVERFLOW_ICON_PAD).max(0);
    let slot_pitch = (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP).max(1);
    Some((scroll_offset + (relative_y / slot_pitch) as usize).min(overflow.len().saturating_sub(1)))
}

pub(crate) fn draw_cluster_overflow_strip(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    monitor: &str,
    damage: Rectangle<i32, Physical>,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let visuals = resolve_overlay_visuals(overlay.tuning);
    let Some(strip) = cluster_overflow_strip_rect(overlay, monitor, now_ms) else {
        return Ok(());
    };
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return Ok(());
    }
    let visibility_mix = cluster_overflow_visibility_mix(overlay, monitor, now_ms);
    let reveal_alpha = (0.45 + 0.55 * visibility_mix).clamp(0.0, 1.0);
    let (scrollbar_track, scrollbar_thumb, scroll_offset) =
        cluster_overflow_scrollbar_metrics(overlay, monitor, strip)
            .map(|(track, thumb, offset)| (Some(track), Some(thumb), offset))
            .unwrap_or((None, None, 0));
    let dragging_member = overlay
        .cluster_overflow_drag_preview_for_monitor(monitor)
        .map(|(member_id, _)| member_id);
    draw_overlay_chip(
        frame,
        overlay.render_state,
        &visuals,
        strip,
        18.0,
        visuals.palette.fill.alpha(0.97 * reveal_alpha),
        true,
        damage,
        reveal_alpha,
    )?;

    let visible_slots = ((strip.size.h - OVERFLOW_ICON_PAD * 2 + OVERFLOW_ICON_GAP)
        / (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP))
        .max(1) as usize;
    for (index, node_id) in overflow
        .iter()
        .copied()
        .filter(|node_id| Some(*node_id) != dragging_member)
        .skip(scroll_offset)
        .take(visible_slots)
        .enumerate()
    {
        let icon_x = strip.loc.x
            + (strip.size.w
                - OVERFLOW_ICON_SIZE
                - if scrollbar_track.is_some() {
                    OVERFLOW_SCROLLBAR_W + OVERFLOW_SCROLLBAR_PAD + 2
                } else {
                    0
                })
                / 2;
        let icon_rect = Rectangle::<i32, Physical>::new(
            (
                icon_x,
                strip.loc.y
                    + OVERFLOW_ICON_PAD
                    + index as i32 * (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP),
            )
                .into(),
            (OVERFLOW_ICON_SIZE, OVERFLOW_ICON_SIZE).into(),
        );
        draw_overflow_member_chip(
            frame,
            overlay,
            &visuals,
            node_id,
            icon_rect,
            visuals.palette.border.alpha(1.0),
            reveal_alpha,
            damage,
        )?;
    }

    if let (Some(track), Some(thumb)) = (scrollbar_track, scrollbar_thumb) {
        draw_overlay_chip_without_shadow(
            frame,
            overlay.render_state,
            &visuals,
            track,
            4.0,
            visuals.palette.key_fill.alpha(0.30 * reveal_alpha),
            false,
            damage,
            reveal_alpha,
        )?;
        draw_overlay_chip_without_shadow(
            frame,
            overlay.render_state,
            &visuals,
            thumb,
            4.0,
            visuals.palette.subtext.alpha(0.72 * reveal_alpha),
            false,
            damage,
            reveal_alpha,
        )?;
    }

    if let Some((node_id, (sx, sy))) = overlay.cluster_overflow_drag_preview_for_monitor(monitor) {
        let icon_rect = Rectangle::<i32, Physical>::new(
            (
                sx.round() as i32 - OVERFLOW_ICON_SIZE / 2,
                sy.round() as i32 - OVERFLOW_ICON_SIZE / 2,
            )
                .into(),
            (OVERFLOW_ICON_SIZE, OVERFLOW_ICON_SIZE).into(),
        );
        draw_overflow_member_chip(
            frame,
            overlay,
            &visuals,
            node_id,
            icon_rect,
            visuals.palette.border.alpha(0.97),
            1.0,
            damage,
        )?;
    }

    Ok(())
}
