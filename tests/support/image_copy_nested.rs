//! Capture real rendered windows through the new protocol, never the host.
use super::image_copy_client::{self as capture, Client};
use super::*;
use std::os::unix::fs::FileExt;
use wayland_client::protocol::wl_shm::Format;

fn socket(fixture: &Fixture) -> PathBuf {
    fs::read_dir(&fixture.path)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name().to_string_lossy().starts_with("wayland-")
                && !e.file_name().to_string_lossy().ends_with(".lock")
        })
        .unwrap()
        .path()
}
fn client(fixture: &Fixture) -> Client {
    Client::new(UnixStream::connect(socket(fixture)).unwrap())
}
fn complete(client: &mut Client, index: usize) {
    wait_for("image copy capture completion", || {
        client.sync();
        let f = &client.state.frames[index];
        (f.ready + f.failures.len() > 0).then_some(())
    });
}
fn copy(
    client: &mut Client,
    session: &capture::session::ExtImageCopyCaptureSessionV1,
    index: usize,
) -> (Vec<u8>, u32, u32) {
    client.sync();
    let &(width, height) = client.state.sessions[index].sizes.last().unwrap();
    let (buffer, file) = client.buffer(
        width as i32,
        height as i32,
        width as i32 * 4,
        0,
        Format::Xrgb8888,
    );
    let frame_index = client.state.frames.len();
    let frame = client.frame(session);
    frame.attach_buffer(&buffer);
    frame.capture();
    complete(client, frame_index);
    let events = &client.state.frames[frame_index];
    assert_eq!(events.ready, 1, "{events:?}");
    assert!(events.failures.is_empty());
    assert_eq!(events.transforms, [0]);
    assert_eq!(events.damage, [(0, 0, width as i32, height as i32)]);
    assert_eq!(events.order, ["transform", "damage", "time", "ready"]);
    let mut pixels = vec![0; width as usize * height as usize * 4];
    file.read_exact_at(&mut pixels, 0).unwrap();
    frame.destroy();
    buffer.destroy();
    client.sync();
    (pixels, width, height)
}
struct Application(Child);
impl Drop for Application {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires a Wayland desktop, XWayland and Kitty; starts an isolated nested compositor"]
fn image_copy_nested_real_window_matches_portal_capture_at_fractional_scales() {
    let mut fixture =
        Fixture::with_capture_config("stacking", "animations:\n  enabled false\nend\n", true);
    let _application = Application(
        Command::new("kitty")
            .args([
                "--config", "NONE", "--title", "Image-copy real terminal",
                "--override", "linux_display_server=wayland",
                "--override", "cursor_blink_interval=0",
                "--override", "initial_window_width=600",
                "--override", "initial_window_height=300",
                "--override", "remember_window_size=no",
                "--override", "enable_audio_bell=no",
                "/bin/sh", "-c",
                "printf 'Halley image-copy capture\\n\\nReal terminal window, native pixels.\\nOne two three four five.\\n'; read -r answer",
            ])
            .env("XDG_RUNTIME_DIR", &fixture.path)
            .env("WAYLAND_DISPLAY", socket(&fixture))
            .env_remove("WAYLAND_SOCKET")
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(fixture.path.join("kitty.log")).unwrap()))
            .spawn().unwrap(),
    );
    fixture.node("Image-copy real terminal");
    let mut client = client(&fixture);
    let manager = client.manager();
    let source = client.source();
    let session = client.session(&manager, &source, false);
    client.sync();
    assert_eq!(client.state.sessions[0].formats, [1]);
    assert!(!client.state.sessions[0].dma);
    for scale in [1.0, 1.5, 1.25, 2.0] {
        fs::write(fixture.path.join("halley.rune"),format!("keybinds:\nend\nview:\n  output:\n    name \"winit\"\n    scale {scale}\n  end\nend\nanimations:\n  enabled false\nend\n")).unwrap();
        fixture.ack(Request::ConfigReload);
        wait_for("effective output scale", || {
            let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
                return None;
            };
            (outputs.outputs[0].scale == scale).then_some(())
        });
        // Wait for the real application to repaint at the new preferred scale.
        thread::sleep(Duration::from_millis(250));
        let (pixels, width, height) = copy(&mut client, &session, 0);
        let (portal, portal_width) = capture_frame_pixels(&mut fixture);
        assert_eq!(width as usize, portal_width);
        assert_eq!(pixels.len(), portal.len());
        let different = pixels
            .chunks_exact(4)
            .zip(portal.chunks_exact(4))
            .filter(|(a, b)| a[..3] != b[..3])
            .count();
        assert!(
            different < pixels.len() / 4 / 100,
            "capture paths disagree at scale {scale}: {different} pixels"
        );
        let colours: std::collections::HashSet<_> =
            pixels.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
        assert!(
            colours.len() > 100,
            "real terminal text and desktop must be present"
        );
        if scale == 1.5 {
            let artifact = std::env::temp_dir().join("halley-image-copy-real-window.png");
            let rgba: Vec<_> = pixels
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            image::save_buffer(&artifact, &rgba, width, height, image::ColorType::Rgba8).unwrap();
            eprintln!("capture artifact: {}", artifact.display());
        }
    }
    assert!(
        client.state.sessions[0].done >= 4,
        "scale changes must resend constraints"
    );
    fixture.ack(Request::Control(halley_ipc::ControlRequest::MonitorFocus(
        halley_ipc::MonitorFocusTarget::Output("winit".into()),
    )));
    let cursor_session = client.session(&manager, &source, true);
    client.sync();
    let (hidden, _, _) = copy(&mut client, &session, 0);
    let (visible, _, _) = copy(&mut client, &cursor_session, 1);
    assert_eq!(hidden.len(), visible.len());
    // An application may intentionally hide its cursor. A separate test below
    // installs a known cursor surface and checks actual pixel differences.
    // Exercise the legacy protocol while both modern sessions remain alive.
    let legacy: wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 = client.registry.bind(client.state.globals["zwlr_screencopy_manager_v1"], 3, &client.queue.handle(), ());
    let output: wayland_client::protocol::wl_output::WlOutput = client.registry.bind(
        client.state.globals["wl_output"],
        4,
        &client.queue.handle(),
        (),
    );
    let legacy_frame = legacy.capture_output(0, &output, &client.queue.handle(), ());
    client.sync();
    let (width, height, stride) = client.state.legacy_buffer.unwrap();
    let (buffer, file) = client.buffer(
        width as i32,
        height as i32,
        stride as i32,
        0,
        Format::Xrgb8888,
    );
    legacy_frame.copy(&buffer);
    wait_for("legacy screencopy alongside image-copy sessions", || {
        client.sync();
        (client.state.legacy_ready == 1).then_some(())
    });
    let mut legacy_pixels = vec![0; stride as usize * height as usize];
    file.read_exact_at(&mut legacy_pixels, 0).unwrap();
    assert_eq!(legacy_pixels.len(), hidden.len());
    assert!(
        legacy_pixels
            .chunks_exact(4)
            .zip(hidden.chunks_exact(4))
            .filter(|(a, b)| a[..3] != b[..3])
            .count()
            < hidden.len() / 4 / 100
    );
    legacy_frame.destroy();
    source.destroy();
    client.sync();
    assert_eq!(client.state.sessions[0].stopped, 1);
    assert_eq!(client.state.sessions[1].stopped, 1);
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn image_copy_nested_unapproved_executable_has_no_capture_globals() {
    let fixture = Fixture::new("stacking");
    let client = client(&fixture);
    for name in [
        "ext_image_copy_capture_manager_v1",
        "ext_output_image_capture_source_manager_v1",
        "zwlr_screencopy_manager_v1",
    ] {
        assert!(!client.state.globals.contains_key(name), "{name}");
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn image_copy_nested_lock_stops_existing_and_new_capture_without_touching_buffers() {
    use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1;
    let fixture = Fixture::with_capture_config("stacking", "", true);
    let mut client = client(&fixture);
    let manager = client.manager();
    let source = client.source();
    let session = client.session(&manager, &source, false);
    client.sync();
    let (width, height) = client.state.sessions[0].sizes[0];
    let (buffer, file) = client.buffer(
        width as i32,
        height as i32,
        width as i32 * 4,
        0,
        Format::Xrgb8888,
    );
    let frame = client.frame(&session);
    frame.attach_buffer(&buffer);
    let locker: ext_session_lock_manager_v1::ExtSessionLockManagerV1 = client.registry.bind(
        client.state.globals["ext_session_lock_manager_v1"],
        1,
        &client.queue.handle(),
        (),
    );
    let lock = locker.lock(&client.queue.handle(), ());
    frame.capture();
    complete(&mut client, 0);
    assert_eq!(client.state.frames[0].failures, [2]);
    assert_eq!(client.state.sessions[0].stopped, 1);
    let _new_session = client.session(&manager, &source, false);
    client.sync();
    assert_eq!(client.state.sessions[1].stopped, 1);
    assert!(client.state.sessions[1].sizes.is_empty());
    let mut bytes = vec![0; width as usize * height as usize * 4];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert!(bytes.iter().all(|b| *b == 0xa5));
    lock.unlock_and_destroy();
    client.sync();
    assert_eq!(client.state.sessions[0].stopped, 1);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn image_copy_nested_cursor_only_session_is_explicitly_stopped() {
    let fixture = Fixture::with_capture_config("stacking", "", true);
    let mut client = client(&fixture);
    let manager = client.manager();
    let source = client.source();
    let seat: wayland_client::protocol::wl_seat::WlSeat = client.registry.bind(
        client.state.globals["wl_seat"],
        1,
        &client.queue.handle(),
        (),
    );
    let pointer = seat.get_pointer(&client.queue.handle(), ());
    let cursor =
        manager.create_pointer_cursor_session(&source, &pointer, &client.queue.handle(), ());
    client.state.sessions.push(Default::default());
    let _session = cursor.get_capture_session(&client.queue.handle(), 0usize);
    client.sync();
    assert_eq!(client.state.sessions[0].stopped, 1);
    assert!(client.state.sessions[0].sizes.is_empty());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn image_copy_nested_embedded_client_cursor_changes_only_requested_capture() {
    let mut fixture =
        Fixture::with_capture_config("stacking", "animations:\n  enabled false\nend\n", true);
    let (mut queue, mut state) = native_window(&fixture);
    let seat: wl_seat::WlSeat =
        state
            .registry
            .as_ref()
            .unwrap()
            .bind(state.globals["wl_seat"], 8, &queue.handle(), ());
    let pointer = seat.get_pointer(&queue.handle(), ());
    queue.roundtrip(&mut state).unwrap();
    let id = fixture.node("surviving native window").id;
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    fixture.ack(Request::Control(halley_ipc::ControlRequest::MonitorFocus(
        halley_ipc::MonitorFocusTarget::Output("winit".into()),
    )));
    wait_for("cursor client pointer focus", || {
        queue.roundtrip(&mut state).unwrap();
        state
            .pointer_warp
            .enter
            .as_ref()
            .filter(|(surface, _)| Some(surface) == state.surface.as_ref())
            .map(|(_, serial)| *serial)
    });
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    let compositor: wl_compositor::WlCompositor = state.registry.as_ref().unwrap().bind(
        state.globals["wl_compositor"],
        6,
        &queue.handle(),
        (),
    );
    use std::os::fd::AsFd;
    let shm: wl_shm::WlShm =
        state
            .registry
            .as_ref()
            .unwrap()
            .bind(state.globals["wl_shm"], 1, &queue.handle(), ());
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(fixture.path.join("cursor.shm"))
        .unwrap();
    let pixels: Vec<u8> = (0..24 * 24).flat_map(|_| [0, 255, 0, 255]).collect();
    file.set_len(pixels.len() as u64).unwrap();
    file.write_all_at(&pixels, 0).unwrap();
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &queue.handle(), ());
    let buffer = pool.create_buffer(0, 24, 24, 24 * 4, Format::Argb8888, &queue.handle(), ());
    let cursor = compositor.create_surface(&queue.handle(), ());
    pointer.set_cursor(serial, Some(&cursor), 0, 0);
    cursor.attach(Some(&buffer), 0, 0);
    cursor.commit();
    queue.roundtrip(&mut state).unwrap();
    let mut client = client(&fixture);
    let manager = client.manager();
    let source = client.source();
    let hidden = client.session(&manager, &source, false);
    let visible = client.session(&manager, &source, true);
    let (hidden_pixels, _, _) = copy(&mut client, &hidden, 0);
    let (visible_pixels, _, _) = copy(&mut client, &visible, 1);
    let is_cursor = |pixel: &&[u8]| pixel[0] == 0 && pixel[1] == 255 && pixel[2] == 0;
    assert_eq!(
        hidden_pixels.chunks_exact(4).filter(is_cursor).count(),
        0,
        "the session without paint_cursors must omit the client cursor"
    );
    assert_eq!(
        visible_pixels.chunks_exact(4).filter(is_cursor).count(),
        24 * 24,
        "the cursor session must embed the complete visible cursor rectangle"
    );
}
