use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The default config file contents are identical to the shipped top-level
/// example. Keeping one canonical template for the whole workspace prevents
/// fresh-install behavior and user-facing documentation from drifting apart.
pub const DEFAULT_CONFIG: &str = include_str!("../halley.default.rune");

/// Resolve the config file path: `$XDG_CONFIG_HOME/halley/halley.rune`,
/// falling back to `$HOME/.config/halley/halley.rune` when
/// `XDG_CONFIG_HOME` is unset. Returns `None` if neither env var is set -
/// no sensible home to put a config in.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("halley").join("halley.rune"))
}

/// If no config file exists yet at `config_path()`, write `DEFAULT_CONFIG`
/// there (creating parent directories as needed). Returns `Ok(true)` if a
/// file was actually written, `Ok(false)` if one already existed or the
/// path couldn't be resolved. Never overwrites an existing config - a
/// user's edits are never at risk from this.
pub fn bootstrap_default_config() -> io::Result<bool> {
    let Some(path) = config_path() else {
        return Ok(false);
    };
    bootstrap_default_config_at(&path)
}

/// Same as `bootstrap_default_config`, but against an explicit path - the
/// actual logic, factored out so it's testable against a temp directory
/// instead of the real `$HOME`.
pub fn bootstrap_default_config_at(path: &Path) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_CONFIG)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_cfg::RuneConfig;

    /// A fresh, uniquely-named scratch directory under the OS temp dir,
    /// scoped to one test so parallel test runs never collide.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(test_name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "halley-config-test-{}-{}-{}",
                std::process::id(),
                test_name,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_config_matches_shipped_example() {
        let example = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/halley.rune"
        ))
        .expect("shipped example exists in the workspace");
        assert_eq!(DEFAULT_CONFIG, example);
    }

    #[test]
    fn writes_default_config_when_absent() {
        let scratch = ScratchDir::new("writes_default_config_when_absent");
        let config_file = scratch.path().join("halley").join("halley.rune");

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(wrote);
        assert!(config_file.exists());
        assert_eq!(fs::read_to_string(&config_file).unwrap(), DEFAULT_CONFIG);
    }

    #[test]
    fn does_not_overwrite_existing_config() {
        let scratch = ScratchDir::new("does_not_overwrite_existing_config");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, "keybinds:\n  mod \"alt\"\nend\n").unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote);
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            "keybinds:\n  mod \"alt\"\nend\n"
        );
    }

    #[test]
    fn creates_missing_parent_directories() {
        let scratch = ScratchDir::new("creates_missing_parent_directories");
        let config_file = scratch
            .path()
            .join("nested")
            .join("halley")
            .join("halley.rune");
        assert!(!config_file.parent().unwrap().exists());

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(wrote);
        assert!(config_file.exists());
    }

    #[test]
    fn template_contains_overview_and_old_halley_controls() {
        for expected in [
            "\"$var.mod+d\" \"halley-lift\"",
            "# \"$var.mod+d\" \"fuzzel\"",
            "\"$var.mod+n\" \"toggle-state\"",
            "\"$var.mod+o\" \"apogee\"",
            "\"alt+tab\" \"cycle-focus\"",
            "\"$var.mod+h\" \"center-last-focused\"",
            "\"$var.mod+p\" \"toggle-focused-pin\"",
            "\"$var.mod+left\" \"focus-left\"",
            "\"$var.mod+ctrl+right\" \"cluster-tile-swap-right\"",
            "\"$var.mod+shift+up\" \"monitor-focus up\"",
            "\"$var.mod+a\" \"arrange-visible\"",
            "\"$var.mod+shift+click-left\" \"drag-pan\"",
            "live-previews true",
            "max-rows 3",
            "overlays:",
            "border-size 3",
            "border-colour \"#d65d26\"",
            "notifications:",
            "success-duration-ms 4000",
            "zoom-indicator:",
            "hold-duration-ms 750",
            "fade-duration-ms 180",
            "background true",
            "opacity 1.0",
            "# text-size 18",
            "# text-colour \"auto\"",
            "# background-colour \"auto\"",
            "# border-colour \"#d65d26\"",
            "# borders true",
            "# radius 8",
            "\"retract\" - reverse \"launch\"",
            "close-restore-nodes false",
            "maximize:",
            "motion \"easing\"",
            "duration-ms 240",
            "collapse-duration-ms 280",
            "damping-ratio 1.0",
            "stiffness 800.0",
            "bloom-direction \"clockwise\"",
            "border-colour \"#474d59\"",
            "border-colour-highlighted \"#d65d26\"",
            "resize-using-border true",
            "hide-on-keyboard-nav true",
            "pins:",
            "corner \"top-right\"",
            "colour \"#d65d26\"",
            "background-colour \"auto\"",
            "size 1.0",
            "titlebars:",
            "colour-focused \"#f4f5f7\"",
            "button-position \"right\"",
            "title-position \"center\"",
            "colour-focused \"#d65d26\"",
            "show-icons false",
            "show-title true",
            "height 32",
            "XF86AudioRaiseVolume\" \"wpctl",
            "with repeat true",
            "overlay-fps false",
        ] {
            assert!(
                DEFAULT_CONFIG.contains(expected),
                "bootstrap template is missing {expected:?}"
            );
        }
        assert!(!DEFAULT_CONFIG.contains("border-colour-hover"));
        assert!(!DEFAULT_CONFIG.contains("border-colour-inactive"));
        assert!(!DEFAULT_CONFIG.contains("gaming:"));
        assert!(!DEFAULT_CONFIG.contains("gamescope"));
        assert!(!DEFAULT_CONFIG.contains("halley-config-version"));
        assert!(DEFAULT_CONFIG.contains("Startup never\n# rewrites an existing config"));
    }

    /// Milestone 1: fresh installations start on an empty Field. The shipped
    /// template must declare no active startup clusters, so bootstrap never
    /// pre-creates numbered workspace cores for a new user.
    #[test]
    fn template_starts_field_first_without_startup_clusters() {
        let config = RuneConfig::from_str(DEFAULT_CONFIG).expect("bootstrap template parses");
        let autostart = crate::parse_autostart(&config).expect("bootstrap autostart parses");

        assert!(
            autostart.once.is_empty(),
            "a fresh config must not launch session services automatically"
        );
        assert!(
            autostart.on_reload.is_empty(),
            "a fresh config must not run reload commands"
        );
        assert!(
            autostart.clusters.is_empty(),
            "a fresh config must not pre-create cluster workspaces: {:?}",
            autostart.clusters
        );
    }

    /// Startup-cluster syntax is still documented in the template, but only as
    /// an inert commented example that cannot create cores on its own.
    #[test]
    fn template_keeps_only_a_commented_startup_cluster_example() {
        assert!(
            DEFAULT_CONFIG.contains("  # cluster:\n  #   name \"Work\"\n  #   members []\n  # end"),
            "the template keeps one concise commented startup-cluster example"
        );
        assert!(
            DEFAULT_CONFIG.contains("docs/clusters.md"),
            "the template points at docs/clusters.md for complete syntax"
        );
    }

    /// Milestone 1 boundary: bootstrap never rewrites an existing config, even
    /// when that config deliberately declares startup clusters.
    #[test]
    fn does_not_rewrite_existing_config_with_startup_clusters() {
        const EXISTING: &str = concat!(
            "keybinds:\n",
            "  mod \"alt\"\n",
            "end\n",
            "\n",
            "autostart:\n",
            "  cluster:\n",
            "    name \"Work\"\n",
            "    members []\n",
            "  end\n",
            "end\n",
        );

        let scratch = ScratchDir::new("does_not_rewrite_existing_config_with_startup_clusters");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, EXISTING).unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote, "bootstrap must not write when a config exists");
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            EXISTING,
            "the existing config must remain byte-for-byte unchanged"
        );

        let config = RuneConfig::from_str(EXISTING).expect("existing config parses");
        let autostart = crate::parse_autostart(&config).expect("existing autostart parses");
        assert_eq!(autostart.clusters.len(), 1);
        assert_eq!(autostart.clusters[0].name, "Work");
        assert!(autostart.clusters[0].members.is_empty());
        assert_eq!(autostart.clusters[0].output, None);
    }

    #[test]
    fn template_uses_ring_only_view_entries() {
        let config = RuneConfig::from_str(DEFAULT_CONFIG).expect("bootstrap template parses");
        let view = crate::parse_view_checked(&config).expect("bootstrap view parses");

        assert!(view.outputs.is_empty());
        assert_eq!(view.focus_rings.by_output.len(), 2);
        assert!(view.focus_rings.by_output.contains_key("DP-1"));
        assert!(view.focus_rings.by_output.contains_key("DP-2"));
    }

    /// Milestone 2: a fresh installation's front door is Halley Lift. The
    /// generated config launches the bundled search and action launcher on
    /// `Mod+D` and keeps Fuzzel only as a commented, opt-in alternative.
    #[test]
    fn template_binds_mod_d_to_halley_lift_with_fuzzel_as_the_alternative() {
        let config = RuneConfig::from_str(DEFAULT_CONFIG).expect("bootstrap template parses");
        let keybinds = crate::parse_keybinds(&config).expect("bootstrap keybinds parse");

        let mut mod_d = keybinds
            .binds
            .iter()
            .filter(|bind| bind.key == "d" && bind.modifiers.super_key);
        let launcher = mod_d.next().expect("a fresh config binds Mod+D");
        assert_eq!(
            launcher.action,
            crate::Action::Spawn("halley-lift".to_string()),
            "Mod+D must launch Halley Lift in a fresh config"
        );
        assert!(
            mod_d.next().is_none(),
            "the generated config activates Mod+D exactly once"
        );
        assert!(
            !keybinds
                .binds
                .iter()
                .any(|bind| bind.action == crate::Action::Spawn("fuzzel".to_string())),
            "Fuzzel must stay a documented alternative, not the default launcher"
        );
        assert!(
            DEFAULT_CONFIG.contains("  # \"$var.mod+d\" \"fuzzel\"\n"),
            "the commented Fuzzel alternative stays in the generated template"
        );

        // The generated file, not just the embedded template, must survive the
        // compositor's own runtime loading path with the Lift binding intact.
        let scratch = ScratchDir::new("template_binds_mod_d_to_halley_lift");
        let config_file = scratch.path().join("halley").join("halley.rune");
        assert!(
            bootstrap_default_config_at(&config_file).unwrap(),
            "a fresh install writes the generated config"
        );
        let runtime = crate::load_runtime_config_at(&config_file).expect("generated config loads");
        assert_eq!(
            runtime
                .keybinds
                .binds
                .iter()
                .find(|bind| bind.key == "d" && bind.modifiers.super_key)
                .expect("generated config binds Mod+D")
                .action,
            crate::Action::Spawn("halley-lift".to_string())
        );
    }

    /// Milestone 2 boundary: bootstrap only ever creates a missing config, so
    /// an existing Fuzzel (or any other) launcher binding is never rewritten.
    #[test]
    fn does_not_rewrite_an_existing_fuzzel_launcher_binding() {
        const EXISTING: &str = concat!(
            "keybinds:\n",
            "  mod \"super\"\n",
            "  \"$var.mod+d\" \"fuzzel\"\n",
            "end\n",
        );

        let scratch = ScratchDir::new("does_not_rewrite_an_existing_fuzzel_launcher_binding");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, EXISTING).unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote, "bootstrap must not write when a config exists");
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            EXISTING,
            "an existing launcher binding must stay byte-for-byte unchanged"
        );

        let config = RuneConfig::from_str(EXISTING).expect("existing config parses");
        let keybinds = crate::parse_keybinds(&config).expect("existing keybinds parse");
        assert_eq!(
            keybinds
                .binds
                .iter()
                .find(|bind| bind.key == "d" && bind.modifiers.super_key)
                .expect("existing Mod+D bind")
                .action,
            crate::Action::Spawn("fuzzel".to_string()),
            "existing users keep their own launcher binding"
        );
    }

    /// Milestone 4: a fresh installation gets conservative decay defaults —
    /// 10 minutes outside the focus ring and 90 minutes inside it — instead of
    /// Halley's shorter built-in 3/30 minutes. The values are checked through
    /// the generated file's own runtime loading path, not just the template.
    #[test]
    fn generated_config_uses_conservative_decay_delays() {
        let scratch = ScratchDir::new("generated_config_uses_conservative_decay_delays");
        let config_file = scratch.path().join("halley").join("halley.rune");
        assert!(
            bootstrap_default_config_at(&config_file).unwrap(),
            "a fresh install writes the generated config"
        );

        let runtime = crate::load_runtime_config_at(&config_file).expect("generated config loads");
        assert!(runtime.decay.enabled);
        assert_eq!(
            runtime.decay.outside_delay_seconds, 600,
            "outside the focus ring a fresh config decays after 10 minutes"
        );
        assert_eq!(
            runtime.decay.inside_delay_seconds, 5_400,
            "inside the focus ring a fresh config decays after 90 minutes"
        );

        let config = RuneConfig::from_str(DEFAULT_CONFIG).expect("bootstrap template parses");
        assert_eq!(crate::parse_decay(&config), runtime.decay);
    }

    /// Milestone 4 boundary: the longer delays are not a migration. An existing
    /// config that states its own decay values is never rewritten, and a config
    /// that omits the section keeps Halley's built-in delays.
    #[test]
    fn existing_configs_keep_their_own_decay_values() {
        const EXISTING: &str = concat!(
            "decay:\n",
            "  enabled true\n",
            "  outside-delay-seconds 45\n",
            "  inside-delay-seconds 90\n",
            "end\n",
            "\n",
            "keybinds:\n",
            "  mod \"super\"\n",
            "end\n",
        );

        let scratch = ScratchDir::new("existing_configs_keep_their_own_decay_values");
        let config_file = scratch.path().join("halley").join("halley.rune");
        fs::create_dir_all(config_file.parent().unwrap()).unwrap();
        fs::write(&config_file, EXISTING).unwrap();

        let wrote = bootstrap_default_config_at(&config_file).unwrap();

        assert!(!wrote, "bootstrap must not write when a config exists");
        assert_eq!(
            fs::read_to_string(&config_file).unwrap(),
            EXISTING,
            "an existing decay section must stay byte-for-byte unchanged"
        );

        let runtime = crate::load_runtime_config_at(&config_file).expect("existing config loads");
        assert_eq!(runtime.decay.outside_delay_seconds, 45);
        assert_eq!(runtime.decay.inside_delay_seconds, 90);
    }
}
