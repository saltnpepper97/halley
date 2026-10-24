//! Real requests through the production nested input and presentation policy.
use super::*;
use wayland_client::protocol::wl_pointer;
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1 as lock_manager, ext_session_lock_v1 as session_lock,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1 as confined, zwp_locked_pointer_v1 as locked,
    zwp_pointer_constraints_v1 as constraints,
};
use wayland_protocols::wp::pointer_warp::v1::client::wp_pointer_warp_v1 as warp;

#[derive(Default)]
pub(super) struct Observations {
    pub(super) enter: Option<(wl_surface::WlSurface, u32)>,
    position: Option<(f64, f64)>,
    pub(super) motions: usize,
    pub(super) locked: bool,
    pub(super) confined: bool,
    pub(super) relative_motions: usize,
}

impl Dispatch<wl_pointer::WlPointer, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let observations = &mut state.pointer_warp;
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                observations.enter = Some((surface, serial));
                observations.position = Some((surface_x, surface_y));
            }
            wl_pointer::Event::Leave { .. } => observations.enter = None,
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                observations.position = Some((surface_x, surface_y));
                observations.motions += 1;
            }
            _ => {}
        }
    }
}
impl Dispatch<locked::ZwpLockedPointerV1, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &locked::ZwpLockedPointerV1,
        event: locked::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            locked::Event::Locked => state.pointer_warp.locked = true,
            locked::Event::Unlocked => state.pointer_warp.locked = false,
            _ => {}
        }
    }
}
impl Dispatch<confined::ZwpConfinedPointerV1, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &confined::ZwpConfinedPointerV1,
        event: confined::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            confined::Event::Confined => state.pointer_warp.confined = true,
            confined::Event::Unconfined => state.pointer_warp.confined = false,
            _ => {}
        }
    }
}
delegate_noop!(NativeState: ignore warp::WpPointerWarpV1);
delegate_noop!(NativeState: ignore constraints::ZwpPointerConstraintsV1);
delegate_noop!(NativeState: ignore wayland_client::protocol::wl_region::WlRegion);
delegate_noop!(NativeState: ignore lock_manager::ExtSessionLockManagerV1);
delegate_noop!(NativeState: ignore session_lock::ExtSessionLockV1);

fn fixture() -> Fixture {
    Fixture::with_config(
        "stacking",
        "animations:\n  window-open:\n    enabled false\n  end\nend\n",
    )
}

pub(super) fn bind(
    queue: &mut EventQueue<NativeState>,
    state: &mut NativeState,
) -> (wl_pointer::WlPointer, warp::WpPointerWarpV1) {
    let registry = state.registry.as_ref().unwrap();
    let seat: wl_seat::WlSeat = registry.bind(state.globals["wl_seat"], 8, &queue.handle(), ());
    let pointer = seat.get_pointer(&queue.handle(), ());
    let warp = registry.bind(state.globals["wp_pointer_warp_v1"], 1, &queue.handle(), ());
    queue.roundtrip(state).unwrap();
    (pointer, warp)
}

fn focus(fixture: &mut Fixture, queue: &mut EventQueue<NativeState>, state: &mut NativeState) {
    let id = fixture.node("surviving native window").id;
    fixture.ack(Request::Node(NodeRequest::Focus {
        selector: Some(NodeSelector::Id(id)),
        output: None,
    }));
    fixture.ack(Request::Control(halley_ipc::ControlRequest::MonitorFocus(
        halley_ipc::MonitorFocusTarget::Output("winit".into()),
    )));
    wait_for("real pointer focus", || {
        queue.roundtrip(state).unwrap();
        state
            .pointer_warp
            .enter
            .as_ref()
            .filter(|(surface, _)| Some(surface) == state.surface.as_ref())
            .map(|_| ())
    });
}

pub(super) fn request(
    queue: &mut EventQueue<NativeState>,
    state: &mut NativeState,
    pointer: &wl_pointer::WlPointer,
    warp: &warp::WpPointerWarpV1,
    point: (f64, f64),
) {
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    warp.warp_pointer(
        state.surface.as_ref().unwrap(),
        pointer,
        point.0,
        point.1,
        serial,
    );
    queue.roundtrip(state).unwrap();
}

pub(super) fn check_position(state: &NativeState, expected: (f64, f64)) {
    let actual = state.pointer_warp.position.unwrap();
    assert!(
        (actual.0 - expected.0).abs() <= 1.0 / 256.0,
        "{actual:?} != {expected:?}"
    );
    assert!(
        (actual.1 - expected.1).abs() <= 1.0 / 256.0,
        "{actual:?} != {expected:?}"
    );
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; warps only the isolated compositor cursor"]
fn valid_warps_follow_field_scale_and_fullscreen_presentation() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    for scale in [1.0, 1.5, 2.0] {
        fs::write(fixture.path.join("halley.rune"), format!(
            "keybinds:\nend\nview:\n  output:\n    name \"winit\"\n    scale {scale}\n  end\nend\nanimations:\n  window-open:\n    enabled false\n  end\nend\n"
        )).unwrap();
        fixture.ack(Request::ConfigReload);
        focus(&mut fixture, &mut queue, &mut state);
        for point in [(40.25, 50.5), (180.5, 120.25)] {
            request(&mut queue, &mut state, &pointer, &warp, point);
            check_position(&state, point);
        }
    }
    state.resize_for_configures = true;
    state.toplevel.as_ref().unwrap().set_fullscreen(None);
    queue.roundtrip(&mut state).unwrap();
    wait_for("fullscreen configure", || {
        queue.roundtrip(&mut state).unwrap();
        state.configured_fullscreen.then_some(())
    });
    // The live transform must be sampled once per request, including while
    // the fullscreen endpoints are moving. Client coordinates remain exact.
    let center = (
        f64::from(state.configured_size.0) * 0.5,
        f64::from(state.configured_size.1) * 0.5,
    );
    request(&mut queue, &mut state, &pointer, &warp, center);
    check_position(&state, center);
    thread::sleep(Duration::from_millis(500));
    focus(&mut fixture, &mut queue, &mut state);
    request(&mut queue, &mut state, &pointer, &warp, (90.5, 100.25));
    check_position(&state, (90.5, 100.25));
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; warps only the isolated compositor cursor"]
fn stale_outside_unmapped_and_foreign_surface_requests_do_not_move_the_pointer() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    request(&mut queue, &mut state, &pointer, &warp, (80.0, 70.0));
    check_position(&state, (80.0, 70.0));
    let before = state.pointer_warp.motions;
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    warp.warp_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        20.0,
        30.0,
        serial.wrapping_sub(1),
    );
    for point in [
        (-1.0, 40.0),
        (240.0, 40.0),
        (40.0, 160.0),
        (1_000.0, 1_000.0),
    ] {
        request(&mut queue, &mut state, &pointer, &warp, point);
    }
    let compositor: wl_compositor::WlCompositor = state.registry.as_ref().unwrap().bind(
        state.globals["wl_compositor"],
        6,
        &queue.handle(),
        (),
    );
    let unrelated = compositor.create_surface(&queue.handle(), ());
    warp.warp_pointer(&unrelated, &pointer, 20.0, 30.0, serial);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);
    check_position(&state, (80.0, 70.0));

    // A second client cannot reuse the first client's valid enter serial.
    let (mut foreign_queue, mut foreign) = native_connection(&fixture);
    let (foreign_pointer, foreign_warp) = bind(&mut foreign_queue, &mut foreign);
    let compositor: wl_compositor::WlCompositor = foreign.registry.as_ref().unwrap().bind(
        foreign.globals["wl_compositor"],
        6,
        &foreign_queue.handle(),
        (),
    );
    let foreign_surface = compositor.create_surface(&foreign_queue.handle(), ());
    foreign_warp.warp_pointer(&foreign_surface, &foreign_pointer, 30.0, 40.0, serial);
    foreign_queue.roundtrip(&mut foreign).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);

    state.surface.as_ref().unwrap().attach(None, 0, 0);
    state.surface.as_ref().unwrap().commit();
    queue.roundtrip(&mut state).unwrap();
    let before = state.pointer_warp.motions;
    warp.warp_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        20.0,
        30.0,
        serial,
    );
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; constrains only the isolated compositor cursor"]
fn pointer_lock_and_confinement_keep_their_authority() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    request(&mut queue, &mut state, &pointer, &warp, (80.0, 70.0));
    let manager: constraints::ZwpPointerConstraintsV1 = state.registry.as_ref().unwrap().bind(
        state.globals["zwp_pointer_constraints_v1"],
        1,
        &queue.handle(),
        (),
    );
    let lock = manager.lock_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        None,
        constraints::Lifetime::Persistent,
        &queue.handle(),
        (),
    );
    wait_for("active pointer lock", || {
        queue.roundtrip(&mut state).unwrap();
        state.pointer_warp.locked.then_some(())
    });
    let before = state.pointer_warp.motions;
    request(&mut queue, &mut state, &pointer, &warp, (120.0, 90.0));
    assert_eq!(state.pointer_warp.motions, before);
    assert!(state.pointer_warp.locked);
    lock.destroy();
    queue.roundtrip(&mut state).unwrap();
    let compositor: wl_compositor::WlCompositor = state.registry.as_ref().unwrap().bind(
        state.globals["wl_compositor"],
        6,
        &queue.handle(),
        (),
    );
    let region = compositor.create_region(&queue.handle(), ());
    region.add(20, 20, 100, 90);
    let confinement = manager.confine_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        Some(&region),
        constraints::Lifetime::Persistent,
        &queue.handle(),
        (),
    );
    wait_for("active pointer confinement", || {
        queue.roundtrip(&mut state).unwrap();
        state.pointer_warp.confined.then_some(())
    });
    request(&mut queue, &mut state, &pointer, &warp, (60.5, 60.25));
    check_position(&state, (60.5, 60.25));
    let before = state.pointer_warp.motions;
    request(&mut queue, &mut state, &pointer, &warp, (180.0, 140.0));
    assert_eq!(state.pointer_warp.motions, before);
    check_position(&state, (60.5, 60.25));
    assert!(state.pointer_warp.confined);
    confinement.destroy();
    queue.roundtrip(&mut state).unwrap();
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; warps only the isolated compositor cursor"]
fn subsurface_offsets_and_input_regions_match_real_hit_testing() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    request(&mut queue, &mut state, &pointer, &warp, (100.0, 80.0));
    let registry = state.registry.as_ref().unwrap();
    let compositor: wl_compositor::WlCompositor =
        registry.bind(state.globals["wl_compositor"], 6, &queue.handle(), ());
    let subcompositor: wl_subcompositor::WlSubcompositor =
        registry.bind(state.globals["wl_subcompositor"], 1, &queue.handle(), ());
    let viewporter: wp_viewporter::WpViewporter =
        registry.bind(state.globals["wp_viewporter"], 1, &queue.handle(), ());
    let child = compositor.create_surface(&queue.handle(), ());
    let sub =
        subcompositor.get_subsurface(&child, state.surface.as_ref().unwrap(), &queue.handle(), ());
    sub.set_position(60, 40);
    let viewport = viewporter.get_viewport(&child, &queue.handle(), ());
    viewport.set_destination(120, 80);
    child.attach(state.buffer.as_ref(), 0, 0);
    child.commit();
    state.surface.as_ref().unwrap().commit();
    wait_for("subsurface pointer focus", || {
        queue.roundtrip(&mut state).unwrap();
        state
            .pointer_warp
            .enter
            .as_ref()
            .filter(|(surface, _)| surface == &child)
            .map(|_| ())
    });
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    warp.warp_pointer(&child, &pointer, 20.25, 30.5, serial);
    queue.roundtrip(&mut state).unwrap();
    check_position(&state, (20.25, 30.5));

    let region = compositor.create_region(&queue.handle(), ());
    region.add(0, 0, 60, 80);
    child.set_input_region(Some(&region));
    child.commit();
    state.surface.as_ref().unwrap().commit();
    queue.roundtrip(&mut state).unwrap();
    let before = state.pointer_warp.motions;
    warp.warp_pointer(&child, &pointer, 90.0, 30.0, serial);
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);
    check_position(&state, (20.25, 30.5));
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; locks only the isolated compositor"]
fn collapse_and_session_lock_reject_old_surface_requests() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    let node = fixture.node("surviving native window").id;
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(node)),
        output: None,
    }));
    queue.roundtrip(&mut state).unwrap();
    let before = state.pointer_warp.motions;
    warp.warp_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        40.0,
        50.0,
        serial,
    );
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);
    fixture.ack(Request::Node(NodeRequest::Restore {
        selector: Some(NodeSelector::Id(node)),
        output: None,
    }));
    focus(&mut fixture, &mut queue, &mut state);
    let serial = state.pointer_warp.enter.as_ref().unwrap().1;
    let manager: lock_manager::ExtSessionLockManagerV1 = state.registry.as_ref().unwrap().bind(
        state.globals["ext_session_lock_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    let _lock = manager.lock(&queue.handle(), ());
    queue.roundtrip(&mut state).unwrap();
    assert!(state.pointer_warp.enter.is_none());
    let before = state.pointer_warp.motions;
    warp.warp_pointer(
        state.surface.as_ref().unwrap(),
        &pointer,
        40.0,
        50.0,
        serial,
    );
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_warp.motions, before);
    assert!(state.pointer_warp.enter.is_none());
    // The fixture kills only its disposable compositor, so the user's
    // session never needs to be locked or unlocked by this regression.
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; checks pointer coordinates during actual window motion"]
fn window_movement_keeps_stationary_pointer_coordinates_on_the_visible_client() {
    let mut fixture = Fixture::with_config(
        "stacking",
        "animations:\n window-open:\n enabled false\n end\n window-move:\n motion \"easing\"\n duration-ms 800\n curve \"linear\"\n end\nend\n",
    );
    let (mut queue, mut state) = native_window(&fixture);
    let node = fixture.node("surviving native window");
    let (pointer, warp) = bind(&mut queue, &mut state);
    focus(&mut fixture, &mut queue, &mut state);
    // Let explicit focus centering settle before testing a stationary pointer.
    thread::sleep(Duration::from_millis(900));
    focus(&mut fixture, &mut queue, &mut state);
    request(&mut queue, &mut state, &pointer, &warp, (120.0, 80.0));
    check_position(&state, (120.0, 80.0));
    fixture.ack(Request::Node(NodeRequest::MoveBy {
        direction: halley_ipc::NodeMoveDirection::Right,
        distance: 80,
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let started = Instant::now();
    let mut samples = Vec::new();
    while started.elapsed() < Duration::from_millis(1100) {
        queue.roundtrip(&mut state).unwrap();
        assert!(
            state.pointer_warp.enter.is_some(),
            "pointer left the moving visible client at {:?}, samples={samples:?}, position={:?}",
            started.elapsed(),
            state.pointer_warp.position
        );
        samples.push(state.pointer_warp.position.unwrap().0);
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        samples.iter().any(|x| *x > 45.0 && *x < 115.0),
        "no intermediate input coordinates: {samples:?}"
    );
    assert!(
        (samples.last().unwrap() - 40.0).abs() <= 1.0,
        "wrong settled pointer coordinates: {samples:?}"
    );
    for pair in samples.windows(2) {
        assert!(
            pair[1] <= pair[0] + 1.0,
            "input transform reversed or jumped: {samples:?}"
        );
    }
}
