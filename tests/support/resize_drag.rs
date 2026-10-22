//! Reproduce GTK4's redundant unset_fullscreen at the start of a CSD drag.
use super::fullscreen_multi_output::Input;
use super::*;

struct Document(Child);
impl Drop for Document {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn document(fixture: &Fixture) -> (Document, String) {
    // A standalone one-page PDF avoids depending on the user's documents.
    let stream = b"BT /F1 24 Tf 60 760 Td (Halley resize and titlebar drag test) Tj ET\n";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", stream.len()).as_bytes(), stream, b"endstream"].concat(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in &offsets[1..] {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len()
        )
        .as_bytes(),
    );
    let path = fixture.path.join("resize-drag.pdf");
    fs::write(&path, pdf).unwrap();
    let socket = fs::read_dir(&fixture.path)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wayland-")
                && p.extension().is_none()
        })
        .unwrap();
    let log = File::create(fixture.path.join("zathura.log")).unwrap();
    let child = Command::new("zathura")
        .args(["--config-dir"])
        .arg(fixture.path.join("zathura-config"))
        .arg("--data-dir")
        .arg(fixture.path.join("zathura-data"))
        .arg("--cache-dir")
        .arg(fixture.path.join("zathura-cache"))
        .arg(&path)
        .env("GDK_BACKEND", "wayland")
        .env("WAYLAND_DEBUG", "client")
        .env("WAYLAND_DISPLAY", socket)
        .env("XDG_RUNTIME_DIR", &fixture.path)
        .env("XDG_CONFIG_HOME", fixture.path.join("config"))
        .env("XDG_CACHE_HOME", fixture.path.join("cache"))
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    (Document(child), path.to_string_lossy().into_owned())
}

fn rect(input: &Input, id: u64) -> [i64; 4] {
    let snapshot = input.state();
    let window = snapshot["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .unwrap();
    std::array::from_fn(|index| window["rect"][index].as_i64().unwrap())
}

fn drag_titlebar(input: &Input, id: u64, before: [i64; 4]) {
    let x = before[0] as f64 + before[2] as f64 * 0.3;
    let y = before[1] as f64 + 16.0; // Inside GTK4's client-side title bar.
    input.motion(x, y);
    input.button(272, true);
    input.motion(x + 15.0, y + 3.0);
    wait_for("GTK titlebar move request without a size reset", || {
        let current = rect(input, id);
        assert_eq!(
            current[2..],
            before[2..],
            "drag request resized the client: {before:?} -> {current:?}"
        );
        let state = input.state();
        (state["pending_move"] == true || state["moving"] == true).then_some(())
    });
    input.motion(x + 55.0, y + 12.0);
    wait_for(
        "active titlebar drag retains the resized dimensions",
        || {
            let current = rect(input, id);
            assert_eq!(
                current[2..],
                before[2..],
                "active drag resized the client: {before:?} -> {current:?}"
            );
            (input.state()["moving"] == true && current[..2] != before[..2]).then_some(())
        },
    );
    input.button(272, false);
    wait_for("titlebar move finishes", || {
        (input.state()["moving"] == false).then_some(())
    });
    assert_eq!(rect(input, id)[2..], before[2..]);
    // GTK may attach its response buffer after the move request and release.
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            rect(input, id)[2..],
            before[2..],
            "late resize after titlebar drag"
        );
    }
}

#[test]
#[ignore = "requires Wayland, XWayland, GTK4 Zathura and a PDF plugin; real CSD pointer drags"]
fn zathura_keyboard_and_corner_resize_survive_titlebar_drag() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (_document, title) = document(&fixture);
    let id = fixture.node(&title).id;
    let initial = rect(&input, id);
    input.key(66, true); // F8: resize-window-right through production key handling.
    input.key(66, false);
    wait_for("Zathura accepts horizontal keyboard resize", || {
        (rect(&input, id)[2] > initial[2]).then_some(())
    });
    input.key(67, true); // F9: resize-window-down.
    input.key(67, false);
    let resized = wait_for("Zathura accepts keyboard resizing", || {
        let current = rect(&input, id);
        (current[2] > initial[2] && current[3] > initial[3]).then_some(current)
    });
    drag_titlebar(&input, id, resized);
    let before = rect(&input, id);
    let x = (before[0] + before[2] + 1) as f64;
    let y = (before[1] + before[3] + 1) as f64;
    input.motion(x, y);
    assert_eq!(input.button(272, true)["resizing"], true);
    input.motion(x - 70.0, y - 50.0);
    input.button(272, false);
    let resized = wait_for("Zathura accepts corner resizing", || {
        let current = rect(&input, id);
        (current[2] < before[2] && current[3] < before[3]).then_some(current)
    });
    drag_titlebar(&input, id, resized);
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; redundant unset_fullscreen"]
fn windowed_unfullscreen_ack_preserves_latest_requested_size() {
    let fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    client.resize_for_configures = true;
    input.key(66, true);
    input.key(66, false);
    wait_for("client acknowledges keyboard resize", || {
        queue.roundtrip(&mut client).unwrap();
        (client.configured_size.0 > 240).then_some(())
    });
    let resized = client.configured_size;
    let count = client.configure_count;
    client.toplevel.as_ref().unwrap().unset_fullscreen();
    wait_for("required configure for redundant unset_fullscreen", || {
        queue.roundtrip(&mut client).unwrap();
        (client.configure_count > count).then_some(())
    });
    assert_eq!(client.configured_size, resized);
    assert!(!client.configured_fullscreen);
    // A second resize is sent but not yet acknowledged when unset arrives.
    input.key(67, true);
    input.key(67, false);
    let count = client.configure_count;
    client.toplevel.as_ref().unwrap().unset_fullscreen();
    wait_for(
        "configure preserves the unacknowledged resize target",
        || {
            queue.roundtrip(&mut client).unwrap();
            (client.configure_count >= count + 2).then_some(())
        },
    );
    assert_eq!(client.configured_size, (resized.0, resized.1 + 80));
    client.toplevel.as_ref().unwrap().set_maximized();
    wait_for("maximize configure", || {
        queue.roundtrip(&mut client).unwrap();
        client.configured_maximized.then_some(())
    });
    let maximized = client.configured_size;
    let count = client.configure_count;
    client.toplevel.as_ref().unwrap().unset_fullscreen();
    wait_for("redundant fullscreen exit preserves maximize", || {
        queue.roundtrip(&mut client).unwrap();
        (client.configure_count > count).then_some(())
    });
    assert!(client.configured_maximized);
    assert!(!client.configured_fullscreen);
    assert_eq!(client.configured_size, maximized);
}
