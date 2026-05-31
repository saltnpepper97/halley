use crate::render::state::RenderState;
use crate::text::ui_text_size_in;

pub(super) fn truncate_overlay_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out = trimmed.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

pub(super) fn truncate_overlay_text_to_width(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    text: &str,
    scale: i32,
    max_width: i32,
) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || max_width <= 0 {
        return String::new();
    }
    if ui_text_size_in(render_state, font, trimmed, scale).0 <= max_width {
        return trimmed.to_string();
    }

    let chars = trimmed.chars().collect::<Vec<_>>();
    for keep in (1..=chars.len()).rev() {
        let mut candidate = chars.iter().take(keep).collect::<String>();
        if keep < chars.len() {
            candidate.push_str("...");
        }
        if ui_text_size_in(render_state, font, candidate.as_str(), scale).0 <= max_width {
            return candidate;
        }
    }

    String::new()
}

pub(super) fn visible_overlay_text_window(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    text: &str,
    scale: i32,
    scroll_x: i32,
    max_width: i32,
) -> String {
    if text.is_empty() || max_width <= 0 {
        return String::new();
    }
    if scroll_x <= 0 && ui_text_size_in(render_state, font, text, scale).0 <= max_width {
        return text.to_string();
    }

    let chars = text.char_indices().collect::<Vec<_>>();
    let mut start_byte = text.len();
    let scroll_x = scroll_x.max(0);
    for (index, (byte_index, _)) in chars.iter().enumerate() {
        let next_byte = chars
            .get(index + 1)
            .map(|(next_byte, _)| *next_byte)
            .unwrap_or(text.len());
        if ui_text_size_in(render_state, font, &text[..next_byte], scale).0 > scroll_x {
            start_byte = *byte_index;
            break;
        }
    }
    if start_byte >= text.len() {
        return String::new();
    }

    let visible = &text[start_byte..];
    let visible_chars = visible.char_indices().collect::<Vec<_>>();
    let mut end_byte = visible.len();
    for (index, (byte_index, _)) in visible_chars.iter().enumerate() {
        let next_byte = visible_chars
            .get(index + 1)
            .map(|(next_byte, _)| *next_byte)
            .unwrap_or(visible.len());
        if ui_text_size_in(render_state, font, &visible[..next_byte], scale).0 > max_width {
            end_byte = *byte_index;
            break;
        }
    }
    visible[..end_byte].to_string()
}
