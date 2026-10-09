//! Client compatibility requests through the production upstream dispatcher.
#[path = "../src/wayland/clipboard_helper.rs"]
mod clipboard_helper;
#[path = "../src/wayland/surface_scale.rs"]
mod surface_scale;
#[path = "../src/wayland/dispatch.rs"]
mod upstream_protocols;

// Exercise the production stacking constraints with real xdg_toplevel parents.
#[allow(dead_code)]
#[path = "../src/window/stacking.rs"]
mod dialog_stacking;
use dialog_stacking as stacking;
#[path = "../src/window/dialog.rs"]
mod modal_dialog;
mod xwayland {
    pub fn is_override_redirect(_: &smithay::desktop::Window) -> bool {
        false
    }
    pub fn parent_window_from<'a>(
        _: impl Iterator<Item = &'a smithay::desktop::Window>,
        _: &smithay::desktop::Window,
    ) -> Option<smithay::desktop::Window> {
        None
    }
}

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, time::Duration};

use smithay::reexports::wayland_server::{Display, protocol::wl_surface::WlSurface};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
    with_states,
};
use smithay::wayland::content_type::{ContentTypeState, ContentTypeSurfaceCachedState};
use smithay::wayland::shell::xdg::dialog::{ToplevelDialogHint, XdgDialogHandler, XdgDialogState};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::single_pixel_buffer::{SinglePixelBufferState, get_single_pixel_buffer};
use smithay::wayland::xdg_toplevel_icon::{
    ToplevelIconCachedState, XdgToplevelIconHandler, XdgToplevelIconManager,
};
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1 as content_manager, wp_content_type_v1 as content,
};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as pixel;
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols::xdg::toplevel_icon::v1::client::{
    xdg_toplevel_icon_manager_v1 as icon_manager, xdg_toplevel_icon_v1 as icon,
};

use smithay::wayland::xdg_foreign::{XdgForeignHandler, XdgForeignState};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};

#[derive(Default)]
struct Observations {
    rgba: Option<[u32; 4]>,
    destroyed_buffers: usize,
    content_type: u32,
    icon_name: Option<String>,
    toplevels: Vec<WlSurface>,
    windows: Vec<smithay::desktop::Window>,
    parent_changes: usize,
    scale_request: Option<(WlSurface, f64)>,
    dialog_changes: Vec<(WlSurface, ToplevelDialogHint)>,
}

struct Server {
    compositor: CompositorState,
    observations: Arc<Mutex<Observations>>,
    shell: XdgShellState,
    foreign: XdgForeignState,
}

#[derive(Default)]
struct ClientData(CompositorClientState);
impl smithay::reexports::wayland_server::backend::ClientData for ClientData {}

impl CompositorHandler for Server {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }
    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a CompositorClientState {
        &client.get_data::<ClientData>().unwrap().0
    }
    fn commit(&mut self, surface: &WlSurface) {
        with_states(surface, |states| {
            let content_type = *states
                .cached_state
                .get::<ContentTypeSurfaceCachedState>()
                .current()
                .content_type() as u32;
            let icon_name = states
                .cached_state
                .get::<ToplevelIconCachedState>()
                .current()
                .icon_name()
                .map(str::to_owned);
            let mut observations = self.observations.lock().unwrap();
            observations.content_type = content_type;
            observations.icon_name = icon_name;
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            if let Some(BufferAssignment::NewBuffer(buffer)) = attributes.current().buffer.as_ref()
                && let Ok(pixel) = get_single_pixel_buffer(buffer)
            {
                observations.rgba = Some([pixel.r, pixel.g, pixel.b, pixel.a]);
            }
        });
    }
}
impl XdgToplevelIconHandler for Server {}
impl XdgDialogHandler for Server {
    fn dialog_hint_changed(&mut self, toplevel: ToplevelSurface, hint: ToplevelDialogHint) {
        self.observations
            .lock()
            .unwrap()
            .dialog_changes
            .push((toplevel.wl_surface().clone(), hint));
    }
}
impl XdgForeignHandler for Server {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.foreign
    }
}
impl XdgShellHandler for Server {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.shell
    }
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.observations.lock().unwrap().windows.push(
            smithay::desktop::Window::new_wayland_window(surface.clone()),
        );
        self.observations
            .lock()
            .unwrap()
            .toplevels
            .push(surface.wl_surface().clone());
    }
    fn parent_changed(&mut self, _: ToplevelSurface) {
        self.observations.lock().unwrap().parent_changes += 1;
    }
    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}
    fn grab(
        &mut self,
        _: PopupSurface,
        _: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _: smithay::utils::Serial,
    ) {
    }
    fn reposition_request(&mut self, _: PopupSurface, _: PositionerState, _: u32) {}
}
impl BufferHandler for Server {
    fn buffer_destroyed(
        &mut self,
        _: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
        self.observations.lock().unwrap().destroyed_buffers += 1;
    }
}
upstream_protocols::delegate_upstream_protocols!(Server);

#[derive(Default)]
struct Client {
    globals: HashMap<String, (u32, u32)>,
    integer_scales: Vec<i32>,
    fractional_scales: Vec<u32>,
    foreign_handles: Vec<String>,
    invalid_imports: usize,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.insert(interface, (name, version));
        }
    }
}
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
impl Dispatch<wl_surface::WlSurface, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.integer_scales.push(factor);
        }
    }
}
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1 as fractional_manager, wp_fractional_scale_v1 as fractional,
};
delegate_noop!(Client: ignore fractional_manager::WpFractionalScaleManagerV1);
impl Dispatch<fractional::WpFractionalScaleV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &fractional::WpFractionalScaleV1,
        event: fractional::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let fractional::Event::PreferredScale { scale } = event {
            state.fractional_scales.push(scale);
        }
    }
}
impl smithay::wayland::fractional_scale::FractionalScaleHandler for Server {}
delegate_noop!(Client: ignore wl_buffer::WlBuffer);
delegate_noop!(Client: ignore pixel::WpSinglePixelBufferManagerV1);
delegate_noop!(Client: ignore content_manager::WpContentTypeManagerV1);
delegate_noop!(Client: ignore content::WpContentTypeV1);
delegate_noop!(Client: ignore icon_manager::XdgToplevelIconManagerV1);
delegate_noop!(Client: ignore icon::XdgToplevelIconV1);
delegate_noop!(Client: ignore xdg_wm_base::XdgWmBase);
delegate_noop!(Client: ignore xdg_surface::XdgSurface);
delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);

delegate_noop!(Client: ignore zxdg_exporter_v2::ZxdgExporterV2);
delegate_noop!(Client: ignore zxdg_importer_v2::ZxdgImporterV2);
impl Dispatch<zxdg_exported_v2::ZxdgExportedV2, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zxdg_exported_v2::ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            state.foreign_handles.push(handle);
        }
    }
}
impl Dispatch<zxdg_imported_v2::ZxdgImportedV2, ()> for Client {
    fn event(
        state: &mut Self,
        _: &zxdg_imported_v2::ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_imported_v2::Event::Destroyed = event {
            state.invalid_imports += 1;
        }
    }
}

struct Fixture {
    display_handle: smithay::reexports::wayland_server::DisplayHandle,
    observations: Arc<Mutex<Observations>>,
    state: Client,
    queue: EventQueue<Client>,
    registry: wl_registry::WlRegistry,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let mut display = Display::<Server>::new().unwrap();
        let mut dh = display.handle();
        let compositor = CompositorState::new_v6::<Server>(&dh);
        let _fractional =
            smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Server>(&dh);
        let _pixels = SinglePixelBufferState::new::<Server>(&dh);
        let _content = ContentTypeState::new::<Server>(&dh);
        let _icons = XdgToplevelIconManager::new::<Server>(&dh);
        let shell = XdgShellState::new::<Server>(&dh);
        let _dialogs = XdgDialogState::new::<Server>(&dh);
        let foreign = XdgForeignState::new::<Server>(&dh);
        dh.insert_client(server_socket, Arc::new(ClientData::default()))
            .unwrap();
        let observations = Arc::new(Mutex::new(Observations::default()));
        let mut server = Server {
            compositor,
            observations: observations.clone(),
            shell,
            foreign,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                display.dispatch_clients(&mut server).unwrap();
                let request = server.observations.lock().unwrap().scale_request.take();
                if let Some((surface, scale)) = request {
                    let output = smithay::output::Output::new(
                        "scale-test".into(),
                        smithay::output::PhysicalProperties {
                            size: (0, 0).into(),
                            subpixel: smithay::output::Subpixel::Unknown,
                            make: "test".into(),
                            model: "test".into(),
                            serial_number: "test".into(),
                        },
                    );
                    output.change_current_state(
                        None,
                        None,
                        Some(smithay::output::Scale::Fractional(scale)),
                        None,
                    );
                    surface_scale::send_tree(&surface, &output);
                }
                display.flush_clients().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        let connection = Connection::from_socket(client_socket).unwrap();
        let mut queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut state = Client::default();
        queue.roundtrip(&mut state).unwrap();
        Self {
            display_handle: dh,
            observations,
            state,
            queue,
            registry,
            stop,
            worker: Some(worker),
        }
    }
    fn sync(&mut self) {
        self.queue.roundtrip(&mut self.state).unwrap();
    }
    fn surface(&self) -> wl_surface::WlSurface {
        let compositor: wl_compositor::WlCompositor = self.registry.bind(
            self.state.globals["wl_compositor"].0,
            6,
            &self.queue.handle(),
            (),
        );
        compositor.create_surface(&self.queue.handle(), ())
    }

    fn toplevel(&mut self, app_id: &str) -> (xdg_toplevel::XdgToplevel, WlSurface) {
        let (toplevel, server_surface, _) = self.toplevel_with_surface(app_id);
        (toplevel, server_surface)
    }

    fn toplevel_with_surface(
        &mut self,
        app_id: &str,
    ) -> (xdg_toplevel::XdgToplevel, WlSurface, wl_surface::WlSurface) {
        let shell: xdg_wm_base::XdgWmBase = self.registry.bind(
            self.state.globals["xdg_wm_base"].0,
            1,
            &self.queue.handle(),
            (),
        );
        let surface = self.surface();
        let xdg_surface = shell.get_xdg_surface(&surface, &self.queue.handle(), ());
        let toplevel = xdg_surface.get_toplevel(&self.queue.handle(), ());
        toplevel.set_app_id(app_id.into());
        self.sync();
        let server_surface = self
            .observations
            .lock()
            .unwrap()
            .toplevels
            .last()
            .unwrap()
            .clone();
        (toplevel, server_surface, surface)
    }
}

const CLIPBOARD_APP_ID: &str = "io.github.bugaevc.wl-clipboard";

#[test]
fn clipboard_helper_remembers_the_exact_caller_with_two_or_three_windows() {
    for count in [2, 3] {
        let mut f = Fixture::new();
        let windows = (0..count)
            .map(|_| f.toplevel("kitty").1)
            .collect::<Vec<_>>();
        for caller in &windows {
            assert!(!clipboard_helper::is_helper(caller));
            assert!(clipboard_helper::saved_focus(caller).is_none());
            let (_, helper) = f.toplevel(CLIPBOARD_APP_ID);
            assert!(clipboard_helper::is_helper(&helper));
            clipboard_helper::remember_focus(&helper, Some(caller), None);
            assert_eq!(
                clipboard_helper::saved_focus(&helper)
                    .unwrap()
                    .window
                    .as_ref(),
                Some(caller)
            );
        }
    }
}

#[test]
fn overlapping_clipboard_helpers_return_to_the_original_terminal() {
    let mut f = Fixture::new();
    let (_, caller) = f.toplevel("kitty");
    let (first_toplevel, first) = f.toplevel(CLIPBOARD_APP_ID);
    clipboard_helper::remember_focus(&first, Some(&caller), None);
    let (_, second) = f.toplevel(CLIPBOARD_APP_ID);
    clipboard_helper::remember_focus(&second, Some(&first), None);
    first_toplevel.destroy();
    f.sync();
    assert_eq!(
        clipboard_helper::saved_focus(&second).unwrap().window,
        Some(caller)
    );
}

#[test]
fn clipboard_helpers_without_a_caller_do_not_invent_a_successor() {
    let mut f = Fixture::new();
    let (_, helper) = f.toplevel(CLIPBOARD_APP_ID);
    clipboard_helper::remember_focus(&helper, None, None);
    let saved = clipboard_helper::saved_focus(&helper).unwrap();
    assert!(saved.window.is_none());
    assert!(saved.layer.is_none());
}

#[test]
fn clipboard_return_identity_survives_metadata_changes_until_teardown() {
    let mut f = Fixture::new();
    let (_, caller) = f.toplevel("kitty");
    let (helper_toplevel, helper) = f.toplevel(CLIPBOARD_APP_ID);
    clipboard_helper::remember_focus(&helper, Some(&caller), None);
    helper_toplevel.set_app_id("changed-after-mapping".into());
    f.sync();
    assert!(!clipboard_helper::is_helper(&helper));
    assert_eq!(
        clipboard_helper::saved_focus(&helper).unwrap().window,
        Some(caller)
    );
}

#[test]
fn content_type_applies_on_commit_and_resets_when_destroyed() {
    let mut f = Fixture::new();
    let (name, version) = f.state.globals["wp_content_type_manager_v1"];
    assert_eq!(version, 1);
    let manager: content_manager::WpContentTypeManagerV1 =
        f.registry.bind(name, 1, &f.queue.handle(), ());
    let surface = f.surface();
    let content = manager.get_surface_content_type(&surface, &f.queue.handle(), ());
    content.set_content_type(content::Type::Game);
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().content_type,
        content::Type::None as u32
    );
    surface.commit();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().content_type,
        content::Type::Game as u32
    );
    content.destroy();
    surface.commit();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().content_type,
        content::Type::None as u32
    );
}

#[test]
fn toplevel_icon_metadata_applies_on_commit_and_can_be_cleared() {
    let mut f = Fixture::new();
    let (name, version) = f.state.globals["xdg_toplevel_icon_manager_v1"];
    assert_eq!(version, 1);
    let manager: icon_manager::XdgToplevelIconManagerV1 =
        f.registry.bind(name, 1, &f.queue.handle(), ());
    let shell: xdg_wm_base::XdgWmBase =
        f.registry
            .bind(f.state.globals["xdg_wm_base"].0, 1, &f.queue.handle(), ());
    let surface = f.surface();
    let xdg_surface = shell.get_xdg_surface(&surface, &f.queue.handle(), ());
    let toplevel = xdg_surface.get_toplevel(&f.queue.handle(), ());
    let icon = manager.create_icon(&f.queue.handle(), ());
    icon.set_name("org.example.Game".into());
    manager.set_icon(&toplevel, Some(&icon));
    f.sync();
    assert!(f.observations.lock().unwrap().icon_name.is_none());
    surface.commit();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().icon_name.as_deref(),
        Some("org.example.Game")
    );
    manager.set_icon(&toplevel, None);
    surface.commit();
    f.sync();
    assert!(f.observations.lock().unwrap().icon_name.is_none());
    icon.destroy();
    manager.destroy();
    f.sync();
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn single_pixel_buffers_preserve_color_and_destroy_cleanly() {
    let mut f = Fixture::new();
    let (name, version) = f.state.globals["wp_single_pixel_buffer_manager_v1"];
    assert_eq!(version, 1);
    let manager: pixel::WpSinglePixelBufferManagerV1 =
        f.registry.bind(name, 1, &f.queue.handle(), ());
    let color = [u32::MAX, 0, u32::MAX / 2, u32::MAX];
    let buffer = manager.create_u32_rgba_buffer(
        color[0],
        color[1],
        color[2],
        color[3],
        &f.queue.handle(),
        (),
    );
    let surface = f.surface();
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    f.sync();
    assert_eq!(f.observations.lock().unwrap().rgba, Some(color));
    surface.attach(None, 0, 0);
    surface.commit();
    buffer.destroy();
    manager.destroy();
    f.sync();
    assert_eq!(f.observations.lock().unwrap().destroyed_buffers, 1);
}

#[test]
fn display_scale_preferences_update_after_fractional_and_integer_changes() {
    let mut f = Fixture::new();
    let compositor: wl_compositor::WlCompositor =
        f.registry
            .bind(f.state.globals["wl_compositor"].0, 6, &f.queue.handle(), ());
    let surface = compositor.create_surface(&f.queue.handle(), ());
    let shell: xdg_wm_base::XdgWmBase =
        f.registry
            .bind(f.state.globals["xdg_wm_base"].0, 1, &f.queue.handle(), ());
    let xdg_surface = shell.get_xdg_surface(&surface, &f.queue.handle(), ());
    let _toplevel = xdg_surface.get_toplevel(&f.queue.handle(), ());
    let manager: fractional_manager::WpFractionalScaleManagerV1 = f.registry.bind(
        f.state.globals["wp_fractional_scale_manager_v1"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let _fractional = manager.get_fractional_scale(&surface, &f.queue.handle(), ());
    f.sync();
    let server_surface = f
        .observations
        .lock()
        .unwrap()
        .toplevels
        .last()
        .unwrap()
        .clone();
    for (scale, integer, fractional) in [(1.5, 2, 180), (2.0, 2, 240), (1.0, 1, 120)] {
        f.observations.lock().unwrap().scale_request = Some((server_surface.clone(), scale));
        // One roundtrip processes the request; a second receives any notifications flushed after its sync callback.
        f.sync();
        f.sync();
        assert_eq!(f.state.integer_scales.last(), Some(&integer));
        assert_eq!(f.state.fractional_scales.last(), Some(&fractional));
    }
}

#[test]
fn native_dialogs_remain_above_raised_parents_without_changing_activation_or_location() {
    let mut fixture = Fixture::new();
    let (parent, _) = fixture.toplevel("parent");
    let (child, _) = fixture.toplevel("dialog");
    let (grandchild, _) = fixture.toplevel("nested-dialog");
    let (_, _) = fixture.toplevel("unrelated");
    child.set_parent(Some(&parent));
    grandchild.set_parent(Some(&child));
    fixture.sync();
    let windows = fixture.observations.lock().unwrap().windows.clone();
    let mut space = smithay::desktop::Space::default();
    for (i, window) in windows.iter().enumerate() {
        space.map_element(window.clone(), (i as i32 * 10, i as i32 * 20), false);
    }
    // Equivalent to raising/focusing Firefox while its dialogs are still open.
    space.raise_element(&windows[0], true);
    let mut order = space.elements().cloned().collect::<Vec<_>>();
    assert!(dialog_stacking::sort_above_parents(
        &space,
        &mut order,
        |w| Some(w)
    ));
    assert_eq!(
        order,
        [
            windows[3].clone(),
            windows[0].clone(),
            windows[1].clone(),
            windows[2].clone()
        ]
    );
    for window in &order {
        space.raise_element(window, false);
    }
    assert!(windows[0].toplevel().unwrap().with_pending_state(|state| state.states.contains(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated)));
    for (i, window) in windows.iter().enumerate() {
        assert_eq!(
            space.element_location(window),
            Some((i as i32 * 10, i as i32 * 20).into())
        );
    }
    assert!(!dialog_stacking::sort_above_parents(
        &space,
        &mut order,
        |w| Some(w)
    ));
    // Another application can cover the complete family.
    space.raise_element(&windows[3], false);
    order = space.elements().cloned().collect();
    assert!(!dialog_stacking::sort_above_parents(
        &space,
        &mut order,
        |w| Some(w)
    ));
    assert_eq!(order.last(), Some(&windows[3]));
    // A late parent change repairs the relationship after presentation sorting.
    grandchild.set_parent(None);
    child.set_parent(Some(&grandchild));
    fixture.sync();
    assert!(dialog_stacking::sort_above_parents(
        &space,
        &mut order,
        |w| Some(w)
    ));
    let child_pos = order.iter().position(|w| w == &windows[1]).unwrap();
    let grandchild_pos = order.iter().position(|w| w == &windows[2]).unwrap();
    assert!(grandchild_pos < child_pos);
    parent.destroy();
    child.destroy();
    grandchild.destroy();
}

#[test]
fn portal_dialog_imports_a_parent_from_another_client_and_stacks_above_it() {
    let mut fixture = Fixture::new();
    let (_, _, parent_surface) = fixture.toplevel_with_surface("firefox");
    let exporter: zxdg_exporter_v2::ZxdgExporterV2 = fixture.registry.bind(
        fixture.state.globals["zxdg_exporter_v2"].0,
        1,
        &fixture.queue.handle(),
        (),
    );
    let exported = exporter.export_toplevel(&parent_surface, &fixture.queue.handle(), ());
    fixture.sync();
    let handle = fixture.state.foreign_handles.last().unwrap().clone();

    // The portal owns a different Wayland connection and cannot use
    // xdg_toplevel.set_parent with Firefox's object ID.
    let (client_socket, server_socket) = UnixStream::pair().unwrap();
    fixture
        .display_handle
        .insert_client(server_socket, Arc::new(ClientData::default()))
        .unwrap();
    let connection = Connection::from_socket(client_socket).unwrap();
    let mut queue = connection.new_event_queue();
    let registry = connection.display().get_registry(&queue.handle(), ());
    let mut portal = Client::default();
    queue.roundtrip(&mut portal).unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(portal.globals["wl_compositor"].0, 6, &queue.handle(), ());
    let shell: xdg_wm_base::XdgWmBase =
        registry.bind(portal.globals["xdg_wm_base"].0, 1, &queue.handle(), ());
    let importer: zxdg_importer_v2::ZxdgImporterV2 =
        registry.bind(portal.globals["zxdg_importer_v2"].0, 1, &queue.handle(), ());
    let dialog_surface = compositor.create_surface(&queue.handle(), ());
    let xdg_surface = shell.get_xdg_surface(&dialog_surface, &queue.handle(), ());
    let dialog = xdg_surface.get_toplevel(&queue.handle(), ());
    dialog.set_app_id("xdg-desktop-portal-gtk".into());
    queue.roundtrip(&mut portal).unwrap();
    let imported = importer.import_toplevel(handle, &queue.handle(), ());
    imported.set_parent_of(&dialog_surface);
    queue.roundtrip(&mut portal).unwrap();
    let windows = fixture.observations.lock().unwrap().windows.clone();
    assert_eq!(windows.len(), 2);
    assert_eq!(
        windows[1].toplevel().unwrap().parent(),
        Some(parent_surface_id(&windows[0]))
    );
    assert_eq!(fixture.observations.lock().unwrap().parent_changes, 1);
    let mut space = smithay::desktop::Space::default();
    space.map_element(windows[1].clone(), (0, 0), false);
    space.map_element(windows[0].clone(), (0, 0), false);
    let mut order = space.elements().cloned().collect::<Vec<_>>();
    assert!(dialog_stacking::sort_above_parents(
        &space,
        &mut order,
        |window| Some(window)
    ));
    assert_eq!(order, windows);

    // Invalid handles cannot create a relationship. Releasing the owning
    // import clears the relationship and notifies the compositor.
    let invalid = importer.import_toplevel("unknown-handle".into(), &queue.handle(), ());
    invalid.set_parent_of(&dialog_surface);
    queue.roundtrip(&mut portal).unwrap();
    assert_eq!(portal.invalid_imports, 1);
    imported.destroy();
    queue.roundtrip(&mut portal).unwrap();
    assert!(windows[1].toplevel().unwrap().parent().is_none());
    assert_eq!(fixture.observations.lock().unwrap().parent_changes, 2);
    let imported = importer.import_toplevel(
        fixture.state.foreign_handles.last().unwrap().clone(),
        &queue.handle(),
        (),
    );
    imported.set_parent_of(&dialog_surface);
    queue.roundtrip(&mut portal).unwrap();
    assert!(windows[1].toplevel().unwrap().parent().is_some());
    exported.destroy();
    fixture.sync();
    queue.roundtrip(&mut portal).unwrap();
    assert!(windows[1].toplevel().unwrap().parent().is_none());
    let revoked = importer.import_toplevel(
        fixture.state.foreign_handles.last().unwrap().clone(),
        &queue.handle(),
        (),
    );
    revoked.set_parent_of(&dialog_surface);
    queue.roundtrip(&mut portal).unwrap();
    assert_eq!(portal.invalid_imports, 2);
    assert!(windows[1].toplevel().unwrap().parent().is_none());
}

fn parent_surface_id(window: &smithay::desktop::Window) -> WlSurface {
    window.toplevel().unwrap().wl_surface().clone()
}

fn dialog_manager(f: &Fixture) -> xdg_wm_dialog_v1::XdgWmDialogV1 {
    let (name, version) = f.state.globals["xdg_wm_dialog_v1"];
    assert_eq!(version, 1);
    f.registry.bind(name, 1, &f.queue.handle(), ())
}

#[test]
fn xdg_dialog_hints_toggle_immediately_and_destroy_removes_them() {
    let mut f = Fixture::new();
    let (parent, _) = f.toplevel("parent");
    let (child, surface) = f.toplevel("child");
    child.set_parent(Some(&parent));
    let manager = dialog_manager(&f);
    let dialog = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    dialog.set_modal();
    dialog.set_modal(); // Repeating the same hint is not another transition.
    dialog.unset_modal();
    dialog.set_modal();
    dialog.destroy();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().dialog_changes,
        vec![
            (surface.clone(), ToplevelDialogHint::Dialog),
            (surface.clone(), ToplevelDialogHint::Modal),
            (surface.clone(), ToplevelDialogHint::Dialog),
            (surface.clone(), ToplevelDialogHint::Modal),
            (surface.clone(), ToplevelDialogHint::Unknown),
        ]
    );
    // Destruction releases the one-object-per-toplevel restriction.
    let replacement = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    replacement.destroy();
    f.sync();
}

#[test]
fn destroying_xdg_dialog_manager_keeps_existing_dialog_usable() {
    let mut f = Fixture::new();
    let (child, surface) = f.toplevel("child");
    let manager = dialog_manager(&f);
    let dialog = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    manager.destroy();
    dialog.set_modal();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().dialog_changes.last(),
        Some(&(surface, ToplevelDialogHint::Modal))
    );
}

#[test]
fn duplicate_xdg_dialog_objects_disconnect_only_the_offending_client() {
    let mut f = Fixture::new();
    let (child, _) = f.toplevel("child");
    let manager = dialog_manager(&f);
    let _first = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    let _duplicate = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    let error = match f.queue.roundtrip(&mut f.state).unwrap_err() {
        wayland_client::DispatchError::Backend(
            wayland_client::backend::WaylandError::Protocol(error),
        ) => error,
        error => panic!("expected protocol error, got {error:?}"),
    };
    assert_eq!(error.object_interface, "xdg_wm_dialog_v1");
    assert_eq!(error.code, 0);
    // The server remains usable by another connection.
    let (client, server) = UnixStream::pair().unwrap();
    f.display_handle
        .clone()
        .insert_client(server, Arc::new(ClientData::default()))
        .unwrap();
    let connection = Connection::from_socket(client).unwrap();
    let mut queue = connection.new_event_queue();
    let _registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = Client::default();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.globals["xdg_wm_dialog_v1"].1, 1);
}

#[test]
fn xdg_dialog_is_inert_after_its_toplevel_is_destroyed() {
    let mut f = Fixture::new();
    let (child, surface) = f.toplevel("child");
    let manager = dialog_manager(&f);
    let dialog = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    f.sync();
    child.destroy();
    dialog.set_modal();
    dialog.unset_modal();
    dialog.destroy();
    f.sync();
    assert_eq!(
        f.observations.lock().unwrap().dialog_changes,
        vec![(surface, ToplevelDialogHint::Dialog)]
    );
}

#[test]
fn modal_focus_is_scoped_to_mapped_eligible_descendants_and_tracks_hint_lifetime() {
    let mut f = Fixture::new();
    let (parent, _) = f.toplevel("parent");
    let (child, child_surface) = f.toplevel("child");
    let (_, _) = f.toplevel("unrelated");
    child.set_parent(Some(&parent));
    let dialog = dialog_manager(&f).get_xdg_dialog(&child, &f.queue.handle(), ());
    f.sync();
    let windows = f.observations.lock().unwrap().windows.clone();
    let mut space = smithay::desktop::Space::default();
    for window in &windows {
        space.map_element(window.clone(), (0, 0), false);
    }
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
    dialog.set_modal();
    f.sync();
    assert_eq!(
        modal_dialog::focus_target(&space, &windows[0], |_| true),
        Some(windows[1].clone())
    );
    assert!(modal_dialog::focus_target(&space, &windows[2], |_| true).is_none());
    assert!(modal_dialog::focus_target(&space, &windows[1], |_| true).is_none());
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| false).is_none());
    space.unmap_elem(&windows[1]);
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
    space.map_element(windows[1].clone(), (0, 0), false);
    child.set_parent(None);
    f.sync();
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
    child.set_parent(Some(&parent));
    dialog.unset_modal();
    f.sync();
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
    dialog.set_modal();
    dialog.destroy();
    f.sync();
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
    assert_eq!(
        f.observations.lock().unwrap().dialog_changes.last(),
        Some(&(child_surface, ToplevelDialogHint::Unknown))
    );
}

#[test]
fn modal_focus_prefers_the_frontmost_nested_dialog_and_skips_ineligible_children() {
    let mut f = Fixture::new();
    let (parent, _) = f.toplevel("parent");
    let (child, _) = f.toplevel("child");
    let (nested, _) = f.toplevel("nested");
    child.set_parent(Some(&parent));
    nested.set_parent(Some(&child));
    let manager = dialog_manager(&f);
    let dialog = manager.get_xdg_dialog(&child, &f.queue.handle(), ());
    let nested_dialog = manager.get_xdg_dialog(&nested, &f.queue.handle(), ());
    dialog.set_modal();
    nested_dialog.set_modal();
    f.sync();
    let windows = f.observations.lock().unwrap().windows.clone();
    let mut space = smithay::desktop::Space::default();
    for window in &windows {
        space.map_element(window.clone(), (0, 0), false);
    }
    assert_eq!(
        modal_dialog::focus_target(&space, &windows[0], |_| true),
        Some(windows[2].clone())
    );
    assert_eq!(
        modal_dialog::focus_target(&space, &windows[1], |_| true),
        Some(windows[2].clone())
    );
    assert_eq!(
        modal_dialog::focus_target(&space, &windows[0], |w| w != &windows[2]),
        Some(windows[1].clone())
    );
    nested_dialog.destroy();
    f.sync();
    assert_eq!(
        modal_dialog::focus_target(&space, &windows[0], |_| true),
        Some(windows[1].clone())
    );
    child.destroy();
    f.sync();
    // A dead role must not redirect focus while its wl_surface still exists.
    assert!(modal_dialog::focus_target(&space, &windows[0], |_| true).is_none());
}
