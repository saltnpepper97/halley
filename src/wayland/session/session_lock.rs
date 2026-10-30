use std::collections::{HashMap, HashSet};

use smithay::backend::input::{
    ButtonState, Event, InputBackend, InputEvent, KeyboardKeyEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::desktop::{WindowSurfaceType, utils::under_from_surface_tree};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::{
    ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    ext_session_lock_v1::{
        Error as SessionLockError, ExtSessionLockV1, Request as SessionLockRequest,
    },
};
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use smithay::wayland::compositor::{send_surface_state, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLockState, SessionLocker,
};

use crate::session::{Session, SessionDriver};

#[derive(Clone, Debug)]
pub struct SurfaceEntry {
    pub surface: LockSurface,
    pub output: Output,
}

struct PendingConfirmation {
    locker: SessionLocker,
    generation: u64,
    awaiting_outputs: HashSet<String>,
}

fn acknowledge_presented_output(
    awaiting: &mut HashSet<String>,
    pending_generation: u64,
    presented_generation: u64,
    output_name: &str,
) -> bool {
    if pending_generation != presented_generation {
        return false;
    }
    awaiting.remove(output_name);
    awaiting.is_empty()
}

fn unlock_authorized(active: bool, confirmed: bool, owner_matches: bool) -> bool {
    active && confirmed && owner_matches
}

/// Security-sensitive state for ext-session-lock-v1.
///
/// `active` deliberately outlives the lock client and all of its surfaces:
/// a crashed locker leaves every output black and input isolated until the
/// compositor itself is restarted.
pub struct State {
    manager: SessionLockManagerState,
    active: bool,
    generation: u64,
    surfaces: HashMap<ObjectId, SurfaceEntry>,
    configured_sizes: HashMap<ObjectId, Size<u32, Logical>>,
    keyboard_focus: Option<ObjectId>,
    pending_confirmation: Option<PendingConfirmation>,
    owner: Option<ObjectId>,
    confirmed: bool,
    rejected_locks: HashSet<ObjectId>,
}

impl State {
    pub fn new<D: SessionDriver>(dh: &DisplayHandle) -> Self {
        Self {
            manager: SessionLockManagerState::new::<Session<D>, _>(dh, |_| true),
            active: false,
            generation: 0,
            surfaces: HashMap::new(),
            configured_sizes: HashMap::new(),
            keyboard_focus: None,
            pending_confirmation: None,
            owner: None,
            confirmed: false,
            rejected_locks: HashSet::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn frame_generation(&self) -> Option<u64> {
        self.active.then_some(self.generation)
    }

    pub fn surface_for_output(&self, output: &Output) -> Option<&LockSurface> {
        self.surfaces
            .values()
            .find(|entry| entry.output == *output && entry.surface.alive())
            .map(|entry| &entry.surface)
    }

    pub fn surfaces_for_output(&self, output: &Output) -> impl Iterator<Item = &LockSurface> {
        self.surfaces
            .values()
            .filter(move |entry| entry.output == *output && entry.surface.alive())
            .map(|entry| &entry.surface)
    }

    pub fn focused_surface(&self) -> Option<WlSurface> {
        self.keyboard_focus
            .as_ref()
            .and_then(|id| self.surfaces.get(id))
            .filter(|entry| entry.surface.alive())
            .map(|entry| entry.surface.wl_surface().clone())
            .or_else(|| {
                self.surfaces
                    .values()
                    .find(|entry| entry.surface.alive())
                    .map(|entry| entry.surface.wl_surface().clone())
            })
    }

    pub fn output_for_surface(&self, surface: &WlSurface) -> Option<&Output> {
        let root = crate::wayland::compositor::root_surface(surface);
        self.surfaces
            .get(&root.id())
            .filter(|entry| entry.surface.alive())
            .map(|entry| &entry.output)
    }

    pub fn geometry_for_surface(&self, surface: &WlSurface) -> Option<Rectangle<i32, Logical>> {
        let root = crate::wayland::compositor::root_surface(surface);
        let size = self.configured_sizes.get(&root.id())?;
        Some(Rectangle::from_size(Size::from((
            i32::try_from(size.w).unwrap_or(i32::MAX),
            i32::try_from(size.h).unwrap_or(i32::MAX),
        ))))
    }

    pub(crate) fn set_focus(&mut self, surface: &WlSurface) {
        let root = crate::wayland::compositor::root_surface(surface);
        if self.surfaces.contains_key(&root.id()) {
            self.keyboard_focus = Some(root.id());
        }
    }

    pub fn focus_at(
        &self,
        space: &smithay::desktop::Space<smithay::desktop::Window>,
        position: (f64, f64),
    ) -> Option<(WlSurface, Point<f64, Logical>, Output)> {
        let output = space.output_under(position).next()?.clone();
        let geometry = space.output_geometry(&output)?;
        let local = Point::<f64, Logical>::from(position) - geometry.loc.to_f64();
        let root = self.surface_for_output(&output)?.wl_surface();
        let (surface, origin) =
            under_from_surface_tree(root, local, (0, 0), WindowSurfaceType::ALL)?;
        Some((surface, origin.to_f64() + geometry.loc.to_f64(), output))
    }

    pub fn presented(&mut self, output: &Output, generation: u64) {
        let Some(pending) = self.pending_confirmation.as_mut() else {
            return;
        };
        if acknowledge_presented_output(
            &mut pending.awaiting_outputs,
            pending.generation,
            generation,
            &output.name(),
        ) {
            let pending = self
                .pending_confirmation
                .take()
                .expect("pending lock confirmation disappeared");
            self.confirmed = true;
            pending.locker.lock();
        }
    }

    fn remove_surface(&mut self, surface: &WlSurface) {
        let root = crate::wayland::compositor::root_surface(surface);
        self.surfaces.remove(&root.id());
        self.configured_sizes.remove(&root.id());
        if self.keyboard_focus.as_ref() == Some(&root.id()) {
            self.keyboard_focus = None;
        }
    }
}

fn configure_surface<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &LockSurface,
    output: &Output,
) {
    let Some(geometry) = session.wayland.space.output_geometry(output) else {
        return;
    };
    let size =
        Size::<u32, Logical>::from((geometry.size.w.max(1) as u32, geometry.size.h.max(1) as u32));
    if session
        .session_lock
        .configured_sizes
        .get(&surface.wl_surface().id())
        == Some(&size)
    {
        return;
    }
    surface.with_pending_state(|state| state.size = Some(size));
    surface.send_configure();
    session
        .session_lock
        .configured_sizes
        .insert(surface.wl_surface().id(), size);
}

pub fn configure_surfaces<D: SessionDriver>(session: &mut Session<D>) {
    let entries = session
        .session_lock
        .surfaces
        .values()
        .filter(|entry| entry.surface.alive())
        .cloned()
        .collect::<Vec<_>>();
    for entry in entries {
        configure_surface(session, &entry.surface, &entry.output);
    }
}

/// A powered-off output already reveals no client content, so it does not
/// need to hold up lock confirmation if DPMS changes during the transition.
pub fn confirm_unlit_outputs<D: SessionDriver>(session: &mut Session<D>) {
    let Some(generation) = session.session_lock.frame_generation() else {
        return;
    };
    let secure = session
        .wayland
        .space
        .outputs()
        .filter(|output| !session.driver.output_requires_lock_frame(output))
        .cloned()
        .collect::<Vec<_>>();
    for output in secure {
        session.session_lock.presented(&output, generation);
    }
}

pub fn surface_committed<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    if !session.session_lock.active {
        return;
    }
    let root = crate::wayland::compositor::root_surface(surface);
    let Some(entry) = session.session_lock.surfaces.get(&root.id()).cloned() else {
        return;
    };
    configure_surface(session, &entry.surface, &entry.output);
    if session.session_lock.keyboard_focus.is_none() {
        session.session_lock.set_focus(&root);
        crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
    }
    session.request_output_redraw(&entry.output);
}

pub fn surface_destroyed<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    if !session.session_lock.active {
        return;
    }
    session.session_lock.remove_surface(surface);
    crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
    session.request_redraw();
}

pub fn send_frames(state: &State, output: &Output, elapsed: std::time::Duration, sequence: u32) {
    for surface in state.surfaces_for_output(output) {
        smithay::desktop::utils::send_frames_surface_tree(
            surface.wl_surface(),
            output,
            elapsed,
            super::super::frame_callbacks::FALLBACK_THROTTLE,
            |surface, states| {
                super::super::frame_callbacks::callback_output(
                    surface, states, output, sequence, true,
                )
            },
        );
    }
}

impl<D: SessionDriver> SessionLockHandler for Session<D> {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock.manager
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if self.session_lock.active {
            self.session_lock
                .rejected_locks
                .insert(confirmation.ext_session_lock().id());
            return;
        }

        self.session_lock.active = true;
        self.wayland.ext_image_copy_capture_state.stop_all();
        self.session_lock.owner = Some(confirmation.ext_session_lock().id());
        self.session_lock.confirmed = false;
        self.session_lock.generation = self.session_lock.generation.wrapping_add(1).max(1);
        self.session_lock.surfaces.clear();
        self.session_lock.configured_sizes.clear();
        self.session_lock.keyboard_focus = None;

        let awaiting_outputs = self
            .wayland
            .space
            .outputs()
            .filter(|output| self.driver.output_requires_lock_frame(output))
            .map(Output::name)
            .collect::<HashSet<_>>();
        self.session_lock.pending_confirmation = Some(PendingConfirmation {
            locker: confirmation,
            generation: self.session_lock.generation,
            awaiting_outputs,
        });
        crate::wayland::dnd::clear(self);
        super::super::session_lock::enter_secure_mode(self);
        if self
            .session_lock
            .pending_confirmation
            .as_ref()
            .is_some_and(|pending| pending.awaiting_outputs.is_empty())
        {
            let pending = self
                .session_lock
                .pending_confirmation
                .take()
                .expect("pending confirmation disappeared");
            self.session_lock.confirmed = true;
            pending.locker.lock();
        }

        self.request_redraw();
    }

    fn unlock(&mut self) {
        self.session_lock.active = false;
        self.session_lock.surfaces.clear();
        self.session_lock.configured_sizes.clear();
        self.session_lock.keyboard_focus = None;
        self.session_lock.pending_confirmation = None;
        self.session_lock.owner = None;
        self.session_lock.confirmed = false;
        super::super::session_lock::leave_secure_mode(self);
        self.request_redraw();
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            return;
        };
        for candidate in self.wayland.space.outputs() {
            if candidate == &output {
                candidate.enter(surface.wl_surface());
            } else {
                candidate.leave(surface.wl_surface());
            }
        }
        with_states(surface.wl_surface(), |states| {
            send_surface_state(
                surface.wl_surface(),
                states,
                output.current_scale().integer_scale(),
                output.current_transform(),
            );
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(output.current_scale().fractional_scale());
            });
        });
        self.session_lock.surfaces.insert(
            surface.wl_surface().id(),
            SurfaceEntry {
                surface: surface.clone(),
                output: output.clone(),
            },
        );
        configure_surface(self, &surface, &output);
        self.session_lock.set_focus(surface.wl_surface());
        crate::session::sync_keyboard_focus(self, SERIAL_COUNTER.next_serial());
        self.request_output_redraw(&output);
    }
}

struct RejectedLockSurface;

impl<D: SessionDriver> Dispatch<ExtSessionLockSurfaceV1, RejectedLockSurface> for Session<D> {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ExtSessionLockSurfaceV1,
        _request: <ExtSessionLockSurfaceV1 as Resource>::Request,
        _data: &RejectedLockSurface,
        _display: &DisplayHandle,
        _init: &mut DataInit<'_, Self>,
    ) {
    }
}

/// Only the confirmed owner may unlock; rejected locks create inert surfaces
/// so pipelined requests cannot reserve outputs or assign roles.
impl<D: SessionDriver> Dispatch<ExtSessionLockV1, SessionLockState> for Session<D> {
    fn request(
        state: &mut Self,
        client: &Client,
        lock: &ExtSessionLockV1,
        request: SessionLockRequest,
        data: &SessionLockState,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if state.session_lock.rejected_locks.contains(&lock.id()) {
            if let SessionLockRequest::GetLockSurface { id, .. } = request {
                // This rejected object must stay inert even for requests already queued.
                data_init.init(id, RejectedLockSurface);
            }
            return;
        }
        if matches!(request, SessionLockRequest::UnlockAndDestroy)
            && !unlock_authorized(
                state.session_lock.active,
                state.session_lock.confirmed,
                state.session_lock.owner.as_ref() == Some(&lock.id()),
            )
        {
            lock.post_error(
                SessionLockError::InvalidUnlock,
                "Only the confirmed active lock may unlock the session.",
            );
            return;
        }
        smithay::wayland::Dispatch2::request(
            data, state, client, lock, request, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client_id: smithay::reexports::wayland_server::backend::ClientId,
        resource: &ExtSessionLockV1,
        data: &SessionLockState,
    ) {
        state.session_lock.rejected_locks.remove(&resource.id());
        smithay::wayland::Dispatch2::destroyed(data, state, client_id, resource);
    }
}

pub fn enter_secure_mode<D: SessionDriver>(session: &mut Session<D>) {
    session
        .wayland
        .ime_clients
        .disconnect_all(&session.wayland.display_handle);
    if let Some(mut grab) = session.popup_grab.take() {
        grab.ungrab(smithay::desktop::PopupUngrabStrategy::All);
    }
    if let Some(pointer) = session.seat.get_pointer() {
        pointer.unset_grab(
            session,
            SERIAL_COUNTER.next_serial(),
            smithay::backend::input::InputTime::now(),
        );
    }
    if let Some(keyboard) = session.seat.get_keyboard() {
        keyboard.unset_grab(session);
    }
    session.cancel_exit_confirmation();
    crate::capture::cancel_selected(session);
    crate::shell::apogee::cancel(session);
    crate::shell::focus_cycle::cancel(session);
    crate::session::pointer::release_for_compositor_warp(session);
    let old_cursor = session
        .cursor
        .set_image(smithay::input::pointer::CursorImageStatus::default_named());
    if let Some(old_cursor) = old_cursor {
        crate::cursor::surface::clear_outputs(&old_cursor, &session.wayland.space);
    }
    crate::session::cancel_compositor_grab(session);
    session.interactions.resize_anchor = None;
    session.interactions.suppressed_buttons.clear();
    session.interactions.suppressed_keys.clear();
    session.interactions.wheel_accumulator.reset_all();
    super::super::session_lock::cancel_client_input(session);
    crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
}

pub fn leave_secure_mode<D: SessionDriver>(session: &mut Session<D>) {
    super::super::session_lock::cancel_client_input(session);
    let old_cursor = session
        .cursor
        .set_image(smithay::input::pointer::CursorImageStatus::default_named());
    if let Some(old_cursor) = old_cursor {
        crate::cursor::surface::clear_outputs(&old_cursor, &session.wayland.space);
    }
    session.cursor.clear_overrides();
    crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
}

fn cancel_client_input<D: SessionDriver>(session: &mut Session<D>) {
    crate::session::touch::cancel_all(session);
    crate::session::gesture::cancel_all(session);
    if let Some(pointer) = session.seat.get_pointer() {
        pointer.motion(
            session,
            None,
            &smithay::input::pointer::MotionEvent {
                location: Point::<f64, Logical>::from(session.pointer.position()),
                serial: SERIAL_COUNTER.next_serial(),
                time: smithay::backend::input::InputTime::from_millis(
                    session.start_time.elapsed().as_millis() as u32,
                ),
            },
        );
        pointer.frame(session);
    }
}

fn pointer_focus<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    session
        .session_lock
        .focus_at(&session.wayland.space, session.pointer.position())
        .map(|(surface, origin, _)| (surface, origin))
}

fn update_pointer_focus<D: SessionDriver>(
    session: &mut Session<D>,
    pointer: &smithay::input::pointer::PointerHandle<Session<D>>,
    time: u32,
) {
    pointer.motion(
        session,
        pointer_focus(session),
        &MotionEvent {
            location: Point::<f64, Logical>::from(session.pointer.position()),
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(time),
        },
    );
}

/// Routes physical input exclusively to the active lock client. No compositor
/// bindings, accessibility hooks, desktop grabs, or ordinary client targets
/// are evaluated on this path.
pub fn handle_input<D, B>(session: &mut Session<D>, event: &InputEvent<B>)
where
    D: SessionDriver,
    B: InputBackend,
{
    if crate::session::touch::handle_session_lock(session, event) {
        return;
    }

    if matches!(
        event,
        InputEvent::PointerMotion { .. }
            | InputEvent::PointerMotionAbsolute { .. }
            | InputEvent::PointerButton { .. }
            | InputEvent::PointerAxis { .. }
    ) {
        session.cursor_policy.pointer_activity();
    }

    if let InputEvent::Keyboard { event } = event {
        if event.state() == smithay::backend::input::KeyState::Pressed {
            session.cursor_policy.keyboard_press();
        }
        let keyboard = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup");
        keyboard.input::<(), _>(
            session,
            event.key_code(),
            event.state(),
            SERIAL_COUNTER.next_serial(),
            event.time(),
            |_, _, _| FilterResult::Forward,
        );
        session.request_redraw();
        return;
    }

    let Some(pointer) = session.seat.get_pointer() else {
        return;
    };
    let before = session.pointer.position();
    session
        .pointer
        .process_input_event(event, &session.wayland.space);
    let after = session.pointer.position();

    match event {
        InputEvent::PointerMotion { event } => {
            update_pointer_focus(session, &pointer, event.time().millis());
            pointer.relative_motion(
                session,
                pointer_focus(session),
                &RelativeMotionEvent {
                    delta: event.delta(),
                    delta_unaccel: event.delta_unaccel(),
                    time: event.time(),
                },
            );
            pointer.frame(session);
            session.request_redraw();
        }
        InputEvent::PointerMotionAbsolute { event } => {
            update_pointer_focus(session, &pointer, event.time().millis());
            let delta = Point::<f64, Logical>::from((after.0 - before.0, after.1 - before.1));
            pointer.relative_motion(
                session,
                pointer_focus(session),
                &RelativeMotionEvent {
                    delta,
                    delta_unaccel: delta,
                    time: event.time(),
                },
            );
            pointer.frame(session);
            session.request_redraw();
        }
        InputEvent::PointerButton { event } => {
            update_pointer_focus(session, &pointer, event.time().millis());
            if event.button_code() == 0x110
                && event.state() == ButtonState::Pressed
                && let Some((surface, _, _)) = session
                    .session_lock
                    .focus_at(&session.wayland.space, session.pointer.position())
            {
                session.session_lock.set_focus(&surface);
                crate::session::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
            }
            pointer.button(
                session,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time: event.time(),
                    button: event.button_code(),
                    state: event.state(),
                },
            );
            pointer.frame(session);
        }
        InputEvent::PointerAxis { event } => {
            update_pointer_focus(session, &pointer, event.time().millis());
            pointer.axis(
                session,
                crate::input::pointer::axis_frame_filtered(event, true, true),
            );
            pointer.frame(session);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{acknowledge_presented_output, unlock_authorized};
    use std::collections::HashSet;

    #[test]
    fn lock_confirmation_requires_every_output_from_the_same_generation() {
        let mut awaiting = ["DP-1".to_string(), "DP-2".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();

        assert!(!acknowledge_presented_output(&mut awaiting, 7, 6, "DP-1"));
        assert_eq!(awaiting.len(), 2);
        assert!(!acknowledge_presented_output(&mut awaiting, 7, 7, "DP-1"));
        assert!(acknowledge_presented_output(&mut awaiting, 7, 7, "DP-2"));
    }

    #[test]
    fn only_the_confirmed_owner_can_unlock() {
        assert!(unlock_authorized(true, true, true));
        assert!(!unlock_authorized(false, true, true));
        assert!(!unlock_authorized(true, false, true));
        assert!(!unlock_authorized(true, true, false));
    }
}
