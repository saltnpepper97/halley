//! Real FIFO requests and Smithay transactions, using the production barrier
//! tracker and dispatcher. Output snapshots model independent refresh events;
//! the nested-session tests exercise the actual rendering path separately.
use super::*;
use smithay::backend::renderer::element::{
    Id, RenderElementPresentationState, RenderElementState, RenderElementStates,
};
use smithay::wayland::compositor::{Barrier, add_blocker};
use wayland_client::protocol::{wl_subcompositor, wl_subsurface};
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1 as manager, wp_fifo_v1 as fifo};

pub(super) enum Action {
    Snapshot(usize, Vec<WlSurface>),
    Present(usize),
    Tick(Duration),
    Block(WlSurface, Barrier),
    FenceReady(Barrier),
    Destroy(WlSurface),
}

fn release(
    server: &mut Server,
    display: &smithay::reexports::wayland_server::DisplayHandle,
    frames: Vec<fifo_state::FrameBarrier>,
) {
    for client in fifo_state::signal(frames) {
        server
            .client_compositor_state(&client)
            .blocker_cleared(server, display);
    }
}

pub(super) fn dispatch_actions(
    server: &mut Server,
    display: &smithay::reexports::wayland_server::DisplayHandle,
) {
    let actions = std::mem::take(&mut server.observations.lock().unwrap().fifo_actions);
    for action in actions {
        match action {
            Action::Snapshot(slot, surfaces) => {
                let mut elements = RenderElementStates::default();
                for surface in surfaces {
                    elements.states.insert(
                        Id::from_wayland_resource(&surface),
                        RenderElementState {
                            visible_area: 1,
                            presentation_state: RenderElementPresentationState::Rendering {
                                reason: None,
                            },
                            needs_capture: false,
                        },
                    );
                }
                let frame = server.fifo.snapshot(&elements);
                server
                    .observations
                    .lock()
                    .unwrap()
                    .fifo_snapshots
                    .insert(slot, frame.len());
                server.fifo_frames.insert(slot, frame);
            }
            Action::Present(slot) => {
                let frame = server.fifo_frames.remove(&slot).unwrap_or_default();
                release(server, display, frame);
            }
            Action::Tick(now) => {
                server.fifo_now = now;
                let frames = server.fifo.take_stalled(now);
                release(server, display, frames);
            }
            Action::Block(surface, barrier) => add_blocker(&surface, barrier),
            Action::FenceReady(barrier) => {
                barrier.signal();
                let clients = server
                    .observations
                    .lock()
                    .unwrap()
                    .fifo_commits
                    .iter()
                    .filter_map(Resource::client)
                    .collect::<Vec<_>>();
                for client in clients {
                    server
                        .client_compositor_state(&client)
                        .blocker_cleared(server, display);
                }
            }
            Action::Destroy(surface) => server.fifo.destroyed(&surface),
        }
    }
}

use smithay::reexports::wayland_server::Resource;
delegate_noop!(Client: ignore manager::WpFifoManagerV1);
delegate_noop!(Client: ignore fifo::WpFifoV1);
delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);

fn action(fixture: &mut Fixture, action: Action) {
    fixture
        .observations
        .lock()
        .unwrap()
        .fifo_actions
        .push(action);
    fixture.sync();
}

fn count(fixture: &Fixture, surface: &WlSurface) -> usize {
    fixture
        .observations
        .lock()
        .unwrap()
        .fifo_commits
        .iter()
        .filter(|s| *s == surface)
        .count()
}

fn setup() -> (Fixture, wl_surface::WlSurface, fifo::WpFifoV1, WlSurface) {
    let mut fixture = Fixture::new();
    let surface = fixture.surface();
    let manager: manager::WpFifoManagerV1 = fixture.registry.bind(
        fixture.state.globals["wp_fifo_manager_v1"].0,
        1,
        &fixture.queue.handle(),
        (),
    );
    let fifo = manager.get_fifo(&surface, &fixture.queue.handle(), ());
    fifo.set_barrier();
    surface.commit();
    fixture.sync();
    let server = fixture
        .observations
        .lock()
        .unwrap()
        .fifo_commits
        .last()
        .unwrap()
        .clone();
    (fixture, surface, fifo, server)
}

#[test]
fn successive_fifo_commits_need_distinct_presentation_opportunities() {
    let (mut fixture, surface, fifo, server) = setup();
    for _ in 0..2 {
        fifo.wait_barrier();
        fifo.set_barrier();
        surface.commit();
    }
    fixture.sync();
    assert_eq!(count(&fixture, &server), 1);
    action(&mut fixture, Action::Snapshot(0, vec![server.clone()]));
    action(&mut fixture, Action::Present(0));
    assert_eq!(count(&fixture, &server), 2);
    action(&mut fixture, Action::Present(0));
    assert_eq!(count(&fixture, &server), 2);
    action(&mut fixture, Action::Snapshot(1, vec![server.clone()]));
    action(&mut fixture, Action::Present(1));
    assert_eq!(count(&fixture, &server), 3);
}

#[test]
fn old_output_and_delayed_gpu_snapshots_cannot_release_a_new_barrier() {
    let (mut fixture, surface, fifo, server) = setup();
    action(&mut fixture, Action::Snapshot(0, vec![server.clone()]));
    action(&mut fixture, Action::Snapshot(1, vec![server.clone()]));
    for _ in 0..2 {
        fifo.wait_barrier();
        fifo.set_barrier();
        surface.commit();
    }
    fixture.sync();
    action(&mut fixture, Action::Present(1));
    assert_eq!(count(&fixture, &server), 2);
    action(&mut fixture, Action::Present(0));
    assert_eq!(
        count(&fixture, &server),
        2,
        "old output must not release the next commit"
    );
    action(&mut fixture, Action::Snapshot(2, vec![server.clone()]));
    action(&mut fixture, Action::Present(2));
    assert_eq!(count(&fixture, &server), 3);
}

#[test]
fn surfaces_without_output_progress_advance_only_on_bounded_fallback() {
    let (mut fixture, surface, fifo, server) = setup();
    fifo.wait_barrier();
    fifo.set_barrier();
    surface.commit();
    fixture.sync();
    action(&mut fixture, Action::Snapshot(0, vec![]));
    assert_eq!(fixture.observations.lock().unwrap().fifo_snapshots[&0], 0);
    action(&mut fixture, Action::Present(0));
    action(&mut fixture, Action::Tick(Duration::from_millis(999)));
    assert_eq!(count(&fixture, &server), 1);
    action(&mut fixture, Action::Tick(Duration::from_secs(1)));
    assert_eq!(count(&fixture, &server), 2);
    fifo.wait_barrier();
    surface.commit();
    fixture.sync();
    action(&mut fixture, Action::Tick(Duration::from_millis(1999)));
    assert_eq!(
        count(&fixture, &server),
        2,
        "new barrier needs its own fallback interval"
    );
    action(&mut fixture, Action::Tick(Duration::from_secs(2)));
    assert_eq!(count(&fixture, &server), 3);
}

#[test]
fn fifo_release_cannot_bypass_an_independent_acquire_fence() {
    let (mut fixture, surface, fifo, server) = setup();
    let fence = Barrier::new(false);
    action(&mut fixture, Action::Block(server.clone(), fence.clone()));
    fifo.wait_barrier();
    fifo.set_barrier();
    surface.commit();
    fixture.sync();
    action(&mut fixture, Action::Snapshot(0, vec![server.clone()]));
    action(&mut fixture, Action::Present(0));
    action(&mut fixture, Action::Tick(Duration::from_secs(5)));
    assert_eq!(count(&fixture, &server), 1);
    action(&mut fixture, Action::FenceReady(fence));
    assert_eq!(count(&fixture, &server), 2);
}

#[test]
fn ordinary_commits_keep_progressing_while_fifo_is_blocked() {
    let (mut fixture, surface, fifo, server) = setup();
    fifo.wait_barrier();
    surface.commit();
    let ordinary = fixture.surface();
    for _ in 0..4 {
        ordinary.commit();
    }
    fixture.sync();
    assert_eq!(count(&fixture, &server), 1);
    assert_eq!(fixture.observations.lock().unwrap().fifo_commits.len(), 5);
}

#[test]
fn destroying_fifo_object_preserves_its_committed_constraint() {
    let (mut fixture, surface, fifo, server) = setup();
    fifo.wait_barrier();
    surface.commit();
    fifo.destroy();
    fixture.sync();
    assert_eq!(count(&fixture, &server), 1);
    action(&mut fixture, Action::Snapshot(0, vec![server.clone()]));
    action(&mut fixture, Action::Present(0));
    assert_eq!(count(&fixture, &server), 2);
}

#[test]
fn destroyed_surfaces_are_not_in_later_frames_or_fallback() {
    let (mut fixture, surface, fifo, server) = setup();
    action(&mut fixture, Action::Snapshot(0, vec![server.clone()]));
    fifo.destroy();
    surface.destroy();
    fixture.sync();
    action(&mut fixture, Action::Destroy(server.clone()));
    action(&mut fixture, Action::Snapshot(1, vec![server]));
    assert_eq!(fixture.observations.lock().unwrap().fifo_snapshots[&1], 0);
    action(&mut fixture, Action::Present(0));
    action(&mut fixture, Action::Tick(Duration::from_secs(5)));
}

#[test]
fn synchronized_subsurface_fifo_wait_does_not_block_parent_transactions() {
    let (mut fixture, child, fifo, server) = setup();
    let parent = fixture.surface();
    let subcompositor: wl_subcompositor::WlSubcompositor = fixture.registry.bind(
        fixture.state.globals["wl_subcompositor"].0,
        1,
        &fixture.queue.handle(),
        (),
    );
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &fixture.queue.handle(), ());
    fifo.wait_barrier();
    fifo.set_barrier();
    child.commit();
    parent.commit();
    fixture.sync();
    assert_eq!(count(&fixture, &server), 2);
}
