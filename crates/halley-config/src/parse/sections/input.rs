use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_i32, pick_input_focus_mode};

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
end
"#,
        )
        .expect("input config should parse");

        let mut out = RuntimeTuning::default();
        load_input_section(&cfg, &mut out);

        assert_eq!(out.input.repeat_rate, 45);
        assert_eq!(out.input.repeat_delay, 650);
        assert_eq!(out.input.focus_mode, InputFocusMode::Hover);
    }

    #[test]
    fn input_defaults_match_v0_1_0_surface() {
        let tuning = RuntimeTuning::default();

        assert_eq!(tuning.input.repeat_rate, 30);
        assert_eq!(tuning.input.repeat_delay, 500);
        assert_eq!(tuning.input.focus_mode, InputFocusMode::Click);
    }

    #[test]
    fn runtime_tuning_loader_reads_input_section() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
input:
  repeat-rate 55
  repeat-delay 700
  focus-mode "hover"
end
"#,
        )
        .expect("full runtime tuning should parse");

        assert_eq!(tuning.input.repeat_rate, 55);
        assert_eq!(tuning.input.repeat_delay, 700);
        assert_eq!(tuning.input.focus_mode, InputFocusMode::Hover);
    }
}
