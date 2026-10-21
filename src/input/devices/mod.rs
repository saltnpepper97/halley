mod keyboard_leds;
mod settings;

use halley_config::DeviceKind;
use smithay::reexports::input::{Device, DeviceCapability};

use keyboard_leds::KeyboardLeds;
use settings::{DeviceDefaults, apply};
use smithay::input::keyboard::LedState;

struct ManagedDevice {
    device: Device,
    kind: DeviceKind,
    defaults: DeviceDefaults,
    touch_capable: bool,
}

/// Owns only physical libinput configuration and device-presence facts.
/// Wayland seat capabilities and event routing remain session policy.
#[derive(Default)]
pub struct PhysicalInputDevices {
    devices: Vec<ManagedDevice>,
    touch_count: usize,
    keyboards: KeyboardLeds,
}

impl PhysicalInputDevices {
    /// Adds and configures one physical device. Returns `true` only when the
    /// first touchscreen appears and the session should advertise wl_touch.
    pub fn added(
        &mut self,
        mut device: Device,
        input: &halley_config::Input,
        leds: LedState,
    ) -> bool {
        if device.has_capability(DeviceCapability::Keyboard) {
            self.keyboards.added(device.clone(), leds);
        }
        let Some(kind) = classify(&device) else {
            return false;
        };
        let touch_capable = device.has_capability(DeviceCapability::Touch);
        let defaults = DeviceDefaults::read(&device);
        apply(&mut device, kind, &defaults, input);
        eventline::debug!("input: managing {:?} {:?}", kind, device.name().as_ref());
        let first_touch = touch_capable && self.touch_count == 0;
        self.touch_count += usize::from(touch_capable);
        self.devices.push(ManagedDevice {
            device,
            kind,
            defaults,
            touch_capable,
        });
        first_touch
    }

    /// Removes one device. Returns `true` only when the final touchscreen
    /// disappears and the session should remove wl_touch.
    pub fn removed(&mut self, device: &Device) -> bool {
        self.keyboards.removed(device);
        let before = self.touch_count;
        self.devices.retain(|candidate| {
            if candidate.device == *device {
                self.touch_count = self
                    .touch_count
                    .saturating_sub(usize::from(candidate.touch_capable));
                false
            } else {
                true
            }
        });
        before > 0 && self.touch_count == 0
    }

    pub fn reload(&mut self, input: &halley_config::Input) {
        for managed in &mut self.devices {
            apply(&mut managed.device, managed.kind, &managed.defaults, input);
        }
    }

    pub fn update_keyboard_leds(&mut self, state: LedState) {
        self.keyboards.update(state);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeviceFacts {
    pointer: bool,
    touch: bool,
    tablet_tool: bool,
    tablet_pad: bool,
    tap_fingers: u32,
    trackball: bool,
    trackpoint: bool,
}

fn classify_facts(facts: DeviceFacts) -> Option<DeviceKind> {
    if facts.touch && !facts.tablet_tool && !facts.tablet_pad {
        return Some(DeviceKind::Touchscreen);
    }
    if !facts.pointer || facts.tablet_tool || facts.tablet_pad {
        return None;
    }
    if facts.tap_fingers > 0 {
        Some(DeviceKind::Touchpad)
    } else if facts.trackball {
        Some(DeviceKind::Trackball)
    } else if facts.trackpoint {
        Some(DeviceKind::Trackpoint)
    } else {
        Some(DeviceKind::Mouse)
    }
}

fn classify(device: &Device) -> Option<DeviceKind> {
    let (trackball, trackpoint) = unsafe { device.udev_device() }.map_or((false, false), |udev| {
        (
            udev.property_value("ID_INPUT_TRACKBALL").is_some(),
            udev.property_value("ID_INPUT_POINTINGSTICK").is_some(),
        )
    });
    classify_facts(DeviceFacts {
        pointer: device.has_capability(DeviceCapability::Pointer),
        touch: device.has_capability(DeviceCapability::Touch),
        tablet_tool: device.has_capability(DeviceCapability::TabletTool),
        tablet_pad: device.has_capability(DeviceCapability::TabletPad),
        tap_fingers: device.config_tap_finger_count(),
        trackball,
        trackpoint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse_facts() -> DeviceFacts {
        DeviceFacts {
            pointer: true,
            touch: false,
            tablet_tool: false,
            tablet_pad: false,
            tap_fingers: 0,
            trackball: false,
            trackpoint: false,
        }
    }

    #[test]
    fn classifies_each_desktop_device_without_overlap() {
        assert_eq!(classify_facts(mouse_facts()), Some(DeviceKind::Mouse));
        assert_eq!(
            classify_facts(DeviceFacts {
                tap_fingers: 3,
                ..mouse_facts()
            }),
            Some(DeviceKind::Touchpad)
        );
        assert_eq!(
            classify_facts(DeviceFacts {
                trackpoint: true,
                ..mouse_facts()
            }),
            Some(DeviceKind::Trackpoint)
        );
        assert_eq!(
            classify_facts(DeviceFacts {
                trackball: true,
                ..mouse_facts()
            }),
            Some(DeviceKind::Trackball)
        );
        assert_eq!(
            classify_facts(DeviceFacts {
                pointer: false,
                touch: true,
                ..mouse_facts()
            }),
            Some(DeviceKind::Touchscreen)
        );
    }

    #[test]
    fn excludes_tablet_and_non_pointer_devices() {
        for facts in [
            DeviceFacts {
                pointer: false,
                ..mouse_facts()
            },
            DeviceFacts {
                tablet_tool: true,
                ..mouse_facts()
            },
            DeviceFacts {
                tablet_pad: true,
                ..mouse_facts()
            },
        ] {
            assert_eq!(classify_facts(facts), None);
        }
    }
}
