use std::error::Error;

use halley_capit::CaptureCrop;
use halley_config::RuntimeTuning;
use halley_ipc::CaptureMode;
use smithay::{
    backend::renderer::{
        gles::{GlesFrame, Uniform},
        Color32F, Texture,
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::render::state::RenderState;
use crate::render::utils::{draw_outline_rect, draw_rect, draw_ring};
use crate::render::{
    screenshot_menu_background_color, screenshot_menu_highlight_color,
    screenshot_menu_icon_texture, screenshot_menu_inactive_highlight_color,
    screenshot_menu_item_fill_color,
};

use super::OverlayView;

const BORDER_THICKNESS: i32 = 2;
const HANDLE_SIZE: i32 = 12;
const DIM_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.40);
const SCREEN_DIM_COLOR: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.28);
const WINDOW_CAPTURE_FILL: Color32F = Color32F::new(0.45, 0.45, 0.45, 0.34);
const WINDOW_CAPTURE_OUTLINE: Color32F = Color32F::new(0.72, 0.72, 0.72, 0.78);
const SHADOW_COLOR_1: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.16);
const SHADOW_COLOR_2: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.10);
const DASH_LEN: i32 = 10;
const GAP_LEN: i32 = 6;
const MENU_BAR_W: i32 = 420;
const MENU_BAR_H: i32 = 80;
const MENU_SLOT_W: i32 = MENU_BAR_W / 3;
const MENU_PAD: i32 = 10;
const MENU_ICON_SIZE: i32 = 42;
const MENU_ITEM_BORDER_PX: f32 = 2.0;
const ACTIVE_BORDER_ALPHA: f32 = 1.0;
const INACTIVE_BORDER_ALPHA: f32 = 0.72;
const MENU_BAR_CORNER_RADIUS: f32 = 3.0;
const MENU_ITEM_CORNER_RADIUS: f32 = 2.0;

#[derive(Clone, Copy)]
pub(crate) enum ScreenshotMenuHit {
    Item(usize),
}

#[derive(Clone, Copy)]
struct RectLocal {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy)]
struct ScreenshotMenuStyle {
    rounded: bool,
    outer_border_px: f32,
    item_border_px: f32,
    bar_corner_radius: f32,
    item_corner_radius: f32,
}

fn resolve_screenshot_menu_style(tuning: &RuntimeTuning) -> ScreenshotMenuStyle {
    let visuals = super::resolve_overlay_visuals(tuning);
    ScreenshotMenuStyle {
        rounded: visuals.rounded,
        outer_border_px: visuals.border_px,
        item_border_px: MENU_ITEM_BORDER_PX,
        bar_corner_radius: if visuals.rounded {
            MENU_BAR_CORNER_RADIUS
        } else {
            0.0
        },
        item_corner_radius: if visuals.rounded {
            MENU_ITEM_CORNER_RADIUS
        } else {
            0.0
        },
    }
}

fn draw_rect_border(
    frame: &mut GlesFrame<'_, '_>,
    rect: Rectangle<i32, Physical>,
    border_px: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let border_px = border_px
        .max(0)
        .min((rect.size.w / 2).max(0))
        .min((rect.size.h / 2).max(0));
    if border_px <= 0 {
        return Ok(());
    }
    draw_rect(
        frame,
        rect.loc.x,
        rect.loc.y,
        rect.size.w,
        border_px,
        color,
        damage,
    )?;
    draw_rect(
        frame,
        rect.loc.x,
        rect.loc.y + rect.size.h - border_px,
        rect.size.w,
        border_px,
        color,
        damage,
    )?;
    draw_rect(
        frame,
        rect.loc.x,
        rect.loc.y,
        border_px,
        rect.size.h,
        color,
        damage,
    )?;
    draw_rect(
        frame,
        rect.loc.x + rect.size.w - border_px,
        rect.loc.y,
        border_px,
        rect.size.h,
        color,
        damage,
    )?;
    Ok(())
}

fn draw_screenshot_menu_chip(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    rect: Rectangle<i32, Physical>,
    rounded: bool,
    corner_radius: f32,
    border_px: f32,
    fill_color: Color32F,
    border_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if let (Some(texture), Some(program)) = (
        render_state.node_circle_texture.as_ref(),
        render_state.ui_rect_program(rounded),
    ) {
        let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
        let src = Rectangle::<f64, Buffer>::new(
            (0.0, 0.0).into(),
            (tex_size.w as f64, tex_size.h as f64).into(),
        );
        let border_px = border_px.max(0.0);
        let uniforms = [
            Uniform::new(
                "node_color",
                (
                    border_color.r(),
                    border_color.g(),
                    border_color.b(),
                    border_color.a(),
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
            1.0,
            Some(program),
            &uniforms,
        )?;
        return Ok(());
    }

    draw_rect(
        frame,
        rect.loc.x,
        rect.loc.y,
        rect.size.w,
        rect.size.h,
        fill_color,
        damage,
    )?;
    draw_rect_border(
        frame,
        rect,
        border_px.round().max(0.0) as i32,
        border_color,
        damage,
    )?;
    Ok(())
}

fn screenshot_menu_modes() -> [CaptureMode; 3] {
    [
        CaptureMode::Region,
        CaptureMode::Screen,
        CaptureMode::Window,
    ]
}

fn screenshot_menu_rect(index: usize, screen_w: i32, screen_h: i32) -> Rectangle<i32, Physical> {
    let start_x = (screen_w - MENU_BAR_W) / 2;
    let y = screen_h - MENU_BAR_H - 24;
    Rectangle::<i32, Physical>::new(
        (start_x + index as i32 * MENU_SLOT_W, y).into(),
        (MENU_SLOT_W, MENU_BAR_H).into(),
    )
}

fn screenshot_menu_bar_rect(screen_w: i32, screen_h: i32) -> Rectangle<i32, Physical> {
    Rectangle::<i32, Physical>::new(
        (((screen_w - MENU_BAR_W) / 2), screen_h - MENU_BAR_H - 24).into(),
        (MENU_BAR_W, MENU_BAR_H).into(),
    )
}

pub(crate) fn screenshot_menu_hit_test(
    screen_w: i32,
    screen_h: i32,
    sx: f32,
    sy: f32,
) -> Option<ScreenshotMenuHit> {
    for idx in 0..3 {
        let rect = screenshot_menu_rect(idx, screen_w, screen_h);
        if (sx.round() as i32) >= rect.loc.x
            && (sx.round() as i32) <= rect.loc.x + rect.size.w
            && (sy.round() as i32) >= rect.loc.y
            && (sy.round() as i32) <= rect.loc.y + rect.size.h
        {
            return Some(ScreenshotMenuHit::Item(idx));
        }
    }
    None
}

fn draw_screenshot_menu(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let (selected_idx, hovered_idx) = st
        .input
        .interaction_state
        .screenshot_session
        .as_ref()
        .map(|s| (s.menu_selected, s.menu_hovered))
        .unwrap_or((0, None));
    let background = screenshot_menu_background_color(overlay.tuning);
    let highlight = screenshot_menu_highlight_color(overlay.tuning);
    let item_fill = screenshot_menu_item_fill_color(overlay.tuning);
    let style = resolve_screenshot_menu_style(overlay.tuning);
    let bar_rect = screenshot_menu_bar_rect(screen_w, screen_h);
    let shadow_rect_1 = Rectangle::<i32, Physical>::new(
        (bar_rect.loc.x + 4, bar_rect.loc.y + 5).into(),
        bar_rect.size,
    );
    let shadow_rect_2 = Rectangle::<i32, Physical>::new(
        (bar_rect.loc.x + 2, bar_rect.loc.y + 2).into(),
        bar_rect.size,
    );
    draw_screenshot_menu_chip(
        frame,
        overlay.render_state,
        shadow_rect_1,
        style.rounded,
        style.bar_corner_radius,
        0.0,
        SHADOW_COLOR_1,
        Color32F::new(0.0, 0.0, 0.0, 0.0),
        damage,
    )?;
    draw_screenshot_menu_chip(
        frame,
        overlay.render_state,
        shadow_rect_2,
        style.rounded,
        style.bar_corner_radius,
        0.0,
        SHADOW_COLOR_2,
        Color32F::new(0.0, 0.0, 0.0, 0.0),
        damage,
    )?;
    draw_screenshot_menu_chip(
        frame,
        overlay.render_state,
        bar_rect,
        style.rounded,
        style.bar_corner_radius,
        style.outer_border_px,
        color32f(background, 0.96),
        color32f(highlight, ACTIVE_BORDER_ALPHA),
        damage,
    )?;
    for (idx, mode) in screenshot_menu_modes().into_iter().enumerate() {
        let rect = screenshot_menu_rect(idx, screen_w, screen_h);
        let active = hovered_idx == Some(idx) || selected_idx == idx;
        let fill = if active {
            color32f(background, 0.96)
        } else {
            color32f(item_fill, 0.94)
        };
        let border = if active {
            color32f(highlight, ACTIVE_BORDER_ALPHA)
        } else {
            color32f(
                screenshot_menu_inactive_highlight_color(overlay.tuning),
                INACTIVE_BORDER_ALPHA,
            )
        };
        let item_rect = Rectangle::<i32, Physical>::new(
            (rect.loc.x + MENU_PAD, rect.loc.y + MENU_PAD).into(),
            (rect.size.w - MENU_PAD * 2, rect.size.h - MENU_PAD * 2).into(),
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
        if let Some(icon) = screenshot_menu_icon_texture(st, mode, active) {
            let dest = Rectangle::<i32, Physical>::new(
                (
                    rect.loc.x + (rect.size.w - MENU_ICON_SIZE) / 2,
                    rect.loc.y + (rect.size.h - MENU_ICON_SIZE) / 2,
                )
                    .into(),
                (MENU_ICON_SIZE, MENU_ICON_SIZE).into(),
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
                1.0,
                None,
                &[],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use halley_config::{OverlayShape, RuntimeTuning};

    use super::{
        resolve_screenshot_menu_style, MENU_BAR_CORNER_RADIUS, MENU_ITEM_BORDER_PX,
        MENU_ITEM_CORNER_RADIUS,
    };

    #[test]
    fn screenshot_menu_style_uses_internal_radii_and_overlay_border_toggle() {
        let mut tuning = RuntimeTuning::default();
        tuning.decorations.border.size_px = 6;
        tuning.overlay_style.shape = OverlayShape::Rounded;
        tuning.overlay_style.borders = true;

        let style = resolve_screenshot_menu_style(&tuning);

        assert!(style.rounded);
        assert_eq!(style.outer_border_px, 6.0);
        assert_eq!(style.item_border_px, MENU_ITEM_BORDER_PX);
        assert_eq!(style.bar_corner_radius, MENU_BAR_CORNER_RADIUS);
        assert_eq!(style.item_corner_radius, MENU_ITEM_CORNER_RADIUS);

        tuning.overlay_style.shape = OverlayShape::Square;
        tuning.overlay_style.borders = false;

        let style = resolve_screenshot_menu_style(&tuning);

        assert!(!style.rounded);
        assert_eq!(style.outer_border_px, 0.0);
        assert_eq!(style.item_border_px, MENU_ITEM_BORDER_PX);
        assert_eq!(style.bar_corner_radius, 0.0);
        assert_eq!(style.item_corner_radius, 0.0);
    }
}

fn color32f(color: halley_config::DecorationBorderColor, alpha: f32) -> Color32F {
    Color32F::new(color.r, color.g, color.b, alpha)
}

fn to_local_rect(crop: CaptureCrop, offset_x: i32, offset_y: i32) -> RectLocal {
    RectLocal {
        x: crop.x - offset_x,
        y: crop.y - offset_y,
        w: crop.w,
        h: crop.h,
    }
}

fn draw_dashed_border(
    frame: &mut GlesFrame<'_, '_>,
    rect: RectLocal,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.w;
    let y1 = rect.y + rect.h;

    let mut x = x0;
    while x < x1 {
        let seg = (x1 - x).min(DASH_LEN);
        draw_rect(frame, x, y0, seg, BORDER_THICKNESS, color, damage)?;
        draw_rect(
            frame,
            x,
            y1 - BORDER_THICKNESS,
            seg,
            BORDER_THICKNESS,
            color,
            damage,
        )?;
        x += DASH_LEN + GAP_LEN;
    }

    let mut y = y0;
    while y < y1 {
        let seg = (y1 - y).min(DASH_LEN);
        draw_rect(frame, x0, y, BORDER_THICKNESS, seg, color, damage)?;
        draw_rect(
            frame,
            x1 - BORDER_THICKNESS,
            y,
            BORDER_THICKNESS,
            seg,
            color,
            damage,
        )?;
        y += DASH_LEN + GAP_LEN;
    }

    Ok(())
}

fn draw_corner_handles(
    frame: &mut GlesFrame<'_, '_>,
    rect: RectLocal,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let radius = (HANDLE_SIZE as f32) * 0.5;
    for (cx, cy) in [
        (rect.x as f32, rect.y as f32),
        ((rect.x + rect.w) as f32, rect.y as f32),
        (rect.x as f32, (rect.y + rect.h) as f32),
        ((rect.x + rect.w) as f32, (rect.y + rect.h) as f32),
    ] {
        draw_rect(
            frame,
            (cx - radius).round() as i32,
            (cy - radius).round() as i32,
            HANDLE_SIZE,
            HANDLE_SIZE,
            color,
            damage,
        )?;
        draw_ring(frame, cx, cy, radius, radius, color, damage)?;
    }
    Ok(())
}

fn draw_screenshot_selection_overlay(
    frame: &mut GlesFrame<'_, '_>,
    selection_rect: Option<CaptureCrop>,
    offset_x: i32,
    offset_y: i32,
    screen_w: i32,
    screen_h: i32,
    border_color: Color32F,
    dim_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if let Some(rect) = selection_rect {
        let sel = to_local_rect(rect, offset_x, offset_y);
        let sel_right = sel.x + sel.w;
        let sel_bottom = sel.y + sel.h;
        let intersects = sel_right > 0 && sel.x < screen_w && sel_bottom > 0 && sel.y < screen_h;
        if intersects {
            let clip_x = sel.x.max(0);
            let clip_y = sel.y.max(0);
            let clip_w = (sel.x + sel.w).min(screen_w) - clip_x;
            let clip_h = (sel.y + sel.h).min(screen_h) - clip_y;
            if clip_y > 0 {
                draw_rect(frame, 0, 0, screen_w, clip_y, dim_color, damage)?;
            }
            if clip_x > 0 {
                draw_rect(frame, 0, clip_y, clip_x, clip_h, dim_color, damage)?;
            }
            let right_x = clip_x + clip_w;
            if right_x < screen_w {
                draw_rect(
                    frame,
                    right_x,
                    clip_y,
                    screen_w - right_x,
                    clip_h,
                    dim_color,
                    damage,
                )?;
            }
            let bottom_y = clip_y + clip_h;
            if bottom_y < screen_h {
                draw_rect(
                    frame,
                    0,
                    bottom_y,
                    screen_w,
                    screen_h - bottom_y,
                    dim_color,
                    damage,
                )?;
            }

            draw_dashed_border(frame, sel, border_color, damage)?;
            draw_corner_handles(frame, sel, border_color, damage)?;
        } else {
            draw_rect(frame, 0, 0, screen_w, screen_h, dim_color, damage)?;
        }
    } else {
        draw_rect(frame, 0, 0, screen_w, screen_h, dim_color, damage)?;
    }
    Ok(())
}

fn draw_screenshot_window_overlay(
    frame: &mut GlesFrame<'_, '_>,
    selection_rect: Option<CaptureCrop>,
    offset_x: i32,
    offset_y: i32,
    screen_w: i32,
    screen_h: i32,
    dim_color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if let Some(rect) = selection_rect {
        let sel = to_local_rect(rect, offset_x, offset_y);
        let sel_right = sel.x + sel.w;
        let sel_bottom = sel.y + sel.h;
        let intersects = sel_right > 0 && sel.x < screen_w && sel_bottom > 0 && sel.y < screen_h;
        if intersects {
            let clip_x = sel.x.max(0);
            let clip_y = sel.y.max(0);
            let clip_w = (sel.x + sel.w).min(screen_w) - clip_x;
            let clip_h = (sel.y + sel.h).min(screen_h) - clip_y;
            if clip_y > 0 {
                draw_rect(frame, 0, 0, screen_w, clip_y, dim_color, damage)?;
            }
            if clip_x > 0 {
                draw_rect(frame, 0, clip_y, clip_x, clip_h, dim_color, damage)?;
            }
            let right_x = clip_x + clip_w;
            if right_x < screen_w {
                draw_rect(
                    frame,
                    right_x,
                    clip_y,
                    screen_w - right_x,
                    clip_h,
                    dim_color,
                    damage,
                )?;
            }
            let bottom_y = clip_y + clip_h;
            if bottom_y < screen_h {
                draw_rect(
                    frame,
                    0,
                    bottom_y,
                    screen_w,
                    screen_h - bottom_y,
                    dim_color,
                    damage,
                )?;
            }

            draw_rect(
                frame,
                clip_x,
                clip_y,
                clip_w.max(1),
                clip_h.max(1),
                WINDOW_CAPTURE_FILL,
                damage,
            )?;
            draw_outline_rect(
                frame,
                clip_x,
                clip_y,
                clip_w.max(1),
                clip_h.max(1),
                WINDOW_CAPTURE_OUTLINE,
                damage,
            )?;
        } else {
            draw_rect(frame, 0, 0, screen_w, screen_h, dim_color, damage)?;
        }
    } else {
        draw_rect(frame, 0, 0, screen_w, screen_h, dim_color, damage)?;
    }
    Ok(())
}

pub(crate) fn draw_screenshot_overlay(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(session) = st.input.interaction_state.screenshot_session.as_ref() else {
        return Ok(());
    };
    let overlay = OverlayView::from_halley(st);
    let screenshot_highlight = color32f(
        screenshot_menu_highlight_color(overlay.tuning),
        ACTIVE_BORDER_ALPHA,
    );
    let Some(space) = overlay
        .monitor_state
        .monitors
        .get(overlay.monitor_state.current_monitor.as_str())
    else {
        return Ok(());
    };

    if session.mode == CaptureMode::Menu {
        if overlay.monitor_state.current_monitor != session.monitor {
            return Ok(());
        }
        return draw_screenshot_menu(frame, &*st, &overlay, screen_w, screen_h, damage);
    }

    match session.mode {
        CaptureMode::Region => {
            draw_screenshot_selection_overlay(
                frame,
                session.selection_rect,
                space.offset_x,
                space.offset_y,
                screen_w,
                screen_h,
                screenshot_highlight,
                DIM_COLOR,
                damage,
            )?;
        }
        CaptureMode::Screen => {
            let selected = overlay.monitor_state.current_monitor == session.monitor;
            if selected {
                draw_outline_rect(
                    frame,
                    0,
                    0,
                    screen_w,
                    screen_h,
                    screenshot_highlight,
                    damage,
                )?;
            } else {
                draw_rect(frame, 0, 0, screen_w, screen_h, SCREEN_DIM_COLOR, damage)?;
            }
        }
        CaptureMode::Window => {
            if overlay.monitor_state.current_monitor == session.monitor {
                draw_screenshot_window_overlay(
                    frame,
                    session.selection_rect,
                    space.offset_x,
                    space.offset_y,
                    screen_w,
                    screen_h,
                    Color32F::new(0.0, 0.0, 0.0, 0.18),
                    damage,
                )?;
            } else {
                draw_rect(
                    frame,
                    0,
                    0,
                    screen_w,
                    screen_h,
                    Color32F::new(0.0, 0.0, 0.0, 0.18),
                    damage,
                )?;
            }
        }
        CaptureMode::Menu => unreachable!(),
    }

    Ok(())
}
