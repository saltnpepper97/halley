//! Apply user-configured libinput settings to physical input devices.
//!
//! Settings are layered: a device's effective configuration is its type section
//! (`input.touchpad`/`input.mouse`) overlaid with any matching `input.devices.<name>` block.
//! Only fields the user actually set are pushed to libinput; everything else keeps libinput's
//! own default. Unsupported settings on a given device are silently ignored (libinput returns
//! `Unsupported`), so this never fails a device.

use halley_config::{
    AccelProfile as CfgAccelProfile, ClickMethod as CfgClickMethod, DeviceSettings, InputConfig,
    ScrollMethod as CfgScrollMethod, TapButtonMap as CfgTapButtonMap,
};
use smithay::reexports::input::{
    AccelProfile, ClickMethod, Device, DeviceCapability, ScrollMethod, SendEventsMode, TapButtonMap,
};

use eventline::debug;

/// Broad device class used to pick which type section applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeviceClass {
    Touchpad,
    Pointer,
    Other,
}

fn classify(device: &Device) -> DeviceClass {
    if device.has_capability(DeviceCapability::Pointer) {
        // A pointer that supports tap is a touchpad; everything else (mice, trackpoints,
        // trackballs) uses the `mouse` type section and can be tuned per-name.
        if device.config_tap_finger_count() > 0 {
            DeviceClass::Touchpad
        } else {
            DeviceClass::Pointer
        }
    } else {
        DeviceClass::Other
    }
}

/// Resolve and apply the effective libinput settings for `device`.
pub(crate) fn apply_device_config(device: &mut Device, input: &InputConfig) {
    let class = classify(device);
    let base = match class {
        DeviceClass::Touchpad => &input.touchpad,
        DeviceClass::Pointer => &input.mouse,
        // No type section drives keyboards/tablets/etc., but a named override still can.
        DeviceClass::Other => &DeviceSettings::default(),
    };

    let mut settings = base.clone();
    let name = device.name().to_string();
    for override_block in &input.devices {
        if device_name_matches(name.as_str(), override_block.name.as_str()) {
            settings = settings.overlay(&override_block.settings);
        }
    }

    apply_settings(device, &settings, name.as_str());
}

/// Match a configured device name against the libinput device name. Exact (case-insensitive)
/// match wins; otherwise a configured substring still matches, which is convenient for long
/// vendor strings.
fn device_name_matches(device_name: &str, configured: &str) -> bool {
    let device_name = device_name.trim();
    let configured = configured.trim();
    device_name.eq_ignore_ascii_case(configured)
        || device_name
            .to_ascii_lowercase()
            .contains(&configured.to_ascii_lowercase())
}

fn apply_settings(device: &mut Device, s: &DeviceSettings, name: &str) {
    if let Some(v) = s.natural_scroll {
        let _ = device.config_scroll_set_natural_scroll_enabled(v);
    }
    if let Some(v) = s.accel_speed {
        let _ = device.config_accel_set_speed(v);
    }
    if let Some(v) = s.accel_profile {
        let _ = device.config_accel_set_profile(map_accel_profile(v));
    }
    if let Some(v) = s.scroll_method {
        let _ = device.config_scroll_set_method(map_scroll_method(v));
    }
    if let Some(v) = s.scroll_button {
        let _ = device.config_scroll_set_button(v);
    }
    if let Some(v) = s.left_handed {
        let _ = device.config_left_handed_set(v);
    }
    if let Some(v) = s.middle_emulation {
        let _ = device.config_middle_emulation_set_enabled(v);
    }
    if let Some(v) = s.tap {
        let _ = device.config_tap_set_enabled(v);
    }
    if let Some(v) = s.tap_button_map {
        let _ = device.config_tap_set_button_map(map_tap_button_map(v));
    }
    if let Some(v) = s.dwt {
        let _ = device.config_dwt_set_enabled(v);
    }
    if let Some(v) = s.click_method {
        let _ = device.config_click_set_method(map_click_method(v));
    }
    if let Some(v) = s.drag {
        let _ = device.config_tap_set_drag_enabled(v);
    }
    if let Some(v) = s.drag_lock {
        let _ = device.config_tap_set_drag_lock_enabled(v);
    }
    // `enabled` and `disabled_on_external_mouse` both drive the send-events mode; the
    // external-mouse mode takes precedence when set.
    if s.disabled_on_external_mouse == Some(true) {
        let _ = device.config_send_events_set_mode(SendEventsMode::DISABLED_ON_EXTERNAL_MOUSE);
    } else if let Some(enabled) = s.enabled {
        let mode = if enabled {
            SendEventsMode::ENABLED
        } else {
            SendEventsMode::DISABLED
        };
        let _ = device.config_send_events_set_mode(mode);
    }

    debug!("applied libinput config to device '{name}'");
}

fn map_accel_profile(profile: CfgAccelProfile) -> AccelProfile {
    match profile {
        CfgAccelProfile::Adaptive => AccelProfile::Adaptive,
        CfgAccelProfile::Flat => AccelProfile::Flat,
    }
}

fn map_scroll_method(method: CfgScrollMethod) -> ScrollMethod {
    match method {
        CfgScrollMethod::NoScroll => ScrollMethod::NoScroll,
        CfgScrollMethod::TwoFinger => ScrollMethod::TwoFinger,
        CfgScrollMethod::Edge => ScrollMethod::Edge,
        CfgScrollMethod::OnButtonDown => ScrollMethod::OnButtonDown,
    }
}

fn map_click_method(method: CfgClickMethod) -> ClickMethod {
    match method {
        CfgClickMethod::ButtonAreas => ClickMethod::ButtonAreas,
        CfgClickMethod::Clickfinger => ClickMethod::Clickfinger,
    }
}

fn map_tap_button_map(map: CfgTapButtonMap) -> TapButtonMap {
    match map {
        CfgTapButtonMap::LeftRightMiddle => TapButtonMap::LeftRightMiddle,
        CfgTapButtonMap::LeftMiddleRight => TapButtonMap::LeftMiddleRight,
    }
}

#[cfg(test)]
mod tests {
    use super::device_name_matches;

    #[test]
    fn name_matching_is_case_insensitive_and_substring() {
        assert!(device_name_matches(
            "Logitech MX Master 3",
            "Logitech MX Master 3"
        ));
        assert!(device_name_matches(
            "Logitech MX Master 3",
            "logitech mx master 3"
        ));
        assert!(device_name_matches(
            "SynPS/2 Synaptics TouchPad",
            "Synaptics"
        ));
        assert!(!device_name_matches("Logitech MX Master 3", "Razer"));
    }
}
