use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_i32, pick_input_focus_mode, pick_string};

pub(crate) fn load_input_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.input.repeat_rate = pick_i32(
        cfg,
        &["input.repeat-rate", "input.repeat_rate"],
        out.input.repeat_rate,
    );
    out.input.repeat_delay = pick_i32(
        cfg,
        &["input.repeat-delay", "input.repeat_delay"],
        out.input.repeat_delay,
    );
    out.input.focus_mode = pick_input_focus_mode(
        cfg,
        &["input.focus-mode", "input.focus_mode"],
        out.input.focus_mode,
    );

    if let Some(layout) = pick_string(cfg, &["input.keyboard.layout"]) {
        out.input.keyboard.layout = layout;
    }
    if let Some(variant) = pick_string(cfg, &["input.keyboard.variant"]) {
        out.input.keyboard.variant = variant;
    }
    if let Some(options) = pick_string(cfg, &["input.keyboard.options"]) {
        out.input.keyboard.options = options;
    }
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{InputFocusMode, RuntimeTuning};

    use super::load_input_section;

    #[test]
    fn input_section_loads_repeat_and_focus_mode() {
        let cfg = RuneConfig::from_str(
            r#"
input:
  repeat-rate 45
  repeat-delay 650
  focus-mode "hover"
  keyboard:
    layout "de"
    variant "nodeadkeys"
    options "compose:ralt"
  end
end
"#,
        )
        .expect("input config should parse");

        let mut out = RuntimeTuning::default();
        load_input_section(&cfg, &mut out);

        assert_eq!(out.input.repeat_rate, 45);
        assert_eq!(out.input.repeat_delay, 650);
        assert_eq!(out.input.focus_mode, InputFocusMode::Hover);
        assert_eq!(out.input.keyboard.layout, "de");
        assert_eq!(out.input.keyboard.variant, "nodeadkeys");
        assert_eq!(out.input.keyboard.options, "compose:ralt");
    }

    #[test]
    fn input_defaults_match_v0_1_0_surface() {
        let tuning = RuntimeTuning::default();

        assert_eq!(tuning.input.repeat_rate, 30);
        assert_eq!(tuning.input.repeat_delay, 500);
        assert_eq!(tuning.input.focus_mode, InputFocusMode::Click);
        assert_eq!(tuning.input.keyboard.layout, "us");
        assert_eq!(tuning.input.keyboard.variant, "");
        assert_eq!(tuning.input.keyboard.options, "");
    }

    #[test]
    fn runtime_tuning_loader_reads_input_section() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
input:
  repeat-rate 55
  repeat-delay 700
  focus-mode "hover"
  keyboard:
    layout "fr"
    variant "oss"
    options "caps:escape"
  end
end
"#,
        )
        .expect("full runtime tuning should parse");

        assert_eq!(tuning.input.repeat_rate, 55);
        assert_eq!(tuning.input.repeat_delay, 700);
        assert_eq!(tuning.input.focus_mode, InputFocusMode::Hover);
        assert_eq!(tuning.input.keyboard.layout, "fr");
        assert_eq!(tuning.input.keyboard.variant, "oss");
        assert_eq!(tuning.input.keyboard.options, "caps:escape");
    }
}
