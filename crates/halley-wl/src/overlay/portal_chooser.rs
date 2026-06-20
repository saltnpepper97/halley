//! Halley-styled source chooser overlay for xdg-desktop-portal ScreenCast.
//!
//! Visual language intentionally mirrors the screenshot capture menu (same
//! palette, rounded chips, glow ring) so portal source picking feels native.
//! Two phases: a bottom bar of "Screen"/"Window" entries, and a click-to-pick
//! window phase with a dimmed backdrop.

use std::error::Error;

use halley_api::CaptureMode;
use halley_capit::CaptureCrop;
use smithay::{
    backend::renderer::Color32F,
    utils::{Physical, Rectangle, Transform},
};

use crate::compositor::portal_chooser::{
    PortalChooserPhase, portal_chooser_active, portal_chooser_entries,
};
use crate::compositor::root::Halley;
use crate::input::active_node_screen_rect;
use crate::render::draw_primitives::{draw_outline_rect, draw_rect};
use crate::render::shadow::draw_shadow_rect;
use crate::render::{
    screenshot_menu_background_color, screenshot_menu_highlight_color,
    screenshot_menu_icon_texture, screenshot_menu_inactive_highlight_color,
    screenshot_menu_item_fill_color,
};

use super::OverlayView;
use super::screenshot::{
    draw_screenshot_menu_chip, draw_screenshot_window_overlay, resolve_screenshot_menu_style,
};

const BACKDROP_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.45);
const BAR_W: i32 = 360;
const BAR_H: i32 = 80;
const PAD: i32 = 10;
const ICON_SIZE: i32 = 42;
const ACTIVE_ALPHA: f32 = 1.0;
const INACTIVE_ALPHA: f32 = 0.72;
const DISABLED_ALPHA: f32 = 0.32;
const SCREEN_PICK_DIM: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.28);
const WINDOW_PICK_DIM: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.18);

fn bar_rect(screen_w: i32, screen_h: i32) -> Rectangle<i32, Physical> {
    let total = BAR_W.min(screen_w - 24);
    Rectangle::new(
        ((screen_w - total) / 2, screen_h - BAR_H - 28).into(),
        (total, BAR_H).into(),
    )
}

pub(crate) fn slot_rect(index: usize, screen_w: i32, screen_h: i32) -> Rectangle<i32, Physical> {
    let bar = bar_rect(screen_w, screen_h);
    let count = 2;
    let slot = bar.size.w / count;
    Rectangle::new(
        (bar.loc.x + index as i32 * slot, bar.loc.y).into(),
        (
            if index + 1 == count as usize {
                bar.size.w - slot * index as i32
            } else {
                slot
            },
            bar.size.h,
        )
            .into(),
    )
}

pub(crate) fn portal_chooser_menu_hit_test(
    screen_w: i32,
    screen_h: i32,
    sx: f32,
    sy: f32,
    count: usize,
) -> Option<usize> {
    let px = sx.round() as i32;
    let py = sy.round() as i32;
    for idx in 0..count {
        let rect = slot_rect(idx, screen_w, screen_h);
        if px >= rect.loc.x
            && px <= rect.loc.x + rect.size.w
            && py >= rect.loc.y
            && py <= rect.loc.y + rect.size.h
        {
            return Some(idx);
        }
    }
    None
}

fn color32f(c: halley_config::DecorationBorderColor, alpha: f32) -> Color32F {
    Color32F::new(c.r, c.g, c.b, alpha)
}

pub(crate) fn draw_portal_chooser_overlay(
    frame: &mut smithay::backend::renderer::gles::GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if !portal_chooser_active(st) {
        return Ok(());
    }
    let overlay = OverlayView::from_halley(st);
    let background = screenshot_menu_background_color(overlay.tuning);
    let highlight = screenshot_menu_highlight_color(overlay.tuning);
    let item_fill = screenshot_menu_item_fill_color(overlay.tuning);
    let inactive = screenshot_menu_inactive_highlight_color(overlay.tuning);
    let style = resolve_screenshot_menu_style(overlay.tuning);

    let (phase, menu_selected, menu_hovered, hovered_monitor, hovered_window) = st
        .input
        .interaction_state
        .portal_chooser
        .as_ref()
        .map(|s| {
            (
                s.phase,
                s.menu_selected,
                s.menu_hovered,
                s.hovered_monitor
                    .clone()
                    .unwrap_or_else(|| s.monitor.clone()),
                s.hovered_window,
            )
        })
        .unwrap_or((PortalChooserPhase::Menu, 0, None, String::new(), None));

    match phase {
        PortalChooserPhase::ScreenPick => {
            let monitor = st.model.monitor_state.current_monitor.as_str();
            let selected = hovered_monitor == monitor;
            if selected {
                draw_outline_rect(
                    frame,
                    0,
                    0,
                    screen_w,
                    screen_h,
                    color32f(highlight, ACTIVE_ALPHA),
                    damage,
                )?;
            } else {
                draw_rect(frame, 0, 0, screen_w, screen_h, SCREEN_PICK_DIM, damage)?;
            }
        }
        PortalChooserPhase::WindowPick => {
            let selection_rect = hovered_window.and_then(|node_id| {
                window_overlay_rect(st, node_id, screen_w, screen_h).map(|rect| CaptureCrop {
                    x: rect.loc.x,
                    y: rect.loc.y,
                    w: rect.size.w,
                    h: rect.size.h,
                })
            });
            draw_screenshot_window_overlay(
                frame,
                selection_rect,
                0,
                0,
                screen_w,
                screen_h,
                WINDOW_PICK_DIM,
                damage,
            )?;
        }
        PortalChooserPhase::Menu => {
            draw_rect(frame, 0, 0, screen_w, screen_h, BACKDROP_COLOR, damage)?;
            let bar = bar_rect(screen_w, screen_h);
            draw_shadow_rect(
                frame,
                overlay.render_state,
                overlay.tuning.effects.shadows.overlay,
                bar,
                if style.rounded {
                    style.bar_corner_radius
                } else {
                    0.0
                },
                1.0,
                damage,
            )?;
            draw_screenshot_menu_chip(
                frame,
                overlay.render_state,
                bar,
                style.rounded,
                style.bar_corner_radius,
                style.outer_border_px,
                color32f(background, 0.96),
                color32f(highlight, ACTIVE_ALPHA),
                damage,
            )?;

            let entries = portal_chooser_entries(st);
            for (idx, entry) in entries.iter().enumerate() {
                let active = entry.enabled && (menu_hovered == Some(idx) || menu_selected == idx);
                let fill = if active {
                    color32f(background, 0.96)
                } else if entry.enabled {
                    color32f(item_fill, 0.94)
                } else {
                    color32f(item_fill, 0.42)
                };
                let border = if active {
                    color32f(highlight, ACTIVE_ALPHA)
                } else if entry.enabled {
                    color32f(inactive, INACTIVE_ALPHA)
                } else {
                    color32f(inactive, DISABLED_ALPHA)
                };
                let rect = slot_rect(idx, screen_w, screen_h);
                let item_rect = Rectangle::new(
                    (rect.loc.x + PAD, rect.loc.y + PAD).into(),
                    (rect.size.w - PAD * 2, rect.size.h - PAD * 2).into(),
                );
                draw_screenshot_menu_chip(
                    frame,
                    overlay.render_state,
                    item_rect,
                    style.rounded,
                    style.item_corner_radius,
                    style.item_border_px,
                    fill,
                    border,
                    damage,
                )?;
                let mode = if entry.is_window {
                    CaptureMode::Window
                } else {
                    CaptureMode::Screen
                };
                if let Some(icon) = screenshot_menu_icon_texture(st, mode, active) {
                    let dest = Rectangle::new(
                        (
                            item_rect.loc.x + (item_rect.size.w - ICON_SIZE) / 2,
                            item_rect.loc.y + (item_rect.size.h - ICON_SIZE) / 2,
                        )
                            .into(),
                        (ICON_SIZE, ICON_SIZE).into(),
                    );
                    let src = Rectangle::new(
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
                        if entry.enabled { 1.0 } else { 0.38 },
                        None,
                        &[],
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn window_overlay_rect(
    st: &Halley,
    node_id: halley_core::field::NodeId,
    screen_w: i32,
    screen_h: i32,
) -> Option<Rectangle<i32, Physical>> {
    let monitor = st.model.monitor_state.current_monitor.as_str();
    let node = st.model.field.node(node_id)?;
    if node.kind != halley_core::field::NodeKind::Surface
        || node.state != halley_core::field::NodeState::Active
        || !st.model.field.is_visible(node_id)
        || st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .map(|name| name.as_str())
            != Some(monitor)
    {
        return None;
    }
    let (left, top, right, bottom) = active_node_screen_rect(
        st,
        screen_w,
        screen_h,
        node_id,
        std::time::Instant::now(),
        None,
    )?;
    let x = left.min(right).round() as i32;
    let y = top.min(bottom).round() as i32;
    let w = (right - left).abs().round().max(1.0) as i32;
    let h = (bottom - top).abs().round().max(1.0) as i32;
    Some(Rectangle::new((x, y).into(), (w, h).into()))
}
