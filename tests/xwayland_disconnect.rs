//! Real nested-session regression: kill only the disposable session's XWayland.
//! Requires a running Wayland desktop and XWayland; run explicitly with --ignored.
#![cfg(all(feature = "xwayland", feature = "winit"))]

use std::collections::HashMap;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use halley_ipc::{ClusterRequest, ClusterTarget, NodeRequest, NodeSelector, Request, Response};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as pixel;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct Fixture {
    path: PathBuf,
    process: Child,
    ipc: Option<halley_ipc::Connection>,
    display: String,
    xwayland: u32,
}

fn wait_for<T>(description: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = probe() {
            return result;
        }
        assert!(Instant::now() < deadline, "timed out: {description}");
        thread::sleep(Duration::from_millis(20));
    }
}

impl Fixture {
    fn new(layout: &str) -> Self {
        Self::with_config(layout, "")
    }

    fn with_config(layout: &str, extra: &str) -> Self {
        let host_runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        let host_display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").unwrap());
        let host_display = if host_display.is_absolute() {
            host_display
        } else {
            host_runtime.join(host_display)
        };
        let path = std::env::temp_dir().join(format!(
            "halley-xwayland-disconnect-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let config = path.join("halley.rune");
        fs::write(&config, format!(
            "autostart:\n  cluster:\n    name \"Disconnect regression\"\n    layout \"{layout}\"\n    members []\n  end\nend\nkeybinds:\nend\n{extra}"
        )).unwrap();
        let log = File::create(path.join("console.log")).unwrap();
        let binary = std::env::var_os("HALLEY_TEST_BINARY")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_halley").into());
        let process = Command::new(binary)
            .args(["--winit", "--config"])
            .arg(config)
            .env("WAYLAND_DISPLAY", host_display)
            .env("XDG_RUNTIME_DIR", &path)
            .env("XDG_STATE_HOME", path.join("state"))
            .env("XDG_CONFIG_HOME", path.join("config"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        // Construct the guard before readiness waits so a failed assertion
        // cannot leave a nested compositor or its XWayland running.
        let socket = path.join("halley/halley.sock");
        let mut fixture = Fixture {
            path,
            process,
            ipc: None,
            display: String::new(),
            xwayland: 0,
        };
        fixture.ipc = Some(wait_for("nested IPC", || {
            halley_ipc::Connection::connect_to(&socket).ok()
        }));
        fixture
            .ipc
            .as_ref()
            .unwrap()
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        fixture.display = wait_for("nested XWayland readiness", || {
            let log = fs::read_to_string(fixture.path.join("console.log")).ok()?;
            log.lines().find_map(|line| {
                line.split_once("xwayland: ready, DISPLAY=")
                    .map(|(_, value)| value.trim().to_owned())
            })
        });
        fixture.xwayland = wait_for("owned XWayland child", || {
            let children =
                fs::read_to_string(format!("/proc/{0}/task/{0}/children", fixture.process.id()))
                    .ok()?;
            children.split_whitespace().find_map(|pid| {
                let command = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
                (command.trim() == "Xwayland").then(|| pid.parse().unwrap())
            })
        });
        fixture
    }

    fn request(&mut self, request: Request) -> Response {
        self.ipc
            .as_mut()
            .unwrap()
            .request(&request, &[])
            .unwrap()
            .response
    }

    fn nodes(&mut self) -> Vec<halley_ipc::NodeInfo> {
        match self.request(Request::Node(NodeRequest::List { output: None })) {
            Response::NodeList(list) => list
                .outputs
                .into_iter()
                .flat_map(|output| output.nodes)
                .collect(),
            response => panic!("unexpected nodes response: {response:?}"),
        }
    }

    fn node(&mut self, title: &str) -> halley_ipc::NodeInfo {
        wait_for(title, || {
            self.nodes().into_iter().find(|node| node.title == title)
        })
    }

    fn ack(&mut self, request: Request) {
        assert!(matches!(self.request(request), Response::Ack));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // The parent owns this child; never touch the desktop's XWayland.
        if self.xwayland != 0 {
            let parent = fs::read_to_string(format!("/proc/{}/status", self.xwayland))
                .ok()
                .and_then(|status| {
                    status.lines().find_map(|line| {
                        line.strip_prefix("PPid:")
                            .and_then(|pid| pid.trim().parse::<u32>().ok())
                    })
                });
            if parent == Some(self.process.id()) {
                unsafe {
                    libc::kill(self.xwayland as i32, libc::SIGKILL);
                }
            }
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
        if std::thread::panicking() {
            eprintln!("nested diagnostic files: {}", self.path.display());
        } else {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn x11_window(connection: &RustConnection, title: &str) -> u32 {
    let screen = &connection.setup().roots[0];
    let window = connection.generate_id().unwrap();
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            30,
            30,
            240,
            160,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(screen.white_pixel),
        )
        .unwrap()
        .check()
        .unwrap();
    connection
        .change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )
        .unwrap()
        .check()
        .unwrap();
    connection.map_window(window).unwrap().check().unwrap();
    connection.flush().unwrap();
    window
}

use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};

#[derive(Default)]
struct NativeState {
    globals: HashMap<String, u32>,
    surface: Option<wl_surface::WlSurface>,
    buffer: Option<wl_buffer::WlBuffer>,
    integer_scales: Vec<i32>,
    fractional_scales: Vec<u32>,
    registry: Option<wl_registry::WlRegistry>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    exported_handles: Vec<String>,
    keyboard_focused: bool,
    viewport: Option<wp_viewport::WpViewport>,
    resize_for_configures: bool,
    configured_fullscreen: bool,
}
delegate_noop!(NativeState: ignore wl_seat::WlSeat);
impl Dispatch<wl_keyboard::WlKeyboard, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => {
                state.keyboard_focused = state.surface.as_ref() == Some(&surface)
            }
            wl_keyboard::Event::Leave { .. } => state.keyboard_focused = false,
            _ => {}
        }
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            state.globals.insert(interface, name);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for NativeState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            let window = state.surface.as_ref().unwrap();
            window.attach(state.buffer.as_ref(), 0, 0);
            if state.resize_for_configures {
                window.damage_buffer(0, 0, i32::MAX, i32::MAX);
            }
            window.commit();
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for NativeState {
    fn event(
        _: &mut Self,
        shell: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}
delegate_noop!(NativeState: ignore wl_compositor::WlCompositor);
impl Dispatch<wl_surface::WlSurface, ()> for NativeState {
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
delegate_noop!(NativeState: ignore fractional_manager::WpFractionalScaleManagerV1);
impl Dispatch<fractional::WpFractionalScaleV1, ()> for NativeState {
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
delegate_noop!(NativeState: ignore wl_buffer::WlBuffer);
delegate_noop!(NativeState: ignore pixel::WpSinglePixelBufferManagerV1);
delegate_noop!(NativeState: ignore wp_viewporter::WpViewporter);
delegate_noop!(NativeState: ignore wp_viewport::WpViewport);
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            state.configured_fullscreen = states.chunks_exact(4).any(|bytes| {
                u32::from_ne_bytes(bytes.try_into().unwrap())
                    == xdg_toplevel::State::Fullscreen as u32
            });
            if state.resize_for_configures && width > 0 && height > 0 {
                state
                    .viewport
                    .as_ref()
                    .unwrap()
                    .set_destination(width, height);
            }
        }
    }
}
delegate_noop!(NativeState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(NativeState: ignore zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1);
delegate_noop!(NativeState: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(NativeState: ignore xdg_dialog_v1::XdgDialogV1);

delegate_noop!(NativeState: ignore zxdg_exporter_v2::ZxdgExporterV2);
delegate_noop!(NativeState: ignore zxdg_importer_v2::ZxdgImporterV2);
delegate_noop!(NativeState: ignore zxdg_imported_v2::ZxdgImportedV2);
impl Dispatch<zxdg_exported_v2::ZxdgExportedV2, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &zxdg_exported_v2::ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            state.exported_handles.push(handle);
        }
    }
}

fn native_window(fixture: &Fixture) -> (EventQueue<NativeState>, NativeState) {
    native_window_named(
        fixture,
        "surviving native window",
        "halley.disconnect.native",
        [u32::MAX, 0, 0, u32::MAX],
    )
}

fn native_window_named(
    fixture: &Fixture,
    title: &str,
    app_id: &str,
    rgba: [u32; 4],
) -> (EventQueue<NativeState>, NativeState) {
    let socket = fs::read_dir(&fixture.path)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_name().to_string_lossy().starts_with("wayland-")
                && !entry.file_name().to_string_lossy().ends_with(".lock")
        })
        .unwrap()
        .path();
    let connection = Connection::from_socket(UnixStream::connect(socket).unwrap()).unwrap();
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    let registry = connection.display().get_registry(&handle, ());
    let mut state = NativeState::default();
    queue.roundtrip(&mut state).unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(state.globals["wl_compositor"], 6, &handle, ());
    let pixels: pixel::WpSinglePixelBufferManagerV1 = registry.bind(
        state.globals["wp_single_pixel_buffer_manager_v1"],
        1,
        &handle,
        (),
    );
    let viewporter: wp_viewporter::WpViewporter =
        registry.bind(state.globals["wp_viewporter"], 1, &handle, ());
    let shell: xdg_wm_base::XdgWmBase = registry.bind(state.globals["xdg_wm_base"], 1, &handle, ());
    let surface = compositor.create_surface(&handle, ());
    let manager: fractional_manager::WpFractionalScaleManagerV1 = registry.bind(
        state.globals["wp_fractional_scale_manager_v1"],
        1,
        &handle,
        (),
    );
    let _fractional = manager.get_fractional_scale(&surface, &handle, ());
    let viewport = viewporter.get_viewport(&surface, &handle, ());
    viewport.set_destination(240, 160);
    state.viewport = Some(viewport);
    let xdg_surface = shell.get_xdg_surface(&surface, &handle, ());
    let toplevel = xdg_surface.get_toplevel(&handle, ());
    toplevel.set_title(title.into());
    toplevel.set_app_id(app_id.into());
    state.registry = Some(registry);
    state.toplevel = Some(toplevel);
    state.buffer =
        Some(pixels.create_u32_rgba_buffer(rgba[0], rgba[1], rgba[2], rgba[3], &handle, ()));
    state.surface = Some(surface.clone());
    surface.commit();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    (queue, state)
}

fn observe_keyboard(
    queue: &mut EventQueue<NativeState>,
    state: &mut NativeState,
) -> wl_keyboard::WlKeyboard {
    let seat: wl_seat::WlSeat =
        state
            .registry
            .as_ref()
            .unwrap()
            .bind(state.globals["wl_seat"], 1, &queue.handle(), ());
    let keyboard = seat.get_keyboard(&queue.handle(), ());
    queue.roundtrip(state).unwrap();
    keyboard
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn closing_portal_dialog_returns_keyboard_focus_to_parent_before_mru_window() {
    let mut fixture = Fixture::new("tiling");
    let (mut parent_queue, mut parent) = native_window_named(
        &fixture,
        "focus parent",
        "firefox",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let parent_id = fixture.node("focus parent").id;
    let _keyboard = observe_keyboard(&mut parent_queue, &mut parent);
    let exporter: zxdg_exporter_v2::ZxdgExporterV2 = parent.registry.as_ref().unwrap().bind(
        parent.globals["zxdg_exporter_v2"],
        1,
        &parent_queue.handle(),
        (),
    );
    let _exported =
        exporter.export_toplevel(parent.surface.as_ref().unwrap(), &parent_queue.handle(), ());
    parent_queue.roundtrip(&mut parent).unwrap();
    thread::sleep(Duration::from_millis(5));
    let (_other_queue, _other) = native_window_named(
        &fixture,
        "focus unrelated",
        "unrelated",
        [0, 0, u32::MAX, u32::MAX],
    );
    fixture.node("focus unrelated");
    let (mut dialog_queue, mut dialog) = native_window_named(
        &fixture,
        "focus portal dialog",
        "xdg-desktop-portal-gtk",
        [0, u32::MAX, 0, u32::MAX],
    );
    let dialog_id = fixture.node("focus portal dialog").id;
    let importer: zxdg_importer_v2::ZxdgImporterV2 = dialog.registry.as_ref().unwrap().bind(
        dialog.globals["zxdg_importer_v2"],
        1,
        &dialog_queue.handle(),
        (),
    );
    let _imported = importer.import_toplevel(
        parent.exported_handles.last().unwrap().clone(),
        &dialog_queue.handle(),
        (),
    );
    _imported.set_parent_of(dialog.surface.as_ref().unwrap());
    dialog_queue.roundtrip(&mut dialog).unwrap();
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(dialog_id)),
        output: None,
    }));
    parent_queue.roundtrip(&mut parent).unwrap();
    assert!(!parent.keyboard_focused);
    // An authoritative unmap, not just a close request or a logical IPC flag.
    dialog.surface.as_ref().unwrap().attach(None, 0, 0);
    dialog.surface.as_ref().unwrap().commit();
    // Do not dispatch the dialog's queued configures, which would remap it.
    dialog_queue.flush().unwrap();
    wait_for(
        "portal close returns real keyboard focus to its parent",
        || {
            parent_queue.roundtrip(&mut parent).unwrap();
            (parent.keyboard_focused
                && fixture
                    .nodes()
                    .iter()
                    .any(|node| node.id == parent_id && node.focused))
            .then_some(())
        },
    );
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn closing_x11_dialog_returns_real_x_focus_to_parent_before_mru_window() {
    use x11rb::protocol::xproto::PropMode;
    let mut fixture = Fixture::new("tiling");
    let (x11, _) = RustConnection::connect(Some(&fixture.display)).unwrap();
    let parent = x11_window(&x11, "X focus parent");
    let parent_id = fixture.node("X focus parent").id;
    thread::sleep(Duration::from_millis(5));
    let _other = x11_window(&x11, "X focus unrelated");
    let other_id = fixture.node("X focus unrelated").id;
    let dialog = x11_window(&x11, "X focus dialog");
    let dialog_id = fixture.node("X focus dialog").id;
    x11.change_property32(
        PropMode::REPLACE,
        dialog,
        AtomEnum::WM_TRANSIENT_FOR,
        AtomEnum::WINDOW,
        &[parent],
    )
    .unwrap()
    .check()
    .unwrap();
    x11.flush().unwrap();
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(parent_id)),
        output: None,
    }));
    let root = x11.setup().roots[0].root;
    wait_for("X dialog stays above its declared parent", || {
        let frames = x11.query_tree(root).ok()?.reply().ok()?.children;
        let parent_frame = x11.query_tree(parent).ok()?.reply().ok()?.parent;
        let dialog_frame = x11.query_tree(dialog).ok()?.reply().ok()?.parent;
        (frames.iter().position(|frame| *frame == parent_frame)?
            < frames.iter().position(|frame| *frame == dialog_frame)?)
        .then_some(())
    });
    thread::sleep(Duration::from_millis(5));
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(other_id)),
        output: None,
    }));
    thread::sleep(Duration::from_millis(5));
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(dialog_id)),
        output: None,
    }));
    x11.unmap_window(dialog).unwrap().check().unwrap();
    x11.flush().unwrap();
    wait_for("X dialog close restores parent input focus", || {
        let actual = x11.get_input_focus().ok()?.reply().ok()?.focus;
        (actual == parent
            && fixture
                .nodes()
                .iter()
                .any(|node| node.id == parent_id && node.focused))
        .then_some(())
    });
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn dialog_close_respects_focus_policy_and_parent_availability() {
    use x11rb::protocol::xproto::PropMode;
    for case in ["background", "disabled", "collapsed", "destroyed", "nested"] {
        let extra = if case == "disabled" {
            "field:\n  close-restore-focus false\nend\n"
        } else {
            ""
        };
        let mut fixture = Fixture::with_config("tiling", extra);
        let (x11, _) = RustConnection::connect(Some(&fixture.display)).unwrap();
        let ancestor = x11_window(&x11, "guard ancestor");
        let ancestor_id = fixture.node("guard ancestor").id;
        let parent = x11_window(&x11, "guard parent");
        let parent_id = fixture.node("guard parent").id;
        let other = x11_window(&x11, "guard unrelated");
        let other_id = fixture.node("guard unrelated").id;
        let dialog = x11_window(&x11, "guard dialog");
        let dialog_id = fixture.node("guard dialog").id;
        for (child, owner) in [(parent, ancestor), (dialog, parent)] {
            x11.change_property32(
                PropMode::REPLACE,
                child,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[owner],
            )
            .unwrap()
            .check()
            .unwrap();
        }
        // Round trips on both connections ensure the late parent notification
        // is handled before testing close succession.
        x11.flush().unwrap();
        fixture.ack(Request::Node(NodeRequest::Focus {
            selector: Some(NodeSelector::Id(parent_id)),
            output: None,
        }));
        let root = x11.setup().roots[0].root;
        wait_for("guard dialog parent relationship processed", || {
            let frames = x11.query_tree(root).ok()?.reply().ok()?.children;
            let parent_frame = x11.query_tree(parent).ok()?.reply().ok()?.parent;
            let dialog_frame = x11.query_tree(dialog).ok()?.reply().ok()?.parent;
            (frames.iter().position(|frame| *frame == parent_frame)?
                < frames.iter().position(|frame| *frame == dialog_frame)?)
            .then_some(())
        });
        if case == "collapsed" || case == "nested" {
            fixture.ack(Request::Node(NodeRequest::Collapse {
                selector: Some(NodeSelector::Id(parent_id)),
                output: None,
            }));
        }
        if case == "collapsed" {
            fixture.ack(Request::Node(NodeRequest::Collapse {
                selector: Some(NodeSelector::Id(ancestor_id)),
                output: None,
            }));
        }
        if case == "destroyed" {
            x11.destroy_window(parent).unwrap().check().unwrap();
            x11.destroy_window(ancestor).unwrap().check().unwrap();
            wait_for("parents removed", || {
                (!fixture
                    .nodes()
                    .iter()
                    .any(|node| node.id == parent_id || node.id == ancestor_id))
                .then_some(())
            });
        }
        thread::sleep(Duration::from_millis(5));
        fixture.ack(Request::Node(NodeRequest::Focus {
            selector: Some(NodeSelector::Id(other_id)),
            output: None,
        }));
        if case != "background" {
            thread::sleep(Duration::from_millis(5));
            fixture.ack(Request::Node(NodeRequest::Focus {
                selector: Some(NodeSelector::Id(dialog_id)),
                output: None,
            }));
        }
        x11.unmap_window(dialog).unwrap().check().unwrap();
        x11.flush().unwrap();
        wait_for(case, || {
            let nodes = fixture.nodes();
            if nodes.iter().any(|node| node.id == dialog_id) {
                return None;
            }
            let actual = x11.get_input_focus().ok()?.reply().ok()?.focus;
            if case == "disabled" {
                (!nodes.iter().any(|node| node.focused)
                    && actual != parent
                    && actual != ancestor
                    && actual != other)
                    .then_some(())
            } else {
                let (expected, expected_id) = if case == "nested" {
                    (ancestor, ancestor_id)
                } else {
                    (other, other_id)
                };
                (actual == expected
                    && nodes
                        .iter()
                        .any(|node| node.id == expected_id && node.focused)
                    && nodes
                        .iter()
                        .filter(|node| {
                            node.id == parent_id || (case == "collapsed" && node.id == ancestor_id)
                        })
                        .all(|node| {
                            case != "collapsed" && case != "nested"
                                || node.state == halley_ipc::NodeState::Node
                        }))
                .then_some(())
            }
        });
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn disconnect_removes_cluster_and_collapsed_x11_windows_preserving_native_windows() {
    for layout in ["stacking", "tiling"] {
        let mut fixture = Fixture::new(layout);
        let _native = native_window(&fixture);
        let native_id = fixture.node("surviving native window").id;
        let (x11, _) = RustConnection::connect(Some(&fixture.display)).unwrap();
        x11_window(&x11, "collapsed X11 window");
        let collapsed = fixture.node("collapsed X11 window").id;
        fixture.ack(Request::Node(NodeRequest::Collapse {
            selector: Some(NodeSelector::Id(collapsed)),
            output: None,
        }));
        let cluster = wait_for("startup cluster", || {
            match fixture.request(Request::Cluster(ClusterRequest::List { output: None })) {
                Response::ClusterList(list) => list
                    .outputs
                    .into_iter()
                    .flat_map(|output| output.clusters)
                    .next(),
                _ => None,
            }
        });
        fixture.ack(Request::Cluster(ClusterRequest::Open {
            target: ClusterTarget::Id(cluster.id),
            output: None,
        }));
        x11_window(&x11, "cluster X11 one");
        fixture.node("cluster X11 one");
        x11_window(&x11, "cluster X11 two");
        fixture.node("cluster X11 two");
        wait_for("two cluster members", || {
            match fixture.request(Request::Cluster(ClusterRequest::Inspect {
                target: ClusterTarget::Id(cluster.id),
                output: None,
            })) {
                Response::ClusterInfo(info) if info.members.len() == 2 => Some(()),
                _ => None,
            }
        });
        assert!(fixture.nodes().iter().any(|node| node.id == collapsed));
        assert_eq!(
            unsafe { libc::kill(fixture.xwayland as i32, libc::SIGKILL) },
            0
        );
        wait_for("all X11 node records removed", || {
            let nodes = fixture.nodes();
            (!nodes.iter().any(|node| node.title.contains("X11"))).then_some(())
        });
        let native = fixture
            .nodes()
            .into_iter()
            .find(|node| node.id == native_id)
            .unwrap();
        assert!(
            !native.focused,
            "hidden Field window must not take cluster focus"
        );
        match fixture.request(Request::Cluster(ClusterRequest::Inspect {
            target: ClusterTarget::Id(cluster.id),
            output: None,
        })) {
            Response::ClusterInfo(info) => {
                assert!(info.members.is_empty());
                assert!(info.summary.active);
            }
            response => panic!("cluster lost: {response:?}"),
        }
        fixture.ack(Request::Node(NodeRequest::Focus {
            selector: Some(NodeSelector::Id(native_id)),
            output: None,
        }));
        wait_for("native client focus after disconnect", || {
            fixture
                .nodes()
                .into_iter()
                .find(|node| node.id == native_id && node.focused)
        });
        assert!(fixture.process.try_wait().unwrap().is_none());
        let log = fs::read_to_string(fixture.path.join("console.log")).unwrap();
        assert!(log.contains("window manager disconnected"));
        assert!(!log.contains("panicked"), "{log}");
        let after_disconnect = log.split_once("window manager disconnected").unwrap().1;
        assert!(
            !after_disconnect.contains("failed to configure window geometry"),
            "dead X11 windows must not keep generating configure requests: {log}"
        );
        eprintln!("{layout}: collapsed and cluster X11 cleanup, native survival and focus passed");
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn display_scale_reload_updates_live_clients_and_native_resolution_capture() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    let mut fixture = Fixture::new("tiling");
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");
    for scale in [1.5, 2.0, 1.0] {
        fs::write(fixture.path.join("halley.rune"), format!("keybinds:\nend\nview:\n  output:\n    name \"winit\"\n    scale {scale}\n  end\nend\n")).unwrap();
        fixture.ack(Request::ConfigReload);
        let output = wait_for("effective scale", || {
            let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
                return None;
            };
            outputs
                .outputs
                .into_iter()
                .find(|output| output.scale == scale)
        });
        queue.roundtrip(&mut state).unwrap();
        queue.roundtrip(&mut state).unwrap();
        assert_eq!(
            state.fractional_scales.last(),
            Some(&((scale * 120.0) as u32))
        );
        assert_eq!(state.integer_scales.last(), Some(&(scale.ceil() as i32)));
        let mode = output.modes[output.current_mode.unwrap()];
        let size = mode.width as usize * mode.height as usize * 4;
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(fixture.path.join(format!("capture-{scale}")))
            .unwrap();
        file.set_len(size as u64).unwrap();
        let request = Request::CaptureFrame(halley_ipc::CaptureFrameRequest {
            stream_handle: "display-scale-regression".into(),
            source: halley_ipc::CaptureSource::Monitor {
                name: output.name,
                x: 0,
                y: 0,
                width: mode.width,
                height: mode.height,
            },
            cursor_mode: halley_ipc::CursorMode::Hidden,
            buffer: halley_ipc::CaptureBuffer::MemFd {
                fd_index: 0,
                offset: 0,
                size: size as u64,
                stride: mode.width as u32 * 4,
            },
        });
        let expected = (240.0 * scale).round() as usize;
        wait_for("native-size captured window", || {
            let response = fixture
                .ipc
                .as_mut()
                .unwrap()
                .request(&request, &[file.as_raw_fd()])
                .unwrap()
                .response;
            assert!(matches!(response, Response::Frame(_)), "{response:?}");
            let mut pixels = vec![0; size];
            file.read_exact_at(&mut pixels, 0).unwrap();
            let max_red = pixels
                .chunks_exact(mode.width as usize * 4)
                .map(|row| {
                    row.chunks_exact(4)
                        .filter(|pixel| pixel[2] > 245 && pixel[1] < 5 && pixel[0] < 5)
                        .count()
                })
                .max()
                .unwrap();
            (max_red.abs_diff(expected) <= 2).then_some(())
        });
        assert!(fixture.process.try_wait().unwrap().is_none());
        eprintln!(
            "scale={scale}: preferred scale and {0}x{1} native capture, window width={expected}",
            mode.width, mode.height
        );
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn parent_dialog_stacking_survives_focus_and_late_x11_parent_changes() {
    use x11rb::protocol::xproto::PropMode;
    let mut fixture = Fixture::new("tiling");
    let (x11, _) = RustConnection::connect(Some(&fixture.display)).unwrap();
    let parent = x11_window(&x11, "stacking parent");
    let parent_id = fixture.node("stacking parent").id;
    let child = x11_window(&x11, "stacking child");
    fixture.node("stacking child");
    let grandchild = x11_window(&x11, "stacking grandchild");
    fixture.node("stacking grandchild");
    let unrelated = x11_window(&x11, "stacking unrelated");
    let unrelated_id = fixture.node("stacking unrelated").id;
    for (window, owner) in [(child, parent), (grandchild, child)] {
        x11.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[owner],
        )
        .unwrap()
        .check()
        .unwrap();
    }
    x11.flush().unwrap();
    let root = x11.setup().roots[0].root;
    let assert_stack = |expected: &[u32]| {
        let mut last = None;
        wait_for("real X server parent/dialog stack", || {
            let tree = x11.query_tree(root).ok()?.reply().ok()?;
            let frames = expected
                .iter()
                .map(|window| {
                    let parent = x11.query_tree(*window).ok()?.reply().ok()?.parent;
                    Some((if parent == root { *window } else { parent }, *window))
                })
                .collect::<Option<Vec<_>>>()?;
            let order = tree
                .children
                .into_iter()
                .filter_map(|frame| {
                    frames
                        .iter()
                        .find(|(id, _)| *id == frame)
                        .map(|(_, window)| *window)
                })
                .collect::<Vec<_>>();
            if last.as_ref() != Some(&order) {
                eprintln!("expected X stack {expected:?}, observed {order:?}");
                last = Some(order.clone());
            }
            (order == expected).then_some(())
        });
    };
    assert_stack(&[parent, child, grandchild, unrelated]);
    // Repeat: deduplicated X stack publishing must still repair each actual
    // XWM raise, even though the compositor's final order is unchanged.
    for _ in 0..3 {
        fixture.ack(Request::Node(NodeRequest::Focus {
            selector: Some(NodeSelector::Id(parent_id)),
            output: None,
        }));
        assert_stack(&[unrelated, parent, child, grandchild]);
        assert!(
            fixture
                .nodes()
                .iter()
                .any(|node| node.id == parent_id && node.focused)
        );
    }
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(unrelated_id)),
        output: None,
    }));
    assert_stack(&[parent, child, grandchild, unrelated]);
    // Detach, then attach to a different parent after the windows are mapped.
    x11.delete_property(grandchild, AtomEnum::WM_TRANSIENT_FOR.into())
        .unwrap()
        .check()
        .unwrap();
    x11.change_property32(
        PropMode::REPLACE,
        child,
        AtomEnum::WM_TRANSIENT_FOR,
        AtomEnum::WINDOW,
        &[grandchild],
    )
    .unwrap()
    .check()
    .unwrap();
    x11.flush().unwrap();
    assert_stack(&[parent, grandchild, child, unrelated]);
    x11.destroy_window(grandchild).unwrap().check().unwrap();
    x11.flush().unwrap();
    wait_for("destroyed dialog parent removed", || {
        (!fixture
            .nodes()
            .iter()
            .any(|node| node.title == "stacking grandchild"))
        .then_some(())
    });
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(parent_id)),
        output: None,
    }));
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn imported_portal_dialog_stays_visible_above_a_raised_browser() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    let mut fixture = Fixture::new("tiling");
    let (mut browser_queue, mut browser) = native_window_named(
        &fixture,
        "portal browser",
        "firefox",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let parent_id = fixture.node("portal browser").id;
    let exporter: zxdg_exporter_v2::ZxdgExporterV2 = browser.registry.as_ref().unwrap().bind(
        browser.globals["zxdg_exporter_v2"],
        1,
        &browser_queue.handle(),
        (),
    );
    let _exported = exporter.export_toplevel(
        browser.surface.as_ref().unwrap(),
        &browser_queue.handle(),
        (),
    );
    browser_queue.roundtrip(&mut browser).unwrap();
    let (mut portal_queue, mut portal) = native_window_named(
        &fixture,
        "portal save dialog",
        "xdg-desktop-portal-gtk",
        [0, u32::MAX, 0, u32::MAX],
    );
    let dialog_id = fixture.node("portal save dialog").id;
    let importer: zxdg_importer_v2::ZxdgImporterV2 = portal.registry.as_ref().unwrap().bind(
        portal.globals["zxdg_importer_v2"],
        1,
        &portal_queue.handle(),
        (),
    );
    let imported = importer.import_toplevel(
        browser.exported_handles.last().unwrap().clone(),
        &portal_queue.handle(),
        (),
    );
    imported.set_parent_of(portal.surface.as_ref().unwrap());
    portal_queue.roundtrip(&mut portal).unwrap();
    wait_for("cross-client parent visible in IPC", || {
        fixture.nodes().into_iter().find(|node| {
            node.id == dialog_id
                && node
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent.node_id == Some(parent_id))
        })
    });
    for _ in 0..3 {
        fixture.ack(Request::Node(NodeRequest::Focus {
            selector: Some(NodeSelector::Id(parent_id)),
            output: None,
        }));
        browser_queue.roundtrip(&mut browser).unwrap();
        portal_queue.roundtrip(&mut portal).unwrap();
    }
    let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
        panic!("missing outputs")
    };
    let output = &outputs.outputs[0];
    let mode = output.modes[output.current_mode.unwrap()];
    let size = mode.width as usize * mode.height as usize * 4;
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(fixture.path.join("portal-stack-capture"))
        .unwrap();
    file.set_len(size as u64).unwrap();
    let request = Request::CaptureFrame(halley_ipc::CaptureFrameRequest {
        stream_handle: "portal-parent-stacking".into(),
        source: halley_ipc::CaptureSource::Monitor {
            name: output.name.clone(),
            x: 0,
            y: 0,
            width: mode.width,
            height: mode.height,
        },
        cursor_mode: halley_ipc::CursorMode::Hidden,
        buffer: halley_ipc::CaptureBuffer::MemFd {
            fd_index: 0,
            offset: 0,
            size: size as u64,
            stride: mode.width as u32 * 4,
        },
    });
    wait_for(
        "portal dialog visible after repeatedly raising browser",
        || {
            let response = fixture
                .ipc
                .as_mut()
                .unwrap()
                .request(&request, &[file.as_raw_fd()])
                .unwrap()
                .response;
            assert!(matches!(response, Response::Frame(_)), "{response:?}");
            let mut pixels = vec![0; size];
            file.read_exact_at(&mut pixels, 0).unwrap();
            let green = pixels
                .chunks_exact(4)
                .filter(|p| p[1] > 245 && p[0] < 5 && p[2] < 5)
                .count();
            // Both client rectangles overlap at the viewport center. A buried
            // dialog loses its green interior underneath the opaque red browser.
            (green > 30_000).then_some(())
        },
    );
    imported.destroy();
    portal_queue.roundtrip(&mut portal).unwrap();
    wait_for("import teardown removes parent", || {
        fixture
            .nodes()
            .into_iter()
            .find(|node| node.id == dialog_id && node.parent.is_none())
    });
    assert!(fixture.process.try_wait().unwrap().is_none());
}

fn import_native_parent(
    parent_queue: &mut EventQueue<NativeState>,
    parent: &mut NativeState,
    child_queue: &mut EventQueue<NativeState>,
    child: &mut NativeState,
) -> zxdg_imported_v2::ZxdgImportedV2 {
    let exporter: zxdg_exporter_v2::ZxdgExporterV2 = parent.registry.as_ref().unwrap().bind(
        parent.globals["zxdg_exporter_v2"],
        1,
        &parent_queue.handle(),
        (),
    );
    let _exported =
        exporter.export_toplevel(parent.surface.as_ref().unwrap(), &parent_queue.handle(), ());
    parent_queue.roundtrip(parent).unwrap();
    let importer: zxdg_importer_v2::ZxdgImporterV2 = child.registry.as_ref().unwrap().bind(
        child.globals["zxdg_importer_v2"],
        1,
        &child_queue.handle(),
        (),
    );
    let imported = importer.import_toplevel(
        parent.exported_handles.last().unwrap().clone(),
        &child_queue.handle(),
        (),
    );
    imported.set_parent_of(child.surface.as_ref().unwrap());
    child_queue.roundtrip(child).unwrap();
    imported
}

fn native_dialog_hint(
    queue: &mut EventQueue<NativeState>,
    state: &mut NativeState,
) -> (xdg_wm_dialog_v1::XdgWmDialogV1, xdg_dialog_v1::XdgDialogV1) {
    let manager: xdg_wm_dialog_v1::XdgWmDialogV1 = state.registry.as_ref().unwrap().bind(
        state.globals["xdg_wm_dialog_v1"],
        1,
        &queue.handle(),
        (),
    );
    let dialog = manager.get_xdg_dialog(state.toplevel.as_ref().unwrap(), &queue.handle(), ());
    queue.roundtrip(state).unwrap();
    (manager, dialog)
}

fn assert_native_focus(
    fixture: &mut Fixture,
    id: u64,
    queue: &mut EventQueue<NativeState>,
    state: &mut NativeState,
) {
    wait_for("matching real keyboard and compositor focus", || {
        queue.roundtrip(state).unwrap();
        (state.keyboard_focused
            && fixture
                .nodes()
                .iter()
                .any(|node| node.id == id && node.focused))
        .then_some(())
    });
}

fn request_native_focus(fixture: &mut Fixture, id: u64) {
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn xdg_dialog_modal_focus_tracks_hints_and_cross_client_parent_lifetime() {
    let mut fixture = Fixture::new("tiling");
    let (mut parent_queue, mut parent) = native_window_named(
        &fixture,
        "modal parent",
        "firefox",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let parent_id = fixture.node("modal parent").id;
    let _parent_keyboard = observe_keyboard(&mut parent_queue, &mut parent);
    let (mut other_queue, mut other) = native_window_named(
        &fixture,
        "modal unrelated",
        "unrelated",
        [0, 0, u32::MAX, u32::MAX],
    );
    let other_id = fixture.node("modal unrelated").id;
    let _other_keyboard = observe_keyboard(&mut other_queue, &mut other);
    let (mut child_queue, mut child) = native_window_named(
        &fixture,
        "modal portal dialog",
        "xdg-desktop-portal-gtk",
        [0, u32::MAX, 0, u32::MAX],
    );
    let child_id = fixture.node("modal portal dialog").id;
    let _child_keyboard = observe_keyboard(&mut child_queue, &mut child);
    let imported =
        import_native_parent(&mut parent_queue, &mut parent, &mut child_queue, &mut child);
    let (manager, dialog) = native_dialog_hint(&mut child_queue, &mut child);
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
    // A late modal hint applies to the focused family immediately.
    dialog.set_modal();
    child_queue.roundtrip(&mut child).unwrap();
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    assert!(fixture.node("modal portal dialog").modal);
    parent_queue.roundtrip(&mut parent).unwrap();
    assert!(!parent.keyboard_focused);
    request_native_focus(&mut fixture, other_id);
    assert_native_focus(&mut fixture, other_id, &mut other_queue, &mut other);
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    dialog.unset_modal();
    child_queue.roundtrip(&mut child).unwrap();
    assert!(!fixture.node("modal portal dialog").modal);
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
    request_native_focus(&mut fixture, other_id);
    dialog.set_modal();
    manager.destroy();
    child_queue.roundtrip(&mut child).unwrap();
    assert_native_focus(&mut fixture, other_id, &mut other_queue, &mut other);
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    dialog.destroy();
    child_queue.roundtrip(&mut child).unwrap();
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
    let (_manager, dialog) = native_dialog_hint(&mut child_queue, &mut child);
    dialog.set_modal();
    child_queue.roundtrip(&mut child).unwrap();
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    // Removing an imported parent makes the remaining modal hint ineffective.
    imported.destroy();
    child_queue.roundtrip(&mut child).unwrap();
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
    // Restoring the relationship to an already-modal child redirects the
    // focused parent immediately, without another focus request.
    let _reimported =
        import_native_parent(&mut parent_queue, &mut parent, &mut child_queue, &mut child);
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    parent.toplevel.as_ref().unwrap().set_fullscreen(None);
    parent_queue.roundtrip(&mut parent).unwrap();
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    // A real null-buffer unmap clears the restriction and restores parent
    // keyboard focus. Do not dispatch configures that would remap the child.
    child.surface.as_ref().unwrap().attach(None, 0, 0);
    child.surface.as_ref().unwrap().commit();
    child_queue.flush().unwrap();
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn xdg_dialog_nested_and_collapsed_children_do_not_strand_keyboard_focus() {
    let mut fixture = Fixture::new("stacking");
    let (mut parent_queue, mut parent) = native_window_named(
        &fixture,
        "nested modal parent",
        "firefox",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let parent_id = fixture.node("nested modal parent").id;
    let _parent_keyboard = observe_keyboard(&mut parent_queue, &mut parent);
    let (mut child_queue, mut child) = native_window_named(
        &fixture,
        "nested modal child",
        "portal",
        [0, u32::MAX, 0, u32::MAX],
    );
    let child_id = fixture.node("nested modal child").id;
    let _child_keyboard = observe_keyboard(&mut child_queue, &mut child);
    let _parent_import =
        import_native_parent(&mut parent_queue, &mut parent, &mut child_queue, &mut child);
    let (_manager, dialog) = native_dialog_hint(&mut child_queue, &mut child);
    dialog.set_modal();
    child_queue.roundtrip(&mut child).unwrap();
    let (mut nested_queue, mut nested) = native_window_named(
        &fixture,
        "nested modal grandchild",
        "portal",
        [0, 0, u32::MAX, u32::MAX],
    );
    let nested_id = fixture.node("nested modal grandchild").id;
    let _nested_keyboard = observe_keyboard(&mut nested_queue, &mut nested);
    let _nested_import =
        import_native_parent(&mut child_queue, &mut child, &mut nested_queue, &mut nested);
    let (_nested_manager, nested_dialog) = native_dialog_hint(&mut nested_queue, &mut nested);
    nested_dialog.set_modal();
    nested_queue.roundtrip(&mut nested).unwrap();
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, nested_id, &mut nested_queue, &mut nested);
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(nested_id)),
        output: None,
    }));
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, child_id, &mut child_queue, &mut child);
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(child_id)),
        output: None,
    }));
    request_native_focus(&mut fixture, parent_id);
    assert_native_focus(&mut fixture, parent_id, &mut parent_queue, &mut parent);
}

fn capture_green_chrome(fixture: &mut Fixture) -> (usize, Option<[usize; 4]>) {
    capture_colour_bounds(fixture)[1]
}

fn capture_colour_bounds(fixture: &mut Fixture) -> [(usize, Option<[usize; 4]>); 3] {
    let (width, pixels) = capture_chrome_pixels(fixture);
    colour_bounds(width, &pixels)
}

fn capture_chrome_pixels(fixture: &mut Fixture) -> (usize, Vec<u8>) {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
        panic!("missing outputs")
    };
    let output = &outputs.outputs[0];
    let mode = output.modes[output.current_mode.unwrap()];
    let size = mode.width as usize * mode.height as usize * 4;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(fixture.path.join("fullscreen-chrome-capture"))
        .unwrap();
    file.set_len(size as u64).unwrap();
    let request = Request::CaptureFrame(halley_ipc::CaptureFrameRequest {
        stream_handle: "fullscreen-chrome-regression".into(),
        source: halley_ipc::CaptureSource::Monitor {
            name: output.name.clone(),
            x: 0,
            y: 0,
            width: mode.width,
            height: mode.height,
        },
        cursor_mode: halley_ipc::CursorMode::Hidden,
        buffer: halley_ipc::CaptureBuffer::MemFd {
            fd_index: 0,
            offset: 0,
            size: size as u64,
            stride: mode.width as u32 * 4,
        },
    });
    let response = fixture
        .ipc
        .as_mut()
        .unwrap()
        .request(&request, &[file.as_raw_fd()])
        .unwrap()
        .response;
    assert!(matches!(response, Response::Frame(_)), "{response:?}");
    let mut pixels = vec![0; size];
    file.read_exact_at(&mut pixels, 0).unwrap();
    (mode.width as usize, pixels)
}

fn colour_bounds(width: usize, pixels: &[u8]) -> [(usize, Option<[usize; 4]>); 3] {
    std::array::from_fn(|channel| {
        let mut count = 0;
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0;
        let mut max_y = 0;
        let (other_a, other_b) = [(1, 2), (0, 2), (0, 1)][channel];
        let threshold = if channel == 1 { 64 } else { 8 };
        for (index, p) in pixels.chunks_exact(4).enumerate() {
            // Detect partially faded green chrome too, not just its opaque endpoint.
            let other = p[other_a].max(p[other_b]);
            if p[channel] > threshold
                && u16::from(p[channel]) > u16::from(other) + 32
                && (channel == 1 || other < 32)
            {
                let x = index % width;
                let y = index / width;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                count += 1;
            }
        }
        (count, (count > 0).then_some([min_x, min_y, max_x, max_y]))
    })
}

fn captured_client_bottom_corner_inset(fixture: &mut Fixture) -> usize {
    let (width, pixels) = capture_chrome_pixels(fixture);
    let [left, _, right, bottom] = colour_bounds(width, &pixels)[2]
        .1
        .expect("visible red client");
    let red_on_bottom = pixels[(bottom * width + left) * 4..(bottom * width + right + 1) * 4]
        .chunks_exact(4)
        .filter(|p| p[2] > 8 && p[0].max(p[1]) < 32 && p[2] > p[0].max(p[1]) + 32)
        .count();
    (right + 1 - left - red_on_bottom) / 2
}

fn count_green_capture_pixels(fixture: &mut Fixture) -> usize {
    capture_green_chrome(fixture).0
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; captures an isolated compositor's chrome"]
fn fullscreen_exit_fades_the_attached_frame_near_the_end_of_the_return() {
    fullscreen_exit_chrome_regression(false);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; captures pan during fullscreen exit"]
fn panning_during_fullscreen_exit_keeps_animating_without_snapping_back() {
    fullscreen_exit_chrome_regression(true);
}

fn fullscreen_exit_chrome_regression(pan_during_exit: bool) {
    // Keep in sync with `duration-ms` below; slow enough to sample mid-exit.
    let duration_ms = 4000;
    let mut fixture = Fixture::with_config(
        "tiling",
        r##"
decorations:
  border:
    size 4
    radius 32
    colour-focused "#0000ff"
    colour-unfocused "#0000ff"
  end
  titlebars:
    enabled true
    height 32
    radius 16
    show-buttons false
    show-icons false
    show-title false
    colour-focused "#00ff00"
    colour-unfocused "#00ff00"
  end
end
animations:
  window-open:
    enabled false
  end
  fullscreen:
    motion "easing"
    duration-ms 4000
    curve "linear"
  end
end
"##,
    );
    let (mut queue, mut state) = native_window_named(
        &fixture,
        "fullscreen chrome pixels",
        "halley.chrome.regression",
        [u32::MAX, 0, 0, u32::MAX],
    );
    fixture.node("fullscreen chrome pixels");
    state.resize_for_configures = true;
    let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        state.registry.as_ref().unwrap().bind(
            state.globals["zxdg_decoration_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
    let decoration =
        manager.get_toplevel_decoration(state.toplevel.as_ref().unwrap(), &queue.handle(), ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    queue.roundtrip(&mut state).unwrap();
    let windowed_bounds = wait_for("visible windowed titlebar", || {
        queue.roundtrip(&mut state).unwrap();
        let (count, bounds) = capture_green_chrome(&mut fixture);
        (count > 1000).then(|| bounds.unwrap())
    });
    let windowed_corner_inset = captured_client_bottom_corner_inset(&mut fixture);
    assert!(windowed_corner_inset > 8);
    state.toplevel.as_ref().unwrap().set_fullscreen(None);
    wait_for("fullscreen configure", || {
        queue.roundtrip(&mut state).unwrap();
        state.configured_fullscreen.then_some(())
    });
    queue.roundtrip(&mut state).unwrap();
    thread::sleep(Duration::from_millis(duration_ms + 100));
    assert_eq!(count_green_capture_pixels(&mut fixture), 0);
    if pan_during_exit {
        assert!(
            matches!(
                fixture.request(Request::Control(halley_ipc::ControlRequest::PanField(
                    halley_ipc::ControlDirection::Right,
                ))),
                Response::ApiError(_)
            ),
            "active fullscreen must still reject panning"
        );
    }
    state.toplevel.as_ref().unwrap().unset_fullscreen();
    wait_for("windowed configure", || {
        queue.roundtrip(&mut state).unwrap();
        (!state.configured_fullscreen).then_some(())
    });
    queue.roundtrip(&mut state).unwrap();
    let exit_started = Instant::now();
    thread::sleep(Duration::from_millis(150));
    // Test the actual composed frame, not just a helper's opacity value.
    assert_eq!(
        count_green_capture_pixels(&mut fixture),
        0,
        "chrome appeared early during fullscreen return motion"
    );
    assert!(
        captured_client_bottom_corner_inset(&mut fixture) <= 2,
        "client corners must stay square before the frame starts fading back"
    );
    if pan_during_exit {
        fixture.ack(Request::Control(halley_ipc::ControlRequest::PanField(
            halley_ipc::ControlDirection::Right,
        )));
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            count_green_capture_pixels(&mut fixture),
            0,
            "panning must not finish the animation or expose early chrome"
        );
        let early_client = capture_colour_bounds(&mut fixture)[2].1.unwrap();
        assert!(
            early_client[2] - early_client[0] > windowed_bounds[2] - windowed_bounds[0],
            "the client must still be returning from fullscreen"
        );
        let colours = wait_for("attached chrome fading during the return", || {
            let colours = capture_colour_bounds(&mut fixture);
            (colours[1].0 > 1000).then_some(colours)
        });
        let fading = colours[1].1.unwrap();
        assert!(
            fading[0] < windowed_bounds[0],
            "pan must move the fading titlebar before exit cleanup"
        );
        let body_border = colours[0]
            .1
            .expect("the fading frame must include its body border");
        assert!(
            body_border[2] - body_border[0] < early_client[2] - early_client[0],
            "the return animation must keep advancing during the pan"
        );
        assert!(body_border[2] - body_border[0] > windowed_bounds[2] - windowed_bounds[0]);
        assert!(body_border[0].abs_diff(fading[0]) <= 1);
        assert!(body_border[2].abs_diff(fading[2]) <= 1);
        assert!(
            (fading[3] + 1).abs_diff(body_border[1]) <= 1,
            "the titlebar must meet the animated body border: {fading:?}, {body_border:?}"
        );
        thread::sleep(
            Duration::from_millis(duration_ms + 500).saturating_sub(exit_started.elapsed()),
        );
        let settled = capture_green_chrome(&mut fixture).1.unwrap();
        assert!(
            settled[0] + 50 < windowed_bounds[0],
            "cleanup must retain the pan instead of restoring the original camera"
        );
        assert_eq!(settled[1], windowed_bounds[1]);
        assert!((settled[2] - settled[0]).abs_diff(windowed_bounds[2] - windowed_bounds[0]) <= 1);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(capture_green_chrome(&mut fixture).1, Some(settled));
        return;
    }
    let colours = wait_for("attached chrome fading during the return", || {
        let colours = capture_colour_bounds(&mut fixture);
        (colours[1].0 > 1000).then_some(colours)
    });
    let (count, fading_bounds) = colours[1];
    assert!(
        count > 1000,
        "chrome stayed hidden until a separate late fade instead of returning with the window"
    );
    let fading_bounds = fading_bounds.unwrap();
    let body_border = colours[0]
        .1
        .expect("the fading frame must include its body border");
    assert!(fading_bounds[2] - fading_bounds[0] > windowed_bounds[2] - windowed_bounds[0]);
    assert!(body_border[0].abs_diff(fading_bounds[0]) <= 1);
    assert!(body_border[2].abs_diff(fading_bounds[2]) <= 1);
    assert!((fading_bounds[3] + 1).abs_diff(body_border[1]) <= 1);
    let fading_corner_inset = captured_client_bottom_corner_inset(&mut fixture);
    assert!(
        fading_corner_inset > 0 && fading_corner_inset < windowed_corner_inset,
        "rounding must grow gradually during the frame fade: {fading_corner_inset}, {windowed_corner_inset}"
    );
    thread::sleep(Duration::from_millis(100));
    let later_bounds = capture_green_chrome(&mut fixture).1.unwrap();
    assert!(later_bounds[2] - later_bounds[0] < fading_bounds[2] - fading_bounds[0]);
    thread::sleep(Duration::from_millis(duration_ms + 500).saturating_sub(exit_started.elapsed()));
    wait_for("fully restored titlebar after animation cleanup", || {
        (count_green_capture_pixels(&mut fixture) > 1000).then_some(())
    });
    assert_eq!(capture_green_chrome(&mut fixture).1, Some(windowed_bounds));
    assert!(
        captured_client_bottom_corner_inset(&mut fixture) > 8,
        "client rounding must be fully restored after fullscreen exit"
    );
}
