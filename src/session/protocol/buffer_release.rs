//! Keep explicit-sync buffers alive while previously submitted GPU reads finish.
//!
//! Upstream's renderer buffer signals its release point when its last reference
//! drops. Replacing RendererSurfaceState alone does not mean asynchronous GLES
//! reads (including snapshots and other outputs) have finished. Retain the old
//! buffer across that replacement, behind a fence on Halley's primary context.
//! KMS scanout continues to retain its own upstream buffer references.

use std::cell::Cell;

use smithay::backend::egl::fence::EGLFence;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{Buffer, with_renderer_surface_state};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{
    SurfaceAttributes, TraversalAction, add_destruction_hook, is_sync_subsurface, with_states,
    with_surface_tree_upward,
};
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;

use super::{Session, SessionDriver};

#[derive(Default)]
struct UsedExplicitSync(Cell<bool>);

pub(super) fn install<D: SessionDriver>(surface: &WlSurface) {
    // Install before on_commit_buffer_handler adds its renderer-state reset
    // hook, so destroying a surface cannot bypass the same lifetime guarantee.
    add_destruction_hook::<Session<D>, _>(surface, |session, surface| {
        retain(
            session,
            current_explicit_buffer(surface).into_iter().collect(),
        );
    });
}

pub(super) fn prepare<D: SessionDriver>(session: &mut Session<D>, surface: &WlSurface) {
    if session.drm_syncobj_state.is_none() || is_sync_subsurface(surface) {
        return;
    }
    retain(session, replaced_buffers(surface));
}

fn replaced_buffers(surface: &WlSurface) -> Vec<Buffer> {
    let mut replaced = Vec::new();
    // Match upstream's buffer-state update traversal: synchronized children are
    // applied by their parent's commit, rather than by their own callback.
    with_surface_tree_upward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |surface, states, _| {
            let replacing = states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer
                .is_some();
            if !replacing {
                return;
            }
            if states
                .data_map
                .get::<UsedExplicitSync>()
                .is_some_and(|used| used.0.get())
            {
                replaced.push(surface.clone());
            }
            if states
                .cached_state
                .get::<DrmSyncobjCachedState>()
                .current()
                .release_point
                .is_some()
            {
                states.data_map.insert_if_missing(UsedExplicitSync::default);
                states
                    .data_map
                    .get::<UsedExplicitSync>()
                    .unwrap()
                    .0
                    .set(true);
            }
        },
        |_, _, _| true,
    );
    // Surface traversal holds each surface's state lock. Query renderer state
    // only after leaving it, because the public accessor takes that lock too.
    replaced
        .iter()
        .filter_map(current_explicit_buffer)
        .collect()
}

fn current_explicit_buffer(surface: &WlSurface) -> Option<Buffer> {
    let explicit = with_states(surface, |states| {
        states
            .data_map
            .get::<UsedExplicitSync>()
            .is_some_and(|used| used.0.get())
    });
    explicit
        .then(|| with_renderer_surface_state(surface, |state| state.buffer().cloned()).flatten())
        .flatten()
}

fn retain<D: SessionDriver>(session: &mut Session<D>, buffers: Vec<Buffer>) {
    if buffers.is_empty() {
        return;
    }
    let fence = session.driver.with_renderer(|renderer| {
        let display = renderer.egl_context().display().clone();
        renderer.with_context(|gl| match EGLFence::create(&display) {
            Ok(fence) => {
                // Submit the fence even when there will be no further frames.
                // SAFETY: with_context made this renderer's GLES context current.
                unsafe { gl.Flush() };
                Some(SyncPoint::from(fence))
            }
            Err(err) => {
                eventline::warn!("explicit sync: could not fence outgoing buffer reads: {err}");
                // Fence creation is exceptional; complete earlier reads before
                // allowing the old release point to signal in this fallback.
                unsafe { gl.Finish() };
                None
            }
        })
    });
    let sync = match fence {
        Ok(Some(sync)) => sync,
        Ok(None) => return,
        Err(err) => {
            // Without a usable context we cannot prove the reads completed.
            // Keep the buffers alive rather than permit the client to overwrite
            // storage still referenced by the GPU.
            eventline::error!("explicit sync: could not complete outgoing buffer reads: {err}");
            std::mem::forget(buffers);
            return;
        }
    };
    let retained = UntilComplete::new(sync.clone(), buffers);
    if let Err(err) = session
        .driver
        .schedule_render_completion(sync, Box::new(move || drop(retained)))
    {
        eventline::warn!("explicit sync: could not watch outgoing buffer reads: {err}");
    }
}

/// Also protect source-registration failure and event-loop teardown: dropping
/// the completion callback must not become an early buffer release.
struct UntilComplete<T> {
    sync: SyncPoint,
    value: Option<T>,
}

impl<T> UntilComplete<T> {
    fn new(sync: SyncPoint, value: T) -> Self {
        Self {
            sync,
            value: Some(value),
        }
    }
}

impl<T> Drop for UntilComplete<T> {
    fn drop(&mut self) {
        if !self.sync.is_reached() && self.sync.wait().is_err() {
            eventline::error!("explicit sync: interrupted while completing outgoing buffer reads");
            if let Some(value) = self.value.take() {
                std::mem::forget(value);
            }
        }
    }
}

#[cfg(test)]
#[path = "buffer_release_tests.rs"]
mod protocol_tests;

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::fd::OwnedFd;
    use std::rc::Rc;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    use calloop::EventLoop;
    use smithay::backend::renderer::sync::{Fence, Interrupted};

    use super::*;

    #[derive(Debug)]
    struct TestFence {
        ready: Arc<AtomicBool>,
        waits: Arc<AtomicBool>,
    }
    impl Fence for TestFence {
        fn is_signaled(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }
        fn wait(&self) -> Result<(), Interrupted> {
            self.waits.store(true, Ordering::SeqCst);
            self.ready.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn is_exportable(&self) -> bool {
            false
        }
        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    struct BufferRelease(Rc<Cell<usize>>);
    impl Drop for BufferRelease {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn replacement_release_waits_for_gpu_and_keeps_the_event_loop_responsive() {
        let mut event_loop = EventLoop::<()>::try_new().unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let waits = Arc::new(AtomicBool::new(false));
        let sync = SyncPoint::from(TestFence {
            ready: ready.clone(),
            waits: waits.clone(),
        });
        let releases = Rc::new(Cell::new(0));
        let retained = UntilComplete::new(sync.clone(), BufferRelease(releases.clone()));
        crate::session::render_completion::schedule(
            &event_loop.handle(),
            sync,
            Box::new(move || drop(retained)),
        )
        .unwrap();
        event_loop
            .dispatch(Duration::from_millis(3), &mut ())
            .unwrap();
        assert_eq!(releases.get(), 0);
        assert!(!waits.load(Ordering::SeqCst));
        ready.store(true, Ordering::SeqCst);
        event_loop
            .dispatch(Duration::from_millis(3), &mut ())
            .unwrap();
        assert_eq!(releases.get(), 1);
        assert!(!waits.load(Ordering::SeqCst));
    }

    #[test]
    fn cancelling_the_completion_source_does_not_release_a_busy_buffer() {
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let waits = Arc::new(AtomicBool::new(false));
        let sync = SyncPoint::from(TestFence {
            ready: ready.clone(),
            waits: waits.clone(),
        });
        let releases = Rc::new(Cell::new(0));
        let retained = UntilComplete::new(sync.clone(), BufferRelease(releases.clone()));
        crate::session::render_completion::schedule(
            &event_loop.handle(),
            sync,
            Box::new(move || drop(retained)),
        )
        .unwrap();
        drop(event_loop);
        assert!(ready.load(Ordering::SeqCst));
        assert!(waits.load(Ordering::SeqCst));
        assert_eq!(releases.get(), 1);
    }
}
