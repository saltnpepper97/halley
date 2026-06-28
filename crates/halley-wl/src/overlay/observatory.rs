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

use halley_core::cluster::ClusterId;
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::field::NodeId;

use super::{
    OverlayView, OverlayVisuals, draw_overflow_member_chip, draw_overlay_backdrop_blur,
    draw_overlay_chip, draw_overlay_chip_with_border_color, draw_overlay_chip_without_shadow,
    draw_overlay_ring, overlay_accent_fill, overlay_text_color_for_fill,
    preview_source::{center_crop_to_aspect, preview_src_uv, window_preview_source_rect},
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

    // Preview from the captured offscreen texture when one is ready. Fullscreen and
    // game tiles are now captured while apogee is open (they're composited then), so
    // they get a real preview too; anything without a usable texture falls back to
    // the app icon below.
    let preview = overlay
        .render_state
        .cache
        .window_offscreen_cache
        .get(&tile.node_id)
        .filter(|cache| cache.has_content)
        .and_then(|cache| Some((cache.texture.as_ref()?, cache.bbox?)));

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

/// Resolve the cluster id for a core tile. Works whether the core node still
/// lives in the field (collapsed cluster) or has been moved into the cluster's
/// active-workspace storage (opened cluster shown collapsed in field view),
/// because the cluster record keeps its `core` reference either way.
fn core_tile_cluster_id(overlay: &OverlayView<'_>, tile: &ApogeeTile) -> Option<ClusterId> {
    overlay.field.cluster_id_for_core_public(tile.node_id)
}

/// Resolve a core tile's display name, preferring the cluster's recorded name
/// (which survives the core node being moved out of the field during a
/// workspace session) over the generic window-label fallback.
fn core_tile_label(st: &Halley, overlay: &OverlayView<'_>, tile: &ApogeeTile) -> String {
    if let Some(cid) = core_tile_cluster_id(overlay, tile)
        && let Some(name) = crate::compositor::clusters::system::cluster_display_name(st, cid)
    {
        return name;
    }
    tile_label(overlay, tile)
}

/// One member's slot inside the expanded cluster viewport, expressed as
/// fractions of the viewport body. `alpha` modulates the thumbnail's draw alpha
/// so stacking back-cards recede and the front card reads as focused.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MemberSlot {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    alpha: f32,
}

/// Layout gap between member thumbnails, as a fraction of the viewport body.
const MEMBER_GAP: f32 = 0.05;

/// Compute the member slots for the expanded cluster viewport. Returns the
/// visible slots (in draw order, back-to-front) plus the count of members not
/// represented (the "+N" overflow tail). Pure / drawable-agnostic so the
/// geometry can be unit-tested independently of the renderer.
fn cluster_member_slots(
    member_count: usize,
    layout: ClusterWorkspaceLayoutKind,
) -> (Vec<MemberSlot>, usize) {
    use ClusterWorkspaceLayoutKind as L;
    let n = member_count.max(1);
    match layout {
        L::Tiling => tiling_member_slots(n),
        L::Stacking => stacking_member_slots(n),
    }
}

/// Tiling: a master slot on the left, a stack column on the right split into up
/// to three rows; surplus stack members roll into the overflow tail.
fn tiling_member_slots(n: usize) -> (Vec<MemberSlot>, usize) {
    let gap = MEMBER_GAP;
    if n == 1 {
        return (
            vec![MemberSlot {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
                alpha: 1.0,
            }],
            0,
        );
    }
    let master_w = 0.58;
    let stack_x = master_w + gap;
    let stack_w = 1.0 - stack_x;
    let mut slots = vec![MemberSlot {
        x: 0.0,
        y: 0.0,
        w: master_w,
        h: 1.0,
        alpha: 1.0,
    }];
    let stack_members = n - 1;
    let visible_rows = stack_members.min(3);
    let row_h = (1.0 - (visible_rows - 1) as f32 * gap) / visible_rows as f32;
    for r in 0..visible_rows {
        let y = r as f32 * (row_h + gap);
        slots.push(MemberSlot {
            x: stack_x,
            y,
            w: stack_w,
            h: row_h,
            alpha: 1.0,
        });
    }
    let overflow = stack_members.saturating_sub(visible_rows);
    (slots, overflow)
}

/// Stacking: up to four offset layered cards, the front one fully opaque and
/// the rearmost faded so real thumbnails still read as a deck.
fn stacking_member_slots(n: usize) -> (Vec<MemberSlot>, usize) {
    let visible = n.min(4);
    let step = 0.06;
    // Draw the back card first so later cards layer on top of it.
    let mut slots = Vec::new();
    for back in (0..visible).rev() {
        let off = back as f32 * step;
        let size = 1.0 - off;
        let alpha = if visible <= 1 {
            1.0
        } else {
            (0.45 + 0.55 * (1.0 - back as f32 / (visible - 1) as f32)).clamp(0.45, 1.0)
        };
        slots.push(MemberSlot {
            x: off * 0.5,
            y: off * 0.5,
            w: size,
            h: size,
            alpha,
        });
    }
    let overflow = n.saturating_sub(visible);
    (slots, overflow)
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
    // Frosted label: a Dual-Kawase backdrop blur (no-op when overlay blur is off,
    // leaving the plain tint below) plus a translucent tint so the text stays
    // legible over the blurred window thumbnail behind it.
    draw_overlay_backdrop_blur(frame, label_rect, 8.0, damage, alpha)?;
    let fill = if hovered {
        overlay_accent_fill(visuals, 0.55, 0.62 * alpha)
    } else {
        Color32F::new(0.02, 0.025, 0.035, 0.5 * alpha)
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

/// Expanded cluster-viewport target size (physical px), before screen clamping.
/// Kept modest so the window mosaic still gets the bulk of the overview; the
/// Apogee core band is sized to reserve room for it (see `apogee_core_bar_height`).
const EXPANDED_CORE_W: f32 = 168.0;
const EXPANDED_CORE_H: f32 = 112.0;

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// The core tile's rect as it expands from the resting round icon into the
/// cluster viewport, keeping the slot's center fixed so the expansion reads as
/// the icon clearing in place. `mix` is 0.0 at the icon and 1.0 fully open.
fn expanded_core_rect(
    slot: Rectangle<i32, Physical>,
    mix: f32,
    screen_w: i32,
    screen_h: i32,
) -> Rectangle<i32, Physical> {
    let cx = slot.loc.x + slot.size.w / 2;
    let cy = slot.loc.y + slot.size.h / 2;
    let rest_w = slot.size.w.max(1) as f32;
    let rest_h = slot.size.h.max(1) as f32;
    let target_w = EXPANDED_CORE_W.min(screen_w.max(1) as f32 * 0.24);
    let target_h = EXPANDED_CORE_H.min(screen_h.max(1) as f32 * 0.20);
    let w = lerp_f32(rest_w, target_w, mix).round().max(1.0) as i32;
    let h = lerp_f32(rest_h, target_h, mix).round().max(1.0) as i32;
    Rectangle::<i32, Physical>::new(
        (cx - w / 2, cy - h / 2).into(),
        (w, h).into(),
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_core_tile(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    slot_rect: Rectangle<i32, Physical>,
    alpha: f32,
    hovered: bool,
    mix: f32,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let mix = mix.clamp(0.0, 1.0);
    // The resting icon fades out exactly as the cluster viewport fades in, so the
    // tile reads as its glass clearing rather than two layers stacking.
    let icon_alpha = alpha * (1.0 - mix);
    let viewport_alpha = alpha * mix;

    if icon_alpha > 0.01 {
        draw_core_icon(frame, st, overlay, visuals, tile, slot_rect, hovered, icon_alpha, damage)?;
    }

    // The expanded rect is always computed so the label slides down with it.
    let body_rect = expanded_core_rect(slot_rect, mix, screen_w, screen_h);
    if viewport_alpha > 0.01 {
        draw_core_cluster_viewport(
            frame,
            st,
            overlay,
            visuals,
            tile,
            body_rect,
            viewport_alpha,
            screen_w,
            screen_h,
            damage,
        )?;
    }

    // Persistent label beneath the current (possibly expanded) rect.
    draw_core_persistent_label(
        frame, st, overlay, visuals, tile, body_rect, alpha, hovered, damage,
    )?;
    Ok(())
}

/// The resting cluster core: its round chip, ring, and cluster icon (or a
/// fallback glyph). Drawn at `alpha`, which the caller lowers as the in-place
/// viewport opens so the icon dissolves into the cluster view.
#[allow(clippy::too_many_arguments)]
fn draw_core_icon(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    rect: Rectangle<i32, Physical>,
    hovered: bool,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if alpha <= 0.01 {
        return Ok(());
    }
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
    let (text_w, text_h) = ui_text_size_in(overlay.render_state, &overlay.tuning.font, label.as_str(), 1);
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


/// Always-on compact name chip beneath a core tile. Strengthens slightly on
/// hover/focus so the expanded cluster's label reads clearly, but stays
/// legible at rest so every cluster is identifiable without hovering.
#[allow(clippy::too_many_arguments)]
fn draw_core_persistent_label(
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
    let text = core_tile_label(st, overlay, tile);
    if text.is_empty() {
        return Ok(());
    }
    let max_w = (rect.size.w + 56).clamp(72, 200);
    let label = truncate_overlay_text_to_width(
        overlay.render_state,
        &overlay.tuning.font,
        text.as_str(),
        1,
        max_w - 14,
    );
    let (text_w, text_h) =
        ui_text_size_in(overlay.render_state, &overlay.tuning.font, label.as_str(), 1);
    let chip_w = (text_w + 14).clamp(40, max_w);
    let chip_h = (text_h + 8).clamp(18, 26);
    let chip = Rectangle::<i32, Physical>::new(
        (
            rect.loc.x + (rect.size.w - chip_w) / 2,
            rect.loc.y + rect.size.h + 6,
        )
            .into(),
        (chip_w, chip_h).into(),
    );
    draw_overlay_backdrop_blur(frame, chip, 6.0, damage, 0.7 * alpha)?;
    let fill = if hovered {
        overlay_accent_fill(visuals, 0.5, 0.7 * alpha)
    } else {
        Color32F::new(0.03, 0.035, 0.05, 0.5 * alpha)
    };
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        chip,
        6.0,
        fill,
        false,
        damage,
        alpha,
    )?;
    let label_color = overlay_text_color_for_fill(fill, alpha);
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        chip.loc.x + ((chip.size.w - text_w).max(0) / 2),
        chip.loc.y + ((chip.size.h - text_h).max(0) / 2),
        label.as_str(),
        1,
        label_color,
        damage,
    )?;
    Ok(())
}

/// The in-place cluster viewport: the core tile's body becomes a small frosted
/// window into the cluster, laid out just like the cluster's workspace (master +
/// stack for tiling, layered cards for stacking) using each member's real
/// offscreen thumbnail. Drawn inside the expanded core rect, so the cluster icon
/// dissolving away reveals this in the same spot.
#[allow(clippy::too_many_arguments)]
fn draw_core_cluster_viewport(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    tile: &ApogeeTile,
    body: Rectangle<i32, Physical>,
    alpha: f32,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let _ = screen_w;
    let _ = screen_h;
    let Some(cid) = core_tile_cluster_id(overlay, tile) else {
        // Not a resolvable cluster: show a plain frosted card so the expand still
        // reads cleanly instead of a half-drawn icon.
        let radius = 14.0_f32.min(body.size.w.min(body.size.h) as f32 * 0.18);
        draw_overlay_backdrop_blur(frame, body, 12.0, damage, alpha)?;
        draw_overlay_chip_without_shadow(
            frame,
            overlay.render_state,
            visuals,
            body,
            radius,
            visuals.palette.fill.alpha(0.78 * alpha),
            true,
            damage,
            alpha,
        )?;
        return Ok(());
    };
    let members: Vec<NodeId> = overlay
        .field
        .cluster(cid)
        .map(|cluster| cluster.members().to_vec())
        .unwrap_or_default();
    let layout = st.runtime.tuning.cluster_layout_kind();

    let radius = 14.0_f32.min(body.size.w.min(body.size.h) as f32 * 0.18);
    draw_overlay_backdrop_blur(frame, body, 12.0, damage, alpha)?;
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        body,
        radius,
        visuals.palette.fill.alpha(0.82 * alpha),
        true,
        damage,
        alpha,
    )?;

    let pad = 6;
    let inner = inset_rect(body, pad);
    let (slots, overflow) = cluster_member_slots(members.len(), layout);
    let aw = inner.size.w.max(1) as f32;
    let ah = inner.size.h.max(1) as f32;
    for (slot, member_id) in slots.iter().zip(members.iter()) {
        let sx = inner.loc.x + (slot.x * aw).round() as i32;
        let sy = inner.loc.y + (slot.y * ah).round() as i32;
        let sw = ((slot.w * aw).round() as i32).max(1);
        let sh = ((slot.h * ah).round() as i32).max(1);
        let slot_rect = Rectangle::<i32, Physical>::new((sx, sy).into(), (sw, sh).into());
        draw_member_thumbnail(
            frame,
            overlay,
            visuals,
            *member_id,
            slot_rect,
            alpha * slot.alpha,
            damage,
        )?;
    }
    if overflow > 0 {
        let text = format!("+{overflow}");
        let (tw, th) = ui_text_size_in(overlay.render_state, &overlay.tuning.font, &text, 1);
        let tx = body.loc.x + body.size.w - tw - 6;
        let ty = body.loc.y + body.size.h - th - 6;
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            tx,
            ty,
            &text,
            1,
            visuals.palette.subtext.alpha(0.95 * alpha),
            damage,
        )?;
    }
    Ok(())
}

/// Draw a single cluster member's thumbnail into `slot_rect`: the member's real
/// offscreen texture aspect-fit into the slot (with rounded corners), or a dark
/// rounded placeholder when no texture has been captured yet. Reuses the same
/// rounded-texture shader path as the Apogee window previews.
fn draw_member_thumbnail(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    visuals: &OverlayVisuals,
    node_id: NodeId,
    slot_rect: Rectangle<i32, Physical>,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let radius = 5.0_f32.min(slot_rect.size.w.min(slot_rect.size.h) as f32 * 0.18);
    // Dark backing so empty slots and CSD-letterboxed previews share a frame.
    draw_overlay_chip_without_shadow(
        frame,
        overlay.render_state,
        visuals,
        slot_rect,
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
    let Some((texture, bbox)) = preview else {
        return Ok(());
    };
    let source = window_preview_source_rect(overlay, node_id, bbox);
    let slot_aspect = slot_rect.size.w.max(1) as f32 / slot_rect.size.h.max(1) as f32;
    let source = center_crop_to_aspect(source, slot_aspect);
    let dst = slot_rect;
    let src = Rectangle::<f64, Buffer>::new(
        (source.x as f64, source.y as f64).into(),
        (source.w.max(1.0) as f64, source.h.max(1.0) as f64).into(),
    );
    let (src_uv_offset, src_uv_scale) = preview_src_uv(texture, source);
    let corner = radius.min(dst.size.w.min(dst.size.h) as f32 * 0.5);
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
        alpha.clamp(0.0, 1.0),
        program,
        if program.is_some() { &uniforms } else { &[] },
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
    // Extract everything needed from the session up front so the immutable
    // borrow of `st` ends before the hover-mix advancement below mutates
    // `st.ui.render_state` (which the `OverlayView` would otherwise alias).
    let (tiles, core_tiles, core_offset, progress, overlay_alpha, tile_alpha, phase_open, hovered_node, hovered_overlay_node) =
        match st.input.interaction_state.apogee_session.as_ref() {
            None => return Ok(false),
            Some(session) => {
                let current_monitor = st.model.monitor_state.current_monitor.clone();
                let Some(monitor_session) = session.monitor_session(current_monitor.as_str())
                else {
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
                (
                    monitor_session.tiles.clone(),
                    monitor_session.core_tiles.clone(),
                    monitor_session.core_scroll_offset,
                    progress,
                    overlay_alpha,
                    tile_alpha,
                    matches!(session.phase, ApogeePhase::Open),
                    st.input.interaction_state.apogee_live_preview_node,
                    st.input.interaction_state.apogee_hover_node,
                )
            }
        };

    // Advance each core's expand/collapse mix. Only the hovered/keyboard-focused
    // core trends toward 1.0 (and only while the overview is fully open); every
    // other core decays back to its resting icon.
    let hovered_core_id = if phase_open { hovered_overlay_node } else { None };
    let mut core_mixes: std::collections::HashMap<NodeId, f32> =
        std::collections::HashMap::with_capacity(core_tiles.len());
    for tile in &core_tiles {
        let hovered = hovered_core_id == Some(tile.node_id);
        core_mixes.insert(
            tile.node_id,
            st.ui.render_state.apogee_core_hover_mix(tile.node_id, hovered),
        );
    }

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
            overlay.tuning.apogee.background_dim * overlay_alpha,
        ),
        damage,
    )?;

    // The single expanding core is deferred so its in-place viewport draws on
    // top of the rest of the rail and the window mosaic.
    let mut deferred_core: Option<(ApogeeTile, Rectangle<i32, Physical>, bool, f32)> = None;

    for tile in &core_tiles {
        let mut target = tile.to;
        target.cx -= core_offset;
        let rect = tile_screen_rect(tile.from.lerp(target, progress));
        if rect.loc.x > screen_w || rect.loc.x + rect.size.w < 0 {
            continue;
        }
        let hovered = phase_open && hovered_overlay_node == Some(tile.node_id);
        let mix = core_mixes.get(&tile.node_id).copied().unwrap_or(0.0);
        if mix > 0.01 {
            // Defer the expanding core so its in-place viewport draws on top of
            // neighbouring cores (and the window mosaic) instead of being
            // overdrawn by the next core in the rail.
            deferred_core = Some((*tile, rect, hovered, mix));
            continue;
        }
        draw_core_tile(
            frame,
            &*st,
            &overlay,
            &visuals,
            tile,
            rect,
            tile_alpha,
            hovered,
            mix,
            screen_w,
            screen_h,
            damage,
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

    // The expanding core draws last so its viewport floats above the rail.
    if let Some((tile, rect, hovered, mix)) = deferred_core {
        draw_core_tile(
            frame,
            &*st,
            &overlay,
            &visuals,
            &tile,
            rect,
            tile_alpha,
            hovered,
            mix,
            screen_w,
            screen_h,
            damage,
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

    #[test]
    fn tiling_member_slots_single_member_is_one_full_slot() {
        let (slots, overflow) = cluster_member_slots(1, ClusterWorkspaceLayoutKind::Tiling);

        assert_eq!(overflow, 0);
        assert_eq!(slots.len(), 1);
        assert!((slots[0].w - 1.0).abs() < 1e-4);
        assert!((slots[0].h - 1.0).abs() < 1e-4);
        assert!((slots[0].alpha - 1.0).abs() < 1e-4);
    }

    #[test]
    fn tiling_member_slots_draws_master_plus_stack_rows() {
        // Four members: master + three stack rows, no overflow.
        let (slots, overflow) = cluster_member_slots(4, ClusterWorkspaceLayoutKind::Tiling);

        assert_eq!(overflow, 0);
        // 1 master + 3 stack rows.
        assert_eq!(slots.len(), 4);
        let master = slots[0];
        // Master sits on the left, stack on the right.
        let stack_slots = &slots[1..];
        assert!(stack_slots.iter().all(|s| s.x > master.x));
        // Stack rows are vertically ordered and non-overlapping.
        for w in stack_slots.windows(2) {
            assert!(w[0].y + w[0].h <= w[1].y + 1e-4);
        }
    }

    #[test]
    fn tiling_member_slots_caps_stack_and_reports_overflow() {
        // Six members: master + three visible stack rows + two overflow.
        let (slots, overflow) = cluster_member_slots(6, ClusterWorkspaceLayoutKind::Tiling);

        assert_eq!(overflow, 2);
        assert_eq!(slots.len(), 4);
    }

    #[test]
    fn stacking_member_slots_layers_cards_front_opaquest() {
        let (slots, _overflow) = cluster_member_slots(3, ClusterWorkspaceLayoutKind::Stacking);

        assert_eq!(slots.len(), 3);
        // Front card is last and at least as opaque as the back card.
        let front = slots.last().unwrap();
        let back = slots.first().unwrap();
        assert!(front.alpha >= back.alpha);
        // Each card stays within the viewport body.
        for s in &slots {
            assert!(s.x >= -1e-4 && s.y >= -1e-4);
            assert!(s.x + s.w <= 1.0 + 1e-4 && s.y + s.h <= 1.0 + 1e-4);
        }
    }

    #[test]
    fn stacking_member_slots_caps_at_four_visible_cards() {
        let (slots, overflow) = cluster_member_slots(9, ClusterWorkspaceLayoutKind::Stacking);

        assert_eq!(slots.len(), 4);
        assert_eq!(overflow, 5);
    }

    #[test]
    fn expanded_core_rect_keeps_center_and_grows_with_mix() {
        let slot = Rectangle::<i32, Physical>::new((100, 80).into(), (68, 68).into());
        let cx = slot.loc.x + slot.size.w / 2;
        let cy = slot.loc.y + slot.size.h / 2;

        // At mix 0 the rect matches the resting icon, centred on the slot.
        let rest = expanded_core_rect(slot, 0.0, 1920, 1080);
        assert_eq!(rest.size, (68, 68).into());
        assert_eq!(rest.loc.x + rest.size.w / 2, cx);
        assert_eq!(rest.loc.y + rest.size.h / 2, cy);

        // At mix 1 it has grown, still centred on the slot, and clamped to screen.
        let open = expanded_core_rect(slot, 1.0, 1920, 1080);
        assert!(open.size.w > rest.size.w);
        assert!(open.size.h > rest.size.h);
        assert_eq!(open.loc.x + open.size.w / 2, cx);
        assert_eq!(open.loc.y + open.size.h / 2, cy);
    }
}
