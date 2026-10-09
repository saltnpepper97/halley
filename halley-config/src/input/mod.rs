use std::collections::HashSet;
use std::fmt;

use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

mod devices;
mod gestures;

pub use devices::{
    AccelProfile, ClickMethod, DeviceKind, DeviceOverride, DeviceSettings, MouseSettings,
    ScrollMethod, TapButtonMap,
};
pub use gestures::{
    GestureAction, GestureBinding, GestureHoldBinding, GestureModifier, GestureScope,
    GestureSettings, GestureSwipeDirection, ScrollPanMode,
};

const DEFAULT_REPEAT_RATE: i32 = 30;
const DEFAULT_REPEAT_DELAY: i32 = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusMode {
    #[default]
    Click,
    Hover,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardConfig {
    pub layout: String,
    pub variant: String,
    pub options: String,
    pub model: String,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: "us".to_string(),
            variant: String::new(),
            options: String::new(),
            model: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub focus_mode: FocusMode,
    pub raise_on_click: bool,
    pub keyboard: KeyboardConfig,
    pub gestures: GestureSettings,
    pub touchpad: DeviceSettings,
    pub mouse: DeviceSettings,
    pub trackpoint: DeviceSettings,
    pub trackball: DeviceSettings,
    pub touchscreen: DeviceSettings,
    pub devices: Vec<DeviceOverride>,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            repeat_rate: DEFAULT_REPEAT_RATE,
            repeat_delay: DEFAULT_REPEAT_DELAY,
            focus_mode: FocusMode::Click,
            raise_on_click: true,
            keyboard: KeyboardConfig::default(),
            gestures: GestureSettings::default(),
            touchpad: DeviceSettings::default(),
            mouse: DeviceSettings::default(),
            trackpoint: DeviceSettings::default(),
            trackball: DeviceSettings::default(),
            touchscreen: DeviceSettings::default(),
            devices: Vec::new(),
        }
    }
}

impl Input {
    pub fn settings_for_device(&self, kind: DeviceKind, name: &str) -> DeviceSettings {
        let base = match kind {
            DeviceKind::Mouse => &self.mouse,
            DeviceKind::Touchpad => &self.touchpad,
            DeviceKind::Trackpoint => &self.trackpoint,
            DeviceKind::Trackball => &self.trackball,
            DeviceKind::Touchscreen => &self.touchscreen,
        };
        self.devices
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
            .map_or_else(|| base.clone(), |entry| base.overlay(&entry.settings))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputParseError(String);

impl fmt::Display for InputParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InputParseError {}

pub fn parse_input(config: &RuneConfig) -> Result<Input, InputParseError> {
    let Value::Object(root) = config
        .get_value("")
        .map_err(|err| InputParseError(format!("input config: {err}")))?
    else {
        return Err(InputParseError(
            "input config root must be an object".to_string(),
        ));
    };

    let Some(input_value) = field(&root, "input") else {
        return Ok(Input::default());
    };
    let input_fields = object(input_value, "input")?;
    validate_fields(
        input_fields,
        &[
            "repeat-rate",
            "repeat-delay",
            "focus-mode",
            "raise-on-click",
            "keyboard",
            "gestures",
            "touchpad",
            "mouse",
            "trackpoint",
            "trackball",
            "touchscreen",
            "devices",
        ],
        "input",
    )?;
    let defaults = Input::default();

    let repeat_rate =
        optional_integer(input_fields, "repeat-rate", "input")?.unwrap_or(defaults.repeat_rate);
    if !(0..=1_000).contains(&repeat_rate) {
        return Err(InputParseError(
            "input.repeat-rate must be between 0 and 1000".to_string(),
        ));
    }
    let repeat_delay =
        optional_integer(input_fields, "repeat-delay", "input")?.unwrap_or(defaults.repeat_delay);
    if !(0..=10_000).contains(&repeat_delay) {
        return Err(InputParseError(
            "input.repeat-delay must be between 0 and 10000".to_string(),
        ));
    }

    let focus_mode = match optional_string(input_fields, "focus-mode", "input")?.as_deref() {
        None | Some("click") => FocusMode::Click,
        Some("hover") => FocusMode::Hover,
        Some(value) => {
            return Err(InputParseError(format!(
                "input.focus-mode must be \"click\" or \"hover\", got {value:?}"
            )));
        }
    };
    let raise_on_click = optional_bool(input_fields, "raise-on-click", "input")?.unwrap_or(true);

    let keyboard = match field(input_fields, "keyboard") {
        Some(value) => parse_keyboard(object(value, "input.keyboard")?)?,
        None => defaults.keyboard,
    };
    let gestures = match field(input_fields, "gestures") {
        Some(value) => parse_gestures(object(value, "input.gestures")?)?,
        None => defaults.gestures,
    };
    let touchpad = parse_device_section(input_fields, "touchpad", DeviceKind::Touchpad)?;
    let mouse = match field(input_fields, "mouse") {
        Some(value) => parse_device_settings(
            object(value, "input.mouse")?,
            "input.mouse",
            Some(DeviceKind::Mouse),
        )?,
        None => DeviceSettings::default(),
    };
    let trackpoint = parse_device_section(input_fields, "trackpoint", DeviceKind::Trackpoint)?;
    let trackball = parse_device_section(input_fields, "trackball", DeviceKind::Trackball)?;
    let touchscreen = parse_device_section(input_fields, "touchscreen", DeviceKind::Touchscreen)?;
    let devices = match field(input_fields, "devices") {
        Some(value) => parse_devices(object(value, "input.devices")?)?,
        None => Vec::new(),
    };

    Ok(Input {
        repeat_rate,
        repeat_delay,
        focus_mode,
        raise_on_click,
        keyboard,
        gestures,
        touchpad,
        mouse,
        trackpoint,
        trackball,
        touchscreen,
        devices,
    })
}

fn parse_keyboard(fields: &[ObjectItem]) -> Result<KeyboardConfig, InputParseError> {
    validate_fields(
        fields,
        &["layout", "variant", "options", "model"],
        "input.keyboard",
    )?;
    let defaults = KeyboardConfig::default();
    Ok(KeyboardConfig {
        layout: optional_string(fields, "layout", "input.keyboard")?.unwrap_or(defaults.layout),
        variant: optional_string(fields, "variant", "input.keyboard")?.unwrap_or(defaults.variant),
        options: optional_string(fields, "options", "input.keyboard")?.unwrap_or(defaults.options),
        model: optional_string(fields, "model", "input.keyboard")?.unwrap_or(defaults.model),
    })
}

fn parse_gestures(fields: &[ObjectItem]) -> Result<GestureSettings, InputParseError> {
    let path = "input.gestures";
    validate_gesture_fields(fields, path)?;
    let defaults = GestureSettings::default();
    let pan_fingers = optional_u32(fields, "pan-fingers", path)?.unwrap_or(defaults.pan_fingers);
    if pan_fingers > 32 {
        return Err(InputParseError(
            "input.gestures.pan-fingers must be between 0 and 32".to_string(),
        ));
    }
    let pan_decay_rate =
        optional_number(fields, "pan-decay-rate", path)?.unwrap_or(defaults.pan_decay_rate.into());
    if !(0.5..=30.0).contains(&pan_decay_rate) {
        return Err(InputParseError(
            "input.gestures.pan-decay-rate must be between 0.5 and 30".to_string(),
        ));
    }
    let flick_min_px_per_s = optional_number(fields, "flick-min-px-per-s", path)?
        .unwrap_or(defaults.flick_min_px_per_s.into());
    if !(0.0..=100_000.0).contains(&flick_min_px_per_s) {
        return Err(InputParseError(
            "input.gestures.flick-min-px-per-s must be between 0 and 100000".to_string(),
        ));
    }
    let swipe_threshold_px = optional_number(fields, "swipe-threshold-px", path)?
        .unwrap_or(defaults.swipe_threshold_px.into());
    if !(8.0..=10_000.0).contains(&swipe_threshold_px) {
        return Err(InputParseError(
            "input.gestures.swipe-threshold-px must be between 8 and 10000".to_string(),
        ));
    }
    let (swipe_bindings, apogee_swipe_bindings, hold_bindings) =
        parse_gesture_bindings(fields, path)?;

    Ok(GestureSettings {
        enabled: optional_bool(fields, "enabled", path)?.unwrap_or(defaults.enabled),
        client_passthrough: optional_bool(fields, "client-passthrough", path)?
            .unwrap_or(defaults.client_passthrough),
        touch_passthrough: optional_bool(fields, "touch-passthrough", path)?
            .unwrap_or(defaults.touch_passthrough),
        pinch_to_zoom: optional_bool(fields, "pinch-to-zoom", path)?
            .unwrap_or(defaults.pinch_to_zoom),
        pinch_scope: parse_scope(fields, "pinch-scope", path)?.unwrap_or(defaults.pinch_scope),
        compositor_scope: parse_scope(fields, "compositor-scope", path)?
            .unwrap_or(defaults.compositor_scope),
        modifier: parse_gesture_modifier(fields, path)?.unwrap_or(defaults.modifier),
        scroll_pan: match optional_string(fields, "scroll-pan", path)?.as_deref() {
            None => defaults.scroll_pan,
            Some("off") => ScrollPanMode::Off,
            Some("empty-field") => ScrollPanMode::EmptyField,
            Some(value) => {
                return Err(InputParseError(format!(
                    "{path}.scroll-pan must be \"off\" or \"empty-field\", got {value:?}"
                )));
            }
        },
        pan_fingers,
        pan_momentum: optional_bool(fields, "pan-momentum", path)?.unwrap_or(defaults.pan_momentum),
        pan_decay_rate: pan_decay_rate as f32,
        flick_min_px_per_s: flick_min_px_per_s as f32,
        swipe_threshold_px: swipe_threshold_px as f32,
        swipe_bindings: if swipe_bindings.is_empty() {
            defaults.swipe_bindings
        } else {
            swipe_bindings
        },
        apogee_swipe_bindings: if apogee_swipe_bindings.is_empty() {
            defaults.apogee_swipe_bindings
        } else {
            apogee_swipe_bindings
        },
        hold_bindings,
    })
}

enum GestureBindingKey {
    Swipe {
        apogee: bool,
        direction: GestureSwipeDirection,
        fingers: u32,
    },
    Hold {
        fingers: u32,
    },
}

fn parse_gesture_binding_key(key: &str) -> Option<GestureBindingKey> {
    if let Some(fingers) = key.strip_prefix("hold-") {
        let fingers = fingers.parse::<u32>().ok()?;
        return (1..=32)
            .contains(&fingers)
            .then_some(GestureBindingKey::Hold { fingers });
    }
    let (apogee, suffix) = if let Some(suffix) = key.strip_prefix("apogee-swipe-") {
        (true, suffix)
    } else {
        (false, key.strip_prefix("swipe-")?)
    };
    let (direction, fingers) = suffix.rsplit_once('-')?;
    let direction = match direction {
        "up" => GestureSwipeDirection::Up,
        "down" => GestureSwipeDirection::Down,
        "left" => GestureSwipeDirection::Left,
        "right" => GestureSwipeDirection::Right,
        _ => return None,
    };
    let fingers = fingers.parse::<u32>().ok()?;
    (1..=32)
        .contains(&fingers)
        .then_some(GestureBindingKey::Swipe {
            apogee,
            direction,
            fingers,
        })
}

fn validate_gesture_fields(fields: &[ObjectItem], path: &str) -> Result<(), InputParseError> {
    const FIXED: &[&str] = &[
        "enabled",
        "client-passthrough",
        "touch-passthrough",
        "pinch-to-zoom",
        "pinch-scope",
        "compositor-scope",
        "modifier",
        "scroll-pan",
        "pan-fingers",
        "pan-momentum",
        "pan-decay-rate",
        "flick-min-px-per-s",
        "swipe-threshold-px",
    ];
    let mut seen = HashSet::new();
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            continue;
        };
        if !FIXED.contains(&key.as_str()) && parse_gesture_binding_key(key).is_none() {
            return Err(InputParseError(format!(
                "{path}: unsupported field {key:?}"
            )));
        }
        if !seen.insert(key) {
            return Err(InputParseError(format!("{path}: duplicate field {key:?}")));
        }
    }
    Ok(())
}

fn parse_gesture_action(value: &Value, path: &str) -> Result<GestureAction, InputParseError> {
    let Value::String(value) = value else {
        return Err(InputParseError(format!("{path} must be a string")));
    };
    match value.as_str() {
        "apogee-open" | "overview-open" => Ok(GestureAction::ApogeeOpen),
        "apogee-close" | "overview-close" => Ok(GestureAction::ApogeeClose),
        _ => match crate::parse::parse_action(value) {
            crate::Action::Spawn(_)
            | crate::Action::PointerMoveWindow
            | crate::Action::PointerResizeWindow
            | crate::Action::PointerPanField
            | crate::Action::PointerPanFieldAlways
            | crate::Action::PointerDragPan => Err(InputParseError(format!(
                "{path}: unsupported gesture action {value:?}"
            ))),
            action => Ok(GestureAction::Compositor(action)),
        },
    }
}

type ParsedGestureBindings = (
    Vec<GestureBinding>,
    Vec<GestureBinding>,
    Vec<GestureHoldBinding>,
);

fn parse_gesture_bindings(
    fields: &[ObjectItem],
    path: &str,
) -> Result<ParsedGestureBindings, InputParseError> {
    let mut swipe = Vec::new();
    let mut apogee_swipe = Vec::new();
    let mut hold = Vec::new();
    for item in fields {
        let ObjectItem::Assign(key, value) = item else {
            continue;
        };
        let Some(binding) = parse_gesture_binding_key(key) else {
            continue;
        };
        let action = parse_gesture_action(value, &format!("{path}.{key}"))?;
        match binding {
            GestureBindingKey::Swipe {
                apogee,
                direction,
                fingers,
            } => {
                let binding = GestureBinding {
                    direction,
                    fingers,
                    action,
                };
                if apogee {
                    apogee_swipe.push(binding);
                } else {
                    swipe.push(binding);
                }
            }
            GestureBindingKey::Hold { fingers } => {
                hold.push(GestureHoldBinding { fingers, action });
            }
        }
    }
    Ok((swipe, apogee_swipe, hold))
}

fn parse_scope(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<GestureScope>, InputParseError> {
    match optional_string(fields, key, path)?.as_deref() {
        None => Ok(None),
        Some("empty-field") => Ok(Some(GestureScope::EmptyField)),
        Some("global") => Ok(Some(GestureScope::Global)),
        Some(value) => Err(InputParseError(format!(
            "{path}.{key} must be \"empty-field\" or \"global\", got {value:?}"
        ))),
    }
}

fn parse_gesture_modifier(
    fields: &[ObjectItem],
    path: &str,
) -> Result<Option<GestureModifier>, InputParseError> {
    let Some(value) = optional_string(fields, "modifier", path)? else {
        return Ok(None);
    };
    let modifier = match value.to_ascii_lowercase().as_str() {
        "$var.mod" | "$mod" | "mod" => GestureModifier::Keybind,
        "off" => GestureModifier::Disabled,
        "super" | "logo" | "mod4" => GestureModifier::Explicit(crate::ModifierKey::Super),
        "lsuper" | "left-super" | "lwin" | "left-logo" => {
            GestureModifier::Explicit(crate::ModifierKey::LeftSuper)
        }
        "rsuper" | "right-super" | "rwin" | "right-logo" => {
            GestureModifier::Explicit(crate::ModifierKey::RightSuper)
        }
        "alt" => GestureModifier::Explicit(crate::ModifierKey::Alt),
        "lalt" | "left-alt" => GestureModifier::Explicit(crate::ModifierKey::LeftAlt),
        "ralt" | "right-alt" => GestureModifier::Explicit(crate::ModifierKey::RightAlt),
        "ctrl" | "control" => GestureModifier::Explicit(crate::ModifierKey::Ctrl),
        "lctrl" | "left-ctrl" | "left-control" => {
            GestureModifier::Explicit(crate::ModifierKey::LeftCtrl)
        }
        "rctrl" | "right-ctrl" | "right-control" => {
            GestureModifier::Explicit(crate::ModifierKey::RightCtrl)
        }
        "shift" => GestureModifier::Explicit(crate::ModifierKey::Shift),
        "lshift" | "left-shift" => GestureModifier::Explicit(crate::ModifierKey::LeftShift),
        "rshift" | "right-shift" => GestureModifier::Explicit(crate::ModifierKey::RightShift),
        _ => {
            return Err(InputParseError(format!(
                "{path}.modifier must be \"mod\", \"off\", \"super\", \"alt\", \"ctrl\", or \
                 \"shift\", got {value:?}"
            )));
        }
    };
    Ok(Some(modifier))
}

fn parse_device_section(
    input_fields: &[ObjectItem],
    name: &str,
    kind: DeviceKind,
) -> Result<DeviceSettings, InputParseError> {
    let path = format!("input.{name}");
    match field(input_fields, name) {
        Some(value) => parse_device_settings(object(value, &path)?, &path, Some(kind)),
        None => Ok(DeviceSettings::default()),
    }
}

fn parse_devices(fields: &[ObjectItem]) -> Result<Vec<DeviceOverride>, InputParseError> {
    let mut seen = HashSet::new();
    let mut devices = Vec::new();
    for item in fields {
        let ObjectItem::Assign(name, value) = item else {
            continue;
        };
        let normalized = name.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(InputParseError(
                "input.devices names must not be empty".to_string(),
            ));
        }
        if !seen.insert(normalized) {
            return Err(InputParseError(format!(
                "duplicate input.devices entry for {name:?}"
            )));
        }
        let path = format!("input.devices.{name}");
        devices.push(DeviceOverride {
            name: name.trim().to_string(),
            settings: parse_device_settings(object(value, &path)?, &path, None)?,
        });
    }
    Ok(devices)
}

fn parse_device_settings(
    fields: &[ObjectItem],
    path: &str,
    kind: Option<DeviceKind>,
) -> Result<DeviceSettings, InputParseError> {
    const POINTER_FIELDS: &[&str] = &[
        "enabled",
        "natural-scroll",
        "accel-speed",
        "sensitivity",
        "accel-profile",
        "scroll-method",
        "scroll-button",
        "scroll-button-lock",
        "left-handed",
        "middle-emulation",
    ];
    const TOUCHPAD_FIELDS: &[&str] = &[
        "enabled",
        "natural-scroll",
        "accel-speed",
        "sensitivity",
        "accel-profile",
        "scroll-method",
        "scroll-button",
        "scroll-button-lock",
        "left-handed",
        "middle-emulation",
        "tap",
        "tap-button-map",
        "dwt",
        "click-method",
        "drag",
        "drag-lock",
        "disabled-on-external-mouse",
    ];
    const TOUCHSCREEN_FIELDS: &[&str] = &["enabled", "map-to-output", "calibration-matrix"];
    const ALL_FIELDS: &[&str] = &[
        "enabled",
        "natural-scroll",
        "accel-speed",
        "sensitivity",
        "accel-profile",
        "scroll-method",
        "scroll-button",
        "scroll-button-lock",
        "left-handed",
        "middle-emulation",
        "tap",
        "tap-button-map",
        "dwt",
        "click-method",
        "drag",
        "drag-lock",
        "disabled-on-external-mouse",
        "map-to-output",
        "calibration-matrix",
    ];
    let allowed = match kind {
        Some(DeviceKind::Touchpad) => TOUCHPAD_FIELDS,
        Some(DeviceKind::Touchscreen) => TOUCHSCREEN_FIELDS,
        Some(DeviceKind::Mouse | DeviceKind::Trackpoint | DeviceKind::Trackball) => POINTER_FIELDS,
        None => ALL_FIELDS,
    };
    validate_fields(fields, allowed, path)?;
    let speed = optional_accel_speed(fields, "accel-speed", path)?;
    let sensitivity = optional_accel_speed(fields, "sensitivity", path)?;
    if let (Some(speed), Some(sensitivity)) = (speed, sensitivity)
        && speed != sensitivity
    {
        return Err(InputParseError(format!(
            "{path}.accel-speed and sensitivity must agree when both are set"
        )));
    }
    let accel_speed = speed.or(sensitivity);
    if accel_speed.is_some_and(|speed| !(-1.0..=1.0).contains(&speed)) {
        return Err(InputParseError(format!(
            "{path}.accel-speed must be between -1.0 and 1.0"
        )));
    }

    let accel_profile = match optional_string(fields, "accel-profile", path)?.as_deref() {
        None => None,
        Some("adaptive") => Some(AccelProfile::Adaptive),
        Some("flat") => Some(AccelProfile::Flat),
        Some(value) => {
            return Err(InputParseError(format!(
                "{path}.accel-profile must be \"adaptive\" or \"flat\", got {value:?}"
            )));
        }
    };
    let scroll_method = match optional_string(fields, "scroll-method", path)?.as_deref() {
        None => None,
        Some("none") => Some(ScrollMethod::None),
        Some("two-finger") => Some(ScrollMethod::TwoFinger),
        Some("edge") => Some(ScrollMethod::Edge),
        Some("on-button-down") => Some(ScrollMethod::OnButtonDown),
        Some(value) => {
            return Err(InputParseError(format!(
                "{path}.scroll-method must be \"none\", \"two-finger\", \"edge\", or \
                 \"on-button-down\", got {value:?}"
            )));
        }
    };

    let tap_button_map = match optional_string(fields, "tap-button-map", path)?.as_deref() {
        None => None,
        Some("left-right-middle") => Some(TapButtonMap::LeftRightMiddle),
        Some("left-middle-right") => Some(TapButtonMap::LeftMiddleRight),
        Some(value) => {
            return Err(InputParseError(format!(
                "{path}.tap-button-map must be \"left-right-middle\" or \"left-middle-right\", got \
                 {value:?}"
            )));
        }
    };
    let click_method = match optional_string(fields, "click-method", path)?.as_deref() {
        None => None,
        Some("clickfinger") => Some(ClickMethod::Clickfinger),
        Some("button-areas") => Some(ClickMethod::ButtonAreas),
        Some(value) => {
            return Err(InputParseError(format!(
                "{path}.click-method must be \"clickfinger\" or \"button-areas\", got {value:?}"
            )));
        }
    };

    Ok(DeviceSettings {
        enabled: optional_bool(fields, "enabled", path)?,
        natural_scroll: optional_bool(fields, "natural-scroll", path)?,
        accel_speed,
        accel_profile,
        scroll_method,
        scroll_button: optional_u32(fields, "scroll-button", path)?,
        scroll_button_lock: optional_bool(fields, "scroll-button-lock", path)?,
        left_handed: optional_bool(fields, "left-handed", path)?,
        middle_emulation: optional_bool(fields, "middle-emulation", path)?,
        tap: optional_bool(fields, "tap", path)?,
        tap_button_map,
        dwt: optional_bool(fields, "dwt", path)?,
        click_method,
        drag: optional_bool(fields, "drag", path)?,
        drag_lock: optional_bool(fields, "drag-lock", path)?,
        disabled_on_external_mouse: optional_bool(fields, "disabled-on-external-mouse", path)?,
        map_to_output: optional_string(fields, "map-to-output", path)?,
        calibration_matrix: optional_matrix(fields, "calibration-matrix", path)?,
    })
}

fn field<'a>(fields: &'a [ObjectItem], key: &str) -> Option<&'a Value> {
    fields.iter().find_map(|item| match item {
        ObjectItem::Assign(candidate, value) if candidate == key => Some(value),
        _ => None,
    })
}

fn validate_fields(
    fields: &[ObjectItem],
    allowed: &[&str],
    path: &str,
) -> Result<(), InputParseError> {
    let mut seen = HashSet::new();
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            continue;
        };
        if !allowed.contains(&key.as_str()) {
            return Err(InputParseError(format!(
                "{path}: unsupported field {key:?}"
            )));
        }
        if !seen.insert(key) {
            return Err(InputParseError(format!("{path}: duplicate field {key:?}")));
        }
    }
    Ok(())
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a [ObjectItem], InputParseError> {
    match value {
        Value::Object(fields) => Ok(fields),
        _ => Err(InputParseError(format!("{path} must be a block"))),
    }
}

fn optional_string(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<String>, InputParseError> {
    match field(fields, key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(InputParseError(format!("{path}.{key} must be a string"))),
    }
}

fn optional_bool(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<bool>, InputParseError> {
    match field(fields, key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(InputParseError(format!("{path}.{key} must be a boolean"))),
    }
}

fn optional_accel_speed(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<f64>, InputParseError> {
    if let Some(Value::String(value)) = field(fields, key) {
        return value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| InputParseError(format!("{path}.{key} must be a finite number")));
    }
    optional_number(fields, key, path)
}

fn optional_number(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<f64>, InputParseError> {
    match field(fields, key) {
        None => Ok(None),
        Some(Value::Number(value)) if value.is_finite() => Ok(Some(*value)),
        Some(_) => Err(InputParseError(format!(
            "{path}.{key} must be a finite number"
        ))),
    }
}

fn optional_integer(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<i32>, InputParseError> {
    optional_number(fields, key, path)?.map_or(Ok(None), |value| {
        if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
            Ok(Some(value as i32))
        } else {
            Err(InputParseError(format!("{path}.{key} must be an integer")))
        }
    })
}

fn optional_u32(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<u32>, InputParseError> {
    optional_number(fields, key, path)?.map_or(Ok(None), |value| {
        if value.fract() == 0.0 && value >= 0.0 && value <= u32::MAX as f64 {
            Ok(Some(value as u32))
        } else {
            Err(InputParseError(format!(
                "{path}.{key} must be a non-negative integer"
            )))
        }
    })
}

fn optional_matrix(
    fields: &[ObjectItem],
    key: &str,
    path: &str,
) -> Result<Option<[f32; 6]>, InputParseError> {
    let Some(value) = field(fields, key) else {
        return Ok(None);
    };
    let Value::Array(values) = value else {
        return Err(InputParseError(format!(
            "{path}.{key} must be an array of six finite numbers"
        )));
    };
    if values.len() != 6 {
        return Err(InputParseError(format!(
            "{path}.{key} must contain exactly six numbers"
        )));
    }
    let mut matrix = [0.0; 6];
    for (index, value) in values.iter().enumerate() {
        let Value::Number(value) = value else {
            return Err(InputParseError(format!(
                "{path}.{key} must contain exactly six finite numbers"
            )));
        };
        if !value.is_finite() || *value < f32::MIN as f64 || *value > f32::MAX as f64 {
            return Err(InputParseError(format!(
                "{path}.{key} must contain exactly six finite numbers"
            )));
        }
        matrix[index] = *value as f32;
    }
    Ok(Some(matrix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Result<Input, InputParseError> {
        let config = RuneConfig::from_str(source).expect("syntactically valid Rune");
        parse_input(&config)
    }

    #[test]
    fn parses_keyboard_focus_and_mouse_settings() {
        let input = parse(
            r#"
input:
  repeat-rate 45
  repeat-delay 350
  focus-mode "hover"
  raise-on-click false
  keyboard:
    layout "ca"
    variant "multix"
    options "caps:escape"
    model "pc105"
  end
  mouse:
    enabled true
    natural-scroll true
    accel-speed 0.25
    accel-profile "flat"
    scroll-method "on-button-down"
    scroll-button 274
    left-handed true
    middle-emulation true
  end
end
"#,
        )
        .unwrap();

        assert_eq!(input.repeat_rate, 45);
        assert_eq!(input.repeat_delay, 350);
        assert_eq!(input.focus_mode, FocusMode::Hover);
        assert!(!input.raise_on_click);
        assert_eq!(input.keyboard.layout, "ca");
        assert_eq!(input.keyboard.variant, "multix");
        assert_eq!(input.mouse.accel_speed, Some(0.25));
        assert_eq!(input.mouse.accel_profile, Some(AccelProfile::Flat));
        assert_eq!(input.mouse.scroll_method, Some(ScrollMethod::OnButtonDown));
        assert_eq!(input.mouse.scroll_button, Some(274));
    }

    #[test]
    fn signed_accel_speed_and_sensitivity_alias() {
        for setting in [
            "accel-speed -0.5",
            "accel-speed \"-0.5\"",
            "sensitivity -0.5",
        ] {
            let input = parse(&format!("input:\n  mouse:\n    {setting}\n  end\nend\n")).unwrap();
            assert_eq!(input.mouse.accel_speed, Some(-0.5));
        }
        for setting in [
            "accel-speed -1.1",
            "sensitivity \"NaN\"",
            "accel-speed -0.5\n    sensitivity 0.5",
        ] {
            assert!(parse(&format!("input:\n  mouse:\n    {setting}\n  end\nend\n")).is_err());
        }
    }

    #[test]
    fn exact_case_insensitive_override_layers_over_mouse() {
        let input = parse(
            r#"
input:
  mouse:
    accel-speed 0.4
    natural-scroll false
  end
  devices:
    "Logitech MX Master 3":
      natural-scroll true
    end
  end
end
"#,
        )
        .unwrap();

        let matching = input.settings_for_device(DeviceKind::Mouse, "logitech mx master 3");
        assert_eq!(matching.accel_speed, Some(0.4));
        assert_eq!(matching.natural_scroll, Some(true));
        assert_eq!(
            input.settings_for_device(DeviceKind::Mouse, "Logitech MX Master 3S"),
            input.mouse
        );
    }

    #[test]
    fn parses_device_classes_gestures_and_touchscreen_mapping() {
        let input = parse(
            r#"
input:
  gestures:
    enabled true
    client-passthrough false
    touch-passthrough true
    pinch-to-zoom true
    pinch-scope "global"
    compositor-scope "empty-field"
    modifier "mod"
    scroll-pan "off"
    pan-fingers 4
    pan-momentum false
    pan-decay-rate 8
    flick-min-px-per-s 250
  end
  touchpad:
    tap true
    tap-button-map "left-middle-right"
    dwt true
    click-method "clickfinger"
    drag true
    drag-lock true
    disabled-on-external-mouse true
  end
  trackpoint:
    scroll-method "on-button-down"
    scroll-button 274
    scroll-button-lock true
  end
  trackball:
    accel-profile "flat"
  end
  touchscreen:
    enabled true
    map-to-output "eDP-1"
    calibration-matrix [1, 0, 0, 0, 1, 0]
  end
end
"#,
        )
        .unwrap();

        assert!(!input.gestures.client_passthrough);
        assert_eq!(input.gestures.pinch_scope, GestureScope::Global);
        assert_eq!(input.gestures.modifier, GestureModifier::Keybind);
        assert_eq!(input.gestures.scroll_pan, ScrollPanMode::Off);
        assert_eq!(input.gestures.pan_fingers, 4);
        assert_eq!(input.touchpad.tap, Some(true));
        assert_eq!(
            input.touchpad.tap_button_map,
            Some(TapButtonMap::LeftMiddleRight)
        );
        assert_eq!(input.trackpoint.scroll_button_lock, Some(true));
        assert_eq!(input.touchscreen.map_to_output.as_deref(), Some("eDP-1"));
        assert_eq!(
            input.touchscreen.calibration_matrix,
            Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
        );
    }

    #[test]
    fn parses_arbitrary_swipe_apogee_swipe_and_hold_bindings() {
        let input = parse(
            r#"
input:
  gestures:
    swipe-left-5 "trail-prev"
    swipe-right-5 "monitor focus DP-1"
    apogee-swipe-up-3 "toggle-state"
    hold-2 "zoom-reset"
  end
end
"#,
        )
        .unwrap();

        assert_eq!(input.gestures.swipe_bindings.len(), 2);
        assert!(input.gestures.swipe_bindings.iter().any(|binding| {
            binding.direction == GestureSwipeDirection::Left
                && binding.fingers == 5
                && binding.action
                    == GestureAction::Compositor(crate::Action::Trail(
                        crate::TrailDirection::Previous,
                    ))
        }));
        assert!(input.gestures.swipe_bindings.iter().any(|binding| {
            binding.direction == GestureSwipeDirection::Right
                && binding.action
                    == GestureAction::Compositor(crate::Action::MonitorFocus(
                        crate::MonitorTarget::Output("DP-1".into()),
                    ))
        }));
        assert_eq!(input.gestures.apogee_swipe_bindings.len(), 1);
        assert_eq!(input.gestures.hold_bindings.len(), 1);
        assert_eq!(
            input.gestures.hold_bindings[0].action,
            GestureAction::Compositor(crate::Action::ZoomReset)
        );
    }

    #[test]
    fn rejects_malformed_gesture_bindings_and_non_compositor_actions() {
        for (binding, expected) in [
            ("swipe-sideways-3 \"zoom-reset\"", "unsupported field"),
            ("hold-0 \"zoom-reset\"", "unsupported field"),
            ("hold-3 \"notify-send hi\"", "unsupported gesture action"),
            ("hold-3 \"move-window\"", "unsupported gesture action"),
            ("hold-3 \"pan-field-always\"", "unsupported gesture action"),
        ] {
            let error =
                parse(&format!("input:\n  gestures:\n    {binding}\n  end\nend\n")).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn device_override_layers_over_the_matching_class() {
        let input = parse(
            r#"
input:
  touchpad:
    natural-scroll true
    tap false
  end
  devices:
    "Built-in Touchpad":
      tap true
    end
  end
end
"#,
        )
        .unwrap();

        let settings = input.settings_for_device(DeviceKind::Touchpad, "built-in touchpad");
        assert_eq!(settings.natural_scroll, Some(true));
        assert_eq!(settings.tap, Some(true));
        assert_eq!(
            input
                .settings_for_device(DeviceKind::Mouse, "Built-in Touchpad")
                .tap,
            Some(true),
            "exact overrides are device-specific and layer over the classified base"
        );
    }

    #[test]
    fn rejects_duplicate_normalized_device_names() {
        let err = parse(
            r#"
input:
  devices:
    "Mouse":
      enabled true
    end
    "mouse":
      enabled false
    end
  end
end
"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_invalid_ranges_types_and_enums() {
        for (source, expected) in [
            ("input:\n  repeat-rate 1001\nend\n", "repeat-rate"),
            ("input:\n  repeat-delay 10001\nend\n", "repeat-delay"),
            ("input:\n  focus-mode \"sloppy\"\nend\n", "focus-mode"),
            (
                "input:\n  mouse:\n    accel-speed 1.1\n  end\nend\n",
                "accel-speed",
            ),
            (
                "input:\n  mouse:\n    accel-profile \"magic\"\n  end\nend\n",
                "accel-profile",
            ),
            (
                "input:\n  mouse:\n    natural-scroll \"yes\"\n  end\nend\n",
                "natural-scroll",
            ),
        ] {
            assert!(
                parse(source)
                    .expect_err("invalid input should fail")
                    .to_string()
                    .contains(expected),
                "{source:?} did not mention {expected:?}"
            );
        }
    }

    #[test]
    fn rejects_typos_and_out_of_scope_device_fields() {
        for (source, expected) in [
            (
                "input:\n  repeat_rate 30\nend\n",
                "unsupported field \"repeat_rate\"",
            ),
            (
                "input:\n  trackball:\n    tap true\n  end\nend\n",
                "unsupported field \"tap\"",
            ),
            (
                "input:\n  mouse:\n    middle-emulaton true\n  end\nend\n",
                "unsupported field \"middle-emulaton\"",
            ),
            (
                "input:\n  keyboard:\n    layout \"us\"\n    layout \"ca\"\n  end\nend\n",
                "duplicate field \"layout\"",
            ),
        ] {
            assert!(
                parse(source)
                    .expect_err("unsupported input should fail")
                    .to_string()
                    .contains(expected)
            );
        }
    }

    #[test]
    fn omitted_mouse_values_preserve_device_defaults() {
        let input = parse("input:\nend\n").unwrap();
        assert_eq!(input.mouse, DeviceSettings::default());
        assert_eq!(input.touchpad, DeviceSettings::default());
        assert_eq!(input.trackpoint, DeviceSettings::default());
        assert_eq!(input.trackball, DeviceSettings::default());
        assert_eq!(input.touchscreen, DeviceSettings::default());
        assert!(input.devices.is_empty());
    }
}
