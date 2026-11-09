//! Reversible visual closure, separate from the client's actual lifetime.
use std::sync::Mutex;

use smithay::desktop::Window;

#[derive(Default)]
struct Attempt {
    generation: u64,
    pending: bool,
}

#[derive(Default)]
struct VisualClose(Mutex<Attempt>);

pub(crate) fn pending(window: &Window) -> bool {
    window
        .user_data()
        .get::<VisualClose>()
        .is_some_and(|state| state.0.lock().unwrap().pending)
}

pub(crate) fn begin(window: &Window) -> Option<u64> {
    window.user_data().insert_if_missing(VisualClose::default);
    let mut attempt = window.user_data().get::<VisualClose>()?.0.lock().unwrap();
    if attempt.pending {
        return None;
    }
    attempt.generation = attempt.generation.wrapping_add(1);
    attempt.pending = true;
    Some(attempt.generation)
}

pub(crate) fn finish(window: &Window) {
    if let Some(state) = window.user_data().get::<VisualClose>() {
        state.0.lock().unwrap().pending = false;
    }
}

pub(crate) fn expire(window: &Window, generation: u64) -> bool {
    window
        .user_data()
        .get::<VisualClose>()
        .is_some_and(|state| state.0.lock().unwrap().expire(generation))
}

impl Attempt {
    fn expire(&mut self, generation: u64) -> bool {
        if !self.pending || self.generation != generation {
            return false;
        }
        self.pending = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Attempt;

    #[test]
    fn stale_timeout_cannot_reveal_a_reopened_window_closing_again() {
        let mut attempt = Attempt {
            generation: 2,
            pending: true,
        };
        assert!(!attempt.expire(1));
        assert!(attempt.pending);
        assert!(attempt.expire(2));
        assert!(!attempt.expire(2));
    }
}
