#[cfg(feature = "dbus")]
mod dbus;
#[cfg(feature = "dbus")]
mod keyboard_monitor;
#[cfg(feature = "dbus")]
mod tree;

#[cfg(feature = "dbus")]
pub use tree::AccessibilityTree;

#[cfg(feature = "dbus")]
pub fn start_configured_orca<D: SessionDriver>(
    session: &Session<D>,
    wayland_display: &std::ffi::OsStr,
) -> Option<OrcaSupervisor> {
    let path = session
        .launch_environment()
        .find_map(|(key, value)| {
            (key == std::ffi::OsStr::new("HALLEY_ORCA_PATH")).then(|| value.to_os_string())
        })
        .or_else(|| std::env::var_os("HALLEY_ORCA_PATH"))?;
    if !std::path::Path::new(&path).is_absolute() {
        eventline::warn!("accessibility: HALLEY_ORCA_PATH must be an absolute executable path");
        return None;
    }
    let service = session.keyboard_monitor.as_ref()?;
    let mut command = std::process::Command::new(path);
    command.envs(session.launch_environment());
    // Orca is a separate child, not the compositor's systemd service.
    for key in [
        "NOTIFY_SOCKET",
        "WATCHDOG_USEC",
        "WATCHDOG_PID",
        "LISTEN_FDS",
        "LISTEN_PID",
    ] {
        command.env_remove(key);
    }
    command.env("WAYLAND_DISPLAY", wayland_display);
    if let Some(display) = session.xwayland.display_name() {
        command.env("DISPLAY", display);
    } else {
        command.env_remove("DISPLAY");
    }
    match service.supervise_orca(command) {
        Ok(supervisor) => Some(supervisor),
        Err(error) => {
            eventline::warn!("accessibility: could not supervise Orca: {error}");
            None
        }
    }
}

#[cfg(feature = "dbus")]
use std::time::Duration;

#[cfg(feature = "dbus")]
use smithay::backend::input::{KeyState, Keycode};
#[cfg(feature = "dbus")]
use smithay::input::keyboard::xkb;

#[cfg(feature = "dbus")]
pub use keyboard_monitor::{KeyboardDisposition, KeyboardMonitorService, OrcaSupervisor};

use crate::session::{Session, SessionDriver};

#[cfg(not(feature = "dbus"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyboardDisposition {
    Pass,
    Intercept,
}

#[cfg(not(feature = "dbus"))]
pub struct KeyboardMonitorService;

#[cfg(not(feature = "dbus"))]
impl KeyboardMonitorService {
    pub fn start() -> Result<Self, std::convert::Infallible> {
        Ok(Self)
    }
}

#[cfg(feature = "dbus")]
fn translate_xkb_state(state: &xkb::State, keycode: Keycode) -> (u32, xkb::Keysym, u32) {
    (
        state.serialize_mods(xkb::STATE_MODS_EFFECTIVE),
        state.key_get_one_sym(keycode),
        state.key_get_utf32(keycode),
    )
}

#[cfg(feature = "dbus")]
pub fn process_key<D: SessionDriver>(
    session: &mut Session<D>,
    time: Duration,
    keycode: Keycode,
    state: KeyState,
) -> KeyboardDisposition {
    if session.keyboard_monitor.is_none() {
        return KeyboardDisposition::Pass;
    }
    let keyboard = session
        .seat
        .get_keyboard()
        .expect("keyboard capability added at seat setup");
    let (modifiers, keysym, unicode) = keyboard.with_xkb_state(session, |context| {
        let xkb = context.xkb().lock().expect("XKB state lock poisoned");
        // SAFETY: the borrowed XKB state does not outlive Smithay's locked
        // keyboard context.
        let state = unsafe { xkb.state() };
        translate_xkb_state(state, keycode)
    });
    let event = keyboard_monitor::KeyboardEvent {
        time,
        keycode,
        released: state == KeyState::Released,
        modifiers,
        keysym,
        unicode,
    };
    session
        .keyboard_monitor
        .as_ref()
        .expect("checked above")
        .process_key(event)
}

#[cfg(not(feature = "dbus"))]
pub fn process_key<D: SessionDriver>(
    _session: &mut Session<D>,
    _time: std::time::Duration,
    _keycode: smithay::backend::input::Keycode,
    _state: smithay::backend::input::KeyState,
) -> KeyboardDisposition {
    KeyboardDisposition::Pass
}

#[cfg(all(test, feature = "dbus"))]
mod tests {
    use super::*;

    #[test]
    fn xkb_translation_reports_keysym_unicode_and_xkb_keycode_space() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("US keymap must compile");
        let state = xkb::State::new(&keymap);
        let keycode = Keycode::new(57 + 8);

        let (modifiers, keysym, unicode) = translate_xkb_state(&state, keycode);
        assert_eq!(modifiers, 0);
        assert_eq!(keysym, xkb::Keysym::space);
        assert_eq!(unicode, ' ' as u32);
        assert_eq!(u16::try_from(keycode.raw()), Ok(65));
    }
}
