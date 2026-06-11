//! Pure (no-IO) resolution and command construction for the `gamescope:` config.
//!
//! `halleyctl gamescope` resolves a launch against [`GamescopeConfig`] into a
//! [`GamescopeDecision`], then turns a `Wrap` outcome plus the target monitor's
//! dimensions into a concrete `gamescope … -- <game>` argv. Both steps are pure
//! so they are fully unit-testable without a compositor or the gamescope binary.

use crate::layout::{GamescopeConfig, GamescopeGameProfile};

/// A monitor dimension or refresh value: `"auto"` (resolve from the target
/// monitor) or a fixed number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimSpec {
    Auto,
    Fixed(u32),
}

impl DimSpec {
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return DimSpec::Auto;
        }
        match trimmed.parse::<u32>() {
            Ok(n) => DimSpec::Fixed(n),
            // Be lenient: an unparseable value falls back to auto rather than
            // breaking the launch.
            Err(_) => DimSpec::Auto,
        }
    }

    /// Resolve to a concrete value, using `auto` (the monitor's value) when this
    /// spec is `Auto`.
    pub fn resolve(self, auto: Option<u32>) -> Option<u32> {
        match self {
            DimSpec::Fixed(n) => Some(n),
            DimSpec::Auto => auto,
        }
    }
}

/// A per-game profile after merging a matched `game:` block over global defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedGamescopeProfile {
    pub monitor: String,
    pub output_width: DimSpec,
    pub output_height: DimSpec,
    pub game_width: DimSpec,
    pub game_height: DimSpec,
    pub refresh: DimSpec,
    pub fullscreen: bool,
    pub borderless: bool,
    pub suppress_overlays: bool,
    pub passthrough_pointer_lock: bool,
    pub bypass_spatial_camera: bool,
}

/// Outcome of resolving a launch against the gamescope config.
#[derive(Clone, Debug, PartialEq)]
pub enum GamescopeDecision {
    /// Global `enabled false`: gamescope wrapping is off entirely.
    Disabled,
    /// A matched `game:` profile is `enabled false`: opt out, run unwrapped.
    Skip,
    /// Wrap the launch with the resolved profile.
    Wrap(ResolvedGamescopeProfile),
}

/// The target monitor's dimensions, resolved live from the running compositor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TargetDims {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub refresh_hz: Option<f64>,
}

/// Resolve a launch by `app_id` against the gamescope config. Matching is by
/// explicit `app-id`; the `steam_app_` prefix is only a hint, never a gate.
pub fn resolve_profile(config: &GamescopeConfig, app_id: Option<&str>) -> GamescopeDecision {
    if !config.enabled {
        return GamescopeDecision::Disabled;
    }
    let matched = app_id.and_then(|id| {
        config
            .games
            .iter()
            .find(|game| game.app_id.as_deref() == Some(id))
    });
    if matched.is_some_and(|profile| profile.enabled == Some(false)) {
        return GamescopeDecision::Skip;
    }
    GamescopeDecision::Wrap(merge_profile(config, matched))
}

fn merge_profile(
    config: &GamescopeConfig,
    profile: Option<&GamescopeGameProfile>,
) -> ResolvedGamescopeProfile {
    let str_field = |over: Option<&String>, base: &str| -> String {
        over.cloned().unwrap_or_else(|| base.to_string())
    };
    ResolvedGamescopeProfile {
        monitor: str_field(profile.and_then(|p| p.monitor.as_ref()), &config.monitor),
        output_width: DimSpec::parse(&str_field(
            profile.and_then(|p| p.output_width.as_ref()),
            &config.output_width,
        )),
        output_height: DimSpec::parse(&str_field(
            profile.and_then(|p| p.output_height.as_ref()),
            &config.output_height,
        )),
        game_width: DimSpec::parse(&str_field(
            profile.and_then(|p| p.game_width.as_ref()),
            &config.game_width,
        )),
        game_height: DimSpec::parse(&str_field(
            profile.and_then(|p| p.game_height.as_ref()),
            &config.game_height,
        )),
        refresh: DimSpec::parse(&str_field(
            profile.and_then(|p| p.refresh.as_ref()),
            &config.refresh,
        )),
        fullscreen: profile
            .and_then(|p| p.fullscreen)
            .unwrap_or(config.fullscreen),
        borderless: profile
            .and_then(|p| p.borderless)
            .unwrap_or(config.borderless),
        suppress_overlays: profile
            .and_then(|p| p.suppress_overlays)
            .unwrap_or(config.suppress_overlays),
        passthrough_pointer_lock: profile
            .and_then(|p| p.passthrough_pointer_lock)
            .unwrap_or(config.passthrough_pointer_lock),
        bypass_spatial_camera: profile
            .and_then(|p| p.bypass_spatial_camera)
            .unwrap_or(config.bypass_spatial_camera),
    }
}

/// Build the `gamescope … -- <game_cmd>` argv from a resolved profile plus the
/// target monitor's dimensions. Returns the argv and any non-fatal diagnostics
/// (e.g. a fullscreen/borderless conflict). Flags whose values are unresolved
/// (`auto` with no monitor data) are omitted, letting gamescope auto-detect.
pub fn build_gamescope_argv(
    profile: &ResolvedGamescopeProfile,
    target: &TargetDims,
    game_cmd: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut argv = vec!["gamescope".to_string()];
    let mut diagnostics = Vec::new();

    let mut push_dim = |flag: &str, value: Option<u32>| {
        if let Some(value) = value.filter(|value| *value > 0) {
            argv.push(flag.to_string());
            argv.push(value.to_string());
        }
    };
    push_dim("-W", profile.output_width.resolve(target.width));
    push_dim("-H", profile.output_height.resolve(target.height));
    push_dim("-w", profile.game_width.resolve(target.width));
    push_dim("-h", profile.game_height.resolve(target.height));

    let refresh = match profile.refresh {
        DimSpec::Fixed(n) => Some(n),
        DimSpec::Auto => target.refresh_hz.map(|hz| hz.round() as u32),
    };
    if let Some(refresh) = refresh.filter(|refresh| *refresh > 0) {
        argv.push("-r".to_string());
        argv.push(refresh.to_string());
    }

    // Fullscreen wins on conflict (req #18): deterministic + diagnostic.
    if profile.fullscreen {
        if profile.borderless {
            diagnostics.push(
                "gamescope: both `fullscreen` and `borderless` are set; using fullscreen (-f)"
                    .to_string(),
            );
        }
        argv.push("-f".to_string());
    } else if profile.borderless {
        argv.push("-b".to_string());
    }

    argv.push("--".to_string());
    argv.extend(game_cmd.iter().cloned());
    (argv, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::GamescopeGameProfile;

    fn cfg() -> GamescopeConfig {
        GamescopeConfig::default()
    }

    fn cmd() -> Vec<String> {
        vec![
            "proton".to_string(),
            "run".to_string(),
            "game.exe".to_string(),
        ]
    }

    #[test]
    fn disabled_when_global_off() {
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            resolve_profile(&c, Some("steam_app_1")),
            GamescopeDecision::Disabled
        );
    }

    #[test]
    fn skip_when_profile_opted_out() {
        let mut c = cfg();
        c.games.push(GamescopeGameProfile {
            app_id: Some("steam_app_1".to_string()),
            enabled: Some(false),
            ..Default::default()
        });
        assert_eq!(
            resolve_profile(&c, Some("steam_app_1")),
            GamescopeDecision::Skip
        );
    }

    #[test]
    fn wrap_with_globals_when_no_profile_matches() {
        let c = cfg();
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&c, Some("steam_app_unknown"))
        else {
            panic!("expected wrap");
        };
        assert_eq!(resolved.monitor, "focused");
        assert!(resolved.fullscreen);
        assert_eq!(resolved.output_width, DimSpec::Auto);
    }

    #[test]
    fn profile_overrides_inherit_globals() {
        let mut c = cfg();
        c.fullscreen = true;
        c.games.push(GamescopeGameProfile {
            app_id: Some("steam_app_1".to_string()),
            fullscreen: Some(false),
            borderless: Some(true),
            game_width: Some("1920".to_string()),
            ..Default::default()
        });
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&c, Some("steam_app_1")) else {
            panic!("expected wrap");
        };
        assert!(!resolved.fullscreen); // overridden
        assert!(resolved.borderless); // overridden
        assert_eq!(resolved.game_width, DimSpec::Fixed(1920)); // overridden
        assert_eq!(resolved.monitor, "focused"); // inherited
        assert_eq!(resolved.refresh, DimSpec::Auto); // inherited
    }

    #[test]
    fn build_auto_dims_uses_target_and_fullscreen() {
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&cfg(), None) else {
            panic!("expected wrap");
        };
        let target = TargetDims {
            width: Some(2560),
            height: Some(1440),
            refresh_hz: Some(143.97),
        };
        let (argv, diags) = build_gamescope_argv(&resolved, &target, &cmd());
        assert_eq!(
            argv,
            vec![
                "gamescope",
                "-W",
                "2560",
                "-H",
                "1440",
                "-w",
                "2560",
                "-h",
                "1440",
                "-r",
                "144",
                "-f",
                "--",
                "proton",
                "run",
                "game.exe"
            ]
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn build_explicit_dims_override_auto() {
        let mut c = cfg();
        c.output_width = "3840".to_string();
        c.output_height = "2160".to_string();
        c.game_width = "1920".to_string();
        c.game_height = "1080".to_string();
        c.refresh = "60".to_string();
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&c, None) else {
            panic!("expected wrap");
        };
        let (argv, _) = build_gamescope_argv(&resolved, &TargetDims::default(), &cmd());
        assert_eq!(
            argv[..11].to_vec(),
            vec![
                "gamescope",
                "-W",
                "3840",
                "-H",
                "2160",
                "-w",
                "1920",
                "-h",
                "1080",
                "-r",
                "60"
            ]
        );
    }

    #[test]
    fn build_borderless_when_not_fullscreen() {
        let mut c = cfg();
        c.fullscreen = false;
        c.borderless = true;
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&c, None) else {
            panic!("expected wrap");
        };
        let (argv, diags) = build_gamescope_argv(&resolved, &TargetDims::default(), &cmd());
        assert!(argv.contains(&"-b".to_string()));
        assert!(!argv.contains(&"-f".to_string()));
        assert!(diags.is_empty());
    }

    #[test]
    fn build_conflict_prefers_fullscreen_with_diagnostic() {
        let mut c = cfg();
        c.fullscreen = true;
        c.borderless = true;
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&c, None) else {
            panic!("expected wrap");
        };
        let (argv, diags) = build_gamescope_argv(&resolved, &TargetDims::default(), &cmd());
        assert!(argv.contains(&"-f".to_string()));
        assert!(!argv.contains(&"-b".to_string()));
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn build_omits_unresolved_auto_dims() {
        let GamescopeDecision::Wrap(resolved) = resolve_profile(&cfg(), None) else {
            panic!("expected wrap");
        };
        let (argv, _) = build_gamescope_argv(&resolved, &TargetDims::default(), &cmd());
        // No monitor data + all-auto ⇒ no -W/-H/-w/-h/-r, just -f and the command.
        assert_eq!(
            argv,
            vec!["gamescope", "-f", "--", "proton", "run", "game.exe"]
        );
    }
}
