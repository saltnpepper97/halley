use std::error::Error;

use smithay::{
    backend::renderer::{Color32F, gles::GlesFrame},
    utils::{Physical, Rectangle},
};

use crate::compositor::clusters::state::ClusterNamingPromptState;
use crate::compositor::root::Halley;
use crate::render::text::{draw_ui_text_in, ui_text_size_in};
use crate::render::utils::draw_rect;

use super::{
    ACTION_ROW_GAP_Y, BANNER_GAP, BANNER_META_SCALE, BANNER_TITLE_SCALE,
    EXIT_CONFIRM_MAX_WIDTH_PAD, OverlayView, draw_overlay_action_row, draw_overlay_chip,
    overlay_action_row_size, resolve_overlay_visuals,
};

const CLUSTER_DIALOG_TITLE: &str = "Create cluster";
const CLUSTER_DIALOG_SUBTITLE: &str = "Choose a name for your new cluster";
const CLUSTER_DIALOG_PAD_X: i32 = 18;
const CLUSTER_DIALOG_PAD_Y: i32 = 16;
const CLUSTER_DIALOG_INPUT_PAD_X: i32 = 12;
const CLUSTER_DIALOG_INPUT_PAD_Y: i32 = 10;
const CLUSTER_DIALOG_BUTTON_PAD_X: i32 = 16;
const CLUSTER_DIALOG_BUTTON_PAD_Y: i32 = 10;
const CLUSTER_DIALOG_MIN_WIDTH: i32 = 360;
const CLUSTER_DIALOG_MAX_WIDTH: i32 = 560;
const CLUSTER_DIALOG_INPUT_MIN_H: i32 = 38;
const CLUSTER_DIALOG_BUTTON_MIN_W: i32 = 110;
const CLUSTER_DIALOG_GAP_Y: i32 = 12;
const CLUSTER_DIALOG_SCALE: i32 = 2;

#[derive(Clone, Copy)]
pub(crate) enum ClusterNamingDialogHit {
    ConfirmButton,
    InputCaret(usize),
}

#[derive(Clone, Copy)]
struct ClusterNamingDialogLayout {
    dialog_rect: Rectangle<i32, Physical>,
    input_rect: Rectangle<i32, Physical>,
    confirm_rect: Rectangle<i32, Physical>,
    text_x: i32,
    text_y: i32,
    text_visible_start: usize,
    text_visible_end: usize,
    caret_x: i32,
    selection_x0: i32,
    selection_x1: i32,
    has_visible_selection: bool,
}

fn prompt_slice(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn prompt_selection_range(prompt: &ClusterNamingPromptState) -> Option<(usize, usize)> {
    (prompt.selection_anchor_char != prompt.selection_focus_char).then(|| {
        (
            prompt
                .selection_anchor_char
                .min(prompt.selection_focus_char),
            prompt
                .selection_anchor_char
                .max(prompt.selection_focus_char),
        )
    })
}

fn text_width(
    render_state: &crate::render::state::RenderState,
    font: &halley_config::FontConfig,
    text: &str,
) -> i32 {
    ui_text_size_in(render_state, font, text, CLUSTER_DIALOG_SCALE).0
}

fn color_mix(a: Color32F, b: Color32F, amount: f32) -> Color32F {
    let t = amount.clamp(0.0, 1.0);
    Color32F::new(
        a.r() + (b.r() - a.r()) * t,
        a.g() + (b.g() - a.g()) * t,
        a.b() + (b.b() - a.b()) * t,
        a.a() + (b.a() - a.a()) * t,
    )
}

fn color_luminance(color: Color32F) -> f32 {
    color.r() * 0.2126 + color.g() * 0.7152 + color.b() * 0.0722
}

fn ensure_prompt_scroll(
    render_state: &crate::render::state::RenderState,
    font: &halley_config::FontConfig,
    prompt: &mut ClusterNamingPromptState,
    visible_width: i32,
) {
    let char_len = prompt.input.chars().count();
    prompt.scroll_char = prompt.scroll_char.min(char_len);
    if prompt.scroll_char > prompt.caret_char {
        prompt.scroll_char = prompt.caret_char;
    }
    while prompt.scroll_char < prompt.caret_char {
        let before_caret =
            prompt_slice(prompt.input.as_str(), prompt.scroll_char, prompt.caret_char);
        if text_width(render_state, font, before_caret.as_str()) <= visible_width.max(1) - 12 {
            break;
        }
        prompt.scroll_char += 1;
    }
    while prompt.scroll_char > 0 {
        let candidate = prompt_slice(
            prompt.input.as_str(),
            prompt.scroll_char.saturating_sub(1),
            prompt.caret_char,
        );
        if text_width(render_state, font, candidate.as_str()) > visible_width.max(1) - 18 {
            break;
        }
        prompt.scroll_char -= 1;
    }
}

fn cluster_naming_dialog_layout(
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    prompt: &mut ClusterNamingPromptState,
) -> ClusterNamingDialogLayout {
    let (title_w, title_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        CLUSTER_DIALOG_TITLE,
        BANNER_TITLE_SCALE,
    );
    let (subtitle_w, subtitle_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        CLUSTER_DIALOG_SUBTITLE,
        BANNER_META_SCALE,
    );
    let (confirm_text_w, confirm_text_h) = ui_text_size_in(
        overlay.render_state,
        &overlay.tuning.font,
        "Confirm",
        CLUSTER_DIALOG_SCALE,
    );
    let button_w =
        (confirm_text_w + CLUSTER_DIALOG_BUTTON_PAD_X * 2).max(CLUSTER_DIALOG_BUTTON_MIN_W);
    let button_h = (confirm_text_h + CLUSTER_DIALOG_BUTTON_PAD_Y * 2).max(34);
    let input_h = (confirm_text_h + CLUSTER_DIALOG_INPUT_PAD_Y * 2).max(CLUSTER_DIALOG_INPUT_MIN_H);
    let action_h = overlay_action_row_size(
        overlay.render_state,
        &overlay.tuning.font,
        &[("Enter", "confirm"), ("Esc", "cancel")],
    )
    .1;
    let width = (title_w.max(subtitle_w).max(280) + CLUSTER_DIALOG_PAD_X * 2).clamp(
        CLUSTER_DIALOG_MIN_WIDTH,
        (screen_w - EXIT_CONFIRM_MAX_WIDTH_PAD).min(CLUSTER_DIALOG_MAX_WIDTH),
    );
    let height = CLUSTER_DIALOG_PAD_Y * 2
        + title_h
        + BANNER_GAP
        + subtitle_h
        + CLUSTER_DIALOG_GAP_Y
        + input_h
        + CLUSTER_DIALOG_GAP_Y
        + button_h
        + ACTION_ROW_GAP_Y
        + action_h;
    let dialog_rect = Rectangle::<i32, Physical>::new(
        (
            ((screen_w - width) / 2).max(18),
            ((screen_h - height) / 2).max(18),
        )
            .into(),
        (width.max(1), height.max(1)).into(),
    );
    let input_rect = Rectangle::<i32, Physical>::new(
        (
            dialog_rect.loc.x + CLUSTER_DIALOG_PAD_X,
            dialog_rect.loc.y
                + CLUSTER_DIALOG_PAD_Y
                + title_h
                + BANNER_GAP
                + subtitle_h
                + CLUSTER_DIALOG_GAP_Y,
        )
            .into(),
        (dialog_rect.size.w - CLUSTER_DIALOG_PAD_X * 2, input_h).into(),
    );
    let confirm_rect = Rectangle::<i32, Physical>::new(
        (
            dialog_rect.loc.x + dialog_rect.size.w - CLUSTER_DIALOG_PAD_X - button_w,
            input_rect.loc.y + input_rect.size.h + CLUSTER_DIALOG_GAP_Y,
        )
            .into(),
        (button_w, button_h).into(),
    );
    let text_x = input_rect.loc.x + CLUSTER_DIALOG_INPUT_PAD_X;
    let text_y = input_rect.loc.y + (input_rect.size.h - confirm_text_h) / 2;
    let visible_width = input_rect.size.w - CLUSTER_DIALOG_INPUT_PAD_X;
    ensure_prompt_scroll(
        overlay.render_state,
        &overlay.tuning.font,
        prompt,
        visible_width,
    );

    let mut visible_end = prompt.scroll_char;
    while visible_end < prompt.input.chars().count() {
        let candidate = prompt_slice(prompt.input.as_str(), prompt.scroll_char, visible_end + 1);
        if text_width(
            overlay.render_state,
            &overlay.tuning.font,
            candidate.as_str(),
        ) > visible_width
        {
            break;
        }
        visible_end += 1;
    }
    let caret_prefix = prompt_slice(prompt.input.as_str(), prompt.scroll_char, prompt.caret_char);
    let caret_x = text_x
        + text_width(
            overlay.render_state,
            &overlay.tuning.font,
            caret_prefix.as_str(),
        );
    let (selection_x0, selection_x1, has_visible_selection) = if let Some((sel_start, sel_end)) =
        prompt_selection_range(prompt)
    {
        let vis_start = sel_start.clamp(prompt.scroll_char, visible_end);
        let vis_end = sel_end.clamp(prompt.scroll_char, visible_end);
        if vis_start < vis_end {
            let start_prefix = prompt_slice(prompt.input.as_str(), prompt.scroll_char, vis_start);
            let selected = prompt_slice(prompt.input.as_str(), vis_start, vis_end);
            let start_x = text_x
                + text_width(
                    overlay.render_state,
                    &overlay.tuning.font,
                    start_prefix.as_str(),
                );
            let end_x = start_x
                + text_width(
                    overlay.render_state,
                    &overlay.tuning.font,
                    selected.as_str(),
                );
            (start_x, end_x, true)
        } else {
            (caret_x, caret_x, false)
        }
    } else {
        (caret_x, caret_x, false)
    };

    ClusterNamingDialogLayout {
        dialog_rect,
        input_rect,
        confirm_rect,
        text_x,
        text_y,
        text_visible_start: prompt.scroll_char,
        text_visible_end: visible_end,
        caret_x,
        selection_x0,
        selection_x1,
        has_visible_selection,
    }
}

pub(crate) fn cluster_naming_dialog_hit_test(
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    sx: f32,
    sy: f32,
) -> Option<ClusterNamingDialogHit> {
    let monitor = st.model.monitor_state.current_monitor.clone();
    let mut prompt = st
        .model
        .cluster_state
        .cluster_name_prompt
        .get(monitor.as_str())
        .cloned()?;
    let overlay = OverlayView::from_halley(st);
    let layout = cluster_naming_dialog_layout(&overlay, screen_w, screen_h, &mut prompt);
    if sx.round() as i32 >= layout.confirm_rect.loc.x
        && sx.round() as i32 <= layout.confirm_rect.loc.x + layout.confirm_rect.size.w
        && sy.round() as i32 >= layout.confirm_rect.loc.y
        && sy.round() as i32 <= layout.confirm_rect.loc.y + layout.confirm_rect.size.h
    {
        return Some(ClusterNamingDialogHit::ConfirmButton);
    }
    if (sx.round() as i32) < layout.input_rect.loc.x
        || (sx.round() as i32) > layout.input_rect.loc.x + layout.input_rect.size.w
        || (sy.round() as i32) < layout.input_rect.loc.y
        || (sy.round() as i32) > layout.input_rect.loc.y + layout.input_rect.size.h
    {
        return None;
    }
    let rel_x = (sx.round() as i32 - layout.text_x).max(0);
    let mut caret = layout.text_visible_start;
    let mut prev_w = 0;
    for idx in layout.text_visible_start..layout.text_visible_end {
        let next = prompt_slice(prompt.input.as_str(), layout.text_visible_start, idx + 1);
        let next_w = text_width(overlay.render_state, &overlay.tuning.font, next.as_str());
        if rel_x < (prev_w + next_w) / 2 {
            break;
        }
        prev_w = next_w;
        caret = idx + 1;
    }
    Some(ClusterNamingDialogHit::InputCaret(caret))
}

pub(crate) fn draw_cluster_naming_dialog(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let monitor = st.model.monitor_state.current_monitor.clone();
    let Some(mut prompt) = st
        .model
        .cluster_state
        .cluster_name_prompt
        .get(monitor.as_str())
        .cloned()
    else {
        return Ok(());
    };
    let pointer_local = st
        .input
        .interaction_state
        .last_pointer_screen_global
        .map(|(sx, sy)| st.local_screen_in_monitor(monitor.as_str(), sx, sy))
        .map(|(_, _, sx, sy)| (sx, sy));
    {
        let overlay = OverlayView::from_halley(st);
        let visuals = resolve_overlay_visuals(overlay.tuning);
        let layout = cluster_naming_dialog_layout(&overlay, screen_w, screen_h, &mut prompt);
        let hovering_confirm = pointer_local.is_some_and(|(sx, sy)| {
            (sx.round() as i32) >= layout.confirm_rect.loc.x
                && (sx.round() as i32) <= layout.confirm_rect.loc.x + layout.confirm_rect.size.w
                && (sy.round() as i32) >= layout.confirm_rect.loc.y
                && (sy.round() as i32) <= layout.confirm_rect.loc.y + layout.confirm_rect.size.h
        });
        let hover_target = if hovering_confirm { 1.0 } else { 0.0 };
        prompt.confirm_hover_mix += (hover_target - prompt.confirm_hover_mix) * 0.16;
        if (prompt.confirm_hover_mix - hover_target).abs() < 0.015 {
            prompt.confirm_hover_mix = hover_target;
        }
        draw_rect(
            frame,
            0,
            0,
            screen_w.max(1),
            screen_h.max(1),
            Color32F::new(0.0, 0.0, 0.0, 0.14),
            damage,
        )?;
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            layout.dialog_rect,
            18.0,
            visuals.palette.fill.alpha(0.98),
            true,
            damage,
            1.0,
        )?;
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            layout.dialog_rect.loc.x + CLUSTER_DIALOG_PAD_X,
            layout.dialog_rect.loc.y + CLUSTER_DIALOG_PAD_Y,
            CLUSTER_DIALOG_TITLE,
            BANNER_TITLE_SCALE,
            visuals.palette.text.alpha(1.0),
            damage,
        )?;
        let (_, title_h) = ui_text_size_in(
            overlay.render_state,
            &overlay.tuning.font,
            CLUSTER_DIALOG_TITLE,
            BANNER_TITLE_SCALE,
        );
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            layout.dialog_rect.loc.x + CLUSTER_DIALOG_PAD_X,
            layout.dialog_rect.loc.y + CLUSTER_DIALOG_PAD_Y + title_h + BANNER_GAP,
            CLUSTER_DIALOG_SUBTITLE,
            BANNER_META_SCALE,
            visuals.palette.subtext.alpha(0.98),
            damage,
        )?;
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            layout.input_rect,
            12.0,
            visuals.palette.key_fill.alpha(0.98),
            true,
            damage,
            1.0,
        )?;
        if layout.has_visible_selection {
            draw_rect(
                frame,
                layout.selection_x0,
                layout.input_rect.loc.y + 7,
                (layout.selection_x1 - layout.selection_x0).max(1),
                (layout.input_rect.size.h - 14).max(1),
                visuals.palette.border.alpha(0.18),
                damage,
            )?;
        }
        let visible_text = prompt_slice(
            prompt.input.as_str(),
            layout.text_visible_start,
            layout.text_visible_end,
        );
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            layout.text_x,
            layout.text_y,
            visible_text.as_str(),
            CLUSTER_DIALOG_SCALE,
            visuals.palette.text.alpha(1.0),
            damage,
        )?;
        if !layout.has_visible_selection {
            draw_rect(
                frame,
                layout.caret_x,
                layout.input_rect.loc.y + 7,
                2,
                (layout.input_rect.size.h - 14).max(1),
                visuals.palette.text.alpha(0.94),
                damage,
            )?;
        }
        let confirm_fill = color_mix(
            visuals.palette.fill.alpha(0.98),
            visuals.palette.border.alpha(0.98),
            prompt.confirm_hover_mix,
        );
        let confirm_text_color = if color_luminance(confirm_fill) < 0.45 {
            Color32F::new(0.96, 0.98, 1.0, 1.0)
        } else {
            visuals.palette.text.alpha(1.0)
        };
        draw_overlay_chip(
            frame,
            overlay.render_state,
            &visuals,
            layout.confirm_rect,
            12.0,
            confirm_fill,
            true,
            damage,
            1.0,
        )?;
        let (confirm_text_w, confirm_text_h) = ui_text_size_in(
            overlay.render_state,
            &overlay.tuning.font,
            "Confirm",
            CLUSTER_DIALOG_SCALE,
        );
        draw_ui_text_in(
            frame,
            overlay.render_state,
            &overlay.tuning.font,
            layout.confirm_rect.loc.x + (layout.confirm_rect.size.w - confirm_text_w) / 2,
            layout.confirm_rect.loc.y + (layout.confirm_rect.size.h - confirm_text_h) / 2,
            "Confirm",
            CLUSTER_DIALOG_SCALE,
            confirm_text_color,
            damage,
        )?;
        draw_overlay_action_row(
            frame,
            overlay.render_state,
            &visuals,
            &overlay.tuning.font,
            layout.dialog_rect.loc.x + CLUSTER_DIALOG_PAD_X,
            layout.confirm_rect.loc.y + layout.confirm_rect.size.h + ACTION_ROW_GAP_Y,
            &[("Enter", "confirm"), ("Esc", "cancel")],
            damage,
            1.0,
        )?;
    }
    if let Some(state) = st
        .model
        .cluster_state
        .cluster_name_prompt
        .get_mut(monitor.as_str())
    {
        state.scroll_char = prompt.scroll_char;
        state.confirm_hover_mix = prompt.confirm_hover_mix;
    }
    Ok(())
}
