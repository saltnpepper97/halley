use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame, gles::Uniform},
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    BANNER_EDGE_PAD, FOCUS_CYCLE_BACKDROP_ALPHA, FOCUS_CYCLE_GAP,
    FOCUS_CYCLE_LABEL_SCALE, FOCUS_CYCLE_META_SCALE, FOCUS_CYCLE_MONITOR_SCALE,
    FOCUS_CYCLE_VISIBLE_RADIUS, OverlayView, OverlayVisuals, draw_overflow_member_chip,
    draw_overlay_action_row, draw_overlay_chip, draw_overlay_chip_without_shadow,
    overlay_accent_fill, overlay_action_row_size, overlay_text_color_for_fill,
    resolve_overlay_visuals, truncate_overlay_text, truncate_overlay_text_to_width,
};

fn focus_cycle_card_height(distance: i32, screen_h: i32) -> i32 {
    // Hero-sized thumbnails scaled to the output: the centered card is ~45% of the
    // screen height (capped), with neighbours stepped down for depth.
    let center = (screen_h as f32 * 0.46).clamp(240.0, 480.0);
    let scale = match distance {
        0 => 1.0,
        1 => 0.82,
        _ => 0.64,
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
        .map(|bbox| bbox.size.w.max(1) as f32 / bbox.size.h.max(1) as f32)
        .unwrap_or(1.6)
        .clamp(0.7, 2.0)
}

fn focus_cycle_card_size(
    overlay: &OverlayView<'_>,
    node_id: halley_core::field::NodeId,
    distance: i32,
    screen_h: i32,
) -> (i32, i32) {
    let h = focus_cycle_card_height(distance, screen_h);
    let w = (h as f32 * focus_cycle_node_aspect(overlay, node_id)).round() as i32;
    (w.max(1), h)
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
        Color32F::new(0.02, 0.03, 0.05, 0.96),
        false,
        damage,
        1.0,
    )?;

    let preview = overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)));

    if let Some((texture, bbox)) = preview {
        let dst = focus_cycle_fit_rect(body, bbox.size.w, bbox.size.h);
        let src = Rectangle::<f64, Buffer>::new(
            (0.0, 0.0).into(),
            (bbox.size.w.max(1) as f64, bbox.size.h.max(1) as f64).into(),
        );
        // Round the thumbnail corners to nest inside the chip, reusing the same
        // shader the main pass uses for offscreen window textures.
        let corner = radius
            .min(dst.size.w.min(dst.size.h) as f32 / 2.0)
            .max(0.0);
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
            1.0,
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
        visuals.palette.key_fill.alpha(0.94)
    } else {
        visuals.palette.key_fill.alpha(0.84)
    };
    draw_overflow_member_chip(
        frame, overlay, visuals, node_id, icon_rect, icon_fill, 1.0, damage,
    )
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
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let fill = if selected {
        overlay_accent_fill(visuals, 0.46, 0.99)
    } else {
        visuals
            .palette
            .fill
            .mix(visuals.palette.border, 0.04 * (distance as f32 * 0.5))
            .alpha((0.82 - distance as f32 * 0.12).clamp(0.5, 0.82))
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
        1.0,
    )?;

    // The preview fills the whole card (full height); chrome is overlaid on top.
    let pad = if distance >= 2 { 4 } else { 6 };
    let body = Rectangle::<i32, Physical>::new(
        (rect.loc.x + pad, rect.loc.y + pad).into(),
        (
            (rect.size.w - pad * 2).max(1),
            (rect.size.h - pad * 2).max(1),
        )
            .into(),
    );
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
    let badge_fill = visuals
        .palette
        .border
        .alpha(if selected { 0.95 } else { 0.78 });
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
        1.0,
    )?;
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        badge_rect.loc.x + 7,
        badge_rect.loc.y + 4,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
        overlay_text_color_for_fill(badge_fill, 1.0),
        damage,
    )?;

    // Overlaid caption band: a small app icon + title floating over the bottom of
    // the thumbnail, so the texture keeps the full card height.
    let title_scale = match distance {
        0 => FOCUS_CYCLE_LABEL_SCALE + 1,
        1 => FOCUS_CYCLE_LABEL_SCALE,
        _ => FOCUS_CYCLE_META_SCALE,
    };
    let (_, title_h) =
        ui_text_size_in(overlay.render_state, &overlay.tuning.font, "Mg", title_scale);
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
        overlay_accent_fill(visuals, 0.30, 0.66)
    } else {
        Color32F::new(0.0, 0.0, 0.0, 0.55)
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
        1.0,
    )?;

    let band_text_color = overlay_text_color_for_fill(band_fill, 1.0);
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
        let icon_fill = visuals
            .palette
            .key_fill
            .alpha(if selected { 0.94 } else { 0.84 });
        draw_overflow_member_chip(
            frame, overlay, visuals, node_id, icon_rect, icon_fill, 1.0, damage,
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

    let sizes = slots
        .iter()
        .map(|(offset, node_id)| focus_cycle_card_size(&overlay, *node_id, offset.abs(), screen_h))
        .collect::<Vec<_>>();
    let base_h = sizes.iter().map(|(_, h)| *h).max().unwrap_or(0);

    // Cumulative x of each card with the row starting at 0, then shift so the
    // selected card is dead-center; neighbours flank it and may clip at the edges.
    let mut card_x = Vec::with_capacity(sizes.len());
    let mut acc = 0;
    for (w, _) in &sizes {
        card_x.push(acc);
        acc += w + FOCUS_CYCLE_GAP;
    }
    let selected_slot = slots.iter().position(|(offset, _)| *offset == 0).unwrap_or(0);
    let selected_center = card_x[selected_slot] + sizes[selected_slot].0 / 2;
    let start_x = screen_w / 2 - selected_center;

    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(0.02, 0.03, 0.05, FOCUS_CYCLE_BACKDROP_ALPHA),
        damage,
    )?;

    let center_y = (screen_h as f32 * 0.5).round() as i32;
    for (slot_index, (offset, node_id)) in slots.iter().enumerate() {
        let distance = offset.abs();
        let (w, h) = sizes[slot_index];
        let x = start_x + card_x[slot_index];
        let rect =
            Rectangle::<i32, Physical>::new((x, center_y - h / 2).into(), (w, h).into());
        let monitor = overlay
            .monitor_state
            .node_monitor
            .get(node_id)
            .map(String::as_str)
            .unwrap_or("?");
        draw_focus_cycle_card(
            frame,
            &overlay,
            &visuals,
            rect,
            *node_id,
            monitor,
            *offset == 0,
            distance,
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
        0.96,
    )?;

    Ok(true)
}
