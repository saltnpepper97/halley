//! Exercise rebased motion through Smithay's real implicit-grab state machine.
use super::click_grab_location;
use smithay::{
    backend::input::ButtonState,
    input::{Seat, SeatHandler, SeatState, pointer::*},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, SERIAL_COUNTER, Serial},
};

#[derive(Clone, Debug, PartialEq)]
struct Target(u8);
impl IsAlive for Target {
    fn alive(&self) -> bool {
        true
    }
}
#[derive(Default)]
struct State {
    seats: SeatState<Self>,
    motions: Vec<(u8, Point<f64, Logical>)>,
    buttons: Vec<(u8, ButtonState)>,
    cursor_resets: usize,
}
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = Target;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seats
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {
        self.cursor_resets += 1;
    }
}
impl smithay::wayland::compositor::CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut smithay::wayland::compositor::CompositorState {
        unreachable!("pointer-only fixture has no compositor")
    }
    fn client_compositor_state<'a>(
        &self,
        _: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        unreachable!("pointer-only fixture has no clients")
    }
    fn commit(&mut self, _: &WlSurface) {}
}
macro_rules! ignore_events {
    ($($name:ident: $event:ty),* $(,)?) => { $(
        fn $name(&self, _: &Seat<State>, _: &mut State, _: &$event) {}
    )* };
}
impl PointerTarget<State> for Target {
    fn enter(&self, _: &Seat<State>, data: &mut State, event: &MotionEvent) {
        data.motions.push((self.0, event.location));
    }
    fn motion(&self, _: &Seat<State>, data: &mut State, event: &MotionEvent) {
        data.motions.push((self.0, event.location));
    }
    fn button(&self, _: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        data.buttons.push((self.0, event.state));
    }
    fn axis(&self, _: &Seat<State>, _: &mut State, _: AxisFrame) {}
    fn frame(&self, _: &Seat<State>, _: &mut State) {}
    fn leave(
        &self,
        _: &Seat<State>,
        _: &mut State,
        _: Serial,
        _: smithay::backend::input::InputTime,
    ) {
    }
    ignore_events! {
        relative_motion: RelativeMotionEvent,
        gesture_swipe_begin: GestureSwipeBeginEvent,
        gesture_swipe_update: GestureSwipeUpdateEvent,
        gesture_swipe_end: GestureSwipeEndEvent,
        gesture_pinch_begin: GesturePinchBeginEvent,
        gesture_pinch_update: GesturePinchUpdateEvent,
        gesture_pinch_end: GesturePinchEndEvent,
        gesture_hold_begin: GestureHoldBeginEvent,
        gesture_hold_end: GestureHoldEndEvent,
    }
}
fn motion(
    pointer: &PointerHandle<State>,
    state: &mut State,
    target: Target,
    origin: Point<f64, Logical>,
    location: Point<f64, Logical>,
) {
    pointer.motion(
        state,
        Some((target, origin)),
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(1),
        },
    );
}
fn button(pointer: &PointerHandle<State>, state: &mut State, code: u32, value: ButtonState) {
    pointer.button(
        state,
        &ButtonEvent {
            button: code,
            state: value,
            serial: SERIAL_COUNTER.next_serial(),
            time: smithay::backend::input::InputTime::from_millis(2),
        },
    );
}

#[test]
fn transformed_drag_keeps_owner_coordinates_and_cursor_until_last_release() {
    let mut state = State::default();
    let pointer = state.seats.new_seat("test").add_pointer();
    let owner = Target(1);
    let panel = Target(2);
    let grab_origin = Point::from((400.0, -300.0));
    motion(
        &pointer,
        &mut state,
        owner.clone(),
        grab_origin,
        grab_origin + Point::from((10.0, 20.0)),
    );
    button(&pointer, &mut state, 0x110, ButtonState::Pressed);
    button(&pointer, &mut state, 0x111, ButtonState::Pressed);
    assert!(pointer.is_grabbed());

    // Owner moved and is displayed at half scale; physical pointer now lies
    // over a panel on another output. Its location must still reach the owner
    // in unscaled client coordinates, even outside the owner's bounds.
    let current_origin = Point::from((700.0, -200.0));
    let source = current_origin + Point::from((1200.0, -40.0));
    let rebased = click_grab_location(grab_origin, source, current_origin);
    motion(&pointer, &mut state, owner.clone(), grab_origin, rebased);
    assert_eq!(
        state.motions.last(),
        Some(&(1, Point::from((1200.0, -40.0))))
    );
    assert_eq!(state.cursor_resets, 0);
    button(&pointer, &mut state, 0x110, ButtonState::Released);
    assert!(
        pointer.is_grabbed(),
        "another held button still owns the grab"
    );
    button(&pointer, &mut state, 0x111, ButtonState::Released);
    assert!(!pointer.is_grabbed());
    assert!(state.buttons.iter().all(|(target, _)| *target == 1));
    // Same post-release refresh used by the session: no new physical motion.
    motion(
        &pointer,
        &mut state,
        panel.clone(),
        (2560.0, 0.0).into(),
        (2600.0, 12.0).into(),
    );
    assert_eq!(pointer.current_focus(), Some(panel));
    assert_eq!(state.motions.last(), Some(&(2, Point::from((40.0, 12.0)))));
    assert_eq!(state.cursor_resets, 1);
}

#[test]
fn layer_and_subsurface_grabs_keep_their_own_origin_after_layout_moves() {
    let grab_origin = Point::from((2560.0, 40.0));
    // Layer moved by 20 px and its child moved by 5 px. Screen coordinates
    // remain screen coordinates regardless of the window now underneath.
    let current_origin = Point::from((2585.0, 40.0));
    let event = click_grab_location(grab_origin, (2700.0, 25.0).into(), current_origin);
    assert_eq!(event - grab_origin, Point::from((115.0, -15.0)));
}
