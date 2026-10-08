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
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as pixel;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
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
            "autostart:\n  cluster:\n    name \"Disconnect regression\"\n    layout \"{layout}\"\n    members []\n  end\nend\nkeybinds:\nend\n"
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
delegate_noop!(NativeState: ignore xdg_toplevel::XdgToplevel);

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
