use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::warn;
use wayland_protocols_misc::zwp_input_method_v2::server::{
    zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
    zwp_input_method_v2::{self, ZwpInputMethodV2},
    zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, Resource, protocol::wl_keyboard::KeymapFormat,
};
use wayland_server::{backend::ClientId, protocol::wl_surface::WlSurface};

use crate::{
    input::{SeatHandler, keyboard::KeyboardHandle},
    utils::{Logical, Rectangle, SERIAL_COUNTER, alive_tracker::AliveTracker},
    wayland::{Dispatch2, compositor, seat::WaylandFocus, text_input::TextInputHandle},
};

use super::{
    INPUT_POPUP_SURFACE_ROLE, InputMethodHandler, InputMethodKeyboardUserData,
    InputMethodPopupSurfaceUserData,
    input_method_keyboard_grab::InputMethodKeyboardGrab,
    input_method_popup_surface::{PopupHandle, PopupParent, PopupSurface},
};

#[derive(Default, Debug)]
pub(crate) struct InputMethod {
    pub instance: Option<Instance>,
    pub popup_handle: PopupHandle,
    pub keyboard_grab: InputMethodKeyboardGrab,
    suspended: bool,
    pending: PendingText,
    // A delayed reply from before lock must not reach a newly focused field.
    // Unlike ordinary stale serials, those replies are deliberately discarded.
    recovery_serial: Option<u32>,
    needs_recovery_serial: bool,
}

#[derive(Default, Debug)]
struct PendingText {
    commit: Option<String>,
    preedit: Option<(String, i32, i32)>,
    delete: Option<(u32, u32)>,
}

#[derive(Debug)]
pub(crate) struct Instance {
    pub object: ZwpInputMethodV2,
    pub serial: u32,
}

impl Instance {
    /// Send the done incrementing the serial.
    pub(crate) fn done(&mut self) {
        self.object.done();
        self.serial = self.serial.wrapping_add(1);
    }
}

/// Handle to an input method instance
#[derive(Default, Debug, Clone)]
pub struct InputMethodHandle {
    pub(crate) inner: Arc<Mutex<InputMethod>>,
}

impl InputMethodHandle {
    pub(super) fn add_instance(&self, instance: &ZwpInputMethodV2) {
        let mut inner = self.inner.lock().unwrap();
        if inner.instance.is_some() {
            instance.unavailable();
        } else {
            inner.instance = Some(Instance {
                object: instance.clone(),
                serial: 0,
            });
        }
    }

    /// Whether there's an active instance of input-method.
    pub fn has_instance(&self) -> bool {
        self.inner.lock().unwrap().instance.is_some()
    }

    /// Suspend input-method activity without disconnecting its client.
    pub fn set_suspended<D: SeatHandler + 'static>(
        &self,
        state: &mut D,
        keyboard: &KeyboardHandle<D>,
        suspended: bool,
    ) {
        if suspended {
            self.deactivate_input_method(state);
        }
        let grab = {
            let mut inner = self.inner.lock().unwrap();
            inner.suspended = suspended;
            inner.pending = PendingText::default();
            if suspended {
                inner.needs_recovery_serial = true;
            }
            inner.keyboard_grab.clone()
        };
        if suspended {
            grab.unset_if_owned(state, keyboard);
        } else if grab.inner.lock().unwrap().grab.is_some() && !keyboard.is_grabbed() {
            keyboard.set_grab(state, grab.clone(), SERIAL_COUNTER.next_serial());
        }
    }

    pub(crate) fn suspended(&self) -> bool {
        self.inner.lock().unwrap().suspended
    }

    /// Callback function to access the input method object
    pub(crate) fn with_instance<F>(&self, f: F)
    where
        F: FnOnce(&mut Instance),
    {
        let mut inner = self.inner.lock().unwrap();
        if let Some(instance) = inner.instance.as_mut() {
            f(instance);
        }
    }

    /// Callback function to access the input method.
    pub(crate) fn with_input_method<F>(&self, mut f: F)
    where
        F: FnMut(&mut InputMethod),
    {
        let mut inner = self.inner.lock().unwrap();
        f(&mut inner);
    }

    /// Indicates that an input method has grabbed a keyboard
    pub fn keyboard_grabbed(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        let keyboard = inner.keyboard_grab.inner.lock().unwrap();
        keyboard.grab.is_some()
    }

    pub(crate) fn set_text_input_rectangle<D: SeatHandler + 'static>(
        &self,
        state: &mut D,
        rect: Rectangle<i32, Logical>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner.popup_handle.rectangle = rect;

        let mut popup_surface = match inner.popup_handle.surface.clone() {
            Some(popup_surface) => popup_surface,
            None => return,
        };

        popup_surface.set_text_input_rectangle(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h);

        if let Some(instance) = &inner.instance {
            let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
            (data.popup_repositioned)(state, popup_surface);
        };
    }

    /// Activate input method on the given surface.
    pub(crate) fn activate_input_method<D: SeatHandler + 'static>(&self, state: &mut D, surface: &WlSurface) {
        self.with_input_method(|im| {
            im.pending = PendingText::default();
            if im.suspended {
                return;
            }
            if im.needs_recovery_serial {
                // The next done completes this activation. Only responses that
                // observed it can belong to the post-unlock text input.
                im.recovery_serial = im
                    .instance
                    .as_ref()
                    .map(|instance| instance.serial.wrapping_add(1));
                im.needs_recovery_serial = false;
            }
            if let Some(instance) = im.instance.as_ref() {
                instance.object.activate();
                if let Some(popup) = im.popup_handle.surface.as_mut() {
                    let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
                    let location = (data.popup_geometry_callback)(state, surface);
                    // Remove old popup.
                    (data.dismiss_popup)(state, popup.clone());

                    // Add a new one with updated parent.
                    let parent = PopupParent {
                        surface: surface.clone(),
                        location,
                    };
                    popup.set_parent(Some(parent));
                    (data.new_popup)(state, popup.clone());
                }
            }
        });
    }

    /// Deactivate the active input method.
    ///
    /// The `done` is always send when deactivating IME.
    pub(crate) fn deactivate_input_method<D: SeatHandler + 'static>(&self, state: &mut D) {
        self.with_input_method(|im| {
            im.pending = PendingText::default();
            if let Some(instance) = im.instance.as_mut() {
                instance.object.deactivate();
                instance.done();
                if let Some(popup) = im.popup_handle.surface.as_mut() {
                    let data = instance.object.data::<InputMethodUserData<D>>().unwrap();
                    if popup.get_parent().is_some() {
                        (data.dismiss_popup)(state, popup.clone());
                    }
                    popup.set_parent(None);
                }
            }
        });
    }
}

/// User data of ZwpInputMethodV2 object
pub struct InputMethodUserData<D: SeatHandler> {
    pub(super) handle: InputMethodHandle,
    pub(crate) text_input_handle: TextInputHandle,
    pub(crate) keyboard_handle: KeyboardHandle<D>,
    pub(crate) popup_geometry_callback: fn(&D, &WlSurface) -> Rectangle<i32, Logical>,
    pub(crate) new_popup: fn(&mut D, PopupSurface),
    pub(crate) popup_repositioned: fn(&mut D, PopupSurface),
    pub(crate) dismiss_popup: fn(&mut D, PopupSurface),
}

impl<D: SeatHandler> fmt::Debug for InputMethodUserData<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputMethodUserData")
            .field("handle", &self.handle)
            .field("text_input_handle", &self.text_input_handle)
            .field("keyboard_handle", &self.keyboard_handle)
            .finish()
    }
}

impl<D> Dispatch2<ZwpInputMethodV2, D> for InputMethodUserData<D>
where
    D: Dispatch<ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData>,
    D: Dispatch<ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>>,
    D: SeatHandler,
    D: InputMethodHandler,
    <D as SeatHandler>::KeyboardFocus: WaylandFocus,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &Client,
        seat: &ZwpInputMethodV2,
        request: zwp_input_method_v2::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        let current = self
            .handle
            .inner
            .lock()
            .unwrap()
            .instance
            .as_ref()
            .is_some_and(|instance| instance.object == *seat);
        if !current {
            match request {
                zwp_input_method_v2::Request::GetInputPopupSurface { id, .. } => {
                    data_init.init(
                        id,
                        InputMethodPopupSurfaceUserData {
                            alive_tracker: AliveTracker::default(),
                        },
                    );
                }
                zwp_input_method_v2::Request::GrabKeyboard { keyboard } => {
                    data_init.init(
                        keyboard,
                        InputMethodKeyboardUserData {
                            handle: InputMethodKeyboardGrab::default(),
                            keyboard_handle: self.keyboard_handle.clone(),
                        },
                    );
                }
                _ => {}
            }
            return;
        }
        match request {
            zwp_input_method_v2::Request::CommitString { text } => {
                let mut inner = self.handle.inner.lock().unwrap();
                if !inner.suspended {
                    inner.pending.commit = Some(text);
                }
            }
            zwp_input_method_v2::Request::SetPreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                let mut inner = self.handle.inner.lock().unwrap();
                if !inner.suspended {
                    inner.pending.preedit = Some((text, cursor_begin, cursor_end));
                }
            }
            zwp_input_method_v2::Request::DeleteSurroundingText {
                before_length,
                after_length,
            } => {
                let mut inner = self.handle.inner.lock().unwrap();
                if !inner.suspended {
                    inner.pending.delete = Some((before_length, after_length));
                }
            }
            zwp_input_method_v2::Request::Commit { serial } => {
                let (pending, valid) = {
                    let mut inner = self.handle.inner.lock().unwrap();
                    let valid = !inner.suspended
                        && inner.instance.as_ref().is_some_and(|i| i.object == *seat)
                        && inner
                            .recovery_serial
                            // Serial numbers wrap. Treat values within the next
                            // half of the u32 range as at or after the floor.
                            .is_none_or(|floor| serial.wrapping_sub(floor) < (1 << 31));
                    (std::mem::take(&mut inner.pending), valid)
                };
                if valid {
                    self.text_input_handle.with_active_text_input(|ti, _| {
                        if let Some((before, after)) = pending.delete {
                            ti.delete_surrounding_text(before, after);
                        }
                        if let Some(ref text) = pending.commit {
                            ti.commit_string(Some(text.clone()));
                        }
                        if let Some((ref text, begin, end)) = pending.preedit {
                            ti.preedit_string(Some(text.clone()), begin, end);
                        }
                    });
                    self.text_input_handle.done(false);
                }
            }
            zwp_input_method_v2::Request::GetInputPopupSurface { id, surface } => {
                if compositor::give_role(&surface, INPUT_POPUP_SURFACE_ROLE).is_err()
                    && compositor::get_role(&surface) != Some(INPUT_POPUP_SURFACE_ROLE)
                {
                    // Protocol requires this raise an error, but doesn't define an error enum
                    seat.post_error(0u32, "Surface already has a role.");
                    return;
                }

                let parent = match self
                    .text_input_handle
                    .focus()
                    .filter(|_| !self.handle.suspended())
                {
                    Some(parent) => {
                        let location = state.parent_geometry(&parent);
                        Some(PopupParent {
                            surface: parent,
                            location,
                        })
                    }
                    None => None,
                };
                let mut input_method = self.handle.inner.lock().unwrap();

                let instance = data_init.init(
                    id,
                    InputMethodPopupSurfaceUserData {
                        alive_tracker: AliveTracker::default(),
                    },
                );
                let popup_rect = Arc::new(Mutex::new(input_method.popup_handle.rectangle));
                let popup = PopupSurface::new(instance, surface, popup_rect, parent);
                input_method.popup_handle.surface = Some(popup.clone());
                if popup.get_parent().is_some() {
                    state.new_popup(popup);
                }
            }
            zwp_input_method_v2::Request::GrabKeyboard { keyboard } => {
                let input_method = self.handle.inner.lock().unwrap();
                if !input_method.suspended {
                    self.keyboard_handle.set_grab(
                        state,
                        input_method.keyboard_grab.clone(),
                        SERIAL_COUNTER.next_serial(),
                    );
                }
                let instance = data_init.init(
                    keyboard,
                    InputMethodKeyboardUserData {
                        handle: input_method.keyboard_grab.clone(),
                        keyboard_handle: self.keyboard_handle.clone(),
                    },
                );
                let mut keyboard = input_method.keyboard_grab.inner.lock().unwrap();
                keyboard.grab = Some(instance.clone());
                keyboard.text_input_handle = self.text_input_handle.clone();
                let guard = self.keyboard_handle.arc.internal.lock().unwrap();
                instance.repeat_info(guard.repeat_rate, guard.repeat_delay);
                let keymap_file = self.keyboard_handle.arc.keymap.lock().unwrap();
                let res = keymap_file.with_fd(false, |fd, size| {
                    instance.keymap(KeymapFormat::XkbV1, fd, size as u32);
                });

                if let Err(err) = res {
                    warn!(err = ?err, "Failed to send keymap to client");
                } else if !input_method.suspended {
                    // Modifiers can be latched when taking the grab, thus we must send them to keep
                    // them in sync.
                    let mods = guard.mods_state.serialized;
                    instance.modifiers(
                        SERIAL_COUNTER.next_serial().into(),
                        mods.depressed,
                        mods.latched,
                        mods.locked,
                        mods.layout_effective,
                    );
                }
            }
            zwp_input_method_v2::Request::Destroy => {
                // Nothing to do
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(&self, state: &mut D, _client: ClientId, input_method: &ZwpInputMethodV2) {
        let grab = {
            let mut inner = self.handle.inner.lock().unwrap();
            if !inner.instance.as_ref().is_some_and(|i| i.object == *input_method) {
                return;
            }
            inner.instance = None;
            inner.pending = PendingText::default();
            inner.recovery_serial = None;
            std::mem::take(&mut inner.keyboard_grab)
        };
        grab.release(state, &self.keyboard_handle);
        self.text_input_handle.leave();
    }
}
