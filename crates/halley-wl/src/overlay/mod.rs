mod cluster_bloom;
mod cluster_naming;
mod screenshot;
mod state;
mod view;

use std::error::Error;

use halley_config::{OverlayColorMode, OverlayShape, RuntimeTuning};
use smithay::{
    backend::renderer::{
        Color32F, Texture,
        gles::{GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::render::state::RenderState;
use crate::render::themed_node_label_colors;
use crate::render::utils::draw_rect;
use crate::render::{node_app_icon_fallback_glyph, node_app_icon_texture_allowed};

use crate::render::text::{draw_ui_text, draw_ui_text_in, ui_text_size, ui_text_size_in};

pub(crate) use cluster_bloom::{
    bloom_token_hit_test, draw_cluster_bloom, ensure_cluster_bloom_icon_resources,
};
pub(crate) use cluster_naming::{
    ClusterNamingDialogHit, cluster_naming_dialog_hit_test, draw_cluster_naming_dialog,
};
pub(crate) use screenshot::{ScreenshotMenuHit, draw_screenshot_overlay, screenshot_menu_hit_test};
pub(crate) use state::{
    ClusterBloomAnimSnapshot, ClusterBloomAnimState, ExitConfirmOverlaySnapshot,
    ExitConfirmOverlayState, OverlayActionHint, OverlayBannerSnapshot, OverlayBannerState,
    OverlayToastSnapshot, OverlayToastState,
};
pub(crate) use view::OverlayView;

const BANNER_PAD_X: i32 = 14;
const BANNER_PAD_Y: i32 = 10;
const BANNER_GAP: i32 = 6;
const BANNER_EDGE_PAD: i32 = 18;
const BANNER_TITLE_SCALE: i32 = 2;
const BANNER_META_SCALE: i32 = 2;
const ACTION_ROW_GAP_Y: i32 = 10;
const ACTION_ITEM_GAP: i32 = 18;
const ACTION_LABEL_GAP: i32 = 8;
const ACTION_KEY_PAD_X: i32 = 8;
const ACTION_KEY_PAD_Y: i32 = 6;
const ACTION_KEY_MIN_W: i32 = 48;
const ACTION_KEY_SCALE: i32 = BANNER_META_SCALE;
const ACTION_LABEL_SCALE: i32 = BANNER_META_SCALE;
const SELECT_MARKER_SCALE: i32 = 2;
const TOAST_PAD_X: i32 = 14;
const TOAST_PAD_Y: i32 = 10;
const TOAST_SCALE: i32 = 2;
const TOAST_META_SCALE: i32 = 2;
const EXIT_CONFIRM_PAD_X: i32 = 18;
const EXIT_CONFIRM_PAD_Y: i32 = 16;
const EXIT_CONFIRM_TITLE_SCALE: i32 = 2;
const EXIT_CONFIRM_MIN_WIDTH: i32 = 280;
const EXIT_CONFIRM_MAX_WIDTH_PAD: i32 = 36;
const SELECT_MARKER_PAD_X: i32 = 8;
const SELECT_MARKER_PAD_Y: i32 = 4;
const OVERFLOW_ICON_PAD: i32 = 8;
const OVERFLOW_ICON_SIZE: i32 = 40;
const OVERFLOW_ICON_GAP: i32 = 8;
const OVERFLOW_VISIBLE_SLOTS: usize = 15;
const OVERFLOW_SCROLLBAR_W: i32 = 4;
const OVERFLOW_SCROLLBAR_PAD: i32 = 6;
const OVERFLOW_REVEAL_ANIM_MS: u64 = 220;
const OVERFLOW_REVEAL_SLIDE_PX: i32 = 28;
const EXIT_CONFIRM_TITLE: &str = "Are you sure you want to leave?";

#[derive(Clone, Copy)]
struct OverlayRgb {
    r: f32,
    g: f32,
    b: f32,
}

impl OverlayRgb {
    fn alpha(self, alpha: f32) -> Color32F {
        Color32F::new(self.r, self.g, self.b, alpha)
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        let t = amount.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
        }
    }

    fn luminance(self) -> f32 {
        self.r * 0.2126 + self.g * 0.7152 + self.b * 0.0722
    }
}

#[derive(Clone, Copy)]
struct OverlayPalette {
    fill: OverlayRgb,
    text: OverlayRgb,
    subtext: OverlayRgb,
    key_fill: OverlayRgb,
    key_text: OverlayRgb,
    border: OverlayRgb,
}

#[derive(Clone, Copy)]
struct OverlayVisuals {
    rounded: bool,
    border_px: f32,
    palette: OverlayPalette,
}

const LIGHT_OVERLAY_FILL: OverlayRgb = OverlayRgb {
    r: 0.92,
    g: 0.95,
    b: 0.98,
};
const DARK_OVERLAY_FILL: OverlayRgb = OverlayRgb {
    r: 0.15,
    g: 0.18,
    b: 0.22,
};
const LIGHT_OVERLAY_TEXT: OverlayRgb = OverlayRgb {
    r: 0.08,
    g: 0.10,
    b: 0.12,
};
const DARK_OVERLAY_TEXT: OverlayRgb = OverlayRgb {
    r: 0.94,
    g: 0.96,
    b: 0.98,
};

fn resolve_overlay_base_background(mode: OverlayColorMode) -> OverlayRgb {
    match mode {
        OverlayColorMode::Auto | OverlayColorMode::Light => LIGHT_OVERLAY_FILL,
        OverlayColorMode::Dark => DARK_OVERLAY_FILL,
        OverlayColorMode::Fixed { r, g, b } => OverlayRgb { r, g, b },
    }
}

fn resolve_overlay_base_text(mode: OverlayColorMode, background: OverlayRgb) -> OverlayRgb {
    match mode {
        OverlayColorMode::Auto => {
            if background.luminance() < 0.45 {
                DARK_OVERLAY_TEXT
            } else {
                LIGHT_OVERLAY_TEXT
            }
        }
        OverlayColorMode::Light => LIGHT_OVERLAY_TEXT,
        OverlayColorMode::Dark => DARK_OVERLAY_TEXT,
        OverlayColorMode::Fixed { r, g, b } => OverlayRgb { r, g, b },
    }
}

fn resolve_overlay_border_color(tuning: &RuntimeTuning) -> OverlayRgb {
    let color = tuning.border_color_focused;
    OverlayRgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn resolve_overlay_visuals(tuning: &RuntimeTuning) -> OverlayVisuals {
    let fill = resolve_overlay_base_background(tuning.overlay_style.background_color);
    let text = resolve_overlay_base_text(tuning.overlay_style.text_color, fill);
    let border = resolve_overlay_border_color(tuning);
    OverlayVisuals {
        rounded: matches!(tuning.overlay_style.shape, OverlayShape::Rounded),
        border_px: if tuning.overlay_style.borders {
            tuning.border_size_px.max(0) as f32
        } else {
            0.0
        },
        palette: OverlayPalette {
            fill,
            text,
            subtext: text.mix(fill, 0.20),
            key_fill: fill.mix(text, 0.10),
            key_text: text,
            border,
        },
    }
}

fn overlay_text_mix(mix: f32) -> f32 {
    let t = ((mix - 0.10) / 0.90).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

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

fn draw_overflow_member_chip(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    node_id: halley_core::field::NodeId,
    icon_rect: Rectangle<i32, Physical>,
    chip_fill: Color32F,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    draw_overlay_chip(
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
        visuals.palette.text.alpha(alpha),
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
        visuals.palette.fill.alpha(0.97),
        1.0,
        damage,
    )?;
    Ok(())
}

fn overlay_action_item_metrics(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    key: &str,
    label: &str,
) -> (i32, i32, i32, i32, i32) {
    let (key_w, key_h) = ui_text_size_in(render_state, font, key, ACTION_KEY_SCALE);
    let (label_w, label_h) = ui_text_size_in(render_state, font, label, ACTION_LABEL_SCALE);
    let keycap_w = (key_w + ACTION_KEY_PAD_X * 2).max(ACTION_KEY_MIN_W);
    let keycap_h = (key_h + ACTION_KEY_PAD_Y * 2).max(20);
    let item_w = keycap_w + ACTION_LABEL_GAP + label_w;
    let item_h = keycap_h.max(label_h);
    (item_w, item_h, keycap_w, keycap_h, label_h)
}

fn overlay_action_row_size(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    actions: &[(&str, &str)],
) -> (i32, i32) {
    let mut total_w = 0;
    let mut total_h = 0;
    for (index, (key, label)) in actions.iter().enumerate() {
        let (item_w, item_h, _, _, _) = overlay_action_item_metrics(render_state, font, key, label);
        if index > 0 {
            total_w += ACTION_ITEM_GAP;
        }
        total_w += item_w;
        total_h = total_h.max(item_h);
    }
    (total_w, total_h)
}

fn draw_overlay_action_row(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    x: i32,
    y: i32,
    actions: &[(&str, &str)],
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let (_, row_h) = overlay_action_row_size(render_state, font, actions);
    let mut cursor_x = x;
    for (index, (key, label)) in actions.iter().enumerate() {
        if index > 0 {
            cursor_x += ACTION_ITEM_GAP;
        }
        let (item_w, _item_h, keycap_w, keycap_h, label_h) =
            overlay_action_item_metrics(render_state, font, key, label);
        let key_rect = Rectangle::<i32, Physical>::new(
            (cursor_x, y + (row_h - keycap_h) / 2).into(),
            (keycap_w, keycap_h).into(),
        );
        draw_overlay_chip(
            frame,
            render_state,
            visuals,
            key_rect,
            10.0,
            visuals.palette.key_fill.alpha(0.98 * alpha),
            false,
            damage,
            alpha,
        )?;
        let (key_w, key_h) = ui_text_size_in(render_state, font, key, ACTION_KEY_SCALE);
        draw_ui_text_in(
            frame,
            render_state,
            font,
            key_rect.loc.x + (key_rect.size.w - key_w) / 2,
            key_rect.loc.y + (key_rect.size.h - key_h) / 2,
            key,
            ACTION_KEY_SCALE,
            visuals.palette.key_text.alpha(alpha),
            damage,
        )?;
        draw_ui_text_in(
            frame,
            render_state,
            font,
            cursor_x + keycap_w + ACTION_LABEL_GAP,
            y + (row_h - label_h) / 2,
            label,
            ACTION_LABEL_SCALE,
            visuals.palette.subtext.alpha(alpha * 0.96),
            damage,
        )?;
        cursor_x += item_w;
    }
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
            visuals.palette.key_fill.alpha(0.68 * reveal_alpha),
            reveal_alpha,
            damage,
        )?;
    }

    if let (Some(track), Some(thumb)) = (scrollbar_track, scrollbar_thumb) {
        draw_overlay_chip(
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
        draw_overlay_chip(
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
            visuals.palette.fill.alpha(0.97),
            1.0,
            damage,
        )?;
    }

    Ok(())
}

fn draw_persistent_banner(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    damage: Rectangle<i32, Physical>,
    banner: &OverlayBannerSnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(banner.mix);
    let actions = banner
        .actions
        .iter()
        .map(|action| (action.key.as_str(), action.label.as_str()))
        .collect::<Vec<_>>();
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
    let (actions_w, actions_h) = overlay_action_row_size(render_state, font, actions.as_slice());
    let width: i32 = title_w.max(meta_w).max(actions_w) + BANNER_PAD_X * 2;
    let height: i32 = BANNER_PAD_Y * 2
        + title_h
        + if banner.subtitle.is_some() {
            BANNER_GAP + meta_h
        } else {
            0
        }
        + if actions.is_empty() {
            0
        } else {
            ACTION_ROW_GAP_Y + actions_h
        };
    let rect = Rectangle::<i32, Physical>::new(
        (BANNER_EDGE_PAD, BANNER_EDGE_PAD).into(),
        (width.max(80), height.max(30)).into(),
    );

    draw_overlay_chip(
        frame,
        render_state,
        visuals,
        rect,
        18.0,
        visuals.palette.fill.alpha(0.97 * banner.mix),
        true,
        damage,
        banner.mix,
    )?;
    let mut row_y = rect.loc.y + BANNER_PAD_Y;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + BANNER_PAD_X,
        row_y,
        banner.title.as_str(),
        BANNER_TITLE_SCALE,
        visuals.palette.text.alpha(text_mix),
        damage,
    )?;
    row_y += title_h;
    if let Some(subtitle) = banner.subtitle.as_ref() {
        row_y += BANNER_GAP;
        draw_ui_text_in(
            frame,
            render_state,
            font,
            rect.loc.x + BANNER_PAD_X,
            row_y,
            subtitle.as_str(),
            BANNER_META_SCALE,
            visuals.palette.subtext.alpha(text_mix * 0.96),
            damage,
        )?;
        row_y += meta_h;
    }
    if !actions.is_empty() {
        row_y += ACTION_ROW_GAP_Y;
        draw_overlay_action_row(
            frame,
            render_state,
            visuals,
            font,
            rect.loc.x + BANNER_PAD_X,
            row_y,
            actions.as_slice(),
            damage,
            text_mix,
        )?;
    }
    Ok(())
}

fn draw_toast(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
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
        visuals,
        rect,
        14.0,
        visuals.palette.fill.alpha(0.94 * toast.mix),
        true,
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
        visuals.palette.text.alpha(text_mix),
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
            visuals.palette.subtext.alpha(text_mix * 0.96),
            damage,
        )?;
    }
    Ok(())
}

fn draw_exit_confirmation(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    exit_confirm: &ExitConfirmOverlaySnapshot,
) -> Result<(), Box<dyn Error>> {
    let text_mix = overlay_text_mix(exit_confirm.mix);
    let actions = [("Enter", "leave"), ("Esc", "cancel")];
    draw_rect(
        frame,
        0,
        0,
        screen_w.max(1),
        screen_h.max(1),
        Color32F::new(0.02, 0.03, 0.05, 0.62 * exit_confirm.mix),
        damage,
    )?;

    let (title_w, title_h) = ui_text_size_in(
        render_state,
        font,
        EXIT_CONFIRM_TITLE,
        EXIT_CONFIRM_TITLE_SCALE,
    );
    let (actions_w, actions_h) = overlay_action_row_size(render_state, font, &actions);
    let rect_w = (title_w.max(actions_w) + EXIT_CONFIRM_PAD_X * 2).clamp(
        EXIT_CONFIRM_MIN_WIDTH,
        (screen_w - EXIT_CONFIRM_MAX_WIDTH_PAD).max(EXIT_CONFIRM_MIN_WIDTH),
    );
    let rect_h = (EXIT_CONFIRM_PAD_Y * 2 + title_h + ACTION_ROW_GAP_Y + actions_h).max(72);
    let rect_x = ((screen_w - rect_w) / 2).max(BANNER_EDGE_PAD);
    let rect_y = ((screen_h - rect_h) / 2).max(BANNER_EDGE_PAD);
    let rect = Rectangle::<i32, Physical>::new((rect_x, rect_y).into(), (rect_w, rect_h).into());

    draw_overlay_chip(
        frame,
        render_state,
        visuals,
        rect,
        18.0,
        visuals.palette.fill.alpha(0.97 * exit_confirm.mix),
        true,
        damage,
        exit_confirm.mix,
    )?;
    draw_ui_text_in(
        frame,
        render_state,
        font,
        rect.loc.x + EXIT_CONFIRM_PAD_X,
        rect.loc.y + EXIT_CONFIRM_PAD_Y,
        EXIT_CONFIRM_TITLE,
        EXIT_CONFIRM_TITLE_SCALE,
        visuals.palette.text.alpha(text_mix),
        damage,
    )?;
    draw_overlay_action_row(
        frame,
        render_state,
        visuals,
        font,
        rect.loc.x + EXIT_CONFIRM_PAD_X,
        rect.loc.y + EXIT_CONFIRM_PAD_Y + title_h + ACTION_ROW_GAP_Y,
        &actions,
        damage,
        text_mix,
    )?;
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
    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    if let Some(exit_confirm) = st
        .ui
        .render_state
        .exit_confirm_snapshot(overlay_monitor.as_str())
    {
        draw_exit_confirmation(
            frame,
            &st.ui.render_state,
            &visuals,
            &st.runtime.tuning.font,
            screen_w,
            screen_h,
            damage,
            &exit_confirm,
        )?;
        return Ok(());
    }
    if let Some(banner) = st
        .ui
        .render_state
        .persistent_mode_banner_snapshot(overlay_monitor.as_str())
    {
        draw_persistent_banner(
            frame,
            &st.ui.render_state,
            &visuals,
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
            &visuals,
            &st.runtime.tuning.font,
            screen_w,
            screen_h,
            damage,
            &toast,
        )?;
    }
    draw_cluster_naming_dialog(frame, st, screen_w, screen_h, damage)?;
    draw_screenshot_overlay(frame, st, screen_w, screen_h, damage)?;
    Ok(())
}

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

pub(crate) fn draw_cluster_selection_markers(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let visuals = resolve_overlay_visuals(overlay.tuning);
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
        let (text_w, text_h) = ui_text_size_in(
            overlay.render_state,
            &overlay.tuning.font,
            "SEL",
            SELECT_MARKER_SCALE,
        );
        let rect = Rectangle::<i32, Physical>::new(
            (
                (sx - (text_w + SELECT_MARKER_PAD_X * 2) / 2),
                (sy - (text_h + SELECT_MARKER_PAD_Y * 2) / 2),
            )
                .into(),
            (
                text_w + SELECT_MARKER_PAD_X * 2,
                text_h + SELECT_MARKER_PAD_Y * 2,
            )
                .into(),
        );
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            rect,
            10.0,
            visuals.palette.key_fill.alpha(0.96),
            false,
            damage,
            1.0,
        )?;
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            rect.loc.x + ((rect.size.w - text_w).max(0) / 2),
            rect.loc.y + (rect.size.h - text_h) / 2,
            "SEL",
            SELECT_MARKER_SCALE,
            visuals.palette.text.alpha(1.0),
            damage,
        )?;
    }
    Ok(())
}

fn draw_overlay_chip(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    draw_border: bool,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = render_state.ui_rect_program(visuals.rounded) else {
        return Ok(());
    };
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let border_px = if draw_border { visuals.border_px } else { 0.0 };
    let uniforms = [
        Uniform::new(
            "node_color",
            (
                visuals.palette.border.r,
                visuals.palette.border.g,
                visuals.palette.border.b,
                1.0f32,
            ),
        ),
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
        Uniform::new(
            "inner_rect_size",
            (
                (rect.size.w as f32 - border_px * 2.0).max(1.0),
                (rect.size.h as f32 - border_px * 2.0).max(1.0),
            ),
        ),
        Uniform::new(
            "inner_rect_offset",
            (border_px.max(0.0), border_px.max(0.0)),
        ),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("inner_corner_radius", (corner_radius - border_px).max(0.0)),
        Uniform::new("border_px", border_px),
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

#[cfg(test)]
mod tests {
    use halley_config::{OverlayColorMode, OverlayShape};

    use super::resolve_overlay_visuals;

    #[test]
    fn overlay_auto_text_tracks_background_contrast() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.overlay_style.background_color = OverlayColorMode::Dark;

        let visuals = resolve_overlay_visuals(&tuning);

        assert!(visuals.palette.text.luminance() > visuals.palette.fill.luminance());
    }

    #[test]
    fn overlay_shape_and_border_width_follow_overlay_config() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.border_size_px = 5;
        tuning.overlay_style.shape = OverlayShape::Rounded;
        tuning.overlay_style.borders = true;

        let visuals = resolve_overlay_visuals(&tuning);

        assert!(visuals.rounded);
        assert_eq!(visuals.border_px, 5.0);

        tuning.overlay_style.borders = false;
        let visuals = resolve_overlay_visuals(&tuning);
        assert_eq!(visuals.border_px, 0.0);
    }
}
