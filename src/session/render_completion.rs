use std::time::Duration;

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::backend::renderer::sync::SyncPoint;

const FENCE_POLL_INTERVAL: Duration = Duration::from_millis(1);

pub(super) fn schedule<T: 'static>(
    loop_handle: &LoopHandle<'static, T>,
    sync: SyncPoint,
    completion: Box<dyn FnOnce() + 'static>,
) -> Result<(), String> {
    if sync.is_reached() {
        completion();
        return Ok(());
    }

    let mut completion = Some(completion);
    if let Some(fence) = sync.export() {
        loop_handle
            .insert_source(
                Generic::new(fence, Interest::READ, Mode::OneShot),
                move |_, _, _| {
                    if let Some(completion) = completion.take() {
                        completion();
                    }
                    Ok(PostAction::Remove)
                },
            )
            .map(|_| ())
            .map_err(|err| format!("failed to watch render fence: {err}"))
    } else {
        // A missing native fd does not imply completion. EGL may only offer a
        // non-native fence, or exporting a native fence may have failed. Check
        // readiness without blocking the compositor or releasing the capture.
        loop_handle
            .insert_source(Timer::from_duration(FENCE_POLL_INTERVAL), move |_, _, _| {
                if !sync.is_reached() {
                    return TimeoutAction::ToDuration(FENCE_POLL_INTERVAL);
                }
                if let Some(completion) = completion.take() {
                    completion();
                }
                TimeoutAction::Drop
            })
            .map(|_| ())
            .map_err(|err| format!("failed to poll render fence: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use calloop::EventLoop;
    use smithay::backend::renderer::sync::{Fence, Interrupted};

    use super::*;

    #[derive(Debug)]
    struct TestFence {
        reached: Arc<AtomicBool>,
        exportable: bool,
        native: Option<UnixStream>,
    }

    impl Fence for TestFence {
        fn is_signaled(&self) -> bool {
            self.reached.load(Ordering::SeqCst)
        }

        fn wait(&self) -> Result<(), Interrupted> {
            panic!("capture completion must not block the event loop")
        }

        fn is_exportable(&self) -> bool {
            self.exportable
        }

        fn export(&self) -> Option<OwnedFd> {
            self.native
                .as_ref()
                .map(|fd| fd.try_clone().unwrap().into())
        }
    }

    fn completion(count: &Rc<Cell<usize>>) -> Box<dyn FnOnce()> {
        let count = Rc::clone(count);
        Box::new(move || count.set(count.get() + 1))
    }

    #[test]
    fn completed_points_finish_immediately_without_a_native_fd() {
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let count = Rc::new(Cell::new(0));
        for sync in [
            SyncPoint::signaled(),
            SyncPoint::from(TestFence {
                reached: Arc::new(AtomicBool::new(true)),
                exportable: false,
                native: None,
            }),
        ] {
            schedule(&event_loop.handle(), sync, completion(&count)).unwrap();
        }
        assert_eq!(count.get(), 2);
    }

    fn check_pending_without_native_fd(exportable: bool) {
        let mut event_loop = EventLoop::<usize>::try_new().unwrap();
        let handle = event_loop.handle();
        let reached = Arc::new(AtomicBool::new(false));
        let count = Rc::new(Cell::new(0));
        schedule(
            &handle,
            SyncPoint::from(TestFence {
                reached: Arc::clone(&reached),
                exportable,
                native: None,
            }),
            completion(&count),
        )
        .unwrap();
        handle
            .insert_source(Timer::immediate(), |_, _, unrelated_work| {
                *unrelated_work += 1;
                TimeoutAction::Drop
            })
            .unwrap();
        let mut unrelated_work = 0;
        event_loop
            .dispatch(Duration::ZERO, &mut unrelated_work)
            .unwrap();
        assert_eq!(
            count.get(),
            0,
            "pending GPU work must not be reported complete"
        );
        assert_eq!(unrelated_work, 1, "unrelated events must remain responsive");
        event_loop
            .dispatch(Duration::from_millis(10), &mut unrelated_work)
            .unwrap();
        assert_eq!(count.get(), 0, "a pending poll must retain the capture");

        reached.store(true, Ordering::SeqCst);
        event_loop
            .dispatch(Duration::from_millis(10), &mut unrelated_work)
            .unwrap();
        assert_eq!(count.get(), 1);
        event_loop
            .dispatch(Duration::ZERO, &mut unrelated_work)
            .unwrap();
        assert_eq!(count.get(), 1, "completion must be one-shot");
    }

    #[test]
    fn non_native_fences_wait_asynchronously_for_completion() {
        check_pending_without_native_fd(false);
    }

    #[test]
    fn failed_native_fence_exports_wait_asynchronously_for_completion() {
        check_pending_without_native_fd(true);
    }

    #[test]
    fn shutting_down_discards_pending_capture_without_reporting_success() {
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let count = Rc::new(Cell::new(0));
        let capture = Rc::new(());
        let retained = Rc::downgrade(&capture);
        let completed = Rc::clone(&count);
        schedule(
            &event_loop.handle(),
            SyncPoint::from(TestFence {
                reached: Arc::new(AtomicBool::new(false)),
                exportable: false,
                native: None,
            }),
            Box::new(move || {
                completed.set(completed.get() + 1);
                drop(capture);
            }),
        )
        .unwrap();
        assert!(retained.upgrade().is_some());
        drop(event_loop);
        assert!(retained.upgrade().is_none());
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn native_fences_complete_once_after_readiness() {
        let mut event_loop = EventLoop::<()>::try_new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let reached = Arc::new(AtomicBool::new(false));
        let count = Rc::new(Cell::new(0));
        schedule(
            &event_loop.handle(),
            SyncPoint::from(TestFence {
                reached: Arc::clone(&reached),
                exportable: true,
                native: Some(reader),
            }),
            completion(&count),
        )
        .unwrap();
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert_eq!(count.get(), 0);
        reached.store(true, Ordering::SeqCst);
        writer.write_all(&[1]).unwrap();
        event_loop
            .dispatch(Duration::from_millis(10), &mut ())
            .unwrap();
        assert_eq!(count.get(), 1);
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert_eq!(count.get(), 1);
    }
}
