use smithay::input::pointer::CursorIcon;

use crate::compositor::root::Halley;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CursorPresentationState {
    interaction_override: Option<CursorIcon>,
    feedback_override: Option<CursorIcon>,
    feedback_until_ms: Option<u64>,
}

impl CursorPresentationState {
    pub(crate) fn effective_override(&self) -> Option<CursorIcon> {
        self.interaction_override.or(self.feedback_override)
    }

    pub(crate) fn feedback_deadline_ms(&self) -> Option<u64> {
        self.feedback_until_ms
    }

    fn set_interaction_override(&mut self, icon: Option<CursorIcon>) {
        self.interaction_override = icon;
    }

    fn set_feedback(&mut self, icon: CursorIcon, until_ms: u64) {
        self.feedback_override = Some(icon);
        self.feedback_until_ms = Some(until_ms);
    }

    fn expire_feedback(&mut self, now_ms: u64) -> bool {
        if !self
            .feedback_until_ms
            .is_some_and(|until_ms| now_ms >= until_ms)
        {
            return false;
        }
        self.feedback_until_ms = None;
        self.feedback_override = None;
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CursorState {
    pub(crate) pending_screen_hint: Option<(f32, f32)>,
    pub(crate) last_screen_global: Option<(f32, f32)>,
    pub(crate) presentation: CursorPresentationState,
}

pub(crate) fn set_override(st: &mut Halley, icon: Option<CursorIcon>) {
    st.input
        .interaction_state
        .cursor
        .presentation
        .set_interaction_override(icon);
}

pub(crate) fn effective_override(st: &Halley) -> Option<CursorIcon> {
    st.input
        .interaction_state
        .cursor
        .presentation
        .effective_override()
}

pub(crate) fn feedback_deadline_ms(st: &Halley) -> Option<u64> {
    st.input
        .interaction_state
        .cursor
        .presentation
        .feedback_deadline_ms()
}

pub(crate) fn show_temporary_feedback(
    st: &mut Halley,
    icon: CursorIcon,
    now_ms: u64,
    duration_ms: u64,
) {
    st.input
        .interaction_state
        .cursor
        .presentation
        .set_feedback(icon, now_ms.saturating_add(duration_ms.max(1)));
    st.request_maintenance();
}

pub(crate) fn expire_temporary_feedback(st: &mut Halley, now_ms: u64) -> bool {
    st.input
        .interaction_state
        .cursor
        .presentation
        .expire_feedback(now_ms)
}

pub(crate) fn take_screen_hint(st: &mut Halley) -> Option<(f32, f32)> {
    st.input.interaction_state.cursor.pending_screen_hint.take()
}

#[cfg(test)]
mod tests {
    use super::CursorPresentationState;
    use smithay::input::pointer::CursorIcon;

    #[test]
    fn interaction_override_wins_over_temporary_feedback() {
        let mut state = CursorPresentationState::default();
        state.set_feedback(CursorIcon::ZoomIn, 50);
        state.set_interaction_override(Some(CursorIcon::Grabbing));

        assert_eq!(state.effective_override(), Some(CursorIcon::Grabbing));
        assert!(state.expire_feedback(50));
        assert_eq!(state.effective_override(), Some(CursorIcon::Grabbing));
    }

    #[test]
    fn temporary_feedback_expires_without_touching_interaction_state() {
        let mut state = CursorPresentationState::default();
        state.set_feedback(CursorIcon::ZoomOut, 50);

        assert!(!state.expire_feedback(49));
        assert_eq!(state.effective_override(), Some(CursorIcon::ZoomOut));
        assert!(state.expire_feedback(50));
        assert_eq!(state.effective_override(), None);
    }
}
