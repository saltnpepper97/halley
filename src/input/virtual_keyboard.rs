//! Preserve direct virtual-keyboard delivery across upstream's input-event API.
//! Client keymaps and modifier masks remain independent of physical input,
//! compositor bindings and IME grabs. Only public upstream APIs are used.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::backend::input::{Event, InputEvent, KeyboardKeyEvent};
use smithay::input::keyboard::{KeymapFile, ModifiersState};
use smithay::input::{Seat, SeatHandler};
use smithay::reexports::wayland_protocols_misc::zwp_virtual_keyboard_v1::server::zwp_virtual_keyboard_v1::{
    Request, ZwpVirtualKeyboardV1,
};
use smithay::reexports::wayland_server::{Client, DataInit, DisplayHandle};
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::Dispatch2;
use smithay::wayland::virtual_keyboard::{
    VirtualKeyboardBackend, VirtualKeyboardDevice, VirtualKeyboardHandler,
    VirtualKeyboardSpecialEvent, VirtualKeyboardUserData,
};

pub(crate) trait Host: SeatHandler + Sized + 'static {
    fn virtual_keyboard_seat(&self) -> Seat<Self>;
    fn virtual_keyboard_locked(&self) -> bool;
}

#[derive(Clone, Copy, Default)]
struct Mask {
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
}

struct Saved {
    keymap: KeymapFile,
    modifiers: ModifiersState,
}

#[derive(Default)]
struct State {
    native: Option<Saved>,
    active: Option<VirtualKeyboardDevice>,
    modifiers: HashMap<VirtualKeyboardDevice, Mask>,
}

fn state<D: Host>(seat: &Seat<D>) -> Arc<Mutex<State>> {
    seat.user_data()
        .get_or_insert_threadsafe(|| Arc::new(Mutex::new(State::default())))
        .clone()
}

/// Keep requests made during lock inert, including keymap and modifier changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn request<D: Host + VirtualKeyboardHandler>(
    data: &VirtualKeyboardUserData,
    host: &mut D,
    client: &Client,
    keyboard: &ZwpVirtualKeyboardV1,
    request: Request,
    display: &DisplayHandle,
    init: &mut DataInit<'_, D>,
) {
    if !host.virtual_keyboard_locked() {
        data.request(host, client, keyboard, request, display, init);
    }
}

fn activate<D: Host>(host: &mut D, device: &VirtualKeyboardDevice) -> bool {
    let seat = host.virtual_keyboard_seat();
    if Seat::<D>::from_resource(device.wl_seat()).as_ref() != Some(&seat) {
        return false;
    }
    let Some(keyboard) = seat.get_keyboard() else {
        return false;
    };
    let shared = state(&seat);
    let (keymap, mask) = {
        let mut state = shared.lock().unwrap();
        let native = state.native.get_or_insert_with(|| Saved {
            keymap: keyboard.keymap_file(),
            modifiers: keyboard.modifier_state(),
        });
        let keymap = device.keymap().unwrap_or_else(|| native.keymap.clone());
        let mask = state.modifiers.get(device).copied().unwrap_or_default();
        (keymap, mask)
    };
    if keyboard.keymap_file().id() != keymap.id()
        && let Err(err) = keyboard.set_keymap_from_keymap_file(host, keymap)
    {
        tracing::warn!(%err, "Failed to activate virtual keyboard keymap");
        return false;
    }
    shared.lock().unwrap().active = Some(device.clone());
    keyboard.with_xkb_state(host, |mut context| {
        context.set_modifier_mask(mask.depressed, mask.latched, mask.locked, mask.group);
    });
    true
}

/// Restore hardware state before ordinary input, configuration changes or lock.
pub(crate) fn restore<D: Host>(host: &mut D) -> bool {
    let seat = host.virtual_keyboard_seat();
    let shared = state(&seat);
    let Some(saved) = shared.lock().unwrap().native.take() else {
        return true;
    };
    let Some(keyboard) = seat.get_keyboard() else {
        shared.lock().unwrap().active = None;
        return true;
    };
    if keyboard.keymap_file().id() != saved.keymap.id()
        && let Err(err) = keyboard.set_keymap_from_keymap_file(host, saved.keymap.clone())
    {
        tracing::warn!(%err, "Failed to restore hardware keyboard keymap");
        shared.lock().unwrap().native = Some(saved);
        return false;
    }
    shared.lock().unwrap().active = None;
    if keyboard.set_modifier_state(saved.modifiers) != 0 {
        keyboard.advertise_modifier_state(host);
    }
    host.led_state_changed(&seat, keyboard.led_state());
    true
}

pub(crate) fn handle_event<D: Host>(host: &mut D, event: InputEvent<VirtualKeyboardBackend>) {
    // Destruction must always retire bookkeeping. Upstream releases held keys
    // before DeviceRemoved; the lock check below suppresses those releases too.
    if let InputEvent::DeviceRemoved { device } = event {
        let shared = state(&host.virtual_keyboard_seat());
        let active = {
            let mut state = shared.lock().unwrap();
            state.modifiers.remove(&device);
            state.active.as_ref() == Some(&device)
        };
        if active {
            restore(host);
        }
        return;
    }
    if host.virtual_keyboard_locked() {
        return;
    }
    match event {
        InputEvent::Keyboard { event } => {
            if activate(host, &event.device()) {
                let keyboard = host.virtual_keyboard_seat().get_keyboard().unwrap();
                // Forward directly as before: virtual keys do not trigger
                // compositor bindings or loop back into an IME keyboard grab.
                keyboard.input_forward_bypassing_grab(
                    host,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time(),
                    false,
                );
            }
        }
        InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
            device,
            mods_depressed,
            mods_latched,
            mods_locked,
            group,
        }) => {
            state(&host.virtual_keyboard_seat())
                .lock()
                .unwrap()
                .modifiers
                .insert(
                    device.clone(),
                    Mask {
                        depressed: mods_depressed,
                        latched: mods_latched,
                        locked: mods_locked,
                        group,
                    },
                );
            activate(host, &device);
        }
        // Keymaps activate lazily with the first key or modifier request.
        InputEvent::DeviceAdded { .. }
        | InputEvent::Special(VirtualKeyboardSpecialEvent::KeymapChanged { .. }) => {}
        _ => {}
    }
}
