use std::env;

/// Return only an explicit user choice. Halley must not set SDL's backend
/// policy: SDL selects the native Wayland or X11 driver appropriate to the
/// application and session.
pub(crate) fn explicit_sdl_video_driver() -> Option<String> {
    explicit_sdl_video_driver_from(env::var("SDL_VIDEODRIVER").ok())
}

fn explicit_sdl_video_driver_from(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::explicit_sdl_video_driver_from;

    #[test]
    fn sdl_video_driver_is_forwarded_only_when_explicit() {
        assert_eq!(explicit_sdl_video_driver_from(None), None);
        assert_eq!(
            explicit_sdl_video_driver_from(Some("   ".to_string())),
            None
        );
        assert_eq!(
            explicit_sdl_video_driver_from(Some("x11".to_string())).as_deref(),
            Some("x11")
        );
    }
}
