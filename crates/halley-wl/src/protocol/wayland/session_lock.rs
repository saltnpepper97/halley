use std::collections::HashMap;

use smithay::{
    delegate_session_lock,
    desktop::{WindowSurfaceType, utils::under_from_surface_tree},
    output::Output,
    reexports::wayland_server::{
        Resource, protocol::wl_output::WlOutput, protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Size},
    wayland::compositor::get_parent,
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};

use crate::compositor::root::Halley;

#[derive(Clone, Debug)]
pub(crate) struct SessionLockSurfaceEntry {
    pub(crate) surface: LockSurface,
    pub(crate) monitor: String,
}

#[derive(Debug)]
pub(crate) struct HalleySessionLockState {
    pub(crate) manager_state: SessionLockManagerState,
    pub(crate) surfaces:
        HashMap<smithay::reexports::wayland_server::backend::ObjectId, SessionLockSurfaceEntry>,
    pub(crate) last_configured_size:
        HashMap<smithay::reexports::wayland_server::backend::ObjectId, Size<u32, Logical>>,
    pub(crate) keyboard_focus: Option<smithay::reexports::wayland_server::backend::ObjectId>,
    pub(crate) active: bool,
}

impl HalleySessionLockState {
    pub(crate) fn new<D, F>(display: &smithay::reexports::wayland_server::DisplayHandle, filter: F) -> Self
    where
        D: smithay::reexports::wayland_server::GlobalDispatch<
                smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1,
                smithay::wayland::session_lock::SessionLockManagerGlobalData,
            >
            + smithay::reexports::wayland_server::Dispatch<
                smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1,
                (),
            >
            + smithay::reexports::wayland_server::Dispatch<
                smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
                smithay::wayland::session_lock::SessionLockState,
            >
            + SessionLockHandler
            + 'static,
        F: for<'c> Fn(&'c smithay::reexports::wayland_server::Client) -> bool + Send + Sync + 'static,
    {
        Self {
            manager_state: SessionLockManagerState::new::<D, _>(display, filter),
            surfaces: HashMap::new(),
            last_configured_size: HashMap::new(),
            keyboard_focus: None,
            active: false,
        }
    }
}

fn surface_tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

fn lock_surface_size(st: &Halley, monitor: &str) -> Size<u32, Logical> {
    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| (space.width.max(1) as u32, space.height.max(1) as u32).into())
        .unwrap_or_else(|| {
            (
                st.runtime.tuning.viewport_size.x.max(1.0).round() as u32,
                st.runtime.tuning.viewport_size.y.max(1.0).round() as u32,
            )
                .into()
        })
}

fn focus_candidate_surface(st: &Halley) -> Option<WlSurface> {
    if let Some(focus_id) = st.platform.session_lock.keyboard_focus.clone()
        && let Some(surface) = st
            .platform
            .session_lock
            .surfaces
            .get(&focus_id)
            .filter(|entry| entry.surface.alive())
            .map(|entry| entry.surface.wl_surface().clone())
    {
        return Some(surface);
    }

    for preferred in [
        st.model.monitor_state.current_monitor.as_str(),
        st.model.monitor_state.focused_monitor.as_str(),
    ] {
        if let Some(surface) = st
            .platform
            .session_lock
            .surfaces
            .values()
            .find(|entry| entry.monitor == preferred && entry.surface.alive())
            .map(|entry| entry.surface.wl_surface().clone())
        {
            return Some(surface);
        }
    }

    st.platform
        .session_lock
        .surfaces
        .values()
        .find(|entry| entry.surface.alive())
        .map(|entry| entry.surface.wl_surface().clone())
}

fn configure_lock_surface(st: &mut Halley, surface: &LockSurface, monitor: &str) {
    let size = lock_surface_size(st, monitor);
    if st
        .platform
        .session_lock
        .last_configured_size
        .get(&surface.wl_surface().id())
        == Some(&size)
    {
        return;
    }
    surface.with_pending_state(|state| {
        state.size = Some(size);
    });
    surface.send_configure();
    st.platform
        .session_lock
        .last_configured_size
        .insert(surface.wl_surface().id(), size);
}

pub(crate) fn session_lock_active(st: &Halley) -> bool {
    st.platform.session_lock.active
}

pub(crate) fn monitor_for_surface(st: &Halley, surface: &WlSurface) -> Option<String> {
    let root = surface_tree_root(surface);
    st.platform
        .session_lock
        .surfaces
        .get(&root.id())
        .map(|entry| entry.monitor.clone())
}

pub(crate) fn is_session_lock_surface(st: &Halley, surface: &WlSurface) -> bool {
    let root = surface_tree_root(surface);
    st.platform.session_lock.surfaces.contains_key(&root.id())
}

pub(crate) fn current_monitor_surfaces(st: &Halley) -> Vec<WlSurface> {
    let monitor = st.model.monitor_state.current_monitor.clone();
    st.platform
        .session_lock
        .surfaces
        .values()
        .filter(|entry| entry.monitor == monitor && entry.surface.alive())
        .map(|entry| entry.surface.wl_surface().clone())
        .collect()
}

pub(crate) fn focus_surface(st: &mut Halley, surface: &WlSurface) -> bool {
    let root = surface_tree_root(surface);
    if !st.platform.session_lock.surfaces.contains_key(&root.id()) {
        return false;
    }

    st.model.monitor_state.layer_keyboard_focus = None;
    st.platform.session_lock.keyboard_focus = Some(root.id());
    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return false;
    };
    keyboard.set_focus(st, Some(root.clone()), SERIAL_COUNTER.next_serial());
    st.update_selection_focus_from_surface(Some(&root));
    true
}

pub(crate) fn maybe_focus_surface_on_commit(st: &mut Halley, surface: &WlSurface) {
    if !session_lock_active(st) {
        return;
    }
    let root = surface_tree_root(surface);
    let Some(entry) = st.platform.session_lock.surfaces.get(&root.id()).cloned() else {
        return;
    };
    configure_lock_surface(st, &entry.surface, entry.monitor.as_str());
    if st.platform.session_lock.keyboard_focus.is_none()
        || st.platform.session_lock.keyboard_focus == Some(root.id())
    {
        let _ = focus_surface(st, &root);
    }
}

pub(crate) fn configure_surfaces(st: &mut Halley) {
    let entries = st
        .platform
        .session_lock
        .surfaces
        .values()
        .filter(|entry| entry.surface.alive())
        .cloned()
        .collect::<Vec<_>>();
    for entry in entries {
        configure_lock_surface(st, &entry.surface, entry.monitor.as_str());
    }
}

pub(crate) fn focus_for_screen(
    st: &mut Halley,
    sx: f32,
    sy: f32,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    if !session_lock_active(st) {
        return None;
    }

    for surface in current_monitor_surfaces(st) {
        let local =
            Point::<f64, Logical>::from((sx.round() as i32 as f64, sy.round() as i32 as f64));
        let Some((hit, surface_loc)) =
            under_from_surface_tree(&surface, local, (0, 0), WindowSurfaceType::ALL)
        else {
            continue;
        };
        return Some((
            hit,
            Point::<f64, Logical>::from((surface_loc.x as f64, surface_loc.y as f64)),
        ));
    }

    None
}

pub(crate) fn reassert_keyboard_focus_if_drifted(st: &mut Halley) {
    if !session_lock_active(st) {
        st.platform.session_lock.keyboard_focus = None;
        return;
    }

    let Some(desired_focus) = focus_candidate_surface(st) else {
        st.platform.session_lock.keyboard_focus = None;
        return;
    };
    st.platform.session_lock.keyboard_focus = Some(desired_focus.id());

    let Some(keyboard) = st.platform.seat.get_keyboard() else {
        return;
    };
    if keyboard
        .current_focus()
        .as_ref()
        .is_some_and(|focus| surface_tree_root(focus).id() == desired_focus.id())
    {
        return;
    }

    keyboard.set_focus(
        st,
        Some(desired_focus.clone()),
        SERIAL_COUNTER.next_serial(),
    );
    st.update_selection_focus_from_surface(Some(&desired_focus));
}

impl SessionLockHandler for Halley {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.platform.session_lock.manager_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if self.platform.session_lock.active {
            return;
        }

        self.platform.session_lock.active = true;
        self.platform.session_lock.keyboard_focus = None;
        self.platform.session_lock.surfaces.clear();
        self.platform.session_lock.last_configured_size.clear();
        self.model.monitor_state.layer_keyboard_focus = None;
        self.clear_keyboard_focus();
        crate::compositor::interaction::pointer::clear_pointer_focus(self);
        self.request_maintenance();
        confirmation.lock();
    }

    fn unlock(&mut self) {
        self.platform.session_lock.active = false;
        self.platform.session_lock.keyboard_focus = None;
        self.platform.session_lock.surfaces.clear();
        self.platform.session_lock.last_configured_size.clear();
        crate::compositor::interaction::pointer::clear_pointer_focus(self);
        self.apply_wayland_focus_state(self.model.focus_state.primary_interaction_focus);
        self.request_maintenance();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let monitor = Output::from_resource(&output)
            .map(|output| output.name())
            .filter(|name| self.model.monitor_state.monitors.contains_key(name))
            .unwrap_or_else(|| self.model.monitor_state.current_monitor.clone());

        for (name, compositor_output) in &self.model.monitor_state.outputs {
            if *name == monitor {
                compositor_output.enter(surface.wl_surface());
            } else {
                compositor_output.leave(surface.wl_surface());
            }
        }

        self.platform.session_lock.surfaces.insert(
            surface.wl_surface().id(),
            SessionLockSurfaceEntry {
                surface: surface.clone(),
                monitor: monitor.clone(),
            },
        );
        configure_lock_surface(self, &surface, monitor.as_str());
        if self.platform.session_lock.keyboard_focus.is_none()
            || monitor == self.model.monitor_state.current_monitor
            || monitor == self.model.monitor_state.focused_monitor
        {
            let _ = focus_surface(self, surface.wl_surface());
        }
        self.request_maintenance();
    }
}

delegate_session_lock!(Halley);
