use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::sync::SyncPoint;

use crate::render::FrameSubmission;

/// Metadata for the single prepared frame still owned by Smithay. While this
/// exists, render_frame must not replace that frame or queue it before ready.
pub(super) struct DeferredFrame {
    pub sync: SyncPoint,
    pub submission: FrameSubmission,
    pub states: RenderElementStates,
    pub direct_scanout: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Readiness {
    Pending,
    Ready,
    Obsolete,
}

impl DeferredFrame {
    pub fn readiness(&self, session_lock_generation: Option<u64>) -> Readiness {
        if self.submission.session_lock_generation != session_lock_generation {
            Readiness::Obsolete
        } else if self.sync.is_reached() {
            Readiness::Ready
        } else {
            Readiness::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use calloop::EventLoop;
    use calloop::timer::{TimeoutAction, Timer};
    use smithay::backend::renderer::sync::{Fence, Interrupted};
    use smithay::desktop::utils::OutputPresentationFeedback;
    use smithay::output::{Output, PhysicalProperties, Subpixel};

    use super::*;

    #[derive(Debug)]
    struct TestFence(Arc<AtomicBool>);

    impl Fence for TestFence {
        fn is_signaled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        fn wait(&self) -> Result<(), Interrupted> {
            panic!("frame submission must never wait on the compositor thread")
        }

        fn is_exportable(&self) -> bool {
            false
        }

        fn export(&self) -> Option<OwnedFd> {
            None
        }
    }

    fn frame(sync: SyncPoint) -> DeferredFrame {
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: String::new(),
            },
        );
        DeferredFrame {
            sync,
            submission: FrameSubmission {
                target_presentation_time: Duration::from_millis(42),
                presentation_feedback: OutputPresentationFeedback::new(&output),
                session_lock_generation: Some(7),
                variable_refresh: false,
            },
            states: RenderElementStates::default(),
            direct_scanout: false,
        }
    }

    #[test]
    fn pending_frame_keeps_metadata_and_does_not_block_other_events() {
        let ready = Arc::new(AtomicBool::new(false));
        let frame = frame(SyncPoint::from(TestFence(Arc::clone(&ready))));
        let mut event_loop = EventLoop::<usize>::try_new().unwrap();
        event_loop
            .handle()
            .insert_source(Timer::immediate(), |_, _, count| {
                *count += 1;
                TimeoutAction::Drop
            })
            .unwrap();
        let mut count = 0;
        for _ in 0..16 {
            assert_eq!(frame.readiness(Some(7)), Readiness::Pending);
            event_loop.dispatch(Duration::ZERO, &mut count).unwrap();
        }
        assert_eq!(count, 1);
        ready.store(true, Ordering::SeqCst);
        assert_eq!(frame.readiness(Some(7)), Readiness::Ready);
        assert_eq!(frame.submission.session_lock_generation, Some(7));
        assert_eq!(
            frame.submission.target_presentation_time,
            Duration::from_millis(42)
        );
    }

    #[test]
    fn discarding_an_unfinished_frame_releases_its_fence_without_waiting() {
        let ready = Arc::new(AtomicBool::new(false));
        let frame = frame(SyncPoint::from(TestFence(Arc::clone(&ready))));
        assert_eq!(Arc::strong_count(&ready), 2);
        drop(frame);
        assert_eq!(Arc::strong_count(&ready), 1);
        assert!(!ready.load(Ordering::SeqCst));
    }

    #[test]
    fn completed_frame_can_be_submitted_without_a_fence() {
        assert_eq!(
            frame(SyncPoint::signaled()).readiness(Some(7)),
            Readiness::Ready
        );
    }

    #[test]
    fn lock_changes_invalidate_even_completed_frames() {
        for signaled in [false, true] {
            let mut frame = frame(SyncPoint::from(TestFence(Arc::new(AtomicBool::new(
                signaled,
            )))));
            assert_eq!(frame.readiness(None), Readiness::Obsolete, "unlock");
            assert_eq!(frame.readiness(Some(8)), Readiness::Obsolete, "new lock");
            frame.submission.session_lock_generation = None;
            assert_eq!(frame.readiness(Some(7)), Readiness::Obsolete, "lock");
        }
    }
}
