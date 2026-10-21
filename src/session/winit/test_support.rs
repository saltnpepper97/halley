//! Two-output nested server compiled only into test executables.
//! Real protocol clients and the production input/session/rendering paths run
//! unchanged; the extra output is offscreen and input is isolated from the host.
use super::*;
use calloop::{Interest, Mode, PostAction, generic::Generic};
use smithay::backend::input::{
    Event, InputBackend, InputEvent, InputTime, KeyState, KeyboardKeyEvent, Keycode, UnusedEvent,
};
use smithay::backend::winit::WinitVirtualDevice;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use std::os::unix::net::UnixDatagram;

pub(super) fn output_state(output: &Output) -> super::super::output::OutputState {
    super::super::output::OutputState {
        output: output.clone(),
        enabled: true,
        mode: output.current_mode().unwrap(),
        location: output.current_location(),
        transform: output.current_transform(),
        scale: output.current_scale().fractional_scale(),
        adaptive_sync: false,
        adaptive_sync_supported: false,
    }
}

pub(super) fn output_info(output: &Output) -> halley_ipc::OutputInfo {
    halley_ipc::OutputInfo {
        name: output.name(),
        modes: vec![crate::ipc::mode_info(output.current_mode().unwrap(), true)],
        current_mode: Some(0),
        offset_x: output.current_location().x,
        offset_y: output.current_location().y,
        scale: output.current_scale().fractional_scale(),
        vrr: "off".into(),
        vrr_supported: false,
        vrr_active: false,
    }
}

pub(super) fn install(app: &mut App, handle: &LoopHandle<'static, App>) {
    let Some(path) = std::env::var_os("HALLEY_TEST_WINIT_CONTROL") else {
        return;
    };
    let output = Output::new(
        "winit-2".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Halley test".into(),
            model: "offscreen".into(),
            serial_number: "2".into(),
        },
    );
    let mode = OutputMode {
        size: (1024, 768).into(),
        refresh: 60_000,
    };
    let x = app.driver.backend.logical_size().w;
    output.change_current_state(Some(mode), None, None, Some((x, 0).into()));
    output.set_preferred(mode);
    app.wayland.ensure_output_global::<App>(&output);
    app.wayland.space.map_output(&output, (x, 0));
    app.cameras.insert(output.name(), mode.size);
    app.driver.extra_outputs.push(output);

    let socket = UnixDatagram::bind(path).unwrap();
    socket.set_nonblocking(true).unwrap();
    handle.insert_source(Generic::new(socket, Interest::READ, Mode::Level), move |_, socket, app| {
        let mut bytes = [0; 256];
        while let Ok((size, peer)) = socket.recv_from(&mut bytes) {
            let command = std::str::from_utf8(&bytes[..size]).unwrap();
            let parts = command.split_whitespace().collect::<Vec<_>>();
            if matches!(parts.first(), Some(&"key") | Some(&"seat-key")) {
                let seat_key = parts[0] == "seat-key";
                assert_eq!(parts.len(), if seat_key { 4 } else { 3 });
                let code = parts[1].parse().unwrap();
                let pressed = parts[2] == "down";
                let count = if seat_key { parts[3].parse().unwrap() } else { u32::from(pressed) };
                let event = TestKey { code, pressed, count, time: app.start_time.elapsed().as_millis() as u32 };
                let socket_name = app.wayland_display.clone().unwrap();
                super::super::input::handle::<_, TestInput>(app, &InputEvent::Keyboard { event }, &socket_name);
            } else { assert_eq!(command, "state"); }
            let now = crate::frame_clock::monotonic_now();
            let windows = app.nodes.records().map(|record| {
                let output = app.wayland.space.outputs().find(|o| o.name() == record.output).unwrap();
                let geometry = app.wayland.space.output_geometry(output).unwrap();
                serde_json::json!({"id": record.id.as_u64(), "fullscreen": app.fullscreen.is_fullscreen_or_pending(&record.surface),
                    "paused": app.fullscreen.is_presentation_paused(&record.surface),
                    "camera": app.fullscreen.camera_frame(output, geometry, now).is_some(),
                    "rect": super::super::presented_window_rect(app, &record.window, output, now).map(|r| [r.loc.x,r.loc.y,r.size.w,r.size.h])})
            }).collect::<Vec<_>>();
            let leds = app.seat.get_keyboard().unwrap().led_state();
            let keyboard = app.seat.get_keyboard().unwrap();
            let modifiers = keyboard.modifier_state();
            let sides = app.keyboard.side_modifiers;
            let last_led_update = app.driver.keyboard_led_update.map(|leds| serde_json::json!({"caps": leds.caps, "num": leds.num, "scroll": leds.scroll}));
            let reply = serde_json::json!({"open": app.shell.focus_cycle.is_open(), "active": app.shell.focus_cycle.is_active(),
                "leds": {"caps": leds.caps, "num": leds.num, "scroll": leds.scroll},
                "led_update_count": app.driver.keyboard_led_update_count, "last_led_update": last_led_update,
                "keyboard": {"pressed": keyboard.pressed_keys().into_iter().map(|key| key.raw() - 8).collect::<Vec<_>>(),
                    "modifiers": [modifiers.shift, modifiers.ctrl, modifiers.alt, modifiers.logo],
                    "sides": [sides.left_shift, sides.right_shift, sides.left_ctrl, sides.right_ctrl,
                        sides.left_alt, sides.right_alt, sides.left_super, sides.right_super]},
                "preview": app.shell.focus_cycle.session().and_then(|s| s.preview()).map(|id| id.as_u64()),
                "focused": app.nodes.focused().map(|id| id.as_u64()), "windows": windows});
            socket.send_to(reply.to_string().as_bytes(), peer.as_pathname().unwrap()).unwrap();
        }
        Ok(PostAction::Continue)
    }).unwrap();
}

#[derive(Debug)]
struct TestInput;
#[derive(Debug)]
struct TestKey {
    code: u32,
    pressed: bool,
    time: u32,
    count: u32,
}
impl Event<TestInput> for TestKey {
    fn time(&self) -> InputTime {
        InputTime::from_millis(self.time)
    }
    fn device(&self) -> WinitVirtualDevice {
        WinitVirtualDevice
    }
}
impl KeyboardKeyEvent<TestInput> for TestKey {
    fn key_code(&self) -> Keycode {
        Keycode::new(self.code + 8)
    }
    fn state(&self) -> KeyState {
        if self.pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        }
    }
    fn count(&self) -> u32 {
        self.count
    }
}
impl InputBackend for TestInput {
    type Device = WinitVirtualDevice;
    type KeyboardKeyEvent = TestKey;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = UnusedEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}
