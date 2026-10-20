//! Real timestamps and surface transactions through production dispatch/state.
use super::*;
use wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1 as timer, wp_commit_timing_manager_v1 as manager,
};
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1 as fifo_manager, wp_fifo_v1};

delegate_noop!(Client: ignore manager::WpCommitTimingManagerV1);
delegate_noop!(Client: ignore timer::WpCommitTimerV1);

pub(super) enum Action {
    Advance(Duration),
    FailSource,
}

pub(super) fn dispatch_actions(
    server: &mut Server,
    display: &smithay::reexports::wayland_server::DisplayHandle,
) {
    let actions = std::mem::take(&mut server.observations.lock().unwrap().timing_actions);
    let destroyed =
        std::mem::take(&mut server.observations.lock().unwrap().timing_destroyed_clients);
    for client in destroyed {
        server
            .client_compositor_state(&client)
            .blocker_cleared(server, display);
    }
    for action in actions {
        if let Action::FailSource = action {
            for surface in server.timing.drain_surfaces() {
                buffer_readiness::reject_commit(
                    &surface,
                    "deadline event source was not registered".into(),
                    |client, error| {
                        server
                            .observations
                            .lock()
                            .unwrap()
                            .buffer_errors
                            .push((client, error));
                    },
                );
            }
            continue;
        }
        let Action::Advance(now) = action else {
            unreachable!()
        };
        server.fifo_now = now;
        let clients = server.timing.release_due(now);
        for client in clients {
            server
                .client_compositor_state(&client)
                .blocker_cleared(server, display);
        }
    }
    server.observations.lock().unwrap().timing_deadline = server.timing.next_deadline();
}

fn advance(f: &mut Fixture, now: Duration) {
    f.observations
        .lock()
        .unwrap()
        .timing_actions
        .push(Action::Advance(now));
    f.sync();
}
fn count(f: &Fixture, surface: &WlSurface) -> usize {
    f.observations
        .lock()
        .unwrap()
        .fifo_commits
        .iter()
        .filter(|s| *s == surface)
        .count()
}
fn deadline(f: &Fixture) -> Option<Duration> {
    f.observations.lock().unwrap().timing_deadline
}
fn timestamp(timer: &timer::WpCommitTimerV1, time: Duration) {
    timer.set_timestamp(
        (time.as_secs() >> 32) as u32,
        time.as_secs() as u32,
        time.subsec_nanos(),
    );
}
fn setup() -> (
    Fixture,
    wl_surface::WlSurface,
    timer::WpCommitTimerV1,
    WlSurface,
) {
    let mut f = Fixture::new();
    let surface = f.surface();
    surface.commit();
    f.sync();
    let server = f
        .observations
        .lock()
        .unwrap()
        .fifo_commits
        .last()
        .unwrap()
        .clone();
    let manager: manager::WpCommitTimingManagerV1 = f.registry.bind(
        f.state.globals["wp_commit_timing_manager_v1"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let timer = manager.get_timer(&surface, &f.queue.handle(), ());
    (f, surface, timer, server)
}

#[test]
fn future_timestamp_never_becomes_eligible_before_its_deadline() {
    let (mut f, surface, timer, server) = setup();
    timestamp(&timer, Duration::from_secs(10));
    surface.commit();
    f.sync();
    assert_eq!(count(&f, &server), 1);
    assert_eq!(deadline(&f), Some(Duration::from_secs(10)));
    advance(&mut f, Duration::from_nanos(9_999_999_999));
    assert_eq!(count(&f, &server), 1);
    advance(&mut f, Duration::from_secs(10));
    assert_eq!(count(&f, &server), 2);
    assert_eq!(deadline(&f), None);
}

#[test]
fn past_and_zero_timestamps_need_no_timer_or_additional_blocker() {
    let (mut f, surface, timer, server) = setup();
    advance(&mut f, Duration::from_secs(10));
    for time in [
        Duration::ZERO,
        Duration::from_secs(9),
        Duration::from_secs(10),
    ] {
        timestamp(&timer, time);
        surface.commit();
        f.sync();
    }
    assert_eq!(count(&f, &server), 4);
    assert_eq!(deadline(&f), None);
}

#[test]
fn timed_and_untimed_updates_keep_surface_commit_order() {
    let (mut f, surface, timer, server) = setup();
    timestamp(&timer, Duration::from_secs(20));
    surface.commit();
    surface.commit();
    timestamp(&timer, Duration::from_secs(10));
    surface.commit();
    f.sync();
    advance(&mut f, Duration::from_secs(10));
    assert_eq!(count(&f, &server), 1);
    advance(&mut f, Duration::from_secs(20));
    assert_eq!(count(&f, &server), 4);
}

#[test]
fn maximum_unsigned_timestamp_does_not_wrap_into_the_past() {
    let (mut f, surface, timer, server) = setup();
    let distant = Duration::new(u64::MAX, 999_999_999);
    timestamp(&timer, distant);
    surface.commit();
    f.sync();
    assert_eq!(deadline(&f), Some(distant));
    advance(&mut f, Duration::from_secs(3600));
    assert_eq!(count(&f, &server), 1);
    let other = f.surface();
    other.commit();
    f.sync();
    assert_eq!(f.observations.lock().unwrap().fifo_commits.len(), 2);
    surface.destroy();
    f.sync();
    assert_eq!(deadline(&f), None);
}

#[test]
fn timer_destruction_preserves_already_committed_constraints() {
    let (mut f, surface, timer, server) = setup();
    timestamp(&timer, Duration::from_secs(5));
    surface.commit();
    timer.destroy();
    f.sync();
    advance(&mut f, Duration::from_secs(4));
    assert_eq!(count(&f, &server), 1);
    advance(&mut f, Duration::from_secs(5));
    assert_eq!(count(&f, &server), 2);
}

#[test]
fn fifo_and_timestamps_must_both_release_before_a_commit_applies() {
    let (mut f, surface, timer, server) = setup();
    let manager: fifo_manager::WpFifoManagerV1 = f.registry.bind(
        f.state.globals["wp_fifo_manager_v1"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let fifo: wp_fifo_v1::WpFifoV1 = manager.get_fifo(&surface, &f.queue.handle(), ());
    fifo.set_barrier();
    surface.commit();
    f.sync();
    timestamp(&timer, Duration::from_secs(5));
    fifo.wait_barrier();
    surface.commit();
    f.sync();
    advance(&mut f, Duration::from_secs(5));
    assert_eq!(count(&f, &server), 2);
    f.observations
        .lock()
        .unwrap()
        .fifo_actions
        .push(fifo_tests::Action::Snapshot(0, vec![server.clone()]));
    f.sync();
    f.observations
        .lock()
        .unwrap()
        .fifo_actions
        .push(fifo_tests::Action::Present(0));
    f.sync();
    assert_eq!(count(&f, &server), 3);
}

#[test]
fn timestamp_release_does_not_release_an_acquire_fence() {
    let (mut f, surface, timer, server) = setup();
    let fence = smithay::wayland::compositor::Barrier::new(false);
    f.observations
        .lock()
        .unwrap()
        .fifo_actions
        .push(fifo_tests::Action::Block(server.clone(), fence.clone()));
    f.sync();
    timestamp(&timer, Duration::from_secs(5));
    surface.commit();
    f.sync();
    advance(&mut f, Duration::from_secs(5));
    assert_eq!(count(&f, &server), 1);
    assert_eq!(deadline(&f), None);
    f.observations
        .lock()
        .unwrap()
        .fifo_actions
        .push(fifo_tests::Action::FenceReady(fence));
    f.sync();
    assert_eq!(count(&f, &server), 2);
}

#[test]
fn earlier_deadlines_on_other_surfaces_are_not_starved() {
    let (mut f, surface, timer, server) = setup();
    timestamp(&timer, Duration::from_secs(20));
    surface.commit();
    f.sync();
    let other = f.surface();
    other.commit();
    f.sync();
    let other_server = f
        .observations
        .lock()
        .unwrap()
        .fifo_commits
        .last()
        .unwrap()
        .clone();
    let manager: manager::WpCommitTimingManagerV1 = f.registry.bind(
        f.state.globals["wp_commit_timing_manager_v1"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let other_timer = manager.get_timer(&other, &f.queue.handle(), ());
    timestamp(&other_timer, Duration::from_secs(5));
    other.commit();
    f.sync();
    assert_eq!(deadline(&f), Some(Duration::from_secs(5)));
    advance(&mut f, Duration::from_secs(5));
    assert_eq!(count(&f, &server), 1);
    assert_eq!(count(&f, &other_server), 2);
    assert_eq!(deadline(&f), Some(Duration::from_secs(20)));
}

#[test]
fn invalid_nanoseconds_and_duplicate_timestamps_are_protocol_errors() {
    for invalid_nsec in [true, false] {
        let (mut f, _, timer, _) = setup();
        timer.set_timestamp(0, 1, if invalid_nsec { 1_000_000_000 } else { 0 });
        if !invalid_nsec {
            timer.set_timestamp(0, 2, 0);
        }
        assert!(f.queue.roundtrip(&mut f.state).is_err());
    }
}

#[test]
fn synchronized_subsurface_timestamp_waits_for_time_and_parent_commit() {
    use wayland_client::protocol::wl_subcompositor;
    let (mut f, child, timer, server) = setup();
    let parent = f.surface();
    let manager: wl_subcompositor::WlSubcompositor = f.registry.bind(
        f.state.globals["wl_subcompositor"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let _sub = manager.get_subsurface(&child, &parent, &f.queue.handle(), ());
    timestamp(&timer, Duration::from_secs(5));
    child.commit();
    f.sync();
    advance(&mut f, Duration::from_secs(5));
    assert_eq!(
        count(&f, &server),
        1,
        "time alone cannot apply a synchronized child"
    );
    parent.commit();
    f.sync();
    assert_eq!(count(&f, &server), 2);
}

#[test]
fn destroyed_timed_subsurface_cannot_leave_its_parent_blocked() {
    use wayland_client::protocol::wl_subcompositor;
    let (mut f, child, timer, _) = setup();
    let parent = f.surface();
    parent.commit();
    f.sync();
    let parent_server = f
        .observations
        .lock()
        .unwrap()
        .fifo_commits
        .last()
        .unwrap()
        .clone();
    let manager: wl_subcompositor::WlSubcompositor = f.registry.bind(
        f.state.globals["wl_subcompositor"].0,
        1,
        &f.queue.handle(),
        (),
    );
    let sub = manager.get_subsurface(&child, &parent, &f.queue.handle(), ());
    timestamp(&timer, Duration::from_secs(500));
    child.commit();
    parent.commit();
    f.sync();
    assert_eq!(count(&f, &parent_server), 1);
    sub.destroy();
    child.destroy();
    f.sync();
    advance(&mut f, Duration::from_secs(1));
    assert_eq!(count(&f, &parent_server), 2);
    assert_eq!(deadline(&f), None);
}

#[test]
fn unavailable_deadline_source_fails_the_client_without_applying_early() {
    let (mut f, surface, timer, server) = setup();
    timestamp(&timer, Duration::from_secs(500));
    surface.commit();
    f.sync();
    f.observations
        .lock()
        .unwrap()
        .timing_actions
        .push(Action::FailSource);
    assert!((0..4).any(|_| f.queue.roundtrip(&mut f.state).is_err()));
    assert_eq!(count(&f, &server), 1);
    assert_eq!(deadline(&f), None);
}
