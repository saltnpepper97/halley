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

/// Word-wrap `text` to fit within `max_width`, returning one string per physical
/// line. Greedy packing on whitespace; a single word wider than `max_width` (e.g.
/// a long file path) is hard-split by characters. Always returns at least one line.
pub(super) fn wrap_overlay_text_to_width(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    text: &str,
    scale: i32,
    max_width: i32,
) -> Vec<String> {
    let trimmed = text.trim_end();
    if max_width <= 0 || trimmed.is_empty() {
        return vec![trimmed.to_string()];
    }
    if ui_text_size_in(render_state, font, trimmed, scale).0 <= max_width {
        return vec![trimmed.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in trimmed.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if ui_text_size_in(render_state, font, candidate.as_str(), scale).0 <= max_width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if ui_text_size_in(render_state, font, word, scale).0 > max_width {
            let pieces = hard_split_to_width(render_state, font, word, scale, max_width);
            let last = pieces.len().saturating_sub(1);
            for (i, piece) in pieces.into_iter().enumerate() {
                if i < last {
                    lines.push(piece);
                } else {
                    current = piece;
                }
            }
        } else {
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Split a single unbreakable token into character-runs that each fit `max_width`.
/// A single glyph wider than `max_width` is kept alone (it overflows its line
/// rather than looping forever).
fn hard_split_to_width(
    render_state: &RenderState,
    font: &halley_config::FontConfig,
    word: &str,
    scale: i32,
    max_width: i32,
) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        let mut candidate = current.clone();
        candidate.push(ch);
        if !current.is_empty()
            && ui_text_size_in(render_state, font, candidate.as_str(), scale).0 > max_width
        {
            pieces.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    if pieces.is_empty() {
        pieces.push(String::new());
    }
    pieces
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
