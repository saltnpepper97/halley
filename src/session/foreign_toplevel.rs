//! Foreign taskbar actions use the same model and presentation paths as keys,
//! client requests and titlebar controls; no second window model is maintained.
use halley_core::field::NodeId;
use smithay::input::Seat;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::foreign_toplevel_list::{
    ForeignToplevelListHandler, ForeignToplevelListState,
};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
};

use super::{Session, SessionDriver};
use crate::wayland::foreign_toplevel::{Action, HandleData, Handler, Snapshot, State};

fn snapshots<D: SessionDriver>(session: &Session<D>) -> Vec<Snapshot> {
    let focus = session
        .seat
        .get_keyboard()
        .and_then(|k| k.current_focus())
        .and_then(|focus| {
            focus.wl_surface().map(|s| {
                let root = crate::wayland::compositor::root_surface(s.as_ref());
                session
                    .wayland
                    .popup_manager
                    .find_popup(&root)
                    .and_then(|popup| smithay::desktop::find_popup_root_surface(&popup).ok())
                    .or_else(|| {
                        crate::xwayland::pointer_constraint_proxy_authority(
                            &session.wayland.space,
                            &root,
                        )
                    })
                    .unwrap_or(root)
            })
        });
    session
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && record.window.alive()
                && !crate::xwayland::is_override_redirect(&record.window)
        })
        .map(|record| {
            let (title, app_id, parent) = if let Some(toplevel) = record.window.toplevel() {
                with_states(toplevel.wl_surface(), |states| {
                    let role = states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    (
                        role.title.clone().unwrap_or_default(),
                        role.app_id.clone().unwrap_or_default(),
                        role.parent
                            .as_ref()
                            .and_then(|surface| session.nodes.id_for_surface(surface)),
                    )
                })
            } else {
                let (title, app_id) = crate::xwayland::metadata(&record.window).unwrap_or_default();
                #[cfg(feature = "xwayland")]
                let parent = record
                    .window
                    .x11_surface()
                    .and_then(|s| s.is_transient_for())
                    .and_then(|xid| {
                        session.nodes.records().find(|parent| {
                            parent.attached
                                && parent
                                    .window
                                    .x11_surface()
                                    .is_some_and(|s| s.window_id() == xid)
                        })
                    })
                    .map(|parent| parent.id);
                #[cfg(not(feature = "xwayland"))]
                let parent = None;
                (title, app_id, parent)
            };
            let output = crate::wayland::window_output_name(&record.window)
                .unwrap_or_else(|| record.output.clone());
            Snapshot {
                key: record.id.as_u64(),
                title,
                app_id,
                activated: focus.as_ref() == Some(&record.surface)
                    && !session.session_lock.active(),
                minimized: record.collapsed,
                maximized: session.maximize.contains(&record.surface),
                fullscreen: session.fullscreen.is_fullscreen_or_pending(&record.surface),
                outputs: session
                    .wayland
                    .space
                    .outputs()
                    .filter(|o| o.name() == output)
                    .cloned()
                    .collect(),
                parent: parent.map(NodeId::as_u64),
            }
        })
        .collect()
}

pub(crate) fn sync<D: SessionDriver>(session: &mut Session<D>) {
    let snapshots = snapshots(session);
    let display = session.wayland.display_handle.clone();
    session
        .wayland
        .foreign_toplevel_state
        .sync::<Session<D>>(&display, snapshots);
}

impl<D: SessionDriver> ForeignToplevelListHandler for Session<D> {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        sync(self);
        &mut self.wayland.foreign_toplevel_state.list
    }
}

impl<D: SessionDriver> Handler for Session<D> {
    fn refresh_foreign_toplevels(&mut self) {
        sync(self);
    }
    fn foreign_toplevel_state(&mut self) -> &mut State {
        &mut self.wayland.foreign_toplevel_state
    }
    fn foreign_toplevel_action(&mut self, key: u64, action: Action) {
        // An external taskbar cannot bypass the lock or cancel an interactive grab.
        if self.session_lock.active()
            || !matches!(self.interactions.grab, crate::input::grab::Grab::None)
        {
            return;
        }
        let id = NodeId::new(key);
        let Some(record) = self
            .nodes
            .record(id)
            .filter(|r| r.attached && r.window.alive())
            .cloned()
        else {
            return;
        };
        match action {
            Action::Activate(seat) => {
                if !seat.is_alive()
                    || Seat::<Self>::from_resource(&seat).as_ref() != Some(&self.seat)
                {
                    return;
                }
                super::activate_node(self, id, false);
            }
            Action::Close => super::request_window_close(self, &record.window),
            Action::Minimize(desired) => {
                if record.collapsed == desired {
                    return;
                }
                if desired {
                    crate::nodes::collapse(self, id, SERIAL_COUNTER.next_serial());
                } else {
                    super::activate_node(self, id, false);
                }
            }
            Action::Maximize(desired) => {
                if desired && !self.maximize.contains(&record.surface) {
                    super::activate_node(self, id, false);
                }
                super::set_surface_field_maximized(self, &record.surface, desired);
            }
            Action::Fullscreen(desired, _output_hint) => {
                // Output is a hint; keep Halley's existing output ownership.
                if desired == self.fullscreen.is_fullscreen_or_pending(&record.surface) {
                    return;
                }
                if desired {
                    super::activate_node(self, id, false);
                }
                let Some(record) = self.nodes.record(id).filter(|r| !r.collapsed).cloned() else {
                    return;
                };
                super::set_record_fullscreen(self, record, desired);
            }
        }
        self.request_redraw();
    }
}

smithay::reexports::wayland_server::delegate_global_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrForeignToplevelManagerV1: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrForeignToplevelManagerV1: ()] => State);
smithay::reexports::wayland_server::delegate_dispatch!(@<D: SessionDriver> Session<D>: [ZwlrForeignToplevelHandleV1: HandleData] => State);
