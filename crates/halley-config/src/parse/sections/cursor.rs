use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_string, pick_u32, pick_u64};

pub(crate) fn load_cursor_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(theme) = pick_string(cfg, &["cursor.theme"]) {
        let theme = theme.trim();
        if !theme.is_empty() {
            out.cursor.theme = theme.to_string();
        }
    }
    out.cursor.size = pick_u32(cfg, &["cursor.size"], out.cursor.size);
    out.cursor.hide_while_typing = pick_bool(
        cfg,
        &["cursor.hide-while-typing", "cursor.hide_while_typing"],
        out.cursor.hide_while_typing,
    );
    out.cursor.hide_after_ms = pick_u64(
        cfg,
        &["cursor.hide-after-ms", "cursor.hide_after_ms"],
        out.cursor.hide_after_ms,
    );
}

