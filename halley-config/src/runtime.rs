use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use rune_cfg::RuneConfig;

use crate::{
    Animations, Apogee, Autostart, Background, BackgroundParseError, Bearings, Clusters, Cursor,
    Debug, Decay, Decorations, Effects, EffectsParseError, Field, FieldParseError, FocusRings,
    Font, Input, InputParseError, Keybinds, LaunchConfigError, LayerRule, NodeParseError, Nodes,
    OutputConfig, OverlayParseError, Overlays, Physics, Screenshot, Trail, WindowRule,
    WindowRuleParseError, parse_animations, parse_apogee, parse_autostart, parse_background,
    parse_bearings, parse_clusters, parse_cursor, parse_debug, parse_decay, parse_decorations,
    parse_effects, parse_env, parse_field_checked, parse_font, parse_input, parse_keybinds,
    parse_nodes_checked, parse_overlays_checked, parse_physics, parse_rules, parse_screenshot,
    parse_trail, parse_view_checked,
};
use crate::{ViewConfig, ViewParseError};

/// One validated snapshot of every setting the running compositor currently
/// understands. Loading the file once avoids independently parsing the same
/// bytes for each subsystem and gives live reload a single atomic unit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeConfig {
    pub env: BTreeMap<String, String>,
    pub autostart: Autostart,
    pub keybinds: Keybinds,
    pub decorations: Decorations,
    pub background: Background,
    pub window_rules: Vec<WindowRule>,
    pub layer_rules: Vec<LayerRule>,
    pub field: Field,
    pub screenshot: Screenshot,
    pub cursor: Cursor,
    pub input: Input,
    pub animations: Animations,
    pub apogee: Apogee,
    pub bearings: Bearings,
    pub trail: Trail,
    pub clusters: Clusters,
    pub focus_rings: FocusRings,
    pub font: Font,
    pub physics: Physics,
    pub decay: Decay,
    pub nodes: Nodes,
    pub overlays: Overlays,
    pub effects: Effects,
    pub debug: Debug,
    pub outputs: Vec<OutputConfig>,
}

#[derive(Debug)]
pub enum RuntimeConfigError {
    Rune(rune_cfg::RuneError),
    Keybind(crate::ParseError),
    Launch(LaunchConfigError),
    Input(InputParseError),
    View(ViewParseError),
    Node(NodeParseError),
    Overlay(OverlayParseError),
    Effects(EffectsParseError),
    Field(FieldParseError),
    Background(BackgroundParseError),
    Rule(WindowRuleParseError),
}

impl fmt::Display for RuntimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rune(err) => write!(f, "{err}"),
            Self::Keybind(err) => write!(f, "{err}"),
            Self::Launch(err) => write!(f, "{err}"),
            Self::Input(err) => write!(f, "{err}"),
            Self::View(err) => write!(f, "{err}"),
            Self::Node(err) => write!(f, "{err}"),
            Self::Overlay(err) => write!(f, "{err}"),
            Self::Effects(err) => write!(f, "{err}"),
            Self::Field(err) => write!(f, "{err}"),
            Self::Background(err) => write!(f, "{err}"),
            Self::Rule(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuntimeConfigError {}

impl From<rune_cfg::RuneError> for RuntimeConfigError {
    fn from(value: rune_cfg::RuneError) -> Self {
        Self::Rune(value)
    }
}

impl From<crate::ParseError> for RuntimeConfigError {
    fn from(value: crate::ParseError) -> Self {
        Self::Keybind(value)
    }
}

impl From<LaunchConfigError> for RuntimeConfigError {
    fn from(value: LaunchConfigError) -> Self {
        Self::Launch(value)
    }
}

impl From<InputParseError> for RuntimeConfigError {
    fn from(value: InputParseError) -> Self {
        Self::Input(value)
    }
}

impl From<ViewParseError> for RuntimeConfigError {
    fn from(value: ViewParseError) -> Self {
        Self::View(value)
    }
}

impl From<NodeParseError> for RuntimeConfigError {
    fn from(value: NodeParseError) -> Self {
        Self::Node(value)
    }
}

impl From<OverlayParseError> for RuntimeConfigError {
    fn from(value: OverlayParseError) -> Self {
        Self::Overlay(value)
    }
}

impl From<EffectsParseError> for RuntimeConfigError {
    fn from(value: EffectsParseError) -> Self {
        Self::Effects(value)
    }
}

impl From<FieldParseError> for RuntimeConfigError {
    fn from(value: FieldParseError) -> Self {
        Self::Field(value)
    }
}

impl From<BackgroundParseError> for RuntimeConfigError {
    fn from(value: BackgroundParseError) -> Self {
        Self::Background(value)
    }
}

impl From<WindowRuleParseError> for RuntimeConfigError {
    fn from(value: WindowRuleParseError) -> Self {
        Self::Rule(value)
    }
}

pub fn parse_runtime_config(config: &RuneConfig) -> Result<RuntimeConfig, RuntimeConfigError> {
    let ViewConfig {
        outputs,
        focus_rings,
    } = parse_view_checked(config)?;
    let rules = parse_rules(config)?;
    Ok(RuntimeConfig {
        env: parse_env(config)?,
        autostart: parse_autostart(config)?,
        keybinds: parse_keybinds(config)?,
        decorations: parse_decorations(config),
        background: parse_background(config)?,
        window_rules: rules.windows,
        layer_rules: rules.layers,
        field: parse_field_checked(config)?,
        screenshot: parse_screenshot(config),
        cursor: parse_cursor(config),
        input: parse_input(config)?,
        animations: parse_animations(config),
        apogee: parse_apogee(config),
        bearings: parse_bearings(config),
        trail: parse_trail(config),
        clusters: parse_clusters(config),
        focus_rings,
        font: parse_font(config),
        physics: parse_physics(config),
        decay: parse_decay(config),
        nodes: parse_nodes_checked(config)?,
        overlays: parse_overlays_checked(config)?,
        effects: parse_effects(config)?,
        debug: parse_debug(config),
        outputs,
    })
}

pub fn load_runtime_config_at(path: &Path) -> Result<RuntimeConfig, RuntimeConfigError> {
    let config = RuneConfig::from_file(path)?;
    parse_runtime_config(&config)
}

/// Strict, read-only load used by both the compositor watcher and
/// `halleyctl config verify`.
///
/// The structured diagnostic intentionally stays inline in the public API so
/// callers retain the existing error type and source-location behavior.
#[allow(clippy::result_large_err)]
pub fn load_runtime_config_diagnostic_at(
    path: &Path,
) -> Result<RuntimeConfig, crate::ConfigDiagnostic> {
    load_runtime_config_at(path)
        .map_err(|error| crate::ConfigDiagnostic::from_runtime_error(path, &error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_supported_sections_as_one_snapshot() {
        let config = RuneConfig::from_str(
            r##"
view:
  output:
    name "DP-1"
    width 2560
    height 1440
  end
end

zoom:
  enabled false
end

screenshot:
  directory "/tmp/screenshots"
end

cursor:
  theme "Breeze"
  size 32
  hide-when-typing true
  hide-on-touch false
  hide-after-ms 750
end

input:
  repeat-rate 45
  focus-mode "hover"
end

env:
  QT_QPA_PLATFORM "wayland"
end

autostart:
  once "waybar"
  once "mako"
  on-reload "notify-send reloaded"
end

decorations:
  border:
    size 7
  end
end

animations:
  enabled false
end

keybinds:
  mod "super"
  "$var.mod+t" "open-terminal"
end
"##,
        )
        .expect("valid Rune config");

        let runtime = parse_runtime_config(&config).unwrap();
        assert_eq!(
            runtime.env,
            BTreeMap::from([("QT_QPA_PLATFORM".to_string(), "wayland".to_string())])
        );
        assert_eq!(runtime.autostart.once, ["waybar", "mako"]);
        assert_eq!(runtime.autostart.on_reload, ["notify-send reloaded"]);
        assert_eq!(runtime.outputs.len(), 1);
        assert!(!runtime.field.zoom.enabled);
        assert_eq!(runtime.screenshot.directory, "/tmp/screenshots");
        assert_eq!(runtime.cursor.theme, "Breeze");
        assert_eq!(runtime.cursor.size, 32);
        assert!(runtime.cursor.hide_when_typing);
        assert!(!runtime.cursor.hide_on_touch);
        assert_eq!(runtime.cursor.hide_after_ms, Some(750));
        assert_eq!(runtime.input.repeat_rate, 45);
        assert_eq!(runtime.input.focus_mode, crate::FocusMode::Hover);
        assert_eq!(runtime.decorations.border_width_px, 7);
        assert!(!runtime.animations.enabled);
        assert_eq!(runtime.keybinds.binds.len(), 1);
    }

    #[test]
    fn rejects_an_incomplete_output_block() {
        let config = RuneConfig::from_str(
            r##"
view:
  output:
    name "DP-1"
    width 2560
  end
end

keybinds:
  mod "super"
end
"##,
        )
        .expect("syntactically valid Rune config");

        assert!(matches!(
            parse_runtime_config(&config),
            Err(RuntimeConfigError::View(_))
        ));
    }

    #[test]
    fn valid_keybind_section_is_authoritative_without_embedded_defaults() {
        let config = RuneConfig::from_str(
            r#"
keybinds:
  mod "super"
  "$var.mod+x" "open-terminal"
end
"#,
        )
        .expect("valid Rune config");

        let runtime = parse_runtime_config(&config).unwrap();
        assert_eq!(runtime.keybinds.binds.len(), 1);
        assert_eq!(runtime.keybinds.binds[0].key, "x");
        assert_eq!(
            runtime.keybinds.binds[0].action,
            crate::Action::OpenTerminal
        );
    }

    #[test]
    fn empty_config_is_invalid_while_runtime_default_is_complete() {
        let empty = RuneConfig::from_str("").expect("empty Rune source is syntactically valid");

        assert!(parse_runtime_config(&empty).is_err());
        assert!(!RuntimeConfig::default().keybinds.binds.is_empty());
    }
}
