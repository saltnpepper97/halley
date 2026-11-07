//! Halley's IME transaction and lock policy, using only public upstream APIs.
//!
//! Smithay owns protocol resources and popup geometry. Halley holds uncommitted
//! client state, validates recovery epochs, and owns the effective keyboard grab.
pub(crate) use smithay::wayland::GlobalData as ManagerData;
use smithay::wayland::{Dispatch2, GlobalData};
pub(crate) use smithay::wayland::{
    input_method::InputMethodUserData as MethodData, text_input::TextInputUserData as TextData,
};
use smithay::{
    backend::input::{InputTime, KeyState, Keycode},
    input::{
        Seat, SeatHandler,
        keyboard::{GrabStartData, KeyboardGrab, KeyboardInnerHandle, KeymapFile, ModifiersState},
    },
    reexports::{
        wayland_protocols::wp::text_input::zv3::server::{
            zwp_text_input_manager_v3 as tm, zwp_text_input_v3 as ti,
        },
        wayland_protocols_misc::zwp_input_method_v2::server::{
            zwp_input_method_keyboard_grab_v2 as kg, zwp_input_method_manager_v2 as imm,
            zwp_input_method_v2 as im,
        },
        wayland_server::{
            self as server, Client, DataInit, DisplayHandle, Resource,
            backend::{ClientId, ObjectId},
        },
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        input_method::{
            InputMethodHandler, InputMethodKeyboardUserData, InputMethodPopupSurfaceUserData,
            InputMethodUserData,
        },
        seat::WaylandFocus,
        text_input::{TextInputSeat, TextInputUserData},
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub(crate) use {
    im::ZwpInputMethodV2 as Method, imm::ZwpInputMethodManagerV2 as MethodManager,
    ti::ZwpTextInputV3 as Text, tm::ZwpTextInputManagerV3 as TextManager,
};

pub(crate) trait ImeHost:
    SeatHandler
    + InputMethodHandler
    + smithay::wayland::text_input::TextInputActivation
    + Sized
    + 'static
{
    fn ime_seat(&self) -> Seat<Self>;
    fn ime_repeat(&self) -> (i32, i32);
    fn ime_keymap(&mut self) -> KeymapFile;
}
#[derive(Default)]
struct Policy {
    suspended: bool,
    registered: bool,
    serial: u32,
    recovery: Option<u32>,
    needs_recovery: bool,
    method: Option<Method>,
    pending: Vec<im::Request>,
    text_pending: HashMap<ObjectId, Vec<ti::Request>>,
    text_serials: HashMap<ObjectId, u32>,
    text_clients: HashMap<ClientId, usize>,
    active: Option<ObjectId>,
    focus: Option<ObjectId>,
    grab: Option<kg::ZwpInputMethodKeyboardGrabV2>,
    grab_serial: Option<Serial>,
}
fn policy<D: SeatHandler + 'static>(seat: &Seat<D>) -> Arc<Mutex<Policy>> {
    seat.user_data()
        .insert_if_missing(|| Arc::new(Mutex::new(Policy::default())));
    seat.user_data()
        .get::<Arc<Mutex<Policy>>>()
        .unwrap()
        .clone()
}
/// Call once for each native Wayland keyboard enter/leave, which upstream
/// accompanies with an IME deactivate/done, including associated X11 surfaces.
pub(crate) fn focus_transition<D: SeatHandler + 'static>(seat: &Seat<D>, focus: Option<ObjectId>) {
    let shared = policy(seat);
    let mut p = shared.lock().unwrap();
    if p.registered {
        p.serial = p.serial.wrapping_add(1);
    }
    p.pending.clear();
    p.text_pending.clear();
    p.active = None;
    p.focus = focus;
}
/// XWayland can defer its initial keyboard enter until surface association.
#[allow(dead_code)] // The protocol fixture has no XWayland surface association.
pub(crate) fn associated_focus<D: SeatHandler + 'static>(seat: &Seat<D>) {
    let focus = seat.text_input().focus().map(|surface| surface.id());
    if focus.is_some() && policy(seat).lock().unwrap().focus != focus {
        focus_transition(seat, focus);
    }
}
#[allow(dead_code)] // The socket fixture shares this module without popup dispatch.
pub(crate) fn keyboard_grabbed<D: SeatHandler + 'static>(seat: &Seat<D>) -> bool {
    let shared = policy(seat);
    let p = shared.lock().unwrap();
    !p.suspended && p.grab.as_ref().is_some_and(Resource::is_alive)
}
pub(crate) fn suspend<D: ImeHost>(state: &mut D, suspended: bool) {
    let seat = state.ime_seat();
    let shared = policy(&seat);
    let grab_serial = {
        let mut p = shared.lock().unwrap();
        p.suspended = suspended;
        p.pending.clear();
        p.text_pending.clear();
        p.active = None;
        if suspended {
            p.needs_recovery = true;
            if let Some(method) = p.method.clone() {
                method.deactivate();
                method.done();
                p.serial = p.serial.wrapping_add(1);
            }
        }
        p.grab_serial.take()
    };
    if let Some(keyboard) = seat.get_keyboard() {
        if suspended {
            if grab_serial.is_some_and(|serial| keyboard.has_grab(serial)) {
                keyboard.unset_grab(state);
            }
        } else {
            restore_grab(state);
        }
    }
}
fn restore_grab<D: ImeHost>(state: &mut D) {
    let seat = state.ime_seat();
    let Some(keyboard) = seat.get_keyboard() else {
        return;
    };
    let shared = policy(&seat);
    let mut p = shared.lock().unwrap();
    if p.suspended || !p.grab.as_ref().is_some_and(Resource::is_alive) || keyboard.is_grabbed() {
        return;
    }
    let serial = SERIAL_COUNTER.next_serial();
    p.grab_serial = Some(serial);
    drop(p);
    let mods = keyboard.modifier_state().serialized;
    if let Some(grab) = shared.lock().unwrap().grab.as_ref() {
        grab.modifiers(
            serial.into(),
            mods.depressed,
            mods.latched,
            mods.locked,
            mods.layout_effective,
        );
    }
    keyboard.set_grab(
        state,
        ImeGrab {
            seat,
            start: GrabStartData { focus: None },
        },
        serial,
    );
}
struct ImeGrab<D: SeatHandler + 'static> {
    seat: Seat<D>,
    start: GrabStartData<D>,
}
impl<D: SeatHandler + 'static> KeyboardGrab<D> for ImeGrab<D> {
    fn input(
        &mut self,
        _: &mut D,
        _: &mut KeyboardInnerHandle<'_, D>,
        key: Keycode,
        state: KeyState,
        mods: Option<ModifiersState>,
        serial: Serial,
        time: InputTime,
    ) {
        let shared = policy(&self.seat);
        let p = shared.lock().unwrap();
        if p.suspended {
            return;
        }
        if let Some(grab) = p.grab.as_ref().filter(|g| g.is_alive()) {
            let serial = p
                .active
                .as_ref()
                .and_then(|id| p.text_serials.get(id))
                .copied()
                .unwrap_or(serial.into());
            grab.key(serial, time.millis(), key.raw() - 8, state.into());
            if let Some(mods) = mods.map(|m| m.serialized) {
                grab.modifiers(
                    serial,
                    mods.depressed,
                    mods.latched,
                    mods.locked,
                    mods.layout_effective,
                );
            }
        }
    }
    fn set_focus(
        &mut self,
        state: &mut D,
        handle: &mut KeyboardInnerHandle<'_, D>,
        focus: Option<D::KeyboardFocus>,
        serial: Serial,
    ) {
        handle.set_focus(state, focus, serial);
    }
    fn start_data(&self) -> &GrabStartData<D> {
        &self.start
    }
    fn unset(&mut self, _: &mut D) {}
}
pub(crate) struct GrabData<D: SeatHandler + 'static>(Seat<D>);
impl<D: ImeHost> Dispatch2<kg::ZwpInputMethodKeyboardGrabV2, D> for GrabData<D> {
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        _: &kg::ZwpInputMethodKeyboardGrabV2,
        _: kg::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
    }
    fn destroyed(&self, state: &mut D, _: ClientId, resource: &kg::ZwpInputMethodKeyboardGrabV2) {
        let shared = policy(&self.0);
        let serial = {
            let mut p = shared.lock().unwrap();
            if p.grab.as_ref() != Some(resource) {
                return;
            }
            p.grab = None;
            p.grab_serial.take()
        };
        if let Some(keyboard) = self.0.get_keyboard()
            && serial.is_some_and(|s| keyboard.has_grab(s))
        {
            keyboard.unset_grab(state);
        }
    }
}
pub(crate) struct Inert;
#[allow(clippy::too_many_arguments)]
pub(crate) fn method_manager<D>(
    state: &mut D,
    client: &Client,
    resource: &MethodManager,
    request: imm::Request,
    data: &GlobalData,
    dh: &DisplayHandle,
    init: &mut DataInit<'_, D>,
) where
    D: ImeHost + server::Dispatch<Method, InputMethodUserData<D>> + server::Dispatch<Method, Inert>,
{
    let seat = state.ime_seat();
    if let imm::Request::GetInputMethod {
        input_method,
        seat: requested,
    } = request
    {
        if Seat::<D>::from_resource(&requested).as_ref() != Some(&seat)
            || policy(&seat).lock().unwrap().registered
        {
            init.init(input_method, Inert).unavailable();
            return;
        }
        let shared = policy(&seat);
        {
            let mut p = shared.lock().unwrap();
            p.serial = 0;
            p.recovery = None;
            p.method = None;
            p.registered = true;
        }
        data.request(
            state,
            client,
            resource,
            imm::Request::GetInputMethod {
                input_method,
                seat: requested,
            },
            dh,
            init,
        );
    } else {
        data.request(state, client, resource, request, dh, init);
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_manager<D>(
    state: &mut D,
    client: &Client,
    resource: &TextManager,
    request: tm::Request,
    data: &GlobalData,
    dh: &DisplayHandle,
    init: &mut DataInit<'_, D>,
) where
    D: ImeHost + server::Dispatch<Text, TextInputUserData>,
{
    if matches!(request, tm::Request::GetTextInput { .. }) {
        *policy(&state.ime_seat())
            .lock()
            .unwrap()
            .text_clients
            .entry(client.id())
            .or_default() += 1;
    }
    data.request(state, client, resource, request, dh, init);
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_request<D: ImeHost + server::Dispatch<Text, TextInputUserData>>(
    state: &mut D,
    client: &Client,
    resource: &Text,
    request: ti::Request,
    data: &TextInputUserData,
    dh: &DisplayHandle,
    init: &mut DataInit<'_, D>,
) {
    let seat = state.ime_seat();
    let shared = policy(&seat);
    let text = seat.text_input().clone();
    let mut p = shared.lock().unwrap();
    let focused = text
        .focus()
        .is_some_and(|f| f.id().same_client_as(&resource.id()));
    if matches!(request, ti::Request::Commit) {
        let serial = p.text_serials.entry(resource.id()).or_default();
        *serial = serial.wrapping_add(1);
        let pending = p.text_pending.remove(&resource.id()).unwrap_or_default();
        if p.suspended || !focused || !p.registered {
            // Upstream must still advance the text-input commit counter, but must
            // not see a focus or any staged requests from a protected field.
            drop(p);
            let focus = text.focus();
            text.set_focus(None);
            data.request(state, client, resource, ti::Request::Commit, dh, init);
            text.set_focus(focus);
            return;
        }
        let enable = pending.iter().rev().find_map(|r| match r {
            ti::Request::Enable => Some(true),
            ti::Request::Disable => Some(false),
            _ => None,
        });
        let eligible = p.active.as_ref().is_none_or(|id| *id == resource.id());
        let sends_done =
            eligible && (enable.is_some() || p.active.as_ref() == Some(&resource.id()));
        if sends_done {
            p.serial = p.serial.wrapping_add(1);
            if enable == Some(true) {
                p.pending.clear();
                p.active = Some(resource.id());
                if p.needs_recovery {
                    p.recovery = Some(p.serial);
                    p.needs_recovery = false;
                }
            } else if enable == Some(false) {
                p.pending.clear();
                p.active = None;
            }
        }
        drop(p);
        for request in pending {
            data.request(state, client, resource, request, dh, init);
        }
        data.request(state, client, resource, ti::Request::Commit, dh, init);
    } else if !p.suspended && focused {
        // Keep protocol double-buffered state in Halley so a focus/lock
        // transition can discard it without touching upstream internals.
        let pending = p.text_pending.entry(resource.id()).or_default();
        let discriminant = std::mem::discriminant(&request);
        pending.retain(|r| {
            std::mem::discriminant(r) != discriminant
                && !(matches!(request, ti::Request::Enable | ti::Request::Disable)
                    && matches!(r, ti::Request::Enable | ti::Request::Disable))
        });
        pending.push(request);
    }
}
pub(crate) fn text_destroyed<D: ImeHost + server::Dispatch<Text, TextInputUserData>>(
    state: &mut D,
    client: ClientId,
    resource: &Text,
    data: &TextInputUserData,
) {
    let seat = state.ime_seat();
    let shared = policy(&seat);
    let focus = seat.text_input().focus();
    let was_active = {
        let mut p = shared.lock().unwrap();
        p.text_pending.remove(&resource.id());
        p.text_serials.remove(&resource.id());
        let count = p.text_clients.entry(client.clone()).or_default();
        *count = count.saturating_sub(1);
        let last = *count == 0;
        if last
            && focus
                .as_ref()
                .is_none_or(|f| f.id().same_client_as(&resource.id()))
            && p.registered
        {
            p.serial = p.serial.wrapping_add(1);
        }
        let was_active = p.active.as_ref() == Some(&resource.id());
        if was_active {
            p.active = None;
            p.pending.clear();
        }
        was_active
    };
    data.destroyed(state, client, resource);
    if was_active {
        // Public focus events clear upstream's active ID even when another
        // text-input from the same client survives the destroyed field.
        seat.text_input().leave();
        seat.text_input().enter();
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn method_request<D>(state: &mut D, client: &Client, resource: &Method, request: im::Request, data: &InputMethodUserData<D>, dh: &DisplayHandle, init: &mut DataInit<'_, D>)
where D: ImeHost + server::Dispatch<Method, InputMethodUserData<D>> + server::Dispatch<smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData> + server::Dispatch<kg::ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>> + server::Dispatch<kg::ZwpInputMethodKeyboardGrabV2, GrabData<D>>, D::KeyboardFocus: WaylandFocus{
    let seat = state.ime_seat();
    let shared = policy(&seat);
    let mut p = shared.lock().unwrap();
    p.method = Some(resource.clone());
    match request {
        im::Request::CommitString { .. }
        | im::Request::SetPreeditString { .. }
        | im::Request::DeleteSurroundingText { .. } => {
            if !p.suspended && p.active.is_some() {
                let discriminant = std::mem::discriminant(&request);
                p.pending
                    .retain(|r| std::mem::discriminant(r) != discriminant);
                p.pending.push(request);
            }
        }
        im::Request::Commit { serial } => {
            let pending = std::mem::take(&mut p.pending);
            let valid = !p.suspended
                && p.active.is_some()
                && p.recovery
                    .is_none_or(|floor| serial.wrapping_sub(floor) < (1 << 31))
                && p.serial.wrapping_sub(serial) < (1 << 31);
            drop(p);
            if valid {
                for request in pending {
                    data.request(state, client, resource, request, dh, init);
                }
                seat.text_input().done(false);
            }
        }
        im::Request::GrabKeyboard { keyboard: new } => {
            let grab = init.init(new, GrabData(seat.clone()));
            let previous = p.grab_serial.take();
            p.grab = Some(grab.clone());
            drop(p);
            if let Some(keyboard) = seat.get_keyboard() {
                if previous.is_some_and(|s| keyboard.has_grab(s)) {
                    keyboard.unset_grab(state);
                }
                let (rate, delay) = state.ime_repeat();
                grab.repeat_info(rate, delay);
                let keymap = state.ime_keymap();
                if let Err(error) = keymap.with_fd(false, |fd, size| {
                    grab.keymap(
                        server::protocol::wl_keyboard::KeymapFormat::XkbV1,
                        fd,
                        size as u32,
                    )
                }) {
                    tracing::warn!(%error, "Failed to send IME keymap");
                }
                if !shared.lock().unwrap().suspended {
                    let mods = keyboard.modifier_state().serialized;
                    grab.modifiers(
                        SERIAL_COUNTER.next_serial().into(),
                        mods.depressed,
                        mods.latched,
                        mods.locked,
                        mods.layout_effective,
                    );
                }
                restore_grab(state);
            }
        }
        request => {
            let suspended = p.suspended;
            drop(p);
            // Upstream derives popup parents from text-input focus. Hide that
            // focus while locked, including when the IME creates its popup late.
            let text = seat.text_input().clone();
            let focus = text.focus();
            if suspended {
                text.set_focus(None);
            }
            data.request(state, client, resource, request, dh, init);
            if suspended {
                text.set_focus(focus);
            }
        }
    }
}
pub(crate) fn method_destroyed<D>(state: &mut D, client: ClientId, resource: &Method, data: &InputMethodUserData<D>)
where D: ImeHost + server::Dispatch<Method, InputMethodUserData<D>> + server::Dispatch<smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, InputMethodPopupSurfaceUserData> + server::Dispatch<kg::ZwpInputMethodKeyboardGrabV2, InputMethodKeyboardUserData<D>>, D::KeyboardFocus: WaylandFocus{
    let seat = state.ime_seat();
    let shared = policy(&seat);
    let serial = {
        let mut p = shared.lock().unwrap();
        p.method = None;
        p.registered = false;
        p.pending.clear();
        p.active = None;
        p.grab = None;
        p.grab_serial.take()
    };
    if let Some(keyboard) = seat.get_keyboard()
        && serial.is_some_and(|s| keyboard.has_grab(s))
    {
        keyboard.unset_grab(state);
    }
    data.destroyed(state, client, resource);
}

macro_rules! delegate_ime_policy {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $state:ty) => {
        crate::ime_policy::ime_dispatch!($(@<$( $lt $( : $clt $(+ $dlt )* )? ),+>)? $state, MethodManager, crate::ime_policy::ManagerData, method_manager, manager_destroyed);
        crate::ime_policy::ime_dispatch!($(@<$( $lt $( : $clt $(+ $dlt )* )? ),+>)? $state, TextManager, crate::ime_policy::ManagerData, text_manager, manager_destroyed);
        crate::ime_policy::ime_dispatch!($(@<$( $lt $( : $clt $(+ $dlt )* )? ),+>)? $state, Text, crate::ime_policy::TextData, text_request, text_destroyed);
        crate::ime_policy::ime_dispatch!($(@<$( $lt $( : $clt $(+ $dlt )* )? ),+>)? $state, Method, crate::ime_policy::MethodData<Self>, method_request, method_destroyed);
        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )?> smithay::reexports::wayland_server::Dispatch<crate::ime_policy::Method, crate::ime_policy::Inert> for $state {
            fn request(_: &mut Self, _: &smithay::reexports::wayland_server::Client, _: &crate::ime_policy::Method, request: <crate::ime_policy::Method as smithay::reexports::wayland_server::Resource>::Request, _: &crate::ime_policy::Inert, _: &smithay::reexports::wayland_server::DisplayHandle, init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>) {
                match request {
                    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_v2::Request::GrabKeyboard { keyboard } => { init.init(keyboard, crate::ime_policy::Inert); }
                    smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_method_v2::Request::GetInputPopupSurface { id, .. } => { init.init(id, crate::ime_policy::Inert); }
                    _ => {}
                }
            }
        }
    };
}
macro_rules! ime_dispatch {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $state:ty, $resource:ident, $data:ty, $request:ident, $destroyed:ident) => {
        impl<$( $( $lt $( : $clt $(+ $dlt )* )? ),+, )?> smithay::reexports::wayland_server::Dispatch<crate::ime_policy::$resource, $data> for $state {
            fn request(state: &mut Self, client: &smithay::reexports::wayland_server::Client, resource: &crate::ime_policy::$resource, request: <crate::ime_policy::$resource as smithay::reexports::wayland_server::Resource>::Request, data: &$data, dh: &smithay::reexports::wayland_server::DisplayHandle, init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>) { crate::ime_policy::$request(state, client, resource, request, data, dh, init); }
            fn destroyed(state: &mut Self, client: smithay::reexports::wayland_server::backend::ClientId, resource: &crate::ime_policy::$resource, data: &$data) { crate::ime_policy::$destroyed(state, client, resource, data); }
        }
    };
}
pub(crate) use {delegate_ime_policy, ime_dispatch};

pub(crate) fn manager_destroyed<D, I>(_: &mut D, _: ClientId, _: &I, _: &GlobalData) {}

impl<D: 'static> Dispatch2<kg::ZwpInputMethodKeyboardGrabV2, D> for Inert {
    fn request(
        &self,
        _: &mut D,
        _: &Client,
        _: &kg::ZwpInputMethodKeyboardGrabV2,
        _: kg::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
    }
}
impl<D: 'static> Dispatch2<smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, D> for Inert { fn request(&self, _: &mut D, _: &Client, _: &smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2, _: <smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::server::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2 as Resource>::Request, _: &DisplayHandle, _: &mut DataInit<'_, D>) {} }
