pub mod config;
pub mod devices;
pub mod grab;
pub mod keybinds;
pub mod pointer;
pub mod zoom;

use std::collections::HashSet;
use std::hash::Hash;

use halley_config::{Action, BindingScope, ModifierKey, Modifiers};
use smithay::backend::input::{ButtonState, KeyState, Keycode};
use smithay::input::keyboard::{Keysym, ModifiersState};

use keybinds::{BackendKind, ResolvedBind, ResolvedTrigger, WheelDirection};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideModifiers {
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub left_super: bool,
    pub right_super: bool,
}

impl SideModifiers {
    pub fn update(&mut self, keycode: Keycode, state: KeyState) {
        let pressed = state == KeyState::Pressed;
        match keycode.raw() {
            50 => self.left_shift = pressed,
            62 => self.right_shift = pressed,
            37 => self.left_ctrl = pressed,
            105 => self.right_ctrl = pressed,
            64 => self.left_alt = pressed,
            108 => self.right_alt = pressed,
            133 => self.left_super = pressed,
            134 => self.right_super = pressed,
            _ => {}
        }
    }

    fn without_trigger(mut self, keycode: Keycode) -> Self {
        match keycode.raw() {
            50 => self.left_shift = false,
            62 => self.right_shift = false,
            37 => self.left_ctrl = false,
            105 => self.right_ctrl = false,
            64 => self.left_alt = false,
            108 => self.right_alt = false,
            133 => self.left_super = false,
            134 => self.right_super = false,
            _ => {}
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindingContext {
    pub field: bool,
    pub cluster: bool,
    pub tile: bool,
    pub stack: bool,
}

impl BindingContext {
    pub const fn field() -> Self {
        Self {
            field: true,
            cluster: false,
            tile: false,
            stack: false,
        }
    }

    pub const fn cluster(tile: bool) -> Self {
        Self {
            field: false,
            cluster: true,
            tile,
            stack: !tile,
        }
    }

    pub(crate) fn allows(self, scope: BindingScope) -> bool {
        match scope {
            BindingScope::Global => true,
            BindingScope::Field => self.field,
            BindingScope::Cluster => self.cluster,
            BindingScope::Tile => self.tile,
            BindingScope::Stack => self.stack,
        }
    }
}

fn modifier_family_matches(
    active: bool,
    left: bool,
    right: bool,
    generic_expected: bool,
    left_expected: bool,
    right_expected: bool,
) -> bool {
    if generic_expected {
        active
    } else if left_expected || right_expected {
        active && left == left_expected && right == right_expected
    } else {
        !active
    }
}

pub fn modifiers_match(state: &ModifiersState, sides: SideModifiers, expected: Modifiers) -> bool {
    modifier_family_matches(
        state.ctrl,
        sides.left_ctrl,
        sides.right_ctrl,
        expected.ctrl,
        expected.left_ctrl,
        expected.right_ctrl,
    ) && modifier_family_matches(
        state.alt,
        sides.left_alt,
        sides.right_alt,
        expected.alt,
        expected.left_alt,
        expected.right_alt,
    ) && modifier_family_matches(
        state.shift,
        sides.left_shift,
        sides.right_shift,
        expected.shift,
        expected.left_shift,
        expected.right_shift,
    ) && modifier_family_matches(
        state.logo,
        sides.left_super,
        sides.right_super,
        expected.super_key,
        expected.left_super,
        expected.right_super,
    )
}

pub(crate) fn keyboard_modifiers_match(
    state: &ModifiersState,
    sides: SideModifiers,
    expected: Modifiers,
    trigger: ResolvedTrigger,
    keycode: Keycode,
) -> bool {
    let mut without_trigger = *state;
    if let ResolvedTrigger::Keysym(keysym) = trigger {
        if matches!(keysym, Keysym::Shift_L | Keysym::Shift_R) {
            without_trigger.shift = false;
        } else if matches!(keysym, Keysym::Control_L | Keysym::Control_R) {
            without_trigger.ctrl = false;
        } else if matches!(keysym, Keysym::Alt_L | Keysym::Alt_R) {
            without_trigger.alt = false;
        } else if matches!(keysym, Keysym::Super_L | Keysym::Super_R) {
            without_trigger.logo = false;
        }
    }
    match keycode.raw() {
        50 | 62 => without_trigger.shift = false,
        37 | 105 => without_trigger.ctrl = false,
        64 | 108 => without_trigger.alt = false,
        133 | 134 => without_trigger.logo = false,
        _ => {}
    }
    modifiers_match(&without_trigger, sides.without_trigger(keycode), expected)
}

/// Whether the given modifier key is currently held, per a live
/// `ModifiersState` query - used by `input::grab`'s pointer-button dispatch,
/// which (unlike keyboard binds) has no filter closure to read modifiers
/// from and instead queries `KeyboardHandle::modifier_state()` directly at
/// button-press time.
pub fn mod_key_held(state: &ModifiersState, sides: SideModifiers, key: ModifierKey) -> bool {
    match key {
        ModifierKey::Super => state.logo,
        ModifierKey::LeftSuper => sides.left_super,
        ModifierKey::RightSuper => sides.right_super,
        ModifierKey::Alt => state.alt,
        ModifierKey::LeftAlt => sides.left_alt,
        ModifierKey::RightAlt => sides.right_alt,
        ModifierKey::Ctrl => state.ctrl,
        ModifierKey::LeftCtrl => sides.left_ctrl,
        ModifierKey::RightCtrl => sides.right_ctrl,
        ModifierKey::Shift => state.shift,
        ModifierKey::LeftShift => sides.left_shift,
        ModifierKey::RightShift => sides.right_shift,
    }
}

/// Looks up a pressed keysym/raw keycode plus modifiers against the resolved
/// bind table. This remains pure and backend-independent so both sessions
/// can share it from their real `KeyboardHandle::input()` filter closures.
#[cfg(test)]
pub fn match_keyboard_bind(
    binds: &[ResolvedBind],
    mods: &ModifiersState,
    sides: SideModifiers,
    context: BindingContext,
    keysym: Option<Keysym>,
    keycode: Keycode,
) -> Option<Action> {
    match_keyboard_binding(binds, mods, sides, context, keysym, keycode)
        .map(|bind| bind.action.clone())
}

/// Returns the complete resolved binding for input paths that also need
/// trigger metadata such as repeat policy.
pub fn match_keyboard_binding<'a>(
    binds: &'a [ResolvedBind],
    mods: &ModifiersState,
    sides: SideModifiers,
    context: BindingContext,
    keysym: Option<Keysym>,
    keycode: Keycode,
) -> Option<&'a ResolvedBind> {
    let bind = binds.iter().find(|bind| {
        let trigger_matches = match bind.trigger {
            ResolvedTrigger::Keysym(expected) => Some(expected) == keysym,
            ResolvedTrigger::Keycode(expected) => expected == keycode,
            ResolvedTrigger::PointerButton(_) | ResolvedTrigger::Wheel(_) => false,
        };
        context.allows(bind.scope)
            && trigger_matches
            && keyboard_modifiers_match(mods, sides, bind.modifiers, bind.trigger, keycode)
    })?;
    eventline::debug!(
        "keybinds: {:?} + {mods:?} -> {:?}",
        bind.trigger,
        bind.action
    );
    Some(bind)
}

pub fn match_pointer_bind(
    binds: &[ResolvedBind],
    mods: &ModifiersState,
    sides: SideModifiers,
    context: BindingContext,
    button: u32,
) -> Option<Action> {
    let bind = binds.iter().find(|bind| {
        matches!(
            bind.trigger,
            ResolvedTrigger::PointerButton(trigger) if trigger.matches(button)
        ) && context.allows(bind.scope)
            && modifiers_match(mods, sides, bind.modifiers)
    })?;
    eventline::debug!(
        "keybinds: {:?} + {mods:?} -> {:?}",
        bind.trigger,
        bind.action
    );
    Some(bind.action.clone())
}

pub fn match_wheel_bind(
    binds: &[ResolvedBind],
    mods: &ModifiersState,
    sides: SideModifiers,
    context: BindingContext,
    direction: WheelDirection,
) -> Option<Action> {
    let bind = binds.iter().find(|bind| {
        bind.trigger == ResolvedTrigger::Wheel(direction)
            && context.allows(bind.scope)
            && modifiers_match(mods, sides, bind.modifiers)
    })?;
    Some(bind.action.clone())
}

pub struct SuppressedReleases<T> {
    inputs: HashSet<T>,
}

impl<T> Default for SuppressedReleases<T> {
    fn default() -> Self {
        Self {
            inputs: HashSet::new(),
        }
    }
}

impl<T: Eq + Hash> SuppressedReleases<T> {
    pub fn suppress(&mut self, input: T) {
        self.inputs.insert(input);
    }

    pub fn release_is_suppressed(&mut self, input: T) -> bool {
        self.inputs.remove(&input)
    }

    pub fn clear(&mut self) {
        self.inputs.clear();
    }
}

pub type SuppressedButtons = SuppressedReleases<u32>;
pub type SuppressedKeys = SuppressedReleases<Keycode>;

#[derive(Debug, PartialEq, Eq)]
pub enum PointerBindingResult {
    Action(Action),
    SuppressedRelease,
    Unhandled,
}

/// Applies the backend-independent pointer-bind policy: consume the release
/// paired with an intercepted press and let an exact configured chord win.
#[allow(clippy::too_many_arguments)]
pub fn process_pointer_binding(
    binds: &[ResolvedBind],
    mods: &ModifiersState,
    sides: SideModifiers,
    context: BindingContext,
    button: u32,
    state: ButtonState,
    bindings_enabled: bool,
    suppressed: &mut SuppressedButtons,
) -> PointerBindingResult {
    if state == ButtonState::Released && suppressed.release_is_suppressed(button) {
        return PointerBindingResult::SuppressedRelease;
    }
    if state != ButtonState::Pressed {
        return PointerBindingResult::Unhandled;
    }

    if !bindings_enabled {
        return PointerBindingResult::Unhandled;
    }
    let Some(action) = match_pointer_bind(binds, mods, sides, context, button) else {
        return PointerBindingResult::Unhandled;
    };
    suppressed.suppress(button);
    PointerBindingResult::Action(action)
}

/// The resolved bind table plus the configured terminal command - nothing
/// else. Used to own a fake `Seat`/`KeyboardHandle` purely to match
/// keybinds, back when there was no real Wayland client to focus or forward
/// to; now that real clients exist, matching happens directly on the real
/// `Seat<App>`/`Seat<TtyApp>` each app already owns, so this is just data.
pub struct Keyboard {
    pub binds: Vec<ResolvedBind>,
    /// The configured mod key, already remapped for this backend (matches
    /// `binds`' own chords) - `input::grab`'s pointer-button dispatch needs
    /// this too, since "mod+click" checks the same mod key keyboard binds
    /// use, just via a live `modifier_state()` query instead of a filter
    /// closure (pointer events don't carry modifier state directly).
    pub effective_mod: ModifierKey,
    pub side_modifiers: SideModifiers,
    /// Resolved once at startup from Halley's built-in terminal priority list.
    terminal_command: Option<String>,
}

impl Keyboard {
    pub fn from_config(
        keybinds: &halley_config::Keybinds,
        backend: BackendKind,
        path: Option<&std::ffi::OsStr>,
    ) -> Self {
        let binds = keybinds::resolve_binds(keybinds, backend);
        let effective_mod = keybinds::effective_mod(keybinds.modifier, backend);
        let terminal_command = halley_config::resolve_default_terminal_in_path(path);

        Self {
            binds,
            effective_mod,
            side_modifiers: SideModifiers::default(),
            terminal_command,
        }
    }

    pub fn reload(
        &mut self,
        keybinds: &halley_config::Keybinds,
        backend: BackendKind,
        path: Option<&std::ffi::OsStr>,
    ) {
        let side_modifiers = self.side_modifiers;
        *self = Self::from_config(keybinds, backend, path);
        self.side_modifiers = side_modifiers;
    }

    /// The command `Action::OpenTerminal` should launch, if one of Halley's
    /// built-in terminal candidates is available on `PATH`.
    pub fn terminal_command(&self) -> Option<&str> {
        self.terminal_command.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keybinds::{PointerButtonTrigger, ResolvedTrigger};

    fn bind(trigger: ResolvedTrigger, modifiers: Modifiers) -> ResolvedBind {
        ResolvedBind {
            scope: BindingScope::Global,
            modifiers,
            trigger,
            action: Action::Quit,
            repeat: false,
        }
    }

    #[test]
    fn pointer_bind_requires_exact_modifiers() {
        let binds = [bind(
            ResolvedTrigger::PointerButton(PointerButtonTrigger::Left),
            Modifiers {
                super_key: true,
                ..Modifiers::default()
            },
        )];
        assert_eq!(
            match_pointer_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                0x110,
            ),
            None
        );
        let mods = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            match_pointer_bind(
                &binds,
                &mods,
                SideModifiers::default(),
                BindingContext::field(),
                0x110,
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn raw_pointer_button_matches_only_its_code() {
        let binds = [bind(
            ResolvedTrigger::PointerButton(PointerButtonTrigger::Code(279)),
            Modifiers::default(),
        )];
        assert_eq!(
            match_pointer_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                278,
            ),
            None
        );
        assert_eq!(
            match_pointer_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                279,
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn wheel_directions_do_not_cross_match() {
        let binds = [bind(
            ResolvedTrigger::Wheel(WheelDirection::Up),
            Modifiers::default(),
        )];
        assert_eq!(
            match_wheel_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                WheelDirection::Down,
            ),
            None
        );
        assert_eq!(
            match_wheel_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                WheelDirection::Up,
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn raw_keycodes_match_even_when_xkb_has_no_symbol() {
        let binds = [bind(
            ResolvedTrigger::Keycode(Keycode::new(255)),
            Modifiers::default(),
        )];
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                None,
                Keycode::new(255),
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn compositor_bindings_do_not_capture_ctrl_space_without_an_exact_bind() {
        let binds = [bind(
            ResolvedTrigger::Keysym(Keysym::space),
            Modifiers {
                super_key: true,
                ..Modifiers::default()
            },
        )];
        let ctrl = ModifiersState {
            ctrl: true,
            ..ModifiersState::default()
        };

        assert_eq!(
            match_keyboard_bind(
                &binds,
                &ctrl,
                SideModifiers::default(),
                BindingContext::field(),
                Some(Keysym::space),
                Keycode::new(65),
            ),
            None
        );
    }

    #[test]
    fn modifier_keys_can_be_bare_triggers() {
        let binds = [
            bind(
                ResolvedTrigger::Keysym(Keysym::Shift_L),
                Modifiers::default(),
            ),
            bind(
                ResolvedTrigger::Keysym(Keysym::Super_R),
                Modifiers::default(),
            ),
        ];
        let shift = ModifiersState {
            shift: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &shift,
                SideModifiers {
                    left_shift: true,
                    ..Default::default()
                },
                BindingContext::field(),
                Some(Keysym::Shift_L),
                Keycode::new(50),
            ),
            Some(Action::Quit)
        );
        let logo = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &logo,
                SideModifiers {
                    right_super: true,
                    ..Default::default()
                },
                BindingContext::field(),
                Some(Keysym::Super_R),
                Keycode::new(134),
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn per_side_modifier_matches_only_the_requested_side() {
        let binds = [bind(
            ResolvedTrigger::Keysym(Keysym::x),
            Modifiers {
                left_super: true,
                ..Modifiers::default()
            },
        )];
        let logo = ModifiersState {
            logo: true,
            ..ModifiersState::default()
        };
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &logo,
                SideModifiers {
                    right_super: true,
                    ..Default::default()
                },
                BindingContext::field(),
                Some(Keysym::x),
                Keycode::new(53),
            ),
            None
        );
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &logo,
                SideModifiers {
                    left_super: true,
                    ..Default::default()
                },
                BindingContext::field(),
                Some(Keysym::x),
                Keycode::new(53),
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn duplicate_chord_selects_the_active_scope() {
        let mut field = bind(ResolvedTrigger::Keysym(Keysym::Left), Modifiers::default());
        field.scope = BindingScope::Field;
        field.action = Action::ResizeWindow(halley_config::Direction::Left);
        let mut tile = field.clone();
        tile.scope = BindingScope::Tile;
        tile.action = Action::ClusterTileSwap(halley_config::Direction::Left);
        let binds = [field, tile];

        assert_eq!(
            match_keyboard_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                Some(Keysym::Left),
                Keycode::new(113),
            ),
            Some(Action::ResizeWindow(halley_config::Direction::Left))
        );
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::cluster(true),
                Some(Keysym::Left),
                Keycode::new(113),
            ),
            Some(Action::ClusterTileSwap(halley_config::Direction::Left))
        );
        assert_eq!(
            match_keyboard_bind(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::cluster(false),
                Some(Keysym::Left),
                Keycode::new(113),
            ),
            None
        );
    }

    #[test]
    fn suppressed_button_release_is_consumed_exactly_once() {
        let mut suppressed = SuppressedButtons::default();
        suppressed.suppress(0x110);
        assert!(!suppressed.release_is_suppressed(0x111));
        assert!(suppressed.release_is_suppressed(0x110));
        assert!(!suppressed.release_is_suppressed(0x110));
    }

    #[test]
    fn bare_pointer_binding_can_drive_background_pan_action() {
        let binds = [bind(
            ResolvedTrigger::PointerButton(PointerButtonTrigger::Left),
            Modifiers::default(),
        )];
        let mut suppressed = SuppressedButtons::default();
        assert_eq!(
            process_pointer_binding(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                0x110,
                ButtonState::Pressed,
                true,
                &mut suppressed,
            ),
            PointerBindingResult::Action(Action::Quit)
        );
    }

    #[test]
    fn pointer_binding_policy_pairs_intercepted_press_and_release() {
        let binds = [bind(
            ResolvedTrigger::PointerButton(PointerButtonTrigger::Left),
            Modifiers::default(),
        )];
        let mut suppressed = SuppressedButtons::default();
        assert_eq!(
            process_pointer_binding(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                0x110,
                ButtonState::Pressed,
                true,
                &mut suppressed,
            ),
            PointerBindingResult::Action(Action::Quit)
        );
        assert_eq!(
            process_pointer_binding(
                &binds,
                &ModifiersState::default(),
                SideModifiers::default(),
                BindingContext::field(),
                0x110,
                ButtonState::Released,
                true,
                &mut suppressed,
            ),
            PointerBindingResult::SuppressedRelease
        );
    }
}
