use smithay::input::keyboard::LedState;
use smithay::reexports::input::Device;

pub(super) trait KeyboardLedDevice: PartialEq {
    fn update_leds(&mut self, state: LedState);
}

impl KeyboardLedDevice for Device {
    fn update_leds(&mut self, state: LedState) {
        self.led_update(state.into());
    }
}

/// Keyboard membership is independent of pointer/touch configuration. A seat's
/// lock indicators belong on every attached keyboard, including a new device.
pub(super) struct KeyboardLeds<D = Device> {
    devices: Vec<D>,
}

impl<D> Default for KeyboardLeds<D> {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
        }
    }
}

impl<D: KeyboardLedDevice> KeyboardLeds<D> {
    pub(super) fn added(&mut self, mut device: D, state: LedState) {
        if let Some(existing) = self
            .devices
            .iter_mut()
            .find(|existing| **existing == device)
        {
            existing.update_leds(state);
        } else {
            device.update_leds(state);
            self.devices.push(device);
        }
    }

    pub(super) fn removed(&mut self, device: &D) {
        self.devices.retain(|existing| existing != device);
    }

    pub(super) fn update(&mut self, state: LedState) {
        for device in &mut self.devices {
            device.update_leds(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone)]
    struct Keyboard {
        id: u8,
        writes: Rc<RefCell<Vec<(u8, LedState)>>>,
    }
    impl PartialEq for Keyboard {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl KeyboardLedDevice for Keyboard {
        fn update_leds(&mut self, state: LedState) {
            self.writes.borrow_mut().push((self.id, state));
        }
    }

    fn state(enabled: bool) -> LedState {
        LedState {
            caps: Some(enabled),
            num: Some(enabled),
            scroll: Some(enabled),
        }
    }

    #[test]
    fn all_keyboards_follow_lock_changes_and_new_keyboards_get_current_indicators() {
        let writes = Rc::new(RefCell::new(Vec::new()));
        let laptop = Keyboard {
            id: 1,
            writes: writes.clone(),
        };
        let external = Keyboard {
            id: 2,
            writes: writes.clone(),
        };
        let mut leds = KeyboardLeds::default();
        leds.added(laptop, state(false));
        leds.update(state(true));
        leds.added(external, state(true));
        leds.update(state(false));
        assert_eq!(
            *writes.borrow(),
            vec![
                (1, state(false)),
                (1, state(true)),
                (2, state(true)),
                (1, state(false)),
                (2, state(false))
            ]
        );
    }

    #[test]
    fn removed_keyboards_stop_receiving_updates_and_reconnect_with_current_state() {
        let writes = Rc::new(RefCell::new(Vec::new()));
        let keyboard = Keyboard {
            id: 1,
            writes: writes.clone(),
        };
        let mut leds = KeyboardLeds::default();
        leds.added(keyboard.clone(), state(false));
        leds.added(keyboard.clone(), state(true));
        writes.borrow_mut().clear();
        leds.update(state(false));
        assert_eq!(*writes.borrow(), vec![(1, state(false))]);
        leds.removed(&keyboard);
        writes.borrow_mut().clear();
        leds.update(state(true));
        assert!(writes.borrow().is_empty());
        leds.added(keyboard, state(true));
        assert_eq!(*writes.borrow(), vec![(1, state(true))]);
    }
}
