use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame},
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    BANNER_EDGE_PAD, FOCUS_CYCLE_BACKDROP_ALPHA, FOCUS_CYCLE_CARD_PAD_X, FOCUS_CYCLE_GAP,
    FOCUS_CYCLE_ICON_PAD, FOCUS_CYCLE_LABEL_SCALE, FOCUS_CYCLE_META_SCALE,
    FOCUS_CYCLE_MONITOR_SCALE, FOCUS_CYCLE_VISIBLE_RADIUS, OverlayView, OverlayVisuals,
    draw_overflow_member_chip, draw_overlay_action_row, draw_overlay_chip,
    draw_overlay_chip_without_shadow, overlay_accent_fill, overlay_action_row_size,
    overlay_text_color_for_fill, resolve_overlay_visuals, truncate_overlay_text,
    truncate_overlay_text_to_width,
};

fn focus_cycle_card_size(distance: i32) -> (i32, i32) {
    match distance {
        0 => (244, 108),
        1 => (204, 92),
        _ => (168, 80),
    }
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
            .alpha((0.78 - distance as f32 * 0.12).clamp(0.46, 0.78))
    };
    draw_overlay_chip(
        frame,
        overlay.render_state,
        visuals,
        rect,
        if selected { 20.0 } else { 18.0 },
        fill,
        true,
        damage,
        1.0,
    )?;

    let icon_size = (rect.size.h - FOCUS_CYCLE_ICON_PAD * 2).clamp(44, 72);
    let icon_rect = Rectangle::<i32, Physical>::new(
        (
            rect.loc.x + FOCUS_CYCLE_ICON_PAD,
            rect.loc.y + (rect.size.h - icon_size) / 2,
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
    )?;

    let raw_label = focus_cycle_label(overlay, node_id);
    let app_id = overlay
        .node_app_ids
        .get(&node_id)
        .map(String::as_str)
        .filter(|app_id| !app_id.trim().is_empty());
    let monitor_label = truncate_overlay_text(monitor, 10);
    let text_color = overlay_text_color_for_fill(fill, if selected { 1.0 } else { 0.96 });
    let subtext_color = visuals
        .palette
        .subtext
        .alpha(if selected { 0.98 } else { 0.90 });
    let text_x = icon_rect.loc.x + icon_rect.size.w + 12;
    let selection_label = "selected";
    let (selection_w, selection_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        selection_label,
        FOCUS_CYCLE_META_SCALE,
    );
    let selection_rect = selected.then(|| {
        Rectangle::<i32, Physical>::new(
            (text_x, rect.loc.y + 10).into(),
            (selection_w + 16, selection_h + 8).into(),
        )
    });

    let badge_w = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
    )
    .0 + 14;
    let text_max_w = (rect.loc.x + rect.size.w - badge_w - 22 - text_x).max(48);
    let label = truncate_overlay_text_to_width(
        overlay.render_state,
        &overlay.tuning.font,
        raw_label.as_str(),
        FOCUS_CYCLE_LABEL_SCALE,
        text_max_w,
    );
    let meta = app_id
        .filter(|app_id| !raw_label.eq_ignore_ascii_case(app_id))
        .map(|app_id| {
            truncate_overlay_text_to_width(
                overlay.render_state,
                &overlay.tuning.font,
                app_id,
                FOCUS_CYCLE_META_SCALE,
                text_max_w,
            )
        })
        .filter(|text| !text.is_empty());

    let (_, label_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        label.as_str(),
        FOCUS_CYCLE_LABEL_SCALE,
    );
    let meta_h = meta
        .as_ref()
        .map(|text| {
            ui_text_size_in(
                overlay.render_state,
                &overlay.tuning.font,
                text.as_str(),
                FOCUS_CYCLE_META_SCALE,
            )
            .1
        })
        .unwrap_or(0);
    let (_, monitor_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
    );
    let selection_reserved_h = selection_rect
        .as_ref()
        .map(|rect| rect.size.h + 8)
        .unwrap_or(0);
    let total_h =
        selection_reserved_h + label_h + monitor_h + if meta.is_some() { meta_h + 8 } else { 4 };
    let base_y = (rect.loc.y + ((rect.size.h - total_h).max(0) / 2))
        .max(rect.loc.y + 10 + selection_reserved_h);

    let badge_rect = Rectangle::<i32, Physical>::new(
        (rect.loc.x + rect.size.w - badge_w - 12, rect.loc.y + 10).into(),
        (badge_w, monitor_h + 8).into(),
    );
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        badge_rect,
        10.0,
        visuals
            .palette
            .border
            .alpha(if selected { 0.95 } else { 0.74 }),
        false,
        damage,
        1.0,
    )?;
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        badge_rect.loc.x + (badge_rect.size.w - (badge_w - 14)) / 2,
        badge_rect.loc.y + 4,
        monitor_label.as_str(),
        FOCUS_CYCLE_MONITOR_SCALE,
        overlay_text_color_for_fill(
            visuals
                .palette
                .border
                .alpha(if selected { 0.95 } else { 0.74 }),
            1.0,
        ),
        damage,
    )?;

    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        text_x.min(rect.loc.x + rect.size.w - FOCUS_CYCLE_CARD_PAD_X - 10),
        base_y,
        label.as_str(),
        FOCUS_CYCLE_LABEL_SCALE,
        text_color,
        damage,
    )?;
    if let Some(meta) = meta.as_ref() {
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            text_x.min(rect.loc.x + rect.size.w - FOCUS_CYCLE_CARD_PAD_X - 10),
            base_y + label_h + 8,
            meta.as_str(),
            FOCUS_CYCLE_META_SCALE,
            subtext_color,
            damage,
        )?;
    }

    if let Some(select_rect) = selection_rect {
        let select_fill = visuals.palette.border.alpha(0.94);
        draw_overlay_chip_without_shadow(
            frame,
            overlay.render_state,
            visuals,
            select_rect,
            10.0,
            select_fill,
            false,
            damage,
            1.0,
        )?;
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            select_rect.loc.x + 8,
            select_rect.loc.y + (select_rect.size.h - selection_h) / 2,
            selection_label,
            FOCUS_CYCLE_META_SCALE,
            overlay_text_color_for_fill(select_fill, 1.0),
            damage,
        )?;
    }

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
    let overlay = OverlayView::from_halley(st);
    let len = session.candidates.len() as i32;
    let visible_count = len.min(FOCUS_CYCLE_VISIBLE_RADIUS * 2 + 1).max(2);
    let left_count = (visible_count - 1) / 2;
    let right_count = visible_count - 1 - left_count;
    let mut slots = Vec::new();
    for offset in -left_count..=right_count {
        let preview = session.preview_index as i32;
        let idx = (preview + offset).rem_euclid(len) as usize;
        slots.push((offset, session.candidates[idx]));
    }

    let widths = slots
        .iter()
        .map(|(offset, _)| focus_cycle_card_size(offset.abs()).0)
        .collect::<Vec<_>>();
    let total_w =
        widths.iter().sum::<i32>() + FOCUS_CYCLE_GAP * (slots.len().saturating_sub(1) as i32);
    let base_h = slots
        .iter()
        .map(|(offset, _)| focus_cycle_card_size(offset.abs()).1)
        .max()
        .unwrap_or(0);

    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(0.02, 0.03, 0.05, FOCUS_CYCLE_BACKDROP_ALPHA),
        damage,
    )?;

    let start_x = ((screen_w - total_w) / 2).max(BANNER_EDGE_PAD);
    let center_y = (screen_h as f32 * 0.52).round() as i32;
    let mut x = start_x;
    for (slot_index, (offset, node_id)) in slots.iter().enumerate() {
        let distance = offset.abs();
        let (w, h) = focus_cycle_card_size(distance);
        let y_offset = match distance {
            0 => 0,
            1 => 10,
            _ => 18,
        };
        let rect =
            Rectangle::<i32, Physical>::new((x, center_y - h / 2 + y_offset).into(), (w, h).into());
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
        x += widths[slot_index] + FOCUS_CYCLE_GAP;
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
