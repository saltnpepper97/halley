use std::{
    error::Error,
    path::{Path, PathBuf},
};

/// Keep probing after a device cannot provide a usable display. Failed attempts
/// must drop their DRM/render resources before the next candidate is opened.
pub(super) fn first_usable<T>(
    candidates: Vec<PathBuf>,
    mut probe: impl FnMut(&Path) -> Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    if candidates.is_empty() {
        return Err("no GPU found on seat".into());
    }
    let mut failures = Vec::new();
    for path in candidates {
        eventline::debug!("tty: probing GPU {}", path.display());
        match probe(&path) {
            Ok(backend) => {
                eventline::info!("tty: selected GPU {}", path.display());
                return Ok(backend);
            }
            Err(err) => {
                eventline::warn!(
                    "tty: GPU {} failed: {err}; trying remaining GPUs",
                    path.display()
                );
                failures.push(format!("{}: {err}", path.display()));
            }
        }
    }
    Err(format!("no usable GPU found; {}", failures.join("; ")).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_headless_gpu_and_stops_after_usable_device() {
        let mut visited = Vec::new();
        let result = first_usable(
            vec!["headless".into(), "display".into(), "unused".into()],
            |path| {
                visited.push(path.to_path_buf());
                if path == Path::new("headless") {
                    Err("no connected connector found".into())
                } else {
                    Ok(42)
                }
            },
        )
        .unwrap();
        assert_eq!(result, 42);
        assert_eq!(
            visited,
            vec![PathBuf::from("headless"), PathBuf::from("display")]
        );
    }

    #[test]
    fn reports_every_failed_device() {
        let err = first_usable::<()>(vec!["first".into(), "second".into()], |_| {
            Err("initialization failed".into())
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("first: initialization failed"));
        assert!(err.contains("second: initialization failed"));
    }

    #[test]
    fn empty_seat_does_not_probe() {
        assert_eq!(
            first_usable::<()>(vec![], |_| panic!("unexpected probe"))
                .unwrap_err()
                .to_string(),
            "no GPU found on seat"
        );
    }

    #[test]
    fn drops_failed_candidate_resources_before_probing_next() {
        use std::{cell::RefCell, rc::Rc};

        let events = Rc::new(RefCell::new(Vec::new()));

        struct ProbeGuard {
            name: &'static str,
            events: Rc<RefCell<Vec<String>>>,
        }

        impl Drop for ProbeGuard {
            fn drop(&mut self) {
                self.events.borrow_mut().push(format!("drop {}", self.name));
            }
        }

        let events_clone = events.clone();
        let result = first_usable(vec!["failed_gpu".into(), "usable_gpu".into()], |path| {
            let name = if path == Path::new("failed_gpu") {
                "failed_gpu"
            } else {
                "usable_gpu"
            };

            events_clone.borrow_mut().push(format!("open {name}"));
            let _guard = ProbeGuard {
                name,
                events: events_clone.clone(),
            };

            if path == Path::new("failed_gpu") {
                Err("connector probe failed".into())
            } else {
                Ok(100)
            }
        })
        .unwrap();

        assert_eq!(result, 100);
        assert_eq!(
            *events.borrow(),
            vec![
                "open failed_gpu",
                "drop failed_gpu",
                "open usable_gpu",
                "drop usable_gpu",
            ]
        );
    }
}
