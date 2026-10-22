use crate::wayland::fullscreen::FullscreenOrigin;

/// Identifies who owns an X11 fullscreen transition.
///
/// Ownership determines whether the client may negotiate geometry and which
/// presentation semantics the shared fullscreen manager applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FullscreenRequestOrigin {
    Initial,
    Client,
    Compositor,
    Maximize,
}

impl FullscreenRequestOrigin {
    pub(super) fn presentation_origin(self) -> FullscreenOrigin {
        match self {
            Self::Compositor => FullscreenOrigin::Compositor,
            Self::Maximize => FullscreenOrigin::Maximize,
            Self::Initial | Self::Client => FullscreenOrigin::Client,
        }
    }

    pub(super) fn client_owns_geometry(self) -> bool {
        matches!(self, Self::Initial | Self::Client)
    }
}

/// An explicit game presentation is independent of the game's video-mode
/// preference. Both explicit entry and exit survive client mode replies until
/// the next compositor toggle or the window's withdrawal.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GameFullscreenControlState {
    compositor_requested: Option<bool>,
    fullscreen: bool,
}

impl GameFullscreenControlState {
    pub(super) fn resolve(&mut self, fullscreen: bool, origin: FullscreenRequestOrigin) -> bool {
        let resolved = match origin {
            FullscreenRequestOrigin::Compositor => {
                self.compositor_requested = Some(fullscreen);
                fullscreen
            }
            FullscreenRequestOrigin::Maximize => {
                self.compositor_requested = None;
                fullscreen
            }
            FullscreenRequestOrigin::Initial | FullscreenRequestOrigin::Client => {
                self.compositor_requested.unwrap_or(fullscreen)
            }
        };
        self.fullscreen = resolved && origin != FullscreenRequestOrigin::Maximize;
        resolved
    }

    pub(super) fn fullscreen(self) -> bool {
        self.fullscreen
    }

    pub(super) fn preserve_startup_fullscreen(&mut self) {
        self.fullscreen = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExternalPresentationPolicy {
    Initial,
    Opening,
    Confined,
    Animated,
    LiveAnimated,
}

impl ExternalPresentationPolicy {
    pub(super) fn select(
        origin: FullscreenRequestOrigin,
        opening: bool,
        confined: bool,
        game: bool,
    ) -> Self {
        if game && !opening && origin != FullscreenRequestOrigin::Initial {
            Self::LiveAnimated
        } else if confined {
            Self::Confined
        } else if origin == FullscreenRequestOrigin::Initial {
            Self::Initial
        } else if opening && origin == FullscreenRequestOrigin::Client {
            // The window-open animation already owns the visual motion. The
            // client geometry itself must settle directly beneath it.
            Self::Initial
        } else if opening {
            Self::Opening
        } else {
            Self::Animated
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Opening => "opening",
            Self::Confined => "confined",
            Self::Animated => "animated",
            Self::LiveAnimated => "live-animated",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExternalPresentationPolicy, FullscreenRequestOrigin};
    use crate::wayland::fullscreen::FullscreenOrigin;

    #[test]
    fn game_explicit_presentation_survives_client_chatter_until_explicit_exit() {
        use super::GameFullscreenControlState;
        let mut state = GameFullscreenControlState::default();
        assert!(!state.resolve(false, FullscreenRequestOrigin::Client));
        assert!(state.resolve(true, FullscreenRequestOrigin::Compositor));
        assert!(state.resolve(false, FullscreenRequestOrigin::Client));
        assert!(state.resolve(true, FullscreenRequestOrigin::Client));
        assert!(!state.resolve(false, FullscreenRequestOrigin::Compositor));
        assert!(!state.resolve(false, FullscreenRequestOrigin::Client));
        assert!(!state.resolve(true, FullscreenRequestOrigin::Client));
        assert!(!state.resolve(true, FullscreenRequestOrigin::Initial));
        assert!(state.resolve(true, FullscreenRequestOrigin::Compositor));
        // A new map lifecycle starts without either explicit override.
        state = GameFullscreenControlState::default();
        assert!(state.resolve(true, FullscreenRequestOrigin::Client));
        assert!(!state.resolve(false, FullscreenRequestOrigin::Client));
    }

    #[test]
    fn external_fullscreen_origin_preserves_compositor_ownership() {
        assert_eq!(
            FullscreenRequestOrigin::Compositor.presentation_origin(),
            FullscreenOrigin::Compositor
        );
        assert_eq!(
            FullscreenRequestOrigin::Maximize.presentation_origin(),
            FullscreenOrigin::Maximize
        );
        for origin in [
            FullscreenRequestOrigin::Initial,
            FullscreenRequestOrigin::Client,
        ] {
            assert_eq!(origin.presentation_origin(), FullscreenOrigin::Client);
        }
    }

    #[test]
    fn only_client_fullscreen_requests_own_x11_geometry_negotiation() {
        assert!(FullscreenRequestOrigin::Initial.client_owns_geometry());
        assert!(FullscreenRequestOrigin::Client.client_owns_geometry());
        assert!(!FullscreenRequestOrigin::Compositor.client_owns_geometry());
        assert!(!FullscreenRequestOrigin::Maximize.client_owns_geometry());
    }

    #[test]
    fn initial_fullscreen_always_uses_the_direct_settle() {
        assert_eq!(
            ExternalPresentationPolicy::select(
                FullscreenRequestOrigin::Initial,
                false,
                false,
                false
            ),
            ExternalPresentationPolicy::Initial
        );
        assert_eq!(
            ExternalPresentationPolicy::select(
                FullscreenRequestOrigin::Initial,
                true,
                false,
                false
            ),
            ExternalPresentationPolicy::Initial
        );
    }

    #[test]
    fn client_fullscreen_settles_under_the_existing_window_open_animation() {
        assert_eq!(
            ExternalPresentationPolicy::select(FullscreenRequestOrigin::Client, true, false, false),
            ExternalPresentationPolicy::Initial
        );
        assert_eq!(
            ExternalPresentationPolicy::select(
                FullscreenRequestOrigin::Compositor,
                true,
                false,
                false
            ),
            ExternalPresentationPolicy::Opening
        );
    }

    #[test]
    fn confined_fullscreen_never_animates() {
        for origin in [
            FullscreenRequestOrigin::Initial,
            FullscreenRequestOrigin::Client,
            FullscreenRequestOrigin::Compositor,
        ] {
            assert_eq!(
                ExternalPresentationPolicy::select(origin, true, true, false),
                ExternalPresentationPolicy::Confined
            );
        }
    }

    #[test]
    fn settled_or_locked_fullscreen_animates_when_not_confined() {
        for origin in [
            FullscreenRequestOrigin::Client,
            FullscreenRequestOrigin::Compositor,
        ] {
            assert_eq!(
                ExternalPresentationPolicy::select(origin, false, false, false),
                ExternalPresentationPolicy::Animated
            );
        }
    }

    #[test]
    fn compositor_fullscreen_exit_uses_the_animated_snapshot_path() {
        assert_eq!(
            ExternalPresentationPolicy::select(
                FullscreenRequestOrigin::Compositor,
                false,
                false,
                false
            ),
            ExternalPresentationPolicy::Animated
        );
    }

    #[test]
    fn settled_games_keep_geometry_motion_with_live_buffers_even_when_confined() {
        for origin in [
            FullscreenRequestOrigin::Client,
            FullscreenRequestOrigin::Compositor,
        ] {
            for confined in [false, true] {
                assert_eq!(
                    ExternalPresentationPolicy::select(origin, false, confined, true),
                    ExternalPresentationPolicy::LiveAnimated
                );
            }
        }
        assert_eq!(
            ExternalPresentationPolicy::select(
                FullscreenRequestOrigin::Initial,
                false,
                false,
                true
            ),
            ExternalPresentationPolicy::Initial
        );
        assert_eq!(
            ExternalPresentationPolicy::select(FullscreenRequestOrigin::Client, true, false, true),
            ExternalPresentationPolicy::Initial
        );
    }
}
