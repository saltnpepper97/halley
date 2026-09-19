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
}
