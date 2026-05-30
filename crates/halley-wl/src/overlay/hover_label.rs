use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;
use crate::presentation::themed_node_label_colors;
use crate::text::{draw_ui_text, ui_text_size};

use super::{BANNER_EDGE_PAD, draw_overlay_chip, resolve_overlay_visuals};

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
