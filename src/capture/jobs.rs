//! Own screenshot encoding jobs and their completion recipients.

use std::collections::HashMap;
use std::path::PathBuf;

use super::{PendingCaptureReply, encoder::ScreenshotEncoder, screenshot::CapturedImage};

#[derive(Default)]
pub(crate) struct ScreenshotJobs {
    encoder: Option<ScreenshotEncoder>,
    pending: HashMap<u64, PendingCaptureReply>,
}

impl ScreenshotJobs {
    pub(crate) fn set_encoder(&mut self, encoder: ScreenshotEncoder) {
        self.encoder = Some(encoder);
    }

    pub(super) fn submit(
        &mut self,
        directory: PathBuf,
        image: CapturedImage,
        reply: PendingCaptureReply,
    ) -> Result<(), (PendingCaptureReply, String)> {
        let Some(encoder) = self.encoder.as_mut() else {
            return Err((reply, "screenshot encoder is not running".into()));
        };
        let preview = matches!(&reply, PendingCaptureReply::Local { .. });
        match encoder.submit(directory, image.width, image.height, image.pixels, preview) {
            Ok(id) => {
                self.pending.insert(id, reply);
                Ok(())
            }
            Err(error) => Err((reply, error)),
        }
    }

    pub(super) fn take_reply(&mut self, id: u64) -> Option<PendingCaptureReply> {
        self.pending.remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_job_returns_its_reply_once() {
        use std::time::Duration;

        let directory =
            std::env::temp_dir().join(format!("halley-screenshot-jobs-{}", std::process::id()));
        let mut event_loop =
            calloop::EventLoop::<Vec<super::super::encoder::EncodeDone>>::try_new()
                .expect("event loop");
        let encoder = ScreenshotEncoder::spawn(&event_loop.handle(), |done: &mut Vec<_>, event| {
            done.push(event)
        })
        .expect("spawn encoder");
        let mut jobs = ScreenshotJobs::default();
        jobs.set_encoder(encoder);
        let reply = PendingCaptureReply::Local {
            output_name: "monitor-a".into(),
            reply: None,
        };
        let image = CapturedImage {
            width: 1,
            height: 1,
            pixels: vec![255; 4],
        };
        assert!(jobs.submit(directory.clone(), image, reply).is_ok());
        let mut completed = Vec::new();
        for _ in 0..50 {
            event_loop
                .dispatch(Some(Duration::from_millis(100)), &mut completed)
                .unwrap();
            if !completed.is_empty() {
                break;
            }
        }
        assert_eq!(completed.len(), 1);
        let done = completed.pop().unwrap();
        assert!(done.result.is_ok());
        assert!(
            matches!(jobs.take_reply(done.id), Some(PendingCaptureReply::Local { output_name, .. }) if output_name == "monitor-a")
        );
        assert!(jobs.take_reply(done.id).is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unavailable_encoder_returns_the_original_reply() {
        let mut jobs = ScreenshotJobs::default();
        let reply = PendingCaptureReply::Local {
            output_name: "monitor-a".into(),
            reply: None,
        };
        let image = CapturedImage {
            width: 1,
            height: 1,
            pixels: vec![0; 4],
        };
        let result = jobs.submit(PathBuf::new(), image, reply);
        let Err((PendingCaptureReply::Local { output_name, .. }, error)) = result else {
            panic!("the failed job must retain its reply");
        };
        assert_eq!(output_name, "monitor-a");
        assert_eq!(error, "screenshot encoder is not running");
        assert!(jobs.take_reply(1).is_none());
    }
}
