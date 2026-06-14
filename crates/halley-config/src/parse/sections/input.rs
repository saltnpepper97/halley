use rune_cfg::RuneConfig;

use crate::layout::{DeviceOverride, DeviceSettings, RuntimeTuning};

use super::super::primitives::{
    opt_accel_profile, opt_bool, opt_click_method, opt_f64, opt_scroll_method, opt_tap_button_map,
    opt_u32, pick_bool, pick_i32, pick_input_focus_mode, pick_string,
};

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
    out.input.raise_on_click = pick_bool(
        cfg,
        &["input.raise-on-click", "input.raise_on_click"],
        out.input.raise_on_click,
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
    if let Some(model) = pick_string(cfg, &["input.keyboard.model"]) {
        out.input.keyboard.model = model;
    }

    out.input.touchpad = load_device_settings(cfg, "input.touchpad");
    out.input.mouse = load_device_settings(cfg, "input.mouse");
    out.input.devices = load_device_overrides(cfg);
}

fn load_device_overrides(cfg: &RuneConfig) -> Vec<DeviceOverride> {
    let Ok(keys) = cfg.get_keys("input.devices") else {
        return Vec::new();
    };
    keys.into_iter()
        .map(|name| {
            let settings = load_device_settings(cfg, &format!("input.devices.{name}"));
            DeviceOverride { name, settings }
        })
        .collect()
}

/// Parse a libinput device block (a `touchpad:`/`mouse:` type section or a
/// `devices.<name>:` override). Only keys that are present become `Some`.
fn load_device_settings(cfg: &RuneConfig, root: &str) -> DeviceSettings {
    DeviceSettings {
        natural_scroll: opt_bool(
            cfg,
            &[
                format!("{root}.natural-scroll").as_str(),
                format!("{root}.natural_scroll").as_str(),
            ],
        ),
        accel_speed: opt_f64(
            cfg,
            &[
                format!("{root}.accel-speed").as_str(),
                format!("{root}.accel_speed").as_str(),
            ],
        ),
        accel_profile: opt_accel_profile(
            cfg,
            &[
                format!("{root}.accel-profile").as_str(),
                format!("{root}.accel_profile").as_str(),
            ],
        ),
        scroll_method: opt_scroll_method(
            cfg,
            &[
                format!("{root}.scroll-method").as_str(),
                format!("{root}.scroll_method").as_str(),
            ],
        ),
        scroll_button: opt_u32(
            cfg,
            &[
                format!("{root}.scroll-button").as_str(),
                format!("{root}.scroll_button").as_str(),
            ],
        ),
        left_handed: opt_bool(
            cfg,
            &[
                format!("{root}.left-handed").as_str(),
                format!("{root}.left_handed").as_str(),
            ],
        ),
        middle_emulation: opt_bool(
            cfg,
            &[
                format!("{root}.middle-emulation").as_str(),
                format!("{root}.middle_emulation").as_str(),
            ],
        ),
        enabled: opt_bool(cfg, &[format!("{root}.enabled").as_str()]),
        tap: opt_bool(
            cfg,
            &[
                format!("{root}.tap").as_str(),
                format!("{root}.tap-to-click").as_str(),
            ],
        ),
        tap_button_map: opt_tap_button_map(
            cfg,
            &[
                format!("{root}.tap-button-map").as_str(),
                format!("{root}.tap_button_map").as_str(),
            ],
        ),
        dwt: opt_bool(
            cfg,
            &[
                format!("{root}.dwt").as_str(),
                format!("{root}.disable-while-typing").as_str(),
            ],
        ),
        click_method: opt_click_method(
            cfg,
            &[
                format!("{root}.click-method").as_str(),
                format!("{root}.click_method").as_str(),
            ],
        ),
        drag: opt_bool(cfg, &[format!("{root}.drag").as_str()]),
        drag_lock: opt_bool(
            cfg,
            &[
                format!("{root}.drag-lock").as_str(),
                format!("{root}.drag_lock").as_str(),
            ],
        ),
        disabled_on_external_mouse: opt_bool(
            cfg,
            &[
                format!("{root}.disabled-on-external-mouse").as_str(),
                format!("{root}.disabled_on_external_mouse").as_str(),
            ],
        ),
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
  raise-on-click false
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
        assert!(!out.input.raise_on_click);
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
        assert!(tuning.input.raise_on_click);
        assert_eq!(tuning.input.keyboard.layout, "us");
        assert_eq!(tuning.input.keyboard.variant, "");
        assert_eq!(tuning.input.keyboard.options, "");
    }

    #[test]
    fn input_section_loads_device_settings_and_overrides() {
        use crate::layout::{AccelProfile, ClickMethod, ScrollMethod};

        let cfg = RuneConfig::from_str(
            r#"
input:
  touchpad:
    tap true
    natural-scroll true
    dwt true
    accel-speed 0.3
    accel-profile "adaptive"
    scroll-method "two-finger"
    click-method "clickfinger"
  end
  mouse:
    natural-scroll false
    accel-speed 0.0
    accel-profile "flat"
  end
  devices:
    "Logitech MX Master 3":
      accel-speed 0.6
      natural-scroll true
    end
  end
end
"#,
        )
        .expect("input device config should parse");

        let mut out = RuntimeTuning::default();
        load_input_section(&cfg, &mut out);

        assert_eq!(out.input.touchpad.tap, Some(true));
        assert_eq!(out.input.touchpad.natural_scroll, Some(true));
        assert_eq!(out.input.touchpad.dwt, Some(true));
        assert_eq!(out.input.touchpad.accel_speed, Some(0.3));
        assert_eq!(
            out.input.touchpad.accel_profile,
            Some(AccelProfile::Adaptive)
        );
        assert_eq!(
            out.input.touchpad.scroll_method,
            Some(ScrollMethod::TwoFinger)
        );
        assert_eq!(
            out.input.touchpad.click_method,
            Some(ClickMethod::Clickfinger)
        );
        // Touchpad-only keys absent from the mouse block stay None.
        assert_eq!(out.input.mouse.tap, None);
        assert_eq!(out.input.mouse.natural_scroll, Some(false));
        assert_eq!(out.input.mouse.accel_profile, Some(AccelProfile::Flat));

        assert_eq!(out.input.devices.len(), 1);
        let dev = &out.input.devices[0];
        assert_eq!(dev.name, "Logitech MX Master 3");
        assert_eq!(dev.settings.accel_speed, Some(0.6));
        assert_eq!(dev.settings.natural_scroll, Some(true));

        // Resolution: a mouse device matching the override layers on top of the mouse block.
        let resolved = out.input.mouse.overlay(&dev.settings);
        assert_eq!(resolved.accel_speed, Some(0.6));
        assert_eq!(resolved.natural_scroll, Some(true));
        assert_eq!(resolved.accel_profile, Some(AccelProfile::Flat));
    }

    #[test]
    fn input_device_sections_default_to_unset() {
        let tuning = RuntimeTuning::default();
        assert_eq!(
            tuning.input.touchpad,
            crate::layout::DeviceSettings::default()
        );
        assert_eq!(tuning.input.mouse, crate::layout::DeviceSettings::default());
        assert!(tuning.input.devices.is_empty());
    }

    #[test]
    fn runtime_tuning_loader_reads_input_section() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
input:
  repeat-rate 55
  repeat-delay 700
  focus-mode "hover"
  raise-on-click false
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
        assert!(!tuning.input.raise_on_click);
        assert_eq!(tuning.input.keyboard.layout, "fr");
        assert_eq!(tuning.input.keyboard.variant, "oss");
        assert_eq!(tuning.input.keyboard.options, "caps:escape");
    }
}
