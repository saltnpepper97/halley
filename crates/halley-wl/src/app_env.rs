use std::env;

pub(crate) const DEFAULT_SDL_VIDEODRIVER: &str = "wayland,x11";

pub(crate) fn preferred_sdl_video_driver() -> String {
    preferred_sdl_video_driver_from(env::var("SDL_VIDEODRIVER").ok())
}

fn preferred_sdl_video_driver_from(value: Option<String>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SDL_VIDEODRIVER.to_string())
}

pub(crate) fn set_default_sdl_video_driver() {
    if env::var("SDL_VIDEODRIVER")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return;
    }

    // SAFETY: This is called from startup/session sync paths before spawning
    // user applications that rely on the activation environment.
    unsafe { env::set_var("SDL_VIDEODRIVER", DEFAULT_SDL_VIDEODRIVER) };
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SDL_VIDEODRIVER, preferred_sdl_video_driver_from};

    #[test]
    fn sdl_video_driver_defaults_to_wayland_then_x11() {
        assert_eq!(
            preferred_sdl_video_driver_from(None),
            DEFAULT_SDL_VIDEODRIVER
        );
        assert_eq!(
            preferred_sdl_video_driver_from(Some("   ".to_string())),
            DEFAULT_SDL_VIDEODRIVER
        );
    }

    #[test]
    fn sdl_video_driver_respects_explicit_override() {
        assert_eq!(
            preferred_sdl_video_driver_from(Some("x11".to_string())),
            "x11"
        );
    }
}
