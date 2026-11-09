//! Surface-owned, opt-in outside-press notifications. No application policy.
use std::sync::Mutex;

use halley_api::wayland::server::{
    halley_surface_dismissal_manager_v1::{
        self as manager, HalleySurfaceDismissalManagerV1 as Manager,
    },
    halley_surface_dismissal_v1::{self as watch, HalleySurfaceDismissalV1 as Watch},
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
    protocol::wl_surface::WlSurface,
};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::LayerSurfaceData;

pub struct State;
struct Watches(Mutex<Vec<Watch>>);
pub struct WatchData(WlSurface);

pub fn create_global<D>(display: &DisplayHandle)
where
    D: GlobalDispatch<Manager, ()> + 'static,
{
    display.create_global::<D, Manager, _>(1, ());
}

/// The caller has already checked mapping, hit testing, session lock, and button.
/// Notifications are one-shot and never claim the triggering input event.
pub fn outside_press(surface: &WlSurface) {
    with_states(surface, |states| {
        if let Some(watches) = states.data_map.get::<Watches>() {
            for watch in watches.0.lock().unwrap().drain(..) {
                if watch.is_alive() {
                    watch.outside_press();
                }
            }
        }
    });
}

/// Unmapping permanently retires watches, even if the same surface later remaps.
pub fn cancel(surface: &WlSurface) {
    with_states(surface, |states| {
        if let Some(watches) = states.data_map.get::<Watches>() {
            watches.0.lock().unwrap().clear();
        }
    });
}

impl<D> GlobalDispatch<Manager, (), D> for State
where
    D: GlobalDispatch<Manager, ()> + Dispatch<Manager, ()> + Dispatch<Watch, WatchData> + 'static,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<Manager>,
        _: &(),
        init: &mut DataInit<'_, D>,
    ) {
        init.init(resource, ());
    }
}

impl<D> Dispatch<Manager, (), D> for State
where
    D: Dispatch<Manager, ()> + Dispatch<Watch, WatchData> + 'static,
{
    fn request(
        _: &mut D,
        client: &Client,
        manager: &Manager,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        if let manager::Request::Watch { id, surface } = request {
            let watch = init.init(id, WatchData(surface.clone()));
            let valid = surface
                .client()
                .is_some_and(|owner| owner.id() == client.id())
                && with_states(&surface, |states| {
                    states.data_map.get::<LayerSurfaceData>().is_some()
                });
            if !valid {
                manager.post_error(
                    manager::Error::InvalidSurface,
                    "watch requires an owned layer-shell surface",
                );
                return;
            }
            with_states(&surface, |states| {
                states
                    .data_map
                    .get_or_insert_threadsafe(|| Watches(Mutex::new(Vec::new())))
                    .0
                    .lock()
                    .unwrap()
                    .push(watch);
            });
        }
    }
}

impl<D> Dispatch<Watch, WatchData, D> for State
where
    D: Dispatch<Watch, WatchData> + 'static,
{
    fn request(
        _: &mut D,
        _: &Client,
        _: &Watch,
        _: watch::Request,
        _: &WatchData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(_: &mut D, _: ClientId, resource: &Watch, data: &WatchData) {
        if data.0.is_alive() {
            with_states(&data.0, |states| {
                if let Some(watches) = states.data_map.get::<Watches>() {
                    watches.0.lock().unwrap().retain(|watch| watch != resource);
                }
            });
        }
    }
}
