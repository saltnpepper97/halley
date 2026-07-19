use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::render::state::RenderState;
use crate::text::{draw_ui_text_in, ui_text_size_in};

use super::{
    BANNER_EDGE_PAD, BANNER_GAP, ERROR_TOAST_BODY_MAX_H, ERROR_TOAST_BODY_PAD_X,
    ERROR_TOAST_BODY_PAD_Y, ERROR_TOAST_CARET_RESERVE, ERROR_TOAST_LINE_GAP,
    ERROR_TOAST_SCROLLBAR_W, OverlayToastKind, OverlayToastSnapshot, OverlayVisuals,
    TOAST_META_SCALE, TOAST_PAD_X, TOAST_PAD_Y, TOAST_SCALE,
    draw_overlay_chip_with_border_color, draw_overlay_chip_without_shadow, overlay_text_mix,
    truncate_overlay_text_to_width, visible_overlay_text_window, wrap_overlay_text_to_width,
};

pub(super) fn draw_toast(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    toast: &OverlayToastSnapshot,
) -> Result<(), Box<dyn Error>> {
    let is_error = matches!(toast.kind, OverlayToastKind::Error);
    let text_mix = overlay_text_mix(toast.mix);
    let layout = toast_layout(render_state, font, screen_w, screen_h, toast, is_error);

    let border_color = if is_error {
        visuals.palette.error.alpha(1.0)
    } else {
        visuals.palette.border.alpha(1.0)
    };
    draw_overlay_chip_with_border_color(
        frame,
        render_state,
        visuals,
        layout.rect,
        14.0,
        visuals.palette.fill.alpha(0.94 * toast.mix),
        border_color,
        true,
        damage,
        toast.mix,
    )?;
    let title_color = if is_error {
        visuals.palette.error.alpha(text_mix)
    } else {
        visuals.palette.text.alpha(text_mix)
    };
    draw_ui_text_in(
        frame,
        render_state,
        font,
        layout.rect.loc.x + TOAST_PAD_X,
        layout.rect.loc.y + TOAST_PAD_Y,
        layout.title.as_str(),
        TOAST_SCALE,
        title_color,
        damage,
    )?;

    if is_error && !layout.body_lines.is_empty() {
        // Expand/collapse affordance: ▾ when expanded, ▸ when collapsed.
        let caret = if toast.expanded { "\u{25BE}" } else { "\u{25B8}" };
        let (caret_w, _) = ui_text_size_in(render_state, font, caret, TOAST_SCALE);
        draw_ui_text_in(
            frame,
            render_state,
            font,
            layout.rect.loc.x + layout.rect.size.w - TOAST_PAD_X - caret_w,
            layout.rect.loc.y + TOAST_PAD_Y,
            caret,
            TOAST_SCALE,
            title_color,
            damage,
        )?;
    }

    if is_error {
        draw_error_toast_body(frame, render_state, visuals, font, damage, toast, &layout)?;
    } else {
        let mut body_y = layout.rect.loc.y + TOAST_PAD_Y + layout.title_h + BANNER_GAP;
        for (body, (_, body_h)) in layout.body_lines.iter().zip(layout.body_metrics.iter()) {
            draw_ui_text_in(
                frame,
                render_state,
                font,
                layout.rect.loc.x + TOAST_PAD_X,
                body_y,
                body.as_str(),
                TOAST_META_SCALE,
                visuals.palette.subtext.alpha(text_mix * 0.96),
                damage,
            )?;
            body_y += *body_h + BANNER_GAP;
        }
    }
    Ok(())
}

fn draw_error_toast_body(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    font: &halley_config::FontConfig,
    damage: Rectangle<i32, Physical>,
    toast: &OverlayToastSnapshot,
    layout: &ToastLayout,
) -> Result<(), Box<dyn Error>> {
    if layout.body_lines.is_empty() {
        return Ok(());
    }
    let body_fill = visuals.palette.fill.mix(visuals.palette.text, 0.07);
    draw_overlay_chip_without_shadow(
        frame,
        render_state,
        visuals,
        layout.body_rect,
        8.0,
        body_fill.alpha(0.72 * toast.mix),
        false,
        damage,
        toast.mix,
    )?;

    let mut y = layout.body_content_rect.loc.y - layout.scroll_y;
    for (body, (_, body_h)) in layout.body_lines.iter().zip(layout.body_metrics.iter()) {
        if y >= layout.body_content_rect.loc.y
            && y + *body_h <= layout.body_content_rect.loc.y + layout.body_content_rect.size.h
        {
            let visible_body = visible_overlay_text_window(
                render_state,
                font,
                body.as_str(),
                TOAST_META_SCALE,
                layout.scroll_x,
                layout.body_content_rect.size.w,
            );
            draw_ui_text_in(
                frame,
                render_state,
                font,
                layout.body_content_rect.loc.x,
                y,
                visible_body.as_str(),
                TOAST_META_SCALE,
                visuals
                    .palette
                    .text
                    .alpha(overlay_text_mix(toast.mix) * 0.98),
                damage,
            )?;
        }
        y += *body_h + ERROR_TOAST_LINE_GAP;
    }

    draw_error_toast_scrollbars(frame, visuals, damage, toast.mix, layout)?;
    Ok(())
}

fn draw_error_toast_scrollbars(
    frame: &mut GlesFrame<'_, '_>,
    visuals: &OverlayVisuals,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
    layout: &ToastLayout,
) -> Result<(), Box<dyn Error>> {
    let track = visuals
        .palette
        .fill
        .mix(visuals.palette.text, 0.18)
        .alpha(0.40 * alpha);
    let thumb = visuals.palette.error.alpha(0.78 * alpha);
    if layout.max_scroll_y > 0 {
        let metrics = vertical_scrollbar_metrics(layout);
        draw_rect(
            frame,
            metrics.track.loc.x,
            metrics.track.loc.y,
            ERROR_TOAST_SCROLLBAR_W,
            metrics.track.size.h,
            track,
            damage,
        )?;
        draw_rect(
            frame,
            metrics.thumb.loc.x,
            metrics.thumb.loc.y,
            ERROR_TOAST_SCROLLBAR_W,
            metrics.thumb.size.h,
            thumb,
            damage,
        )?;
    }
    if layout.max_scroll_x > 0 {
        let metrics = horizontal_scrollbar_metrics(layout);
        draw_rect(
            frame,
            metrics.track.loc.x,
            metrics.track.loc.y,
            metrics.track.size.w,
            ERROR_TOAST_SCROLLBAR_W,
            track,
            damage,
        )?;
        draw_rect(
            frame,
            metrics.thumb.loc.x,
            metrics.thumb.loc.y,
            metrics.thumb.size.w,
            ERROR_TOAST_SCROLLBAR_W,
            thumb,
            damage,
        )?;
    }
    Ok(())
}

pub(crate) fn error_toast_hit_test(
    st: &Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    sx: f64,
    sy: f64,
) -> bool {
    let Some(toast) = st.ui.render_state.overlay_toast_state(monitor) else {
        return false;
    };
    if !matches!(toast.kind, OverlayToastKind::Error) {
        return false;
    }
    let Some(message) = toast.message.as_deref() else {
        return false;
    };
    let rect = error_toast_rect(
        &st.ui.render_state,
        &st.runtime.tuning.font,
        screen_w,
        screen_h,
        message,
        toast.expanded,
    );
    sx >= rect.loc.x as f64
        && sx < (rect.loc.x + rect.size.w) as f64
        && sy >= rect.loc.y as f64
        && sy < (rect.loc.y + rect.size.h) as f64
}

pub(crate) fn scroll_error_toast(
    st: &mut Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    dx: i32,
    dy: i32,
) -> bool {
    let Some(toast) = st.ui.render_state.overlay_toast_state(monitor) else {
        return false;
    };
    if !matches!(toast.kind, OverlayToastKind::Error) {
        return false;
    }
    let snapshot = OverlayToastSnapshot {
        message: toast.message.clone().unwrap_or_default(),
        kind: toast.kind,
        expanded: toast.expanded,
        scroll_x: toast.scroll_x,
        scroll_y: toast.scroll_y,
        mix: toast.mix,
    };
    let layout = toast_layout(
        &st.ui.render_state,
        &st.runtime.tuning.font,
        screen_w,
        screen_h,
        &snapshot,
        true,
    );
    st.ui.render_state.adjust_overlay_error_toast_scroll(
        monitor,
        dx,
        dy,
        layout.max_scroll_x,
        layout.max_scroll_y,
    )
}

fn error_toast_rect(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    message: &str,
    expanded: bool,
) -> Rectangle<i32, Physical> {
    let snapshot = OverlayToastSnapshot {
        message: message.to_string(),
        kind: OverlayToastKind::Error,
        expanded,
        scroll_x: 0,
        scroll_y: 0,
        mix: 1.0,
    };
    toast_layout(render_state, font, screen_w, screen_h, &snapshot, true).rect
}

struct ToastLayout {
    rect: Rectangle<i32, Physical>,
    title: String,
    title_h: i32,
    body_rect: Rectangle<i32, Physical>,
    body_content_rect: Rectangle<i32, Physical>,
    body_lines: Vec<String>,
    body_metrics: Vec<(i32, i32)>,
    body_content_w: i32,
    body_content_h: i32,
    scroll_x: i32,
    scroll_y: i32,
    max_scroll_x: i32,
    max_scroll_y: i32,
}

fn toast_layout(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    screen_w: i32,
    screen_h: i32,
    toast: &OverlayToastSnapshot,
    is_error: bool,
) -> ToastLayout {
    let mut lines = toast.message.lines();
    let title_raw = lines.next().unwrap_or_default();
    let expanded = is_error && toast.expanded;
    let max_content_width = if is_error {
        // Expanded errors widen (up to 720px) so wrapped lines read comfortably and
        // the box uses more of the screen; collapsed stays compact.
        let cap = if expanded { 720 } else { 420 };
        (screen_w - BANNER_EDGE_PAD * 2 - TOAST_PAD_X * 2).clamp(180, cap)
    } else {
        (screen_w - BANNER_EDGE_PAD * 2 - TOAST_PAD_X * 2).max(120)
    };
    // Error toasts draw an expand caret at the top-right, so keep the title clear of it.
    let title_max_width = if is_error {
        (max_content_width - ERROR_TOAST_CARET_RESERVE).max(1)
    } else {
        max_content_width
    };
    let title = truncate_overlay_text_to_width(
        render_state,
        font,
        title_raw,
        TOAST_SCALE,
        title_max_width,
    );
    let body_lines = if is_error {
        if expanded {
            // Grow-to-fit + wrap: break long lines to the content width so nothing
            // runs off-edge and the horizontal scrollbar becomes unnecessary.
            let wrap_w = (max_content_width - ERROR_TOAST_BODY_PAD_X * 2).max(1);
            lines
                .flat_map(|line| {
                    wrap_overlay_text_to_width(render_state, font, line, TOAST_META_SCALE, wrap_w)
                })
                .collect::<Vec<_>>()
        } else {
            lines.map(str::to_string).collect::<Vec<_>>()
        }
    } else {
        let body = lines.collect::<Vec<_>>().join(" ");
        if body.is_empty() {
            Vec::new()
        } else {
            vec![body]
        }
    };
    let (title_w, title_h) = ui_text_size_in(render_state, font, title.as_str(), TOAST_SCALE);
    let body_metrics = body_lines
        .iter()
        .map(|text| ui_text_size_in(render_state, font, text.as_str(), TOAST_META_SCALE))
        .collect::<Vec<_>>();
    let body_content_w = body_metrics.iter().map(|(w, _)| *w).max().unwrap_or(0);
    let body_content_h = body_metrics.iter().map(|(_, h)| *h).sum::<i32>()
        + ERROR_TOAST_LINE_GAP * body_metrics.len().saturating_sub(1) as i32;

    let has_body = !body_lines.is_empty();
    let body_rect_w = if has_body {
        (body_content_w + ERROR_TOAST_BODY_PAD_X * 2)
            .max(180.min(max_content_width))
            .min(max_content_width)
    } else {
        0
    };
    // Expanded errors grow to fit all wrapped lines, bounded only by the screen
    // (title + paddings + top/bottom edge gaps reserved). Vertical scroll only kicks
    // in if the wrapped content still exceeds this screen-bounded cap.
    let body_max_h = if expanded {
        (screen_h - BANNER_EDGE_PAD * 2 - TOAST_PAD_Y * 2 - title_h - BANNER_GAP)
            .max(ERROR_TOAST_BODY_MAX_H)
    } else {
        ERROR_TOAST_BODY_MAX_H
    };
    let mut body_rect_h = if has_body {
        (body_content_h + ERROR_TOAST_BODY_PAD_Y * 2).clamp(44, body_max_h)
    } else {
        0
    };
    let mut body_content_rect_w = (body_rect_w - ERROR_TOAST_BODY_PAD_X * 2).max(1);
    let mut body_content_rect_h = (body_rect_h - ERROR_TOAST_BODY_PAD_Y * 2).max(1);
    let overflow_y = body_content_h > body_content_rect_h;
    if overflow_y {
        body_content_rect_w = (body_content_rect_w - ERROR_TOAST_SCROLLBAR_W - 6).max(1);
    }
    let overflow_x = body_content_w > body_content_rect_w;
    if overflow_x {
        body_content_rect_h = (body_content_rect_h - ERROR_TOAST_SCROLLBAR_W - 6).max(1);
        body_rect_h =
            body_content_rect_h + ERROR_TOAST_BODY_PAD_Y * 2 + ERROR_TOAST_SCROLLBAR_W + 6;
    }
    let max_scroll_x = (body_content_w - body_content_rect_w).max(0);
    let max_scroll_y = (body_content_h - body_content_rect_h).max(0);
    let scroll_x = toast.scroll_x.clamp(0, max_scroll_x);
    let scroll_y = toast.scroll_y.clamp(0, max_scroll_y);

    let rect_w = (title_w.max(body_rect_w) + TOAST_PAD_X * 2).max(180);
    let rect_h = (TOAST_PAD_Y * 2
        + title_h
        + if has_body {
            BANNER_GAP + body_rect_h
        } else {
            0
        })
    .max(32);
    let rect_x = if is_error {
        (screen_w - rect_w - BANNER_EDGE_PAD).max(BANNER_EDGE_PAD)
    } else {
        ((screen_w - rect_w) / 2).max(BANNER_EDGE_PAD)
    };
    let rect_y = if is_error {
        BANNER_EDGE_PAD
    } else {
        ((screen_h - rect_h) / 2).max(BANNER_EDGE_PAD)
    };
    let rect = Rectangle::<i32, Physical>::new((rect_x, rect_y).into(), (rect_w, rect_h).into());
    let body_rect = Rectangle::<i32, Physical>::new(
        (
            rect.loc.x + TOAST_PAD_X,
            rect.loc.y + TOAST_PAD_Y + title_h + BANNER_GAP,
        )
            .into(),
        (body_rect_w.max(1), body_rect_h.max(1)).into(),
    );
    let body_content_rect = Rectangle::<i32, Physical>::new(
        (
            body_rect.loc.x + ERROR_TOAST_BODY_PAD_X,
            body_rect.loc.y + ERROR_TOAST_BODY_PAD_Y,
        )
            .into(),
        (body_content_rect_w, body_content_rect_h).into(),
    );
    ToastLayout {
        rect,
        title,
        title_h,
        body_rect,
        body_content_rect,
        body_lines,
        body_metrics,
        body_content_w,
        body_content_h,
        scroll_x,
        scroll_y,
        max_scroll_x,
        max_scroll_y,
    }
}

#[derive(Clone, Copy)]
struct ScrollbarMetrics {
    track: Rectangle<i32, Physical>,
    thumb: Rectangle<i32, Physical>,
}

fn vertical_scrollbar_metrics(layout: &ToastLayout) -> ScrollbarMetrics {
    let track_x = layout.body_rect.loc.x + layout.body_rect.size.w - ERROR_TOAST_SCROLLBAR_W - 4;
    let track_y = layout.body_content_rect.loc.y;
    let track_h = layout.body_content_rect.size.h.max(1);
    let track = Rectangle::<i32, Physical>::new(
        (track_x, track_y).into(),
        (ERROR_TOAST_SCROLLBAR_W, track_h).into(),
    );
    let thumb_h = ((layout.body_content_rect.size.h as f32 / layout.body_content_h.max(1) as f32)
        * track_h as f32)
        .round() as i32;
    let thumb_h = thumb_h.clamp(14, track_h.max(14));
    let travel = (track_h - thumb_h).max(0);
    let thumb_y = track_y
        + ((layout.scroll_y as f32 / layout.max_scroll_y.max(1) as f32) * travel as f32).round()
            as i32;
    let thumb = Rectangle::<i32, Physical>::new(
        (track_x, thumb_y).into(),
        (ERROR_TOAST_SCROLLBAR_W, thumb_h).into(),
    );
    ScrollbarMetrics { track, thumb }
}

fn horizontal_scrollbar_metrics(layout: &ToastLayout) -> ScrollbarMetrics {
    let track_x = layout.body_content_rect.loc.x;
    let track_y = layout.body_rect.loc.y + layout.body_rect.size.h - ERROR_TOAST_SCROLLBAR_W - 4;
    let track_w = layout.body_content_rect.size.w.max(1);
    let track = Rectangle::<i32, Physical>::new(
        (track_x, track_y).into(),
        (track_w, ERROR_TOAST_SCROLLBAR_W).into(),
    );
    let thumb_w = ((layout.body_content_rect.size.w as f32 / layout.body_content_w.max(1) as f32)
        * track_w as f32)
        .round() as i32;
    let thumb_w = thumb_w.clamp(24, track_w.max(24));
    let travel = (track_w - thumb_w).max(0);
    let thumb_x = track_x
        + ((layout.scroll_x as f32 / layout.max_scroll_x.max(1) as f32) * travel as f32).round()
            as i32;
    let thumb = Rectangle::<i32, Physical>::new(
        (thumb_x, track_y).into(),
        (thumb_w, ERROR_TOAST_SCROLLBAR_W).into(),
    );
    ScrollbarMetrics { track, thumb }
}
