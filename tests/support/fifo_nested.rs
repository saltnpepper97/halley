//! FIFO clients in the actual nested compositor, including hidden and locked
//! surface progress. No changes to the host compositor or user configuration.
use super::*;
use wayland_client::protocol::wl_callback;
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1 as manager, wp_fifo_v1 as fifo};

delegate_noop!(NativeState: ignore manager::WpFifoManagerV1);
delegate_noop!(NativeState: ignore fifo::WpFifoV1);

impl Dispatch<wl_callback::WlCallback, u32> for NativeState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        index: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { callback_data } = event {
            state.fifo_frames.push((*index, callback_data));
        }
    }
}

fn bind(queue: &EventQueue<NativeState>, state: &NativeState) -> fifo::WpFifoV1 {
    let manager: manager::WpFifoManagerV1 = state.registry.as_ref().unwrap().bind(
        state.globals["wp_fifo_manager_v1"],
        1,
        &queue.handle(),
        (),
    );
    manager.get_fifo(state.surface.as_ref().unwrap(), &queue.handle(), ())
}

fn enqueue(
    queue: &EventQueue<NativeState>,
    state: &NativeState,
    fifo: &fifo::WpFifoV1,
    count: u32,
) {
    let surface = state.surface.as_ref().unwrap();
    for index in 0..count {
        if index > 0 {
            fifo.wait_barrier();
        }
        fifo.set_barrier();
        surface.frame(&queue.handle(), index);
        surface.commit();
    }
}

fn finish(queue: &mut EventQueue<NativeState>, state: &mut NativeState, count: usize) {
    wait_for("FIFO frame callbacks", || {
        queue.roundtrip(state).unwrap();
        (state.fifo_frames.len() >= count).then_some(())
    });
    assert_eq!(
        state
            .fifo_frames
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        (0..count as u32).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn fifo_nested_successive_commits_progress_on_distinct_refresh_opportunities() {
    let fixture = Fixture::with_config(
        "stacking",
        "animations:\n  window-open:\n    enabled false\n  end\nend\n",
    );
    let (mut queue, mut state) = native_window(&fixture);
    let fifo = bind(&queue, &state);
    enqueue(&queue, &state, &fifo, 8);
    finish(&mut queue, &mut state, 8);
    for pair in state.fifo_frames.windows(2) {
        assert!(
            pair[1].1.wrapping_sub(pair[0].1) >= 5,
            "FIFO frames merged or released too early: {:?}",
            state.fifo_frames
        );
    }
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn fifo_nested_collapsed_window_keeps_making_progress() {
    let mut fixture = Fixture::new("stacking");
    let (mut queue, mut state) = native_window(&fixture);
    let node = fixture.node("surviving native window");
    fixture.ack(Request::Node(NodeRequest::Collapse {
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    let fifo = bind(&queue, &state);
    enqueue(&queue, &state, &fifo, 3);
    // Collapsed windows have no ordinary output callbacks. Restore after
    // the fallback has advanced the queued commits, then collect callbacks.
    thread::sleep(Duration::from_millis(4200));
    fixture.ack(Request::Node(NodeRequest::Restore {
        selector: Some(NodeSelector::Id(node.id)),
        output: None,
    }));
    finish(&mut queue, &mut state, 3);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn fifo_nested_lock_does_not_deadlock_an_ordinary_client() {
    use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1;
    let fixture = Fixture::new("stacking");
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
    let fifo = bind(&queue, &state);
    enqueue(&queue, &state, &fifo, 3);
    thread::sleep(Duration::from_millis(4200));
    lock.unlock_and_destroy();
    queue.roundtrip(&mut state).unwrap();
    finish(&mut queue, &mut state, 3);
}
