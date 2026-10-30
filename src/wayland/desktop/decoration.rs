use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::wayland::shell::xdg::{ToplevelState, ToplevelSurface};

/// A toplevel just created a decoration object - default it to server-side.
/// This is the direction halley is headed (her own simple built-in
/// decorations), so it's the preference to advertise before any client asks
/// for something else. Clients that never create a decoration object at all
/// (kitty, alacritty - CSD-only, no negotiation) never reach this path; they
/// draw their own chrome regardless of what we do here.
pub fn new_decoration(toplevel: ToplevelSurface) {
    set_mode(&toplevel, Mode::ServerSide);
}

/// A client explicitly asked for a mode - honor exactly what it asked for.
/// Forcing a different mode during window creation can leave some clients
/// permanently hidden (https://github.com/libsdl-org/SDL/issues/8173).
pub fn request_mode(toplevel: ToplevelSurface, mode: Mode) {
    set_mode(&toplevel, mode);
}

/// A client unset its preference - fall back to our default.
pub fn unset_mode(toplevel: ToplevelSurface) {
    set_mode(&toplevel, Mode::ServerSide);
}

fn set_mode(toplevel: &ToplevelSurface, mode: Mode) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(mode);
        apply_tiled_hint(state);
    });
    toplevel.send_configure();
}

pub fn apply_tiled_hint(state: &mut ToplevelState) {
    if state.states.contains(State::Fullscreen) {
        clear_tiled_hint(state);
        return;
    }
    for edge in [
        State::TiledTop,
        State::TiledBottom,
        State::TiledLeft,
        State::TiledRight,
    ] {
        state.states.set(edge);
    }
}

pub fn clear_tiled_hint(state: &mut ToplevelState) {
    for edge in [
        State::TiledTop,
        State::TiledBottom,
        State::TiledLeft,
        State::TiledRight,
    ] {
        state.states.unset(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiled_hint_marks_every_edge() {
        let mut state = ToplevelState::default();

        apply_tiled_hint(&mut state);

        for edge in [
            State::TiledTop,
            State::TiledBottom,
            State::TiledLeft,
            State::TiledRight,
        ] {
            assert!(state.states.contains(edge));
        }
    }

    #[test]
    fn fullscreen_clears_tiled_edges() {
        let mut state = ToplevelState::default();
        apply_tiled_hint(&mut state);
        state.states.set(State::Fullscreen);

        apply_tiled_hint(&mut state);

        for edge in [
            State::TiledTop,
            State::TiledBottom,
            State::TiledLeft,
            State::TiledRight,
        ] {
            assert!(!state.states.contains(edge));
        }
    }
}
