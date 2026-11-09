//! Application-independent watches through production Wayland and input routing.
use super::fullscreen_multi_output::Input;
use super::*;
use halley_api::wayland::client::{
    halley_surface_dismissal_manager_v1::HalleySurfaceDismissalManagerV1 as Manager,
    halley_surface_dismissal_v1::{self as watch, HalleySurfaceDismissalV1 as Watch},
};

delegate_noop!(NativeState: ignore Manager);
impl Dispatch<Watch, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &Watch,
        event: watch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let watch::Event::OutsidePress = event {
            state.outside_presses += 1;
        }
    }
}

fn layer(
    fixture: &Fixture,
    namespace: &str,
) -> (EventQueue<NativeState>, NativeState, Manager, Watch) {
    let (mut queue, mut state) = native_connection(fixture);
    let qh = queue.handle();
    let registry = state.registry.as_ref().unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(state.globals["wl_compositor"], 6, &qh, ());
    let pixels: pixel::WpSinglePixelBufferManagerV1 = registry.bind(
        state.globals["wp_single_pixel_buffer_manager_v1"],
        1,
        &qh,
        (),
    );
    let viewporter: wp_viewporter::WpViewporter =
        registry.bind(state.globals["wp_viewporter"], 1, &qh, ());
    let shell: layer_shell::ZwlrLayerShellV1 =
        registry.bind(state.globals["zwlr_layer_shell_v1"], 4, &qh, ());
    let manager: Manager = registry.bind(
        state.globals["halley_surface_dismissal_manager_v1"],
        1,
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    viewport.set_destination(180, 120);
    state.viewport = Some(viewport);
    let layer = shell.get_layer_surface(
        &surface,
        None,
        layer_shell::Layer::Overlay,
        namespace.into(),
        &qh,
        (),
    );
    layer.set_size(180, 120);
    layer.set_anchor(layer_surface::Anchor::Top | layer_surface::Anchor::Left);
    layer.set_keyboard_interactivity(layer_surface::KeyboardInteractivity::Exclusive);
    // Arm before initial configure, exactly as an application does at startup.
    let watch = manager.watch(&surface, &qh, ());
    state.buffer = Some(pixels.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &qh, ()));
    state.surface = Some(surface.clone());
    surface.commit();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    (queue, state, manager, watch)
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn surface_dismissal_is_opt_in_one_shot_and_preserves_the_outside_click() {
    let mut fixture = Fixture::with_two_outputs(
        "animations:\n enabled false\nend\ninput:\n focus-mode \"click\"\nend\n",
    );
    let input = Input::new(&fixture);
    let (mut window_queue, mut window) = native_window(&fixture);
    let node = fixture.node("surviving native window").id;
    let registry = window.registry.as_ref().unwrap();
    let seat: wl_seat::WlSeat =
        registry.bind(window.globals["wl_seat"], 8, &window_queue.handle(), ());
    let _pointer = seat.get_pointer(&window_queue.handle(), ());
    window_queue.roundtrip(&mut window).unwrap();
    fixture.ack(Request::Node(NodeRequest::MoveTo {
        x: 300,
        y: 200,
        selector: Some(NodeSelector::Id(node)),
        output: None,
    }));
    let (mut queue, mut client, manager, watch) = layer(&fixture, "example.command-palette");
    input.motion(40.0, 40.0);
    input.button(272, true);
    input.button(272, false);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(
        client.outside_presses, 0,
        "inside press is private to the layer"
    );
    input.motion(350.0, 250.0);
    input.button(273, true);
    input.button(273, false);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.outside_presses, 0, "right clicks do not dismiss");
    input.button(272, true);
    input.button(272, false);
    wait_for("outside notification", || {
        queue.roundtrip(&mut client).unwrap();
        (client.outside_presses == 1).then_some(())
    });
    // The application has deliberately kept its surface open. Halley only
    // reports the press; dismissing the application remains client policy.
    queue.roundtrip(&mut client).unwrap();
    input.button(272, true);
    input.button(272, false);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.outside_presses, 1, "watch is one-shot");
    watch.destroy();
    let cancelled = manager.watch(client.surface.as_ref().unwrap(), &queue.handle(), ());
    cancelled.destroy();
    queue.roundtrip(&mut client).unwrap();
    input.button(272, true);
    input.button(272, false);
    queue.roundtrip(&mut client).unwrap();
    assert_eq!(client.outside_presses, 1, "destroy cancels the watch");
    window_queue.roundtrip(&mut window).unwrap();
    assert!(
        window.pointer_buttons.contains(&(
            272,
            wayland_client::WEnum::Value(
                wayland_client::protocol::wl_pointer::ButtonState::Pressed
            )
        )),
        "outside press reaches the underlying client"
    );
    assert!(
        window.pointer_buttons.contains(&(
            272,
            wayland_client::WEnum::Value(
                wayland_client::protocol::wl_pointer::ButtonState::Released
            )
        )),
        "outside release reaches the underlying client"
    );
    // Once the client dismisses its own layer, the outside click must still
    // have reached the underlying window, with no extra click required.
    client.surface.as_ref().unwrap().attach(None, 0, 0);
    client.surface.as_ref().unwrap().commit();
    queue.roundtrip(&mut client).unwrap();
    window_queue.roundtrip(&mut window).unwrap();
    assert!(
        fixture.node("surviving native window").focused,
        "outside click focuses the underlying window"
    );
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn surface_dismissal_rejects_non_layer_surfaces() {
    let fixture = Fixture::new("tiling");
    let (mut queue, mut client) = native_window(&fixture);
    let registry = client.registry.as_ref().unwrap();
    let manager: Manager = registry.bind(
        client.globals["halley_surface_dismissal_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    let _watch = manager.watch(client.surface.as_ref().unwrap(), &queue.handle(), ());
    assert!(
        queue.roundtrip(&mut client).is_err(),
        "a toplevel cannot register as a layer surface"
    );
}
