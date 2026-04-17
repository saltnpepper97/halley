use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_string, pick_u32};

pub(crate) fn load_font_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(family) = pick_string(cfg, &["font.family"]) {
        let family = family.trim();
        if !family.is_empty() {
            out.font.family = family.to_string();
        }
    }
    out.font.size = pick_u32(cfg, &["font.size"], out.font.size);
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::RuntimeTuning;

    use super::load_font_section;

    #[test]
    fn font_section_preserves_family_names_with_weight_suffixes() {
        let cfg = RuneConfig::from_str(
            r#"
font:
  family "CommitMono Nerd Font Bold"
  size 12
end
"#,
        )
        .expect("font config should parse");

        let mut out = RuntimeTuning::default();
        load_font_section(&cfg, &mut out);

        assert_eq!(out.font.family, "CommitMono Nerd Font Bold");
        assert_eq!(out.font.size, 12);
    }
}
