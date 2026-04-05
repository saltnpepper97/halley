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
        &[
            "cursor.hide-while-typing",
            "cursor.hide-when-typing",
            "cursor.hide_while_typing",
            "cursor.hide_when_typing",
        ],
        out.cursor.hide_while_typing,
    );
    out.cursor.hide_after_ms = pick_u64(
        cfg,
        &[
            "cursor.hide-after-ms",
            "cursor.hide-after-inactive-ms",
            "cursor.hide_after_ms",
            "cursor.hide_after_inactive_ms",
        ],
        out.cursor.hide_after_ms,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_cursor_section;

    #[test]
    fn cursor_section_accepts_niri_style_hide_keys() {
        let cfg = RuneConfig::from_str(
            r#"
cursor:
  hide-when-typing false
  hide-after-inactive-ms 1500
end
"#,
        )
        .expect("cursor config should parse");

        let mut out = RuntimeTuning::default();
        load_cursor_section(&cfg, &mut out);

        assert!(!out.cursor.hide_while_typing);
        assert_eq!(out.cursor.hide_after_ms, 1500);
    }

    #[test]
    fn cursor_defaults_do_not_idle_hide() {
        assert_eq!(RuntimeTuning::default().cursor.hide_after_ms, 0);
    }
}
