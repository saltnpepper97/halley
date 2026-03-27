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

use crate::state::RenderState;
use crate::state::Halley;

use crate::render::utils::{bitmap_text_size, draw_bitmap_text};

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

pub(crate) fn cluster_overflow_icon_hit_test(
    overlay: &OverlayView<'_>,
    monitor: &str,
    sx: f32,
    sy: f32,
    now_ms: u64,
) -> Option<halley_core::field::NodeId> {
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
            (rect.h.round() as i32).max(80),
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
                .then_some(node_id)
        })
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

    let strip = Rectangle::<i32, Physical>::new(
        (rect.x.round() as i32, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(48),
            (rect.h.round() as i32).max(80),
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
    for (index, node_id) in overflow.iter().copied().take(visible_slots).enumerate() {
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
        if let Some(crate::state::NodeAppIconCacheEntry::Ready(icon)) =
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
        let (text_w, text_h) = bitmap_text_size(&glyph, 2);
        draw_bitmap_text(
            frame,
            icon_rect.loc.x + (icon_rect.size.w - text_w) / 2,
            icon_rect.loc.y + (icon_rect.size.h - text_h) / 2,
            &glyph,
            2,
            Color32F::new(0.94, 0.97, 0.99, 1.0),
            damage,
        )?;
    }

    Ok(())
}

pub(crate) fn draw_persistent_banner(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    damage: Rectangle<i32, Physical>,
    banner: &OverlayBannerSnapshot,
) -> Result<(), Box<dyn Error>> {
    let (title_w, title_h) = bitmap_text_size(banner.title.as_str(), BANNER_TITLE_SCALE);
    let (meta_w, meta_h) = banner
        .subtitle
        .as_ref()
        .map(|text| bitmap_text_size(text.as_str(), BANNER_META_SCALE))
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
    draw_bitmap_text(
        frame,
        rect.loc.x + BANNER_PAD_X,
        rect.loc.y + BANNER_PAD_Y,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
        Color32F::new(0.06, 0.08, 0.10, banner.mix),
        damage,
    )?;
    if let Some(subtitle) = banner.subtitle.as_ref() {
        draw_bitmap_text(
            frame,
            rect.loc.x + BANNER_PAD_X,
            rect.loc.y + BANNER_PAD_Y + title_h + BANNER_GAP,
            subtitle.as_str(),
            BANNER_META_SCALE,
            Color32F::new(0.27, 0.32, 0.37, banner.mix * 0.96),
            damage,
        )?;
    }
    Ok(())
}

pub(crate) fn draw_toast(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    toast: &OverlayToastSnapshot,
) -> Result<(), Box<dyn Error>> {
    let mut lines = toast.message.lines();
    let title = lines.next().unwrap_or_default();
    let body = lines.collect::<Vec<_>>().join(" ");
    let body = (!body.is_empty()).then_some(body);
    let (title_w, title_h) = bitmap_text_size(title, TOAST_SCALE);
    let (body_w, body_h) = body
        .as_ref()
        .map(|text| bitmap_text_size(text.as_str(), TOAST_META_SCALE))
        .unwrap_or((0, 0));
    let rect_w: i32 = (title_w.max(body_w) + TOAST_PAD_X * 2).max(180);
    let rect_h: i32 = (TOAST_PAD_Y * 2
        + title_h
        + if body.is_some() { BANNER_GAP + body_h } else { 0 })
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
    draw_bitmap_text(
        frame,
        rect.loc.x + TOAST_PAD_X,
        rect.loc.y + TOAST_PAD_Y,
        title,
        TOAST_SCALE,
        Color32F::new(0.06, 0.08, 0.10, toast.mix),
        damage,
    )?;
    if let Some(body) = body.as_ref() {
        draw_bitmap_text(
            frame,
            rect.loc.x + TOAST_PAD_X,
            rect.loc.y + TOAST_PAD_Y + title_h + BANNER_GAP,
            body.as_str(),
            TOAST_META_SCALE,
            Color32F::new(0.27, 0.32, 0.37, toast.mix * 0.96),
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
    if let Some(banner) = st.persistent_mode_banner_snapshot(overlay_monitor.as_str()) {
        draw_persistent_banner(frame, &st.ui.render_state, damage, &banner)?;
    }
    if let Some(toast) = st.overlay_toast_snapshot(overlay_monitor.as_str(), now) {
        draw_toast(frame, &st.ui.render_state, screen_w, screen_h, damage, &toast)?;
    }
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
        let (_, text_h) = bitmap_text_size("SEL", BANNER_META_SCALE);
        draw_bitmap_text(
            frame,
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
