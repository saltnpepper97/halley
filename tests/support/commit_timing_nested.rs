//! Exercise the production deadline event source in a real nested compositor.
use super::*;
use wayland_client::protocol::wl_callback;
use wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1 as timer, wp_commit_timing_manager_v1 as manager,
};
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1 as fifo_manager, wp_fifo_v1};

#[derive(Default)]
pub(super) struct Observations {
    frames: Vec<(u32, Duration, Duration)>,
}
struct Mark {
    index: u32,
    deadline: Duration,
}
fn now() -> Duration {
    smithay::utils::Clock::<smithay::utils::Monotonic>::new()
        .now()
        .into()
}

delegate_noop!(NativeState: ignore manager::WpCommitTimingManagerV1);
delegate_noop!(NativeState: ignore timer::WpCommitTimerV1);
impl Dispatch<wl_callback::WlCallback, Mark> for NativeState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        mark: &Mark,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state
            .commit_timing
            .frames
            .push((mark.index, mark.deadline, now()));
    }
}

fn fixture() -> Fixture {
    Fixture::with_config(
        "stacking",
        "animations:\n  window-open:\n    enabled false\n  end\nend\n",
    )
}
fn bind(queue: &EventQueue<NativeState>, state: &NativeState) -> timer::WpCommitTimerV1 {
    let manager: manager::WpCommitTimingManagerV1 = state.registry.as_ref().unwrap().bind(
        state.globals["wp_commit_timing_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    manager.get_timer(state.surface.as_ref().unwrap(), &queue.handle(), ())
}
fn timestamp(timer: &timer::WpCommitTimerV1, deadline: Duration) {
    timer.set_timestamp(
        (deadline.as_secs() >> 32) as u32,
        deadline.as_secs() as u32,
        deadline.subsec_nanos(),
    );
}
fn enqueue(
    queue: &EventQueue<NativeState>,
    state: &NativeState,
    timer: &timer::WpCommitTimerV1,
    index: u32,
    deadline: Duration,
) {
    timestamp(timer, deadline);
    state
        .surface
        .as_ref()
        .unwrap()
        .frame(&queue.handle(), Mark { index, deadline });
    state.surface.as_ref().unwrap().commit();
}
fn finish(queue: &mut EventQueue<NativeState>, state: &mut NativeState, count: usize) {
    wait_for("timed client frame callbacks", || {
        queue.roundtrip(state).unwrap();
        (state.commit_timing.frames.len() >= count).then_some(())
    });
    assert_eq!(
        state
            .commit_timing
            .frames
            .iter()
            .map(|(index, _, _)| *index)
            .collect::<Vec<_>>(),
        (0..count as u32).collect::<Vec<_>>()
    );
    for (_, deadline, observed) in &state.commit_timing.frames {
        assert!(
            observed >= deadline,
            "frame callback preceded its commit eligibility: {observed:?} < {deadline:?}"
        );
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn timing_nested_future_content_is_not_applied_early_and_ipc_remains_responsive() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    let before = fixture.node("surviving native window");
    let timer = bind(&queue, &state);
    let deadline = now() + Duration::from_millis(400);
    state.viewport.as_ref().unwrap().set_destination(312, 173);
    enqueue(&queue, &state, &timer, 0, deadline);
    queue.roundtrip(&mut state).unwrap();
    while now() + Duration::from_millis(40) < deadline {
        let node = fixture.node("surviving native window");
        assert_eq!((node.width, node.height), (before.width, before.height));
        assert!(state.commit_timing.frames.is_empty());
        queue.roundtrip(&mut state).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    finish(&mut queue, &mut state, 1);
    let after = fixture.node("surviving native window");
    assert_ne!((after.width, after.height), (before.width, before.height));
    assert!(
        now() < deadline + Duration::from_secs(2),
        "deadline timer did not wake an idle output promptly"
    );
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn timing_nested_fifo_and_timestamps_progress_together() {
    let fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    let timer = bind(&queue, &state);
    let manager: fifo_manager::WpFifoManagerV1 = state.registry.as_ref().unwrap().bind(
        state.globals["wp_fifo_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    let fifo: wp_fifo_v1::WpFifoV1 =
        manager.get_fifo(state.surface.as_ref().unwrap(), &queue.handle(), ());
    let start = now();
    for index in 0..6 {
        if index > 0 {
            fifo.wait_barrier();
        }
        fifo.set_barrier();
        enqueue(
            &queue,
            &state,
            &timer,
            index,
            start + Duration::from_millis(50 * u64::from(index + 1)),
        );
    }
    finish(&mut queue, &mut state, 6);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn timing_nested_earlier_deadline_replaces_a_distant_alarm_and_disconnect_cleans_up() {
    let mut fixture = fixture();
    let (mut blocked_queue, mut blocked) = native_window(&fixture);
    let blocked_timer = bind(&blocked_queue, &blocked);
    enqueue(
        &blocked_queue,
        &blocked,
        &blocked_timer,
        0,
        Duration::new(u64::MAX, 999_999_999),
    );
    blocked_queue.roundtrip(&mut blocked).unwrap();
    let (mut queue, mut state) = native_window_named(
        &fixture,
        "earlier deadline",
        "halley.timing.earlier",
        [0, u32::MAX, 0, u32::MAX],
    );
    let timer = bind(&queue, &state);
    let deadline = now() + Duration::from_millis(100);
    enqueue(&queue, &state, &timer, 0, deadline);
    finish(&mut queue, &mut state, 1);
    assert!(now() < deadline + Duration::from_secs(2));
    drop((blocked_queue, blocked, blocked_timer));
    wait_for("disconnected timed window removal", || {
        fixture
            .nodes()
            .iter()
            .all(|node| node.title != "surviving native window")
            .then_some(())
    });
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn timing_nested_collapsed_surface_unmaps_at_deadline_without_output_callbacks() {
    let mut fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    let node = fixture.node("surviving native window");
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let timer = bind(&queue, &state);
    let deadline = now() + Duration::from_millis(200);
    timestamp(&timer, deadline);
    state.surface.as_ref().unwrap().attach(None, 0, 0);
    state.surface.as_ref().unwrap().commit();
    queue.roundtrip(&mut state).unwrap();
    while now() + Duration::from_millis(40) < deadline {
        assert!(
            fixture
                .nodes()
                .iter()
                .any(|current| current.id == node.id && current.visible)
        );
        thread::sleep(Duration::from_millis(10));
    }
    wait_for("timed unmap of a collapsed surface", || {
        fixture
            .nodes()
            .iter()
            .find(|current| current.id == node.id)
            .is_none_or(|current| !current.visible)
            .then_some(())
    });
    assert!(now() >= deadline);
    assert!(now() < deadline + Duration::from_secs(2));
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; locks only the isolated compositor"]
fn timing_nested_lock_does_not_change_absolute_deadlines() {
    use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1;
    let fixture = fixture();
    let (mut queue, mut state) = native_window(&fixture);
    let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
        state.registry.as_ref().unwrap().bind(
            state.globals["ext_session_lock_manager_v1"],
            1,
            &queue.handle(),
            (),
        );
    let lock = manager.lock(&queue.handle(), ());
    queue.roundtrip(&mut state).unwrap();
    let timer = bind(&queue, &state);
    enqueue(
        &queue,
        &state,
        &timer,
        0,
        now() + Duration::from_millis(100),
    );
    queue.roundtrip(&mut state).unwrap();
    thread::sleep(Duration::from_millis(250));
    lock.unlock_and_destroy();
    finish(&mut queue, &mut state, 1);
}
