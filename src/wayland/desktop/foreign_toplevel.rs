//! Taskbar projection of Halley's managed windows. Smithay owns the ext list;
//! this module owns wlr manager bindings, including stopped-manager handles.
use std::collections::BTreeMap;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::server::{
    ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1 as ExtHandle,
    ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 as ExtList,
};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    backend::ClientId,
    protocol::{wl_output::WlOutput, wl_seat::WlSeat},
};
use smithay::wayland::foreign_toplevel_list::{
    ForeignToplevelHandle, ForeignToplevelListGlobalData, ForeignToplevelListHandler,
    ForeignToplevelListState,
};
use wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::{self as handle, ZwlrForeignToplevelHandleV1 as Handle},
    zwlr_foreign_toplevel_manager_v1::{self as manager, ZwlrForeignToplevelManagerV1 as Manager},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub key: u64,
    pub title: String,
    pub app_id: String,
    pub activated: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub outputs: Vec<Output>,
    pub parent: Option<u64>,
}

pub enum Action {
    Activate(WlSeat),
    Close,
    Minimize(bool),
    Maximize(bool),
    Fullscreen(bool, Option<WlOutput>),
}

pub trait Handler: ForeignToplevelListHandler {
    /// Refresh before binds and requests, so stale handles cannot act on an
    /// unmapped window even when several requests share an event-loop turn.
    fn refresh_foreign_toplevels(&mut self);
    fn foreign_toplevel_state(&mut self) -> &mut State;
    fn foreign_toplevel_action(&mut self, key: u64, action: Action);
}

pub struct State {
    pub list: ForeignToplevelListState,
    entries: BTreeMap<u64, Entry>,
    bindings: Vec<Binding>,
    next_generation: u64,
}

struct Entry {
    snapshot: Snapshot,
    generation: u64,
    ext: ForeignToplevelHandle,
}
struct Binding {
    // Stop ends new announcements; existing handles remain valid.
    manager: Option<Manager>,
    handles: BTreeMap<u64, Instance>,
}
struct Instance {
    handle: Handle,
    sent: Option<Snapshot>,
    outputs: Vec<WlOutput>,
    parent: Option<Handle>,
}

#[derive(Clone, Copy)]
pub struct HandleData {
    key: u64,
    generation: u64,
}

impl State {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: Handler
            + GlobalDispatch<Manager, ()>
            + Dispatch<Manager, ()>
            + Dispatch<Handle, HandleData>
            + GlobalDispatch<ExtList, ForeignToplevelListGlobalData>,
    {
        display.create_global::<D, Manager, _>(3, ());
        Self {
            list: ForeignToplevelListState::new::<D>(display),
            entries: BTreeMap::new(),
            bindings: Vec::new(),
            next_generation: 0,
        }
    }

    pub fn sync<D>(&mut self, display: &DisplayHandle, snapshots: Vec<Snapshot>)
    where
        D: Handler + Dispatch<Handle, HandleData> + Dispatch<ExtHandle, ForeignToplevelHandle>,
    {
        let snapshots: BTreeMap<_, _> = snapshots.into_iter().map(|s| (s.key, s)).collect();
        let removed: Vec<_> = self
            .entries
            .keys()
            .filter(|key| !snapshots.contains_key(key))
            .copied()
            .collect();
        for key in removed {
            self.unmap(key);
        }
        for (key, snapshot) in snapshots {
            if let Some(entry) = self.entries.get_mut(&key) {
                if entry.snapshot.title != snapshot.title
                    || entry.snapshot.app_id != snapshot.app_id
                {
                    entry.ext.send_title(&snapshot.title);
                    entry.ext.send_app_id(&snapshot.app_id);
                    entry.ext.send_done();
                }
                entry.snapshot = snapshot;
            } else {
                self.next_generation = self
                    .next_generation
                    .checked_add(1)
                    .expect("toplevel generation overflow");
                let ext = self
                    .list
                    .new_toplevel::<D>(&snapshot.title, &snapshot.app_id);
                self.entries.insert(
                    key,
                    Entry {
                        snapshot,
                        generation: self.next_generation,
                        ext,
                    },
                );
            }
        }
        self.bindings.retain(|binding| {
            binding.manager.as_ref().is_some_and(Resource::is_alive)
                || binding.handles.values().any(|i| i.handle.is_alive())
        });
        for binding in &mut self.bindings {
            sync_binding::<D>(display, &self.entries, binding);
        }
    }

    /// Retire at the authoritative unmap boundary, including an unmap and
    /// remap dispatched in the same batch before the next snapshot.
    pub fn unmap(&mut self, key: u64) {
        if let Some(entry) = self.entries.remove(&key) {
            self.list.remove_toplevel(&entry.ext);
        }
        for binding in &mut self.bindings {
            if let Some(instance) = binding.handles.remove(&key)
                && instance.handle.is_alive()
            {
                instance.handle.closed();
            }
        }
    }

    pub fn output_bound(&mut self) {
        // Derive associations from actual wl_output resources, including late
        // binds and identity changes when a monitor reconnects with the same name.
        for binding in &mut self.bindings {
            update_binding(&self.entries, binding);
        }
    }
}

fn sync_binding<D: Dispatch<Handle, HandleData> + 'static>(
    display: &DisplayHandle,
    entries: &BTreeMap<u64, Entry>,
    binding: &mut Binding,
) {
    if let Some(manager) = binding.manager.as_ref().filter(|m| m.is_alive())
        && let Some(client) = manager.client()
    {
        // Allocate every handle before sending parents: a parent may follow
        // its child in model order, and handles belong to one manager binding.
        for (&key, entry) in entries {
            if binding.handles.contains_key(&key) {
                continue;
            }
            if let Ok(handle) = client.create_resource::<Handle, _, D>(
                display,
                manager.version(),
                HandleData {
                    key,
                    generation: entry.generation,
                },
            ) {
                manager.toplevel(&handle);
                binding.handles.insert(
                    key,
                    Instance {
                        handle,
                        sent: None,
                        outputs: Vec::new(),
                        parent: None,
                    },
                );
            }
        }
    }
    update_binding(entries, binding);
}

fn update_binding(entries: &BTreeMap<u64, Entry>, binding: &mut Binding) {
    let parents: BTreeMap<_, _> = binding
        .handles
        .iter()
        .filter(|(_, i)| i.handle.is_alive())
        .map(|(&key, i)| (key, i.handle.clone()))
        .collect();
    // Deactivation precedes activation when focus changes.
    let mut keys: Vec<_> = binding.handles.keys().copied().collect();
    keys.sort_by_key(|key| entries.get(key).is_some_and(|e| e.snapshot.activated));
    for key in keys {
        let Some(entry) = entries.get(&key) else {
            continue;
        };
        let instance = binding.handles.get_mut(&key).unwrap();
        let handle = &instance.handle;
        if !handle.is_alive() {
            continue;
        } // Keep a tombstone, never recreate it.
        let snapshot = &entry.snapshot;
        let old = instance.sent.as_ref();
        let mut changed = old.is_none();
        if old.is_none_or(|s| s.title != snapshot.title) {
            handle.title(snapshot.title.clone());
            changed = true;
        }
        if old.is_none_or(|s| s.app_id != snapshot.app_id) {
            handle.app_id(snapshot.app_id.clone());
            changed = true;
        }
        let states = state_bytes(snapshot, handle.version());
        if old.is_none_or(|s| state_bytes(s, handle.version()) != states) {
            handle.state(states);
            changed = true;
        }
        let mut outputs = Vec::new();
        if let Some(client) = handle.client() {
            for output in &snapshot.outputs {
                outputs.extend(output.client_outputs(&client));
            }
        }
        outputs.retain(Resource::is_alive);
        for output in &instance.outputs {
            if output.is_alive() && !outputs.contains(output) {
                handle.output_leave(output);
                changed = true;
            }
        }
        for output in &outputs {
            if !instance.outputs.contains(output) {
                handle.output_enter(output);
                changed = true;
            }
        }
        instance.outputs = outputs;
        let parent = snapshot.parent.and_then(|key| parents.get(&key)).cloned();
        if handle.version() >= 3 && (old.is_none() || instance.parent != parent) {
            // The protocol forbids an event solely because the client destroyed
            // its parent handle. A model parent change still sends null.
            let destroyed_parent = old.is_some_and(|s| s.parent == snapshot.parent)
                && instance.parent.as_ref().is_some_and(|p| !p.is_alive())
                && parent.is_none();
            if !destroyed_parent {
                handle.parent(parent.as_ref());
                changed = true;
            }
        }
        instance.parent = parent;
        instance.sent = Some(snapshot.clone());
        if changed {
            handle.done();
        }
    }
}

fn state_bytes(snapshot: &Snapshot, version: u32) -> Vec<u8> {
    [
        (snapshot.maximized, handle::State::Maximized),
        (snapshot.minimized, handle::State::Minimized),
        (snapshot.activated, handle::State::Activated),
        (
            snapshot.fullscreen && version >= 2,
            handle::State::Fullscreen,
        ),
    ]
    .into_iter()
    .filter(|(enabled, _)| *enabled)
    .flat_map(|(_, state)| (state as u32).to_ne_bytes())
    .collect()
}

impl<D> GlobalDispatch<Manager, (), D> for State
where
    D: Handler + Dispatch<Manager, ()> + Dispatch<Handle, HandleData>,
{
    fn bind(
        state: &mut D,
        display: &DisplayHandle,
        _client: &Client,
        resource: New<Manager>,
        _: &(),
        init: &mut DataInit<'_, D>,
    ) {
        state.refresh_foreign_toplevels();
        let manager = init.init(resource, ());
        let protocol = state.foreign_toplevel_state();
        let mut binding = Binding {
            manager: Some(manager),
            handles: BTreeMap::new(),
        };
        sync_binding::<D>(display, &protocol.entries, &mut binding);
        protocol.bindings.push(binding);
    }
}
impl<D: Handler> Dispatch<Manager, (), D> for State {
    fn request(
        state: &mut D,
        _: &Client,
        manager: &Manager,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        if let manager::Request::Stop = request {
            let protocol = state.foreign_toplevel_state();
            if let Some(binding) = protocol
                .bindings
                .iter_mut()
                .find(|b| b.manager.as_ref() == Some(manager))
            {
                binding.manager = None;
                manager.finished();
            }
        }
    }
    fn destroyed(state: &mut D, _: ClientId, manager: &Manager, _: &()) {
        for binding in &mut state.foreign_toplevel_state().bindings {
            if binding.manager.as_ref() == Some(manager) {
                binding.manager = None;
            }
        }
    }
}
impl<D: Handler> Dispatch<Handle, HandleData, D> for State {
    fn request(
        state: &mut D,
        _: &Client,
        handle: &Handle,
        request: handle::Request,
        data: &HandleData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        if matches!(request, handle::Request::Destroy) {
            return;
        }
        state.refresh_foreign_toplevels();
        if !state
            .foreign_toplevel_state()
            .entries
            .get(&data.key)
            .is_some_and(|entry| entry.generation == data.generation)
        {
            return;
        }
        let action = match request {
            handle::Request::Activate { seat } => Action::Activate(seat),
            handle::Request::Close => Action::Close,
            handle::Request::SetMaximized => Action::Maximize(true),
            handle::Request::UnsetMaximized => Action::Maximize(false),
            handle::Request::SetMinimized => Action::Minimize(true),
            handle::Request::UnsetMinimized => Action::Minimize(false),
            handle::Request::SetFullscreen { output } => Action::Fullscreen(true, output),
            handle::Request::UnsetFullscreen => Action::Fullscreen(false, None),
            handle::Request::SetRectangle { width, height, .. } => {
                if width < 0 || height < 0 {
                    handle.post_error(handle::Error::InvalidRectangle, "negative rectangle size");
                }
                // Optional animation hint: Halley's collapse destination is the
                // node position, rather than the taskbar's rectangle.
                return;
            }
            _ => return,
        };
        state.foreign_toplevel_action(data.key, action);
    }
}
