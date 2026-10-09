pub(crate) mod dialog;
pub(crate) mod recovery;
pub(crate) mod routing;
pub(crate) mod rules;
pub(crate) mod stacking;

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::seat::WaylandFocus;

use crate::wayland::WaylandState;

/// Bottom-to-top order for managed windows.
///
/// Smithay's space also has an element order, but it includes unmanaged
/// surfaces and is changed by presentation mechanics. Window-management
/// policy uses this order instead.
#[derive(Debug, Default)]
pub struct ManagedWindowStack {
    order: Vec<WlSurface>,
}

fn raise_in_order<T: Eq>(order: &mut Vec<T>, item: T) {
    order.retain(|candidate| candidate != &item);
    order.push(item);
}

impl ManagedWindowStack {
    pub fn raise(&mut self, surface: WlSurface) {
        raise_in_order(&mut self.order, surface);
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.order.retain(|candidate| candidate != surface);
    }

    pub fn top_to_bottom(&self) -> impl Iterator<Item = &WlSurface> {
        self.order.iter().rev()
    }

    pub fn contains(&self, surface: &WlSurface) -> bool {
        self.order.iter().any(|candidate| candidate == surface)
    }
}

/// Override-redirect X11 windows are deliberately outside normal window
/// manager focus policy. ICCCM leaves keyboard focus for those windows to the
/// client (typically via a grab or a managed owner using WM_TAKE_FOCUS).
pub fn accepts_wm_focus(window: &Window) -> bool {
    focus_policy(crate::xwayland::is_override_redirect(window))
}

/// Compositor move/resize grabs are window-management operations. X11 menus,
/// tooltips, and other override-redirect surfaces remain interactive client
/// surfaces, but are never draggable or resizable as managed windows.
pub fn accepts_compositor_grab(window: &Window) -> bool {
    compositor_grab_policy(crate::xwayland::is_override_redirect(window))
}

pub fn focus(wayland: &mut WaylandState, window: &Window, raise: bool) {
    if !accepts_wm_focus(window) {
        return;
    }
    wayland.focused_layer = None;
    for mapped in wayland.space.elements() {
        if mapped.set_activated(mapped == window)
            && let Some(toplevel) = mapped.toplevel()
            && toplevel.is_initial_configure_sent()
        {
            toplevel.send_pending_configure();
        }
    }
    if raise {
        raise_managed(wayland, window);
    }
    wayland.focused_window = window.wl_surface().map(|surface| surface.into_owned());
}

pub fn raise_managed(wayland: &mut WaylandState, window: &Window) {
    if !accepts_wm_focus(window) {
        return;
    }
    if let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) {
        wayland.managed_windows.raise(surface);
    }
    if let Some(location) = wayland.space.element_location(window) {
        wayland.space.map_element(window.clone(), location, true);
    }
    enforce_dialog_stacking(wayland);
}

/// Raises a presentation group as one block without changing keyboard focus or
/// client activation. `windows` must be in bottom-to-top order so their
/// existing relative depth is preserved while non-members move underneath.
pub fn raise_managed_group(wayland: &mut WaylandState, windows: &[Window]) {
    for window in windows {
        if !accepts_wm_focus(window) {
            continue;
        }
        if let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) {
            wayland.managed_windows.raise(surface);
        }
        wayland.space.raise_element(window, false);
    }
    enforce_dialog_stacking(wayland);
}

pub(crate) fn enforce_dialog_stacking(wayland: &mut crate::wayland::WaylandState) -> bool {
    let mut windows = wayland.space.elements().cloned().collect::<Vec<_>>();
    let changed = stacking::sort_above_parents(&wayland.space, &mut windows, |window| Some(window));
    if changed {
        // Raising in bottom-to-top order preserves locations and activation.
        for window in &windows {
            wayland.space.raise_element(window, false);
        }
    }
    let mut managed = wayland.managed_windows.order.clone();
    let parents = managed
        .iter()
        .map(|surface| {
            use smithay::wayland::seat::WaylandFocus;
            let window = windows
                .iter()
                .find(|w| w.wl_surface().is_some_and(|s| s.as_ref() == surface))?;
            let parent = stacking::parent_window(&wayland.space, window)?;
            let surface = parent.wl_surface()?;
            managed.iter().position(|s| s == surface.as_ref())
        })
        .collect::<Vec<_>>();
    if parents
        .iter()
        .enumerate()
        .any(|(i, parent)| parent.is_some_and(|p| p >= i))
    {
        let order = stacking::parent_order(&parents);
        managed = order.into_iter().map(|i| managed[i].clone()).collect();
        wayland.managed_windows.order = managed;
    }
    changed
}

pub fn focus_and_raise(wayland: &mut WaylandState, window: &Window) {
    focus(wayland, window, true);
}

pub fn clear_focus(wayland: &mut WaylandState) {
    wayland.focused_layer = None;
    for mapped in wayland.space.elements() {
        if mapped.set_activated(false)
            && let Some(toplevel) = mapped.toplevel()
            && toplevel.is_initial_configure_sent()
        {
            toplevel.send_pending_configure();
        }
    }
    wayland.focused_window = None;
}

fn focus_policy(override_redirect: bool) -> bool {
    !override_redirect
}

fn compositor_grab_policy(override_redirect: bool) -> bool {
    !override_redirect
}

#[cfg(test)]
mod tests {
    #[test]
    fn override_redirect_policy_is_unmanaged() {
        assert!(!super::focus_policy(true));
        assert!(super::focus_policy(false));
    }

    #[test]
    fn override_redirect_cannot_start_a_compositor_grab() {
        assert!(!super::compositor_grab_policy(true));
        assert!(super::compositor_grab_policy(false));
    }

    #[test]
    fn presentation_entry_raise_is_one_shot_not_always_on_top() {
        let mut order = vec!["maximize-target", "existing-foreground"];

        super::raise_in_order(&mut order, "maximize-target");
        assert_eq!(order, ["existing-foreground", "maximize-target"]);

        super::raise_in_order(&mut order, "existing-foreground");
        assert_eq!(order, ["maximize-target", "existing-foreground"]);
    }

    #[test]
    fn arrangement_group_moves_above_non_members_without_reordering_itself() {
        let mut order = vec!["arranged-back", "overlap", "arranged-front"];

        for member in ["arranged-back", "arranged-front"] {
            super::raise_in_order(&mut order, member);
        }

        assert_eq!(order, ["overlap", "arranged-back", "arranged-front"]);
    }
}

/// Only standalone normal/utility pop-outs may be explicitly moved. This does
/// not grant WM focus or resize privileges to override-redirect surfaces.
#[cfg(feature = "xwayland")]
pub fn accepts_popup_move(window: &Window) -> bool {
    window.x11_surface().is_some_and(|surface| {
        popup_move_policy(
            surface.is_override_redirect(),
            surface.is_transient_for().is_some()
                || crate::wayland::window_presentation_owner(window).is_some(),
            surface.window_type(),
        )
    })
}

#[cfg(not(feature = "xwayland"))]
pub fn accepts_popup_move(_window: &Window) -> bool {
    false
}

#[cfg(feature = "xwayland")]
fn popup_move_policy(
    override_redirect: bool,
    attached: bool,
    kind: Option<smithay::xwayland::xwm::WmWindowType>,
) -> bool {
    use smithay::xwayland::xwm::WmWindowType;
    override_redirect
        && !attached
        && matches!(kind, Some(WmWindowType::Normal | WmWindowType::Utility))
}

#[cfg(all(test, feature = "xwayland"))]
mod popup_move_tests {
    use super::popup_move_policy;
    use smithay::xwayland::xwm::WmWindowType::*;
    #[test]
    fn standalone_popouts_allow_explicit_moves_but_not_attached_or_managed_windows() {
        for kind in [Normal, Utility] {
            assert!(popup_move_policy(true, false, Some(kind)));
            assert!(!popup_move_policy(true, true, Some(kind)));
            assert!(!popup_move_policy(false, false, Some(kind)));
        }
    }
    #[test]
    fn menus_tooltips_and_unclassified_surfaces_remain_client_managed() {
        for kind in [
            None,
            Some(Menu),
            Some(PopupMenu),
            Some(DropdownMenu),
            Some(Tooltip),
            Some(Combo),
            Some(Dnd),
            Some(Notification),
            Some(Dock),
        ] {
            assert!(!popup_move_policy(true, false, kind));
        }
    }
}
