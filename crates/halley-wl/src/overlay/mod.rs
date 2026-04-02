mod cluster_bloom;
mod state;
mod view;

use std::error::Error;

use smithay::{
    backend::renderer::{
        Color32F, Texture,
        gles::{GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::render::state::RenderState;

use crate::render::text::{draw_ui_text, draw_ui_text_in, ui_text_size, ui_text_size_in};

pub(crate) use cluster_bloom::{
    bloom_token_hit_test, draw_cluster_bloom, ensure_cluster_bloom_icon_resources,
};
pub(crate) use state::{
    ClusterBloomAnimSnapshot, ClusterBloomAnimState, OverlayBannerSnapshot, OverlayBannerState,
    OverlayToastSnapshot, OverlayToastState,
};
pub(crate) use view::OverlayView;

const BANNER_PAD_X: i32 = 14;
const BANNER_PAD_Y: i32 = 10;
const BANNER_GAP: i32 = 6;
const BANNER_EDGE_PAD: i32 = 18;
const BANNER_TITLE_SCALE: i32 = 2;
const BANNER_META_SCALE: i32 = 1;
const TOAST_PAD_X: i32 = 14;
const TOAST_PAD_Y: i32 = 10;
const TOAST_SCALE: i32 = 2;
const TOAST_META_SCALE: i32 = 1;
const SELECT_MARKER_W: i32 = 34;
const SELECT_MARKER_H: i32 = 20;
const OVERFLOW_ICON_PAD: i32 = 8;
const OVERFLOW_ICON_SIZE: i32 = 40;
const OVERFLOW_ICON_GAP: i32 = 8;

fn overlay_text_mix(mix: f32) -> f32 {
    let t = ((mix - 0.10) / 0.90).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
    if !overlay.cluster_overflow_visible_for_monitor(monitor, now_ms) {
        return None;
    }
    let rect = overlay.cluster_overflow_rect_for_monitor(monitor)?;
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return None;
    }

    let strip = Rectangle::<i32, Physical>::new(
        (rect.x.round() as i32, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(48),
            (rect.h.round() as i32).max(1),
        )
            .into(),
    );
    let visible_slots = ((strip.size.h - OVERFLOW_ICON_PAD * 2 + OVERFLOW_ICON_GAP)
        / (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP))
        .max(1) as usize;

    overflow
        .iter()
        .copied()
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
    if !overlay.cluster_overflow_visible_for_monitor(monitor, now_ms) {
        return None;
    }
    let rect = overlay.cluster_overflow_rect_for_monitor(monitor)?;
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return None;
    }
    let strip = Rectangle::<i32, Physical>::new(
        (rect.x.round() as i32, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(48),
            (rect.h.round() as i32).max(1),
        )
            .into(),
    );
    if sx < strip.loc.x as f32
        || sx > (strip.loc.x + strip.size.w) as f32
        || sy < strip.loc.y as f32
        || sy > (strip.loc.y + strip.size.h) as f32
    {
        return None;
    }
    let relative_y = (sy.round() as i32 - strip.loc.y - OVERFLOW_ICON_PAD).max(0);
    let slot_pitch = (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP).max(1);
    Some(((relative_y / slot_pitch) as usize).min(overflow.len().saturating_sub(1)))
}

pub(crate) fn draw_cluster_overflow_strip(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    monitor: &str,
    damage: Rectangle<i32, Physical>,
    now_ms: u64,
) -> Result<(), Box<dyn Error>> {
    if !overlay.cluster_overflow_visible_for_monitor(monitor, now_ms) {
        return Ok(());
    }
    let Some(rect) = overlay.cluster_overflow_rect_for_monitor(monitor) else {
        return Ok(());
    };
    let overflow = overlay.cluster_overflow_member_ids_for_monitor(monitor);
    if overflow.is_empty() {
        return Ok(());
    }
    let dragging_member = overlay
        .cluster_overflow_drag_preview_for_monitor(monitor)
        .map(|(member_id, _)| member_id);

    let strip = Rectangle::<i32, Physical>::new(
        (rect.x.round() as i32, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(48),
            (rect.h.round() as i32).max(1),
        )
            .into(),
    );
    draw_overlay_chip(
        frame,
        overlay.render_state,
        strip,
        18.0,
        Color32F::new(0.10, 0.14, 0.18, 0.92),
        damage,
        1.0,
    )?;

    let visible_slots = ((strip.size.h - OVERFLOW_ICON_PAD * 2 + OVERFLOW_ICON_GAP)
        / (OVERFLOW_ICON_SIZE + OVERFLOW_ICON_GAP))
        .max(1) as usize;
    for (index, node_id) in overflow
        .iter()
        .copied()
        .filter(|node_id| Some(*node_id) != dragging_member)
        .take(visible_slots)
        .enumerate()
    {
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
        draw_overlay_chip(
            frame,
            overlay.render_state,
            icon_rect,
            12.0,
            Color32F::new(0.93, 0.96, 0.99, 0.12),
            damage,
            1.0,
        )?;
        if overlay.tuning.tile_queue_show_icons
            && let Some(crate::render::state::NodeAppIconCacheEntry::Ready(icon)) =
                overlay.node_app_icon_entry(node_id)
        {
            let icon_dest = Rectangle::<i32, Physical>::new(
                (icon_rect.loc.x + 4, icon_rect.loc.y + 4).into(),
                (OVERFLOW_ICON_SIZE - 8, OVERFLOW_ICON_SIZE - 8).into(),
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
                1.0,
                None,
                &[],
            )?;
            continue;
        }

        let fallback = overlay
            .node_app_ids
            .get(&node_id)
            .map(String::as_str)
            .or_else(|| overlay.field.node(node_id).map(|n| n.label.as_str()))
            .unwrap_or("?");
        let glyph = fallback
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .unwrap_or('?')
            .to_ascii_uppercase()
            .to_string();
        let (text_w, text_h) =
            ui_text_size_in(overlay.render_state, &overlay.tuning.font, &glyph, 2);
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            icon_rect.loc.x + (icon_rect.size.w - text_w) / 2,
            icon_rect.loc.y + (icon_rect.size.h - text_h) / 2,
            &glyph,
            2,
            Color32F::new(0.94, 0.97, 0.99, 1.0),
            damage,
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
        draw_overlay_chip(
            frame,
            overlay.render_state,
            icon_rect,
            12.0,
            Color32F::new(0.10, 0.14, 0.18, 0.96),
            damage,
            1.0,
        )?;
        if overlay.tuning.tile_queue_show_icons
            && let Some(crate::render::state::NodeAppIconCacheEntry::Ready(icon)) =
                overlay.node_app_icon_entry(node_id)
        {
            let icon_dest = Rectangle::<i32, Physical>::new(
                (icon_rect.loc.x + 4, icon_rect.loc.y + 4).into(),
                (OVERFLOW_ICON_SIZE - 8, OVERFLOW_ICON_SIZE - 8).into(),
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
                1.0,
                None,
                &[],
            )?;
        } else {
            let fallback = overlay
                .node_app_ids
                .get(&node_id)
                .map(String::as_str)
                .or_else(|| overlay.field.node(node_id).map(|n| n.label.as_str()))
                .unwrap_or("?");
            let glyph = fallback
                .chars()
                .find(|ch| ch.is_ascii_alphanumeric())
                .unwrap_or('?')
                .to_ascii_uppercase()
                .to_string();
            let (text_w, text_h) =
                ui_text_size_in(overlay.render_state, &overlay.tuning.font, &glyph, 2);
            draw_ui_text_in(
                frame,
                overlay.render_state,
                &overlay.tuning.font,
                icon_rect.loc.x + (icon_rect.size.w - text_w) / 2,
                icon_rect.loc.y + (icon_rect.size.h - text_h) / 2,
                &glyph,
                2,
                Color32F::new(0.94, 0.97, 0.99, 1.0),
                damage,
            )?;
        }
    }

    Ok(())
}

pub(crate) fn draw_persistent_banner(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    damage: Rectangle<i32, Physical>,
    banner: &OverlayBannerSnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(banner.mix);
    let (title_w, title_h) = ui_text_size_in(
        render_state,
        font,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
    );
    let (meta_w, meta_h) = banner
        .subtitle
        .as_ref()
        .map(|text| ui_text_size_in(render_state, font, text.as_str(), BANNER_META_SCALE))
        .unwrap_or((0, 0));
    let width: i32 = title_w.max(meta_w) + BANNER_PAD_X * 2;
    let height: i32 = BANNER_PAD_Y * 2
        + title_h
        + if banner.subtitle.is_some() {
            BANNER_GAP + meta_h
        } else {
            0
        };
    let rect = Rectangle::<i32, Physical>::new(
        (BANNER_EDGE_PAD, BANNER_EDGE_PAD).into(),
        (width.max(80), height.max(30)).into(),
    );

    draw_overlay_chip(
        frame,
        render_state,
        rect,
        14.0,
        Color32F::new(0.95, 0.97, 0.99, 0.94 * banner.mix),
        damage,
        banner.mix,
    )?;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + BANNER_PAD_X,
        rect.loc.y + BANNER_PAD_Y,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
        Color32F::new(0.06, 0.08, 0.10, text_mix),
        damage,
    )?;
    if let Some(subtitle) = banner.subtitle.as_ref() {
        draw_ui_text_in(
            frame,
            render_state,
            font,
            rect.loc.x + BANNER_PAD_X,
            rect.loc.y + BANNER_PAD_Y + title_h + BANNER_GAP,
            subtitle.as_str(),
            BANNER_META_SCALE,
            Color32F::new(0.27, 0.32, 0.37, text_mix * 0.96),
            damage,
        )?;
    }
    Ok(())
}

pub(crate) fn draw_toast(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    toast: &OverlayToastSnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(toast.mix);
    let mut lines = toast.message.lines();
    let title = lines.next().unwrap_or_default();
    let body = lines.collect::<Vec<_>>().join(" ");
    let body = (!body.is_empty()).then_some(body);
    let (title_w, title_h) = ui_text_size_in(render_state, font, title, TOAST_SCALE);
    let (body_w, body_h) = body
        .as_ref()
        .map(|text| ui_text_size_in(render_state, font, text.as_str(), TOAST_META_SCALE))
        .unwrap_or((0, 0));
    let rect_w: i32 = (title_w.max(body_w) + TOAST_PAD_X * 2).max(180);
    let rect_h: i32 = (TOAST_PAD_Y * 2
        + title_h
        + if body.is_some() {
            BANNER_GAP + body_h
        } else {
            0
        })
    .max(32);
    let rect_x: i32 = ((screen_w - rect_w) / 2).max(BANNER_EDGE_PAD);
    let rect_y: i32 = ((screen_h - rect_h) / 2).max(BANNER_EDGE_PAD);
    let rect = Rectangle::<i32, Physical>::new((rect_x, rect_y).into(), (rect_w, rect_h).into());

    draw_overlay_chip(
        frame,
        render_state,
        rect,
        14.0,
        Color32F::new(0.95, 0.97, 0.99, 0.94 * toast.mix),
        damage,
        toast.mix,
    )?;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + TOAST_PAD_X,
        rect.loc.y + TOAST_PAD_Y,
        title,
        TOAST_SCALE,
        Color32F::new(0.06, 0.08, 0.10, text_mix),
        damage,
    )?;
    if let Some(body) = body.as_ref() {
        draw_ui_text_in(
            frame,
            render_state,
            font,
            rect.loc.x + TOAST_PAD_X,
            rect.loc.y + TOAST_PAD_Y + title_h + BANNER_GAP,
            body.as_str(),
            TOAST_META_SCALE,
            Color32F::new(0.27, 0.32, 0.37, text_mix * 0.96),
            damage,
        )?;
    }
    Ok(())
}

pub(crate) fn draw_monitor_hud(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<(), Box<dyn Error>> {
    let overlay_monitor = st.model.monitor_state.current_monitor.clone();
    if let Some(banner) = st
        .ui
        .render_state
        .persistent_mode_banner_snapshot(overlay_monitor.as_str())
    {
        draw_persistent_banner(
            frame,
            &st.ui.render_state,
            &st.runtime.tuning.font,
            damage,
            &banner,
        )?;
    }
    if let Some(toast) = st
        .ui
        .render_state
        .overlay_toast_snapshot(overlay_monitor.as_str(), st.now_ms(now))
    {
        draw_toast(
            frame,
            &st.ui.render_state,
            &st.runtime.tuning.font,
            screen_w,
            screen_h,
            damage,
            &toast,
        )?;
    }
    Ok(())
}

pub(crate) fn draw_overlay_hover_label(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(target) = st
        .input
        .interaction_state
        .overlay_hover_target
        .clone()
        .filter(|target| target.monitor == st.model.monitor_state.current_monitor)
    else {
        return Ok(());
    };
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
    let mut text = label.to_ascii_uppercase();
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

    draw_overlay_chip(
        frame,
        &st.ui.render_state,
        rect,
        (label_h as f32) * 0.32,
        Color32F::new(0.96, 0.98, 1.0, 0.96 * label_fade),
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
        Color32F::new(0.16, 0.18, 0.22, 0.94 * label_fade),
        damage,
    )?;
    Ok(())
}

pub(crate) fn draw_cluster_selection_markers(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let selected = overlay
        .cluster_state
        .cluster_mode_selected_nodes
        .get(overlay.monitor_state.current_monitor.as_str())
        .into_iter()
        .flat_map(|nodes| nodes.iter());
    for &node_id in selected {
        let Some(node) = overlay.field.node(node_id) else {
            continue;
        };
        if !overlay.field.is_visible(node_id) || !overlay.node_visible_on_current_monitor(node_id) {
            continue;
        }
        let (sx, sy) = overlay.world_to_screen(screen_w, screen_h, node.pos.x, node.pos.y);
        let rect = Rectangle::<i32, Physical>::new(
            ((sx - SELECT_MARKER_W / 2), (sy - SELECT_MARKER_H / 2)).into(),
            (SELECT_MARKER_W, SELECT_MARKER_H).into(),
        );
        draw_overlay_chip(
            frame,
            overlay.render_state,
            rect,
            9.0,
            Color32F::new(0.10, 0.62, 0.58, 0.92),
            damage,
            1.0,
        )?;
        let (_, text_h) = ui_text_size_in(
            overlay.render_state,
            &overlay.tuning.font,
            "SEL",
            BANNER_META_SCALE,
        );
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            rect.loc.x + 8,
            rect.loc.y + (rect.size.h - text_h) / 2,
            "SEL",
            BANNER_META_SCALE,
            Color32F::new(0.96, 0.99, 0.98, 1.0),
            damage,
        )?;
    }
    Ok(())
}

fn draw_overlay_chip(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = render_state.node_label_program.as_ref() else {
        return Ok(());
    };
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("node_color", (0.0f32, 0.0f32, 0.0f32, 0.0f32)),
        Uniform::new(
            "fill_color",
            (
                fill_color.r(),
                fill_color.g(),
                fill_color.b(),
                fill_color.a(),
            ),
        ),
        Uniform::new("rect_size", (rect.size.w as f32, rect.size.h as f32)),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("border_px", 0.0f32),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        rect,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )?;
    Ok(())
}
