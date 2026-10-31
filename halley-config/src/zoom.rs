use rune_cfg::RuneConfig;

/// A "zoom out" effect - visually shrinks everything toward the screen
/// center. Deliberately capped at 1.0x with no way to zoom in: there's no
/// `max` field here (callers always clamp against a hardcoded `1.0`), and no
/// `smooth` toggle (an instant jump isn't "an effect", so smoothing is
/// always on). Mirrors old halley's `zoom_step`/`zoom_min`/`zoom_smooth_rate`
/// defaults (`halley-config/src/layout/defaults.rs`); its `zoom_max`,
/// `zoom_filter`, and `zoom_sharpen` aren't ported - the first is replaced by
/// the hardcoded 1.0 ceiling, the other two only matter when zooming in past
/// native resolution, which can't happen here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zoom {
    pub enabled: bool,
    pub min: f32,
    pub step: f32,
    pub smooth_rate: f32,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            enabled: true,
            min: 0.35,
            step: 1.10,
            smooth_rate: 12.5,
        }
    }
}

/// Parse the `zoom` section - tolerant of a missing section or missing
/// individual keys (each just falls back to `Zoom`'s default), matching
/// `parse_decorations`'s style.
pub fn parse_zoom(config: &RuneConfig) -> Zoom {
    crate::parse_field(config).zoom
}

/// Loads `Zoom` from the user's config file, falling back to `Zoom::default()`
/// on any failure (missing `$HOME`, unwritable config dir, parse error) -
/// mirrors `load_decorations`'s load-or-default shape.
pub fn load_zoom() -> Zoom {
    let Some(path) = crate::config_path() else {
        eprintln!("zoom: no config path resolvable, using defaults");
        return Zoom::default();
    };

    if let Err(err) = crate::bootstrap_default_config_at(&path) {
        eprintln!("zoom: failed to bootstrap default config: {err}");
    }

    match RuneConfig::from_file(&path) {
        Ok(config) => parse_zoom(&config),
        Err(err) => {
            eprintln!("zoom: failed to load {path:?}, using defaults: {err}");
            Zoom::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zoom_section() {
        let config = RuneConfig::from_str(
            r##"
zoom:
  enabled false
  min 0.5
  step 1.25
  smooth-rate 8.0
end
"##,
        )
        .expect("valid rune-cfg source");

        let zoom = parse_zoom(&config);

        assert_eq!(
            zoom,
            Zoom {
                enabled: false,
                min: 0.5,
                step: 1.25,
                smooth_rate: 8.0,
            }
        );
    }
}
