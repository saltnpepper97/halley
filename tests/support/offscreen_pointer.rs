//! Off-output cursor hints through real pointer constraints and production input.
use super::fullscreen_multi_output::Input;
use super::*;
use wayland_client::protocol::wl_pointer;
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as constraints;
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1 as relative_manager, zwp_relative_pointer_v1 as relative,
};

delegate_noop!(NativeState: ignore relative_manager::ZwpRelativePointerManagerV1);
impl Dispatch<relative::ZwpRelativePointerV1, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &relative::ZwpRelativePointerV1,
        event: relative::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, relative::Event::RelativeMotion { .. }) {
            state.pointer_warp.relative_motions += 1;
        }
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; isolated offscreen pointer capture"]
fn offscreen_unlock_hint_keeps_cursor_on_output_and_allows_recapture() {
    for scale in [1.0, 1.25] {
        unlock_hint_scenario(scale);
    }
}

fn unlock_hint_scenario(scale: f64) {
    let mut fixture = Fixture::with_two_outputs(&format!(
        "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\nview:\n output:\n name \"winit\"\n scale {scale}\n end\nend\n"
    ));
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    let node = fixture.node("surviving native window");
    request_native_focus(&mut fixture, node.id);
    let snapshot = input.state();
    let window = snapshot["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["id"] == node.id)
        .unwrap();
    let viewport = &window["viewport"];
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: viewport[0].as_i64().unwrap() as i32 - 180,
        y: viewport[1].as_i64().unwrap() as i32 + 100,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let rect = wait_for("partially offscreen client", || {
        let snapshot = input.state();
        let window = snapshot["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|window| window["id"] == node.id)
            .unwrap();
        let rect = window["rect"].as_array()?;
        let rect: Vec<f64> = rect.iter().map(|n| n.as_f64().unwrap()).collect();
        (rect[0] + rect[2] / 2.0 < 0.0 && rect[0] + rect[2] > 20.0).then_some(rect)
    });
    let registry = client.registry.as_ref().unwrap();
    let seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 8, &queue.handle(), ());
    let pointer: wl_pointer::WlPointer = seat.get_pointer(&queue.handle(), ());
    let manager: constraints::ZwpPointerConstraintsV1 = registry.bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &queue.handle(),
        (),
    );
    let relative_manager: relative_manager::ZwpRelativePointerManagerV1 = registry.bind(
        client.globals["zwp_relative_pointer_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    let _relative = relative_manager.get_relative_pointer(&pointer, &queue.handle(), ());
    // The private motion command feeds physical deltas into production input;
    // compensate for output scale when targeting a logical screen position.
    let motion = |x: f64, y: f64| {
        let snapshot = input.state();
        let current_x = snapshot["pointer"][0].as_f64().unwrap();
        let current_y = snapshot["pointer"][1].as_f64().unwrap();
        input.motion(
            current_x + (x - current_x) * scale,
            current_y + (y - current_y) * scale,
        )
    };
    motion(rect[0] + rect[2] - 10.0, rect[1] + rect[3] / 2.0);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.pointer_warp.enter.as_ref().map(|e| &e.0),
        client.surface.as_ref()
    );
    for _ in 0..3 {
        client.pointer_warp.locked = false;
        let lock = manager.lock_pointer(
            client.surface.as_ref().unwrap(),
            &pointer,
            None,
            constraints::Lifetime::Persistent,
            &queue.handle(),
            (),
        );
        wait_for("offscreen window recaptures pointer", || {
            queue.roundtrip(&mut client).unwrap();
            client.pointer_warp.locked.then_some(())
        });
        let before = input.state()["pointer"].clone();
        let motions = client.pointer_warp.motions;
        let relative_motions = client.pointer_warp.relative_motions;
        input.motion(30.0, 300.0);
        queue.roundtrip(&mut client).unwrap();
        assert_eq!(
            input.state()["pointer"],
            before,
            "a lock must retain its anchor"
        );
        assert_eq!(
            client.pointer_warp.motions, motions,
            "locked input is relative-only"
        );
        assert!(
            client.pointer_warp.relative_motions > relative_motions,
            "the game must continue receiving relative motion"
        );
        lock.set_cursor_position_hint(120.0, 80.0);
        client.surface.as_ref().unwrap().commit();
        queue.roundtrip(&mut client).unwrap();
        lock.destroy();
        client.surface.as_ref().unwrap().commit();
        queue.roundtrip(&mut client).unwrap();
        let snapshot = input.state();
        let x = snapshot["pointer"][0].as_f64().unwrap();
        let y = snapshot["pointer"][1].as_f64().unwrap();
        assert!(
            x >= 0.0 && y >= 0.0,
            "unlock hint left cursor off-output: {snapshot}"
        );
        assert!(
            x < 20.0,
            "unlock should clamp to the visible output edge: {snapshot}"
        );
        // A real event must restore routing at the clamped position, allowing
        // capture again without focusing or moving the window back into view.
        motion(x + 1.0, y);
        queue.roundtrip(&mut client).unwrap();
        fixture.request(Request::Outputs);
    }
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; isolated offscreen confinement"]
fn offscreen_confinement_keeps_visible_intersection_and_releases_empty_region() {
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (mut queue, mut client) = native_window(&fixture);
    let node = fixture.node("surviving native window");
    request_native_focus(&mut fixture, node.id);
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: -180,
        y: 100,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let registry = client.registry.as_ref().unwrap();
    let seat: wl_seat::WlSeat = registry.bind(client.globals["wl_seat"], 8, &queue.handle(), ());
    let pointer = seat.get_pointer(&queue.handle(), ());
    let manager: constraints::ZwpPointerConstraintsV1 = registry.bind(
        client.globals["zwp_pointer_constraints_v1"],
        1,
        &queue.handle(),
        (),
    );
    let compositor: wl_compositor::WlCompositor =
        registry.bind(client.globals["wl_compositor"], 6, &queue.handle(), ());
    input.motion(30.0, 180.0);
    queue.roundtrip(&mut client).unwrap();
    let confined = manager.confine_pointer(
        client.surface.as_ref().unwrap(),
        &pointer,
        None,
        constraints::Lifetime::Persistent,
        &queue.handle(),
        (),
    );
    wait_for("visible confinement", || {
        queue.roundtrip(&mut client).unwrap();
        client.pointer_warp.confined.then_some(())
    });
    let region = compositor.create_region(&queue.handle(), ());
    region.add(0, 0, 200, 160);
    confined.set_region(Some(&region));
    client.surface.as_ref().unwrap().commit();
    queue.roundtrip(&mut client).unwrap();
    let snapshot = input.motion(1.0, 200.5);
    let x = snapshot["pointer"][0].as_f64().unwrap();
    assert!(
        (0.0..20.0).contains(&x),
        "confinement must use the visible intersection: {snapshot}"
    );
    assert_eq!(snapshot["pointer"][1], 200.5);
    assert!(client.pointer_warp.confined);
    // This region is valid inside the surface but is entirely off-output.
    let hidden = compositor.create_region(&queue.handle(), ());
    hidden.add(0, 0, 100, 160);
    confined.set_region(Some(&hidden));
    client.surface.as_ref().unwrap().commit();
    wait_for("offscreen confinement is released", || {
        queue.roundtrip(&mut client).unwrap();
        (!client.pointer_warp.confined).then_some(())
    });
    let snapshot = input.state();
    assert!(
        snapshot["pointer"][0].as_f64().unwrap() >= 0.0,
        "cursor escaped outputs: {snapshot}"
    );
    let moved = input.motion(400.0, 300.0);
    assert_eq!(moved["pointer"], serde_json::json!([400.0, 300.0]));
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER; real offscreen X11 grabs and warps"]
fn offscreen_x11_capture_and_recenter_preserve_input_and_live_frames() {
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, EventMask, GrabMode, GrabStatus};
    let mut fixture = Fixture::with_two_outputs("animations:\n enabled false\nend\n");
    let input = Input::new(&fixture);
    let (x11, _) = x11rb::connect(Some(&fixture.display)).unwrap();
    let window = x11_window(&x11, "Offscreen X11 capture");
    let node = fixture.node("Offscreen X11 capture");
    request_native_focus(&mut fixture, node.id);
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: -180,
        y: 100,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    input.motion(30.0, 180.0);
    for color in [0xff0000, 0x0000ff, 0x00ff00] {
        let grab = x11
            .grab_pointer(
                false,
                window,
                EventMask::POINTER_MOTION,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                window,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(grab.status, GrabStatus::SUCCESS);
        for _ in 0..8 {
            x11.warp_pointer(x11rb::NONE, window, 0, 0, 0, 0, 120, 80)
                .unwrap();
            x11.flush().unwrap();
            let snapshot = input.motion(10.0, 190.0);
            assert!(
                snapshot["pointer"][0].as_f64().unwrap() >= 0.0,
                "X11 recenter escaped the output: {snapshot}"
            );
        }
        x11.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().background_pixel(color),
        )
        .unwrap();
        x11.clear_area(false, window, 0, 0, 0, 0).unwrap();
        x11.flush().unwrap();
        wait_for("fresh client frame during offscreen X11 capture", || {
            let (pixels, width) = capture_frame_pixels_on(&mut fixture, Some("winit"));
            let offset = (180 * width + 30) * 4;
            let pixel = &pixels[offset..offset + 3];
            let expected = [
                (color & 255) as u8,
                ((color >> 8) & 255) as u8,
                ((color >> 16) & 255) as u8,
            ];
            (pixel == expected).then_some(())
        });
        x11.ungrab_pointer(x11rb::CURRENT_TIME).unwrap();
        x11.flush().unwrap();
        input.motion(30.0, 180.0);
    }
    assert_eq!(
        input.motion(500.0, 400.0)["pointer"],
        serde_json::json!([500.0, 400.0])
    );
}
