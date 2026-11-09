use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::backend::input::Keycode;
use smithay::input::keyboard::ModifiersState;
use smithay::output::Output;

use super::{Session, SessionDriver};
use crate::input::keybinds::ResolvedBind;

#[derive(Clone)]
struct ActiveRepeat {
    generation: u64,
    keycode: Keycode,
    bind: ResolvedBind,
    interval: Duration,
}

#[derive(Default)]
struct Tracker {
    generation: u64,
    active: Option<ActiveRepeat>,
}

impl Tracker {
    fn start(
        &mut self,
        keycode: Keycode,
        bind: ResolvedBind,
        delay_ms: i32,
        rate: i32,
    ) -> Option<(u64, Duration)> {
        self.cancel();
        if !bind.repeat || rate <= 0 {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.active = Some(ActiveRepeat {
            generation,
            keycode,
            bind,
            interval: repeat_interval(rate),
        });
        Some((generation, Duration::from_millis(delay_ms.max(0) as u64)))
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.active = None;
    }

    fn release(&mut self, keycode: Keycode) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.keycode == keycode)
        {
            self.cancel();
            true
        } else {
            false
        }
    }

    fn tick(
        &mut self,
        generation: u64,
        modifiers: &ModifiersState,
        sides: crate::input::SideModifiers,
        context: crate::input::BindingContext,
    ) -> Option<(halley_config::Action, Duration)> {
        let active = self
            .active
            .as_ref()
            .filter(|active| active.generation == generation)?;
        if !context.allows(active.bind.scope)
            || !crate::input::keyboard_modifiers_match(
                modifiers,
                sides,
                active.bind.modifiers,
                active.bind.trigger,
                active.keycode,
            )
        {
            self.cancel();
            return None;
        }
        Some((active.bind.action.clone(), active.interval))
    }
}

fn repeat_interval(rate: i32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(rate.max(1)))
}

/// Owns the single compositor key-repeat timer for a seat.
pub(crate) struct Policy<D: SessionDriver> {
    tracker: Tracker,
    loop_handle: LoopHandle<'static, Session<D>>,
    timer: Option<RegistrationToken>,
}

impl<D: SessionDriver> Policy<D> {
    pub fn new(loop_handle: LoopHandle<'static, Session<D>>) -> Self {
        Self {
            tracker: Tracker::default(),
            loop_handle,
            timer: None,
        }
    }

    pub fn start(&mut self, keycode: Keycode, bind: ResolvedBind, delay_ms: i32, rate: i32) {
        self.cancel_timer();
        let Some((generation, delay)) = self.tracker.start(keycode, bind, delay_ms, rate) else {
            return;
        };
        match self.loop_handle.insert_source(
            Timer::from_duration(delay.max(Duration::from_millis(1))),
            move |_, _, session| repeat_tick(session, generation),
        ) {
            Ok(token) => self.timer = Some(token),
            Err(error) => {
                self.tracker.cancel();
                eventline::warn!("keybinds: failed to arm repeat timer: {error}");
            }
        }
    }

    pub fn release(&mut self, keycode: Keycode) {
        if self.tracker.release(keycode) {
            self.cancel_timer();
        }
    }

    pub fn cancel(&mut self) {
        self.tracker.cancel();
        self.cancel_timer();
    }

    fn cancel_timer(&mut self) {
        if let Some(token) = self.timer.take() {
            self.loop_handle.remove(token);
        }
    }
}

fn repeat_tick<D: SessionDriver>(session: &mut Session<D>, generation: u64) -> TimeoutAction {
    let modifiers = session.keyboard_handle().modifier_state();
    let sides = session.keyboard.side_modifiers;
    let context = super::keyboard_binding_context(session);
    let Some((action, interval)) = session
        .key_repeat
        .tracker
        .tick(generation, &modifiers, sides, context)
    else {
        session.key_repeat.timer = None;
        return TimeoutAction::Drop;
    };
    if !repeat_allowed(session, &action) {
        session.key_repeat.tracker.cancel();
        session.key_repeat.timer = None;
        return TimeoutAction::Drop;
    }
    let Some(socket_name) = session.wayland_display.clone() else {
        session.key_repeat.tracker.cancel();
        session.key_repeat.timer = None;
        return TimeoutAction::Drop;
    };
    let pointer_output = session
        .wayland
        .space
        .output_under(session.pointer.position())
        .next()
        .map(Output::name);
    super::actions::dispatch(
        session,
        action,
        &socket_name,
        pointer_output.as_deref(),
        None,
        super::actions::DispatchOrigin::Keyboard,
    );
    TimeoutAction::ToDuration(interval)
}

fn repeat_allowed<D: SessionDriver>(session: &Session<D>, action: &halley_config::Action) -> bool {
    // An immediate visual handoff must not let the original held shortcut
    // repeat onto the successor while the first client is still closing.
    if *action == halley_config::Action::CloseFocusedWindow
        && session
            .nodes
            .records()
            .any(|record| crate::window::closing::pending(&record.window))
    {
        return false;
    }
    // Once the final window is gone, holding its close shortcut must never
    // turn the repeat timer into a cluster deletion confirmation.
    if *action == halley_config::Action::CloseFocusedWindow
        && crate::wayland::focus::selected_output(&session.wayland)
            .and_then(|output| session.clusters.active_on(&output.name()))
            .is_some_and(|id| session.clusters.member_ids(id).is_empty())
    {
        return false;
    }
    if session.session_lock.active() || !super::bindings_enabled(session) {
        return false;
    }
    if session.shell.focus_cycle.is_open() {
        return matches!(action, halley_config::Action::FocusCycle(_));
    }
    !session.shell.overlays.confirmation_modal_active()
        && !session.capture.is_active()
        && !session.clusters.accepts_modal_input()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybinds::ResolvedTrigger;
    use halley_config::{Action, Modifiers};
    use smithay::input::keyboard::Keysym;

    fn bind(repeat: bool) -> ResolvedBind {
        ResolvedBind {
            scope: halley_config::BindingScope::Field,
            modifiers: Modifiers {
                super_key: true,
                ..Modifiers::default()
            },
            trigger: ResolvedTrigger::Keysym(Keysym::Left),
            action: Action::MoveNode(halley_config::Direction::Left),
            repeat,
        }
    }

    #[test]
    fn repeat_waits_for_delay_then_uses_configured_rate() {
        let mut tracker = Tracker::default();
        let (generation, delay) = tracker
            .start(Keycode::new(113), bind(true), 500, 20)
            .unwrap();
        assert_eq!(delay, Duration::from_millis(500));
        let modifiers = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        let (_, interval) = tracker
            .tick(
                generation,
                &modifiers,
                crate::input::SideModifiers {
                    left_super: true,
                    ..Default::default()
                },
                crate::input::BindingContext::field(),
            )
            .unwrap();
        assert_eq!(interval, Duration::from_millis(50));
    }

    #[test]
    fn disabled_or_non_repeating_bind_never_arms() {
        let mut tracker = Tracker::default();
        assert!(
            tracker
                .start(Keycode::new(113), bind(false), 500, 30)
                .is_none()
        );
        assert!(
            tracker
                .start(Keycode::new(113), bind(true), 500, 0)
                .is_none()
        );
    }

    #[test]
    fn release_modifier_change_and_replacement_retire_repeat() {
        let mut tracker = Tracker::default();
        let (first, _) = tracker
            .start(Keycode::new(113), bind(true), 500, 30)
            .unwrap();
        let no_modifiers = ModifiersState::default();
        assert!(
            tracker
                .tick(
                    first,
                    &no_modifiers,
                    Default::default(),
                    crate::input::BindingContext::field(),
                )
                .is_none()
        );

        let (second, _) = tracker
            .start(Keycode::new(113), bind(true), 500, 30)
            .unwrap();
        assert!(!tracker.release(Keycode::new(114)));
        assert!(tracker.release(Keycode::new(113)));
        assert!(
            tracker
                .tick(
                    second,
                    &no_modifiers,
                    Default::default(),
                    crate::input::BindingContext::field(),
                )
                .is_none()
        );

        let (third, _) = tracker
            .start(Keycode::new(113), bind(true), 500, 30)
            .unwrap();
        let (fourth, _) = tracker
            .start(Keycode::new(114), bind(true), 500, 30)
            .unwrap();
        let modifiers = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        assert!(
            tracker
                .tick(
                    third,
                    &modifiers,
                    Default::default(),
                    crate::input::BindingContext::field(),
                )
                .is_none()
        );
        assert!(
            tracker
                .tick(
                    fourth,
                    &modifiers,
                    Default::default(),
                    crate::input::BindingContext::field(),
                )
                .is_some()
        );
    }
}
