use rune_cfg::RuneConfig;

use crate::keybinds::{
    BearingsBindingAction, CompositorBindingAction, DirectionalAction, FocusCycleBindingAction,
    KeyModifiers, NodeBindingAction, StackBindingAction, StackCycleDirection, TileBindingAction,
    TrailBindingAction, parse_modifiers,
};
use crate::layout::{
    CompositorGestureScope, DeviceOverride, DeviceSettings, GestureBinding, GestureBindingAction,
    GestureScrollPanMode, GestureSwipeDirection, RuntimeTuning,
};

use super::super::primitives::{
    opt_accel_profile, opt_bool, opt_click_method, opt_f64, opt_scroll_method, opt_tap_button_map,
    opt_u32, pick_bool, pick_f32, pick_i32, pick_input_focus_mode, pick_string,
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

    out.input.gestures.enabled =
        pick_bool(cfg, &["input.gestures.enabled"], out.input.gestures.enabled);
    out.input.gestures.client_passthrough = pick_bool(
        cfg,
        &[
            "input.gestures.client-passthrough",
            "input.gestures.client_passthrough",
        ],
        out.input.gestures.client_passthrough,
    );
    out.input.gestures.touch_passthrough = pick_bool(
        cfg,
        &[
            "input.gestures.touch-passthrough",
            "input.gestures.touch_passthrough",
        ],
        out.input.gestures.touch_passthrough,
    );
    out.input.gestures.pinch_to_zoom = pick_bool(
        cfg,
        &[
            "input.gestures.pinch-to-zoom",
            "input.gestures.pinch_to_zoom",
        ],
        out.input.gestures.pinch_to_zoom,
    );
    if let Some(scope) = pick_string(
        cfg,
        &[
            "input.gestures.compositor-scope",
            "input.gestures.compositor_scope",
        ],
    ) {
        out.input.gestures.compositor_scope = match scope.trim().trim_matches('"') {
            "empty-field" | "empty_field" => CompositorGestureScope::EmptyField,
            "global" => CompositorGestureScope::Global,
            _ => out.input.gestures.compositor_scope,
        };
    }
    if let Some(scope) = pick_string(
        cfg,
        &["input.gestures.pinch-scope", "input.gestures.pinch_scope"],
    ) {
        out.input.gestures.pinch_scope = match scope.trim().trim_matches('"') {
            "empty-field" | "empty_field" => CompositorGestureScope::EmptyField,
            "global" => CompositorGestureScope::Global,
            _ => out.input.gestures.pinch_scope,
        };
    }
    out.input.gestures.modifier = load_gesture_modifier(cfg, out);
    if let Some(mode) = pick_string(
        cfg,
        &["input.gestures.scroll-pan", "input.gestures.scroll_pan"],
    ) {
        out.input.gestures.scroll_pan = match mode.trim().trim_matches('"') {
            "off" | "false" | "none" => GestureScrollPanMode::Off,
            "empty-field" | "empty_field" => GestureScrollPanMode::EmptyField,
            _ => out.input.gestures.scroll_pan,
        };
    }
    out.input.gestures.swipe_threshold_px = pick_f32(
        cfg,
        &[
            "input.gestures.swipe-threshold-px",
            "input.gestures.swipe_threshold_px",
        ],
        out.input.gestures.swipe_threshold_px,
    )
    .max(1.0);
    out.input.gestures.swipe_bindings = load_gesture_swipe_bindings(cfg, false);
    out.input.gestures.apogee_swipe_bindings = load_gesture_swipe_bindings(cfg, true);

    out.input.touchpad = load_device_settings(cfg, "input.touchpad");
    out.input.mouse = load_device_settings(cfg, "input.mouse");
    out.input.devices = load_device_overrides(cfg);
}

fn load_gesture_modifier(cfg: &RuneConfig, out: &RuntimeTuning) -> KeyModifiers {
    let Some(raw) = pick_string(
        cfg,
        &[
            "input.gestures.modifier",
            "input.gestures.global-modifier",
            "input.gestures.global_modifier",
            "input.gestures.scroll-pan-modifier",
            "input.gestures.scroll_pan_modifier",
        ],
    ) else {
        return out.keybinds.modifier;
    };
    let raw = raw.trim().trim_matches('"');
    if matches!(raw, "$mod" | "mod" | "$var.mod") {
        return out.keybinds.modifier;
    }
    if matches!(raw, "off" | "false") {
        return KeyModifiers::default();
    }
    parse_modifiers(raw).unwrap_or(out.input.gestures.modifier)
}

fn load_gesture_swipe_bindings(cfg: &RuneConfig, apogee_context: bool) -> Vec<GestureBinding> {
    let mut bindings = Vec::new();
    let Ok(keys) = cfg.get_keys("input.gestures") else {
        let defaults = crate::layout::GestureInputConfig::default();
        return if apogee_context {
            defaults.apogee_swipe_bindings
        } else {
            defaults.swipe_bindings
        };
    };

    for key in keys {
        let Some((direction, fingers)) = parse_swipe_key(key.as_str(), apogee_context) else {
            continue;
        };
        let path = format!("input.gestures.{key}");
        let Some(action_text) = pick_string(cfg, &[path.as_str()]) else {
            continue;
        };
        let Some(action) = parse_gesture_binding_action(action_text.as_str()) else {
            continue;
        };
        bindings.push(GestureBinding {
            direction,
            fingers,
            action,
        });
    }

    if bindings.is_empty() {
        let defaults = crate::layout::GestureInputConfig::default();
        if apogee_context {
            defaults.apogee_swipe_bindings
        } else {
            defaults.swipe_bindings
        }
    } else {
        bindings
    }
}

fn parse_swipe_key(key: &str, apogee_context: bool) -> Option<(GestureSwipeDirection, u32)> {
    let prefix = if apogee_context {
        "apogee-swipe-"
    } else {
        "swipe-"
    };
    let key = key.strip_prefix(prefix)?;
    let mut parts = key.split('-');
    let direction = match parts.next()? {
        "up" => GestureSwipeDirection::Up,
        "down" => GestureSwipeDirection::Down,
        "left" => GestureSwipeDirection::Left,
        "right" => GestureSwipeDirection::Right,
        _ => return None,
    };
    let fingers = parts.next()?.parse::<u32>().ok()?;
    (parts.next().is_none() && fingers > 0).then_some((direction, fingers))
}

fn parse_gesture_binding_action(action: &str) -> Option<GestureBindingAction> {
    let key = action.trim().trim_matches('"').to_ascii_lowercase();
    let compositor = match key.as_str() {
        "apogee-open" | "overview-open" => return Some(GestureBindingAction::ApogeeOpen),
        "apogee-close" | "overview-close" => return Some(GestureBindingAction::ApogeeClose),
        "apogee" | "overview" => CompositorBindingAction::Apogee,
        "toggle-state" | "toggle_state" => CompositorBindingAction::ToggleState,
        "maximize-focused" | "maximize_focused" => CompositorBindingAction::MaximizeFocusedWindow,
        "toggle-fullscreen" | "toggle_fullscreen" | "fullscreen" => {
            CompositorBindingAction::ToggleFullscreen
        }
        "toggle-focused-pin" | "toggle_focused_pin" | "toggle-pin" | "toggle_pin" => {
            CompositorBindingAction::ToggleFocusedPin
        }
        "close-focused" | "close_focused" => CompositorBindingAction::CloseFocusedWindow,
        "cluster-mode" | "cluster_mode" => CompositorBindingAction::ClusterMode,
        "cycle-focus" | "cycle_focus" => {
            CompositorBindingAction::FocusCycle(FocusCycleBindingAction::Forward)
        }
        "cycle-focus-backward" | "cycle_focus_backward" => {
            CompositorBindingAction::FocusCycle(FocusCycleBindingAction::Backward)
        }
        "bearings-show" | "bearings_show" => {
            CompositorBindingAction::Bearings(BearingsBindingAction::Show)
        }
        "bearings-toggle" | "bearings_toggle" => {
            CompositorBindingAction::Bearings(BearingsBindingAction::Toggle)
        }
        "trail-prev" | "trail_prev" => CompositorBindingAction::Trail(TrailBindingAction::Prev),
        "trail-next" | "trail_next" => CompositorBindingAction::Trail(TrailBindingAction::Next),
        "zoom-in" | "zoom_in" => CompositorBindingAction::ZoomIn,
        "zoom-out" | "zoom_out" => CompositorBindingAction::ZoomOut,
        "zoom-reset" | "zoom_reset" => CompositorBindingAction::ZoomReset,
        "move-left" | "move_left" => {
            CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left))
        }
        "move-right" | "move_right" => {
            CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Right))
        }
        "move-up" | "move_up" => {
            CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Up))
        }
        "move-down" | "move_down" => {
            CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Down))
        }
        "stack-cycle-forward" | "stack_cycle_forward" => {
            CompositorBindingAction::Stack(StackBindingAction::Cycle(StackCycleDirection::Forward))
        }
        "stack-cycle-backward" | "stack_cycle_backward" => {
            CompositorBindingAction::Stack(StackBindingAction::Cycle(StackCycleDirection::Backward))
        }
        "tile-focus-left" | "tile_focus_left" => {
            CompositorBindingAction::Tile(TileBindingAction::Focus(DirectionalAction::Left))
        }
        "tile-focus-right" | "tile_focus_right" => {
            CompositorBindingAction::Tile(TileBindingAction::Focus(DirectionalAction::Right))
        }
        "tile-focus-up" | "tile_focus_up" => {
            CompositorBindingAction::Tile(TileBindingAction::Focus(DirectionalAction::Up))
        }
        "tile-focus-down" | "tile_focus_down" => {
            CompositorBindingAction::Tile(TileBindingAction::Focus(DirectionalAction::Down))
        }
        _ => return None,
    };
    Some(GestureBindingAction::Compositor(compositor))
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

    use crate::keybinds::KeyModifiers;
    use crate::layout::{
        CompositorGestureScope, GestureBindingAction, GestureScrollPanMode, GestureSwipeDirection,
        InputFocusMode, RuntimeTuning,
    };

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
    gestures:
      enabled true
      client-passthrough false
      touch-passthrough true
      pinch-to-zoom false
      pinch-scope "empty-field"
      compositor-scope "global"
      modifier "ctrl+shift"
      scroll-pan "empty-field"
      swipe-threshold-px 96
    swipe-up-3 "apogee-open"
    apogee-swipe-up-3 "apogee-close"
    swipe-left-4 "trail-prev"
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
        assert!(out.input.gestures.enabled);
        assert!(!out.input.gestures.client_passthrough);
        assert!(out.input.gestures.touch_passthrough);
        assert!(!out.input.gestures.pinch_to_zoom);
        assert_eq!(
            out.input.gestures.pinch_scope,
            CompositorGestureScope::EmptyField
        );
        assert_eq!(
            out.input.gestures.compositor_scope,
            CompositorGestureScope::Global
        );
        assert_eq!(
            out.input.gestures.scroll_pan,
            GestureScrollPanMode::EmptyField
        );
        assert_eq!(
            out.input.gestures.modifier,
            KeyModifiers {
                ctrl: true,
                shift: true,
                ..KeyModifiers::default()
            }
        );
        assert_eq!(out.input.gestures.swipe_threshold_px, 96.0);
        assert_eq!(out.input.gestures.swipe_bindings.len(), 2);
        assert!(out.input.gestures.swipe_bindings.iter().any(|binding| {
            binding.direction == GestureSwipeDirection::Up
                && binding.fingers == 3
                && binding.action == GestureBindingAction::ApogeeOpen
        }));
        assert!(
            out.input
                .gestures
                .apogee_swipe_bindings
                .iter()
                .any(|binding| {
                    binding.direction == GestureSwipeDirection::Up
                        && binding.fingers == 3
                        && binding.action == GestureBindingAction::ApogeeClose
                })
        );
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
        assert!(tuning.input.gestures.enabled);
        assert!(tuning.input.gestures.client_passthrough);
        assert!(tuning.input.gestures.touch_passthrough);
        assert!(tuning.input.gestures.pinch_to_zoom);
        assert_eq!(
            tuning.input.gestures.pinch_scope,
            CompositorGestureScope::EmptyField
        );
        assert_eq!(
            tuning.input.gestures.compositor_scope,
            CompositorGestureScope::Global
        );
        assert_eq!(
            tuning.input.gestures.scroll_pan,
            GestureScrollPanMode::EmptyField
        );
        assert_eq!(
            tuning.input.gestures.modifier,
            KeyModifiers {
                left_alt: true,
                ..KeyModifiers::default()
            }
        );
        assert_eq!(tuning.input.gestures.swipe_threshold_px, 120.0);
        assert!(tuning.input.gestures.swipe_bindings.iter().any(|binding| {
            binding.direction == GestureSwipeDirection::Up
                && binding.fingers == 3
                && binding.action == GestureBindingAction::ApogeeOpen
        }));
        assert!(
            tuning
                .input
                .gestures
                .apogee_swipe_bindings
                .iter()
                .any(|binding| {
                    binding.direction == GestureSwipeDirection::Up
                        && binding.fingers == 3
                        && binding.action == GestureBindingAction::ApogeeClose
                })
        );
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
