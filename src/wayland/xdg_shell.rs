use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::presentation::camera::OutputCameras;

use super::WaylandState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToplevelCommit {
    None,
    Mapped(WlSurface),
    Unmapped(WlSurface),
    /// The commit belonged to layer shell, which owns no toplevel. Reported
    /// rather than folded into `None` because rearranging the layer map can
    /// change the exclusive zone, and callers publish that outwards.
    Layer,
}

#[derive(Clone)]
pub struct ToplevelCommitContext<'a> {
    pub cameras: &'a OutputCameras,
    pub primary_output: &'a Output,
    pub rule: crate::window::rules::ResolvedWindowRule,
    pub cursor_position: smithay::utils::Point<f64, smithay::utils::Logical>,
    pub gap: f32,
    pub decorations: &'a halley_config::Decorations,
    pub font: &'a halley_config::Font,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MappingTransition {
    StayMapped,
    Unmap,
    Map,
    StayUnmapped,
}

fn mapping_transition(currently_mapped: bool, has_buffer: bool) -> MappingTransition {
    match (currently_mapped, has_buffer) {
        (true, true) => MappingTransition::StayMapped,
        (true, false) => MappingTransition::Unmap,
        (false, true) => MappingTransition::Map,
        (false, false) => MappingTransition::StayUnmapped,
    }
}

pub fn will_unmap(wayland: &WaylandState, surface: &WlSurface) -> bool {
    let managed = wayland.space.elements().any(|window| {
        window
            .toplevel()
            .is_some_and(|toplevel| toplevel.wl_surface() == surface)
    }) || wayland.collapsed.contains_key(surface);
    managed
        && !with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

/// A new toplevel role was created - stash it as unmapped, nothing shown
/// yet. Deliberately just this: no spawn-rule resolution, no monitor/
/// cluster placement, no reveal animation. Old halley fused exactly this
/// kind of window-manager policy into `XdgShellHandler::new_toplevel`
/// itself; that policy, if and when it exists, belongs downstream of
/// mapping, not inside protocol handling.
pub fn new_toplevel(wayland: &mut WaylandState, surface: ToplevelSurface) {
    let wl_surface = surface.wl_surface().clone();
    let window = Window::new_wayland_window(surface);
    wayland.unmapped.insert(wl_surface, window);
}

/// Removes a destroyed toplevel from whichever lifecycle state owns it.
///
/// This must explicitly unmap a mapped window: Smithay calls this handler
/// while the underlying `wl_surface` may still be alive, so waiting for
/// `Space::refresh()` can leave Halley's compositor-drawn border in the next
/// frame. Focus clears to `None`; there is no fallback-refocus policy yet.
pub fn toplevel_destroyed(wayland: &mut WaylandState, surface: &ToplevelSurface) {
    wayland.unmapped.remove(surface.wl_surface());
    wayland.collapsed.remove(surface.wl_surface());
    wayland.unmapped_locations.remove(surface.wl_surface());
    let mapped = wayland
        .space
        .elements()
        .find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
        })
        .cloned();
    if let Some(window) = mapped {
        wayland.space.unmap_elem(&window);
    }
    if wayland.focused_window.as_ref() == Some(surface.wl_surface()) {
        wayland.focused_window = None;
    }
}

/// The commit-path half of the unmapped -> mapped transition: sends the
/// initial configure for a still-unmapped toplevel, then promotes it into
/// `space` once it has actually attached a buffer. This is the one
/// deliberate deviation from smallvil, which maps a toplevel into `Space`
/// immediately in `new_toplevel` and relies on Smithay's render-element
/// code silently skipping bufferless surfaces - here "mapped" means
/// "actually visible," not "present in `Space` but incidentally invisible."
pub fn handle_commit(
    wayland: &mut WaylandState,
    surface: &WlSurface,
    context: ToplevelCommitContext<'_>,
) -> ToplevelCommit {
    let ToplevelCommitContext {
        cameras,
        primary_output,
        rule,
        cursor_position,
        gap,
        decorations,
        font,
    } = context;
    let has_buffer =
        with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false);
    let mapped = {
        wayland
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface)
            })
            .cloned()
    };
    if let Some(window) = mapped {
        match mapping_transition(true, has_buffer) {
            MappingTransition::StayMapped => return ToplevelCommit::None,
            MappingTransition::Unmap => {}
            MappingTransition::Map | MappingTransition::StayUnmapped => unreachable!(),
        }
        if let Some(location) = wayland.space.element_location(&window) {
            wayland.unmapped_locations.insert(surface.clone(), location);
        }
        wayland.space.unmap_elem(&window);
        wayland.unmapped.insert(surface.clone(), window);
        if wayland.focused_window.as_ref() == Some(surface) {
            wayland.focused_window = None;
        }
        return ToplevelCommit::Unmapped(surface.clone());
    }

    if let Some(window) = wayland.collapsed.get(surface).cloned() {
        match mapping_transition(true, has_buffer) {
            MappingTransition::StayMapped => return ToplevelCommit::None,
            MappingTransition::Unmap => {}
            MappingTransition::Map | MappingTransition::StayUnmapped => unreachable!(),
        }
        wayland.collapsed.remove(surface);
        wayland.unmapped.insert(surface.clone(), window);
        if wayland.focused_window.as_ref() == Some(surface) {
            wayland.focused_window = None;
        }
        return ToplevelCommit::Unmapped(surface.clone());
    }

    let Some(window) = wayland.unmapped.get(surface) else {
        return ToplevelCommit::None;
    };
    let Some(toplevel) = window.toplevel() else {
        return ToplevelCommit::None;
    };

    if !toplevel.is_initial_configure_sent() {
        toplevel.with_pending_state(super::decoration::apply_tiled_hint);
        toplevel.send_configure();
        return ToplevelCommit::None;
    }

    if mapping_transition(false, has_buffer) == MappingTransition::Map {
        let window = wayland.unmapped.remove(surface).expect("checked above");
        let client_size = window.geometry().size;
        let outer_size =
            crate::titlebar::outer_size_for_client(&window, client_size, decorations, font);
        let placement = crate::window::routing::initial_window_placement(
            crate::window::routing::InitialWindowPlacementRequest {
                wayland,
                cameras,
                primary_output,
                window: Some(&window),
                window_size: outer_size,
                placement: rule.spawn_placement,
                output_name: rule.open_on_output.as_deref(),
                cursor_position,
                gap,
            },
        );
        let location = wayland
            .unmapped_locations
            .remove(surface)
            .unwrap_or_else(|| {
                crate::titlebar::client_location_for_outer(
                    &window,
                    placement.location,
                    decorations,
                    font,
                )
            });
        super::set_window_output(&window, &placement.output);
        let location = if super::clipboard_helper::is_helper(surface) {
            super::clipboard_helper::remember_focus(
                surface,
                wayland.focused_window.as_ref(),
                wayland.focused_layer.as_ref(),
            );
            // The helper needs a mapped surface and keyboard enter serial,
            // but must not appear as desktop chrome or intercept hover input.
            (-1_000_000, -1_000_000).into()
        } else {
            location
        };
        wayland.space.map_element(window.clone(), location, false);
        // New windows steal focus - matches most WMs' default behavior.
        // Also raises+activates via `focus_and_raise`, same as clicking a
        // window now does.
        let selected_output =
            crate::wayland::focus::output_for_new_surface(wayland, None, primary_output);
        let focus = rule
            .open_focused
            .unwrap_or(rule.open_on_output.is_none() || placement.output == selected_output);
        if focus {
            crate::window::focus_and_raise(wayland, &window);
        }
        return ToplevelCommit::Mapped(surface.clone());
    }

    ToplevelCommit::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_null_buffer_unmaps_and_a_later_buffer_remaps() {
        assert_eq!(mapping_transition(true, false), MappingTransition::Unmap);
        assert_eq!(
            mapping_transition(false, false),
            MappingTransition::StayUnmapped
        );
        assert_eq!(mapping_transition(false, true), MappingTransition::Map);
        assert_eq!(
            mapping_transition(true, true),
            MappingTransition::StayMapped
        );
    }
}
