use std::collections::BTreeMap;
use std::fmt;

use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRing {
    pub radius_x: f32,
    pub radius_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FocusRings {
    pub fallback: FocusRing,
    pub by_output: BTreeMap<String, FocusRing>,
}

impl FocusRings {
    pub fn for_output(&self, output: &str) -> FocusRing {
        self.by_output.get(output).copied().unwrap_or(self.fallback)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusRingParseError(String);

impl fmt::Display for FocusRingParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FocusRingParseError {}

impl Default for FocusRing {
    fn default() -> Self {
        Self {
            radius_x: 820.0,
            radius_y: 420.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decay {
    pub enabled: bool,
    pub outside_delay_seconds: u64,
    pub inside_delay_seconds: u64,
}

impl Default for Decay {
    /// The built-in delays used when a configuration omits the `decay:`
    /// section. They deliberately stay at 0.8.0's original 3 and 30 minutes:
    /// a config that predates the release must keep behaving exactly as it did.
    /// Only a configuration Halley generates itself carries the longer
    /// conservative defaults (see `halley.default.rune`).
    fn default() -> Self {
        Self {
            enabled: true,
            outside_delay_seconds: 180,
            inside_delay_seconds: 1_800,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeShape {
    Square,
    #[default]
    Squircle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeDisplayPolicy {
    Off,
    Hover,
    #[default]
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestoreCentering {
    #[default]
    Never,
    IfOffscreen,
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum NodeBackgroundColor {
    #[default]
    Auto,
    System,
    Light,
    Dark,
    Fixed(f32, f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nodes {
    pub shape: NodeShape,
    pub label_shape: NodeShape,
    pub show_labels: NodeDisplayPolicy,
    pub show_app_icons: NodeDisplayPolicy,
    pub icon_size: f32,
    pub opacity: f32,
    pub restore_centering: RestoreCentering,
    pub background_color: NodeBackgroundColor,
    pub border_color: crate::BorderColor,
    pub border_color_highlighted: crate::BorderColor,
}

impl Default for Nodes {
    fn default() -> Self {
        Self {
            shape: NodeShape::Squircle,
            label_shape: NodeShape::Squircle,
            show_labels: NodeDisplayPolicy::Hover,
            show_app_icons: NodeDisplayPolicy::Always,
            icon_size: 0.72,
            opacity: 1.0,
            restore_centering: RestoreCentering::Never,
            background_color: NodeBackgroundColor::Auto,
            border_color: crate::BorderColor {
                r: 0x47 as f32 / 255.0,
                g: 0x4d as f32 / 255.0,
                b: 0x59 as f32 / 255.0,
            },
            border_color_highlighted: crate::BorderColor {
                r: 0xd6 as f32 / 255.0,
                g: 0x5d as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandmarkPlacement {
    pub gap_px: f32,
}

impl Nodes {
    pub fn resolved_for_system(mut self, scheme: crate::SystemColorScheme) -> Self {
        if self.background_color == NodeBackgroundColor::System {
            self.background_color = match scheme {
                crate::SystemColorScheme::PreferDark => NodeBackgroundColor::Dark,
                crate::SystemColorScheme::PreferLight => NodeBackgroundColor::Light,
                crate::SystemColorScheme::NoPreference => NodeBackgroundColor::Auto,
            };
        }
        self
    }
}

impl Default for LandmarkPlacement {
    fn default() -> Self {
        Self { gap_px: 20.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Debug {
    pub show_focus_ring: bool,
    pub overlay_fps: bool,
}

pub fn parse_landmark_placement(config: &RuneConfig) -> LandmarkPlacement {
    let defaults = LandmarkPlacement::default();
    LandmarkPlacement {
        gap_px: finite_clamp(
            config.get_or("field.gap", config.get_or("field.gap-px", defaults.gap_px)),
            0.0,
            256.0,
            defaults.gap_px,
        ),
    }
}

pub fn parse_decay(config: &RuneConfig) -> Decay {
    let defaults = Decay::default();
    Decay {
        enabled: config.get_or("decay.enabled", defaults.enabled),
        outside_delay_seconds: config
            .get_or(
                "decay.outside-delay-seconds",
                defaults.outside_delay_seconds,
            )
            .max(1),
        inside_delay_seconds: config
            .get_or("decay.inside-delay-seconds", defaults.inside_delay_seconds)
            .max(1),
    }
}

pub fn parse_nodes(config: &RuneConfig) -> Nodes {
    let defaults = Nodes::default();
    Nodes {
        shape: parse_shape(config, "node.shape", defaults.shape),
        label_shape: parse_shape(config, "node.label-shape", defaults.label_shape),
        show_labels: parse_display_policy(config, "node.show-labels", defaults.show_labels),
        show_app_icons: parse_display_policy(
            config,
            "node.show-app-icons",
            defaults.show_app_icons,
        ),
        icon_size: finite_clamp(
            config.get_or("node.icon-size", defaults.icon_size),
            0.1,
            1.0,
            defaults.icon_size,
        ),
        opacity: finite_clamp(
            config.get_or("node.opacity", defaults.opacity),
            0.0,
            1.0,
            defaults.opacity,
        ),
        restore_centering: config
            .get_optional::<String>("node.click-collapsed-pan")
            .ok()
            .flatten()
            .or_else(|| {
                config
                    .get_optional::<String>("node.restore-centering")
                    .ok()
                    .flatten()
            })
            .and_then(|value| match value.as_str() {
                "never" => Some(RestoreCentering::Never),
                "if-offscreen" | "if_offscreen" => Some(RestoreCentering::IfOffscreen),
                "always" => Some(RestoreCentering::Always),
                _ => None,
            })
            .unwrap_or(defaults.restore_centering),
        background_color: parse_background_color(config, defaults.background_color),
        border_color: parse_node_color(
            config,
            &["node.border-colour", "node.border-color"],
            defaults.border_color,
        ),
        border_color_highlighted: parse_node_color(
            config,
            &[
                "node.border-colour-highlighted",
                "node.border-color-highlighted",
            ],
            defaults.border_color_highlighted,
        ),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeParseError(String);

impl fmt::Display for NodeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NodeParseError {}

pub fn parse_nodes_checked(config: &RuneConfig) -> Result<Nodes, NodeParseError> {
    let Value::Object(root) = config
        .get_value("")
        .map_err(|error| NodeParseError(format!("node config: {error}")))?
    else {
        return Err(NodeParseError(
            "node config root must be an object".to_string(),
        ));
    };
    for fields in root.iter().filter_map(|item| match item {
        ObjectItem::Assign(key, Value::Object(fields)) if key == "node" => Some(fields),
        _ => None,
    }) {
        for (removed, replacement) in [
            ("node-shape", "shape"),
            ("node_shape", "shape"),
            ("node-label-shape", "label-shape"),
            ("node_label_shape", "label-shape"),
            ("label_shape", "label-shape"),
        ] {
            if object_field(fields, &[removed]).is_some() {
                return Err(NodeParseError(format!(
                    "node.{removed} was removed; use node.{replacement}"
                )));
            }
        }
    }
    Ok(parse_nodes(config))
}

pub fn parse_debug(config: &RuneConfig) -> Debug {
    Debug {
        show_focus_ring: config.get_or("debug.show-focus-ring", false),
        overlay_fps: config.get_or("debug.overlay-fps", false),
    }
}

fn parse_shape(config: &RuneConfig, path: &str, default: NodeShape) -> NodeShape {
    config
        .get_optional::<String>(path)
        .ok()
        .flatten()
        .and_then(|value| match value.as_str() {
            "square" => Some(NodeShape::Square),
            "squircle" => Some(NodeShape::Squircle),
            _ => None,
        })
        .unwrap_or(default)
}

fn parse_background_color(
    config: &RuneConfig,
    default: NodeBackgroundColor,
) -> NodeBackgroundColor {
    let Some(value) = config
        .get_optional::<String>("node.background-colour")
        .ok()
        .flatten()
    else {
        return default;
    };
    match value.to_ascii_lowercase().as_str() {
        "auto" | "theme" => NodeBackgroundColor::Auto,
        "system" => NodeBackgroundColor::System,
        "light" => NodeBackgroundColor::Light,
        "dark" => NodeBackgroundColor::Dark,
        _ => parse_hex_rgb(&value)
            .map(|[r, g, b]| NodeBackgroundColor::Fixed(r, g, b))
            .unwrap_or(default),
    }
}

fn parse_node_color(
    config: &RuneConfig,
    paths: &[&str],
    default: crate::BorderColor,
) -> crate::BorderColor {
    let value = paths
        .iter()
        .find_map(|path| config.get_optional::<String>(path).ok().flatten());
    let Some(value) = value else {
        return default;
    };
    parse_hex_rgb(&value)
        .map(|[r, g, b]| crate::BorderColor { r, g, b })
        .unwrap_or(default)
}

fn parse_hex_rgb(value: &str) -> Option<[f32; 3]> {
    let hex = value.strip_prefix('#')?;
    let expanded;
    let hex = if hex.len() == 3 {
        expanded = hex.chars().flat_map(|ch| [ch, ch]).collect::<String>();
        expanded.as_str()
    } else {
        hex
    };
    if hex.len() != 6 {
        return None;
    }
    let raw = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((raw >> 16) & 0xff) as f32 / 255.0,
        ((raw >> 8) & 0xff) as f32 / 255.0,
        (raw & 0xff) as f32 / 255.0,
    ])
}

pub(crate) fn parse_focus_ring_fields(
    fields: &[ObjectItem],
    output: &str,
) -> Result<FocusRing, FocusRingParseError> {
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            continue;
        };
        if !matches!(
            key.as_str(),
            "radius-x"
                | "radius_x"
                | "radius-y"
                | "radius_y"
                | "offset-x"
                | "offset_x"
                | "offset-y"
                | "offset_y"
        ) {
            return Err(FocusRingParseError(format!(
                "focus-ring for output {output:?}: unknown setting {key:?}"
            )));
        }
    }
    for aliases in [
        &["radius-x", "radius_x"][..],
        &["radius-y", "radius_y"][..],
        &["offset-x", "offset_x"][..],
        &["offset-y", "offset_y"][..],
    ] {
        let count = fields
            .iter()
            .filter(|item| {
                matches!(item, ObjectItem::Assign(key, _) if aliases.contains(&key.as_str()))
            })
            .count();
        if count > 1 {
            return Err(FocusRingParseError(format!(
                "focus-ring for output {output:?}: {} may only be specified once",
                aliases[0]
            )));
        }
    }

    let defaults = FocusRing::default();
    let read = |names: &[&str], default: f32, positive: bool| {
        let Some(value) = object_field(fields, names) else {
            return Ok(default);
        };
        let Value::Number(value) = value else {
            return Err(FocusRingParseError(format!(
                "focus-ring for output {output:?}: {} must be numeric",
                names[0]
            )));
        };
        let value = *value as f32;
        if !value.is_finite() || (positive && value <= 0.0) {
            return Err(FocusRingParseError(format!(
                "focus-ring for output {output:?}: {} must be {}finite",
                names[0],
                if positive { "positive and " } else { "" }
            )));
        }
        Ok(value)
    };
    Ok(FocusRing {
        radius_x: read(&["radius-x", "radius_x"], defaults.radius_x, true)?,
        radius_y: read(&["radius-y", "radius_y"], defaults.radius_y, true)?,
        offset_x: read(&["offset-x", "offset_x"], defaults.offset_x, false)?,
        offset_y: read(&["offset-y", "offset_y"], defaults.offset_y, false)?,
    })
}

fn object_field<'a>(fields: &'a [ObjectItem], names: &[&str]) -> Option<&'a Value> {
    fields.iter().find_map(|item| match item {
        ObjectItem::Assign(key, value) if names.contains(&key.as_str()) => Some(value),
        _ => None,
    })
}

fn parse_display_policy(
    config: &RuneConfig,
    path: &str,
    default: NodeDisplayPolicy,
) -> NodeDisplayPolicy {
    config
        .get_optional::<String>(path)
        .ok()
        .flatten()
        .and_then(|value| match value.as_str() {
            "off" => Some(NodeDisplayPolicy::Off),
            "hover" => Some(NodeDisplayPolicy::Hover),
            "always" => Some(NodeDisplayPolicy::Always),
            _ => None,
        })
        .unwrap_or(default)
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_node_decay_and_debug_sections() {
        let config = RuneConfig::from_str(
            r#"
decay:
  enabled false
  outside-delay-seconds 9
  inside-delay-seconds 99
end
node:
  shape "square"
  show-labels "always"
  show-app-icons "off"
  restore-centering "if-offscreen"
end
debug:
  show-focus-ring true
  overlay-fps true
end
"#,
        )
        .unwrap();

        assert_eq!(parse_decay(&config).outside_delay_seconds, 9);
        assert!(!parse_decay(&config).enabled);
        assert_eq!(parse_nodes(&config).shape, NodeShape::Square);
        assert_eq!(
            parse_nodes(&config).restore_centering,
            RestoreCentering::IfOffscreen
        );
        assert!(parse_debug(&config).show_focus_ring);
        assert!(parse_debug(&config).overlay_fps);
    }

    /// Milestone 4 boundary: the longer conservative delays reach users only
    /// through a newly generated config. An older config that omits the
    /// `decay:` section keeps Halley's built-in 180 s / 1800 s behavior.
    #[test]
    fn a_config_without_a_decay_section_keeps_the_built_in_delays() {
        let config = RuneConfig::from_str("keybinds:\n  mod \"super\"\nend\n").unwrap();
        let decay = parse_decay(&config);

        assert!(decay.enabled);
        assert_eq!(decay.outside_delay_seconds, 180);
        assert_eq!(decay.inside_delay_seconds, 1_800);
        assert_eq!(decay, Decay::default());
    }

    #[test]
    fn parses_canonical_node_names_and_landmark_gap() {
        let config = RuneConfig::from_str(
            r##"
field:
  gap-px 24
end
node:
  shape "square"
  label-shape "square"
  background-colour "#102030"
  border-colour "#112233"
  border-color-highlighted "#aabbcc"
  click-collapsed-pan "always"
end
"##,
        )
        .unwrap();
        assert_eq!(parse_landmark_placement(&config).gap_px, 24.0);
        let nodes = parse_nodes_checked(&config).unwrap();
        assert_eq!(nodes.shape, NodeShape::Square);
        assert_eq!(nodes.label_shape, NodeShape::Square);
        assert_eq!(nodes.restore_centering, RestoreCentering::Always);
        assert!(matches!(
            nodes.background_color,
            NodeBackgroundColor::Fixed(_, _, _)
        ));
        assert_eq!(
            nodes.border_color,
            crate::BorderColor {
                r: 0x11 as f32 / 255.0,
                g: 0x22 as f32 / 255.0,
                b: 0x33 as f32 / 255.0,
            }
        );
        assert_eq!(
            nodes.border_color_highlighted,
            crate::BorderColor {
                r: 0xaa as f32 / 255.0,
                g: 0xbb as f32 / 255.0,
                b: 0xcc as f32 / 255.0,
            }
        );
    }

    #[test]
    fn system_node_background_resolves_without_changing_owned_border_colours() {
        let config = RuneConfig::from_str(
            "node:\n  background-colour \"system\"\n  border-colour \"#102030\"\n  border-colour-highlighted \"#abcdef\"\nend\n",
        )
        .unwrap();
        let nodes = parse_nodes(&config);
        assert_eq!(nodes.background_color, NodeBackgroundColor::System);

        let dark = nodes.resolved_for_system(crate::SystemColorScheme::PreferDark);
        assert_eq!(dark.background_color, NodeBackgroundColor::Dark);
        assert_eq!(dark.border_color, nodes.border_color);
        assert_eq!(
            dark.border_color_highlighted,
            nodes.border_color_highlighted
        );

        assert_eq!(
            nodes
                .resolved_for_system(crate::SystemColorScheme::NoPreference)
                .background_color,
            NodeBackgroundColor::Auto
        );
    }

    #[test]
    fn removed_shape_names_fail_with_their_replacements() {
        for (removed, replacement) in [
            ("node-shape", "node.shape"),
            ("node_shape", "node.shape"),
            ("node-label-shape", "node.label-shape"),
            ("node_label_shape", "node.label-shape"),
            ("label_shape", "node.label-shape"),
        ] {
            let config =
                RuneConfig::from_str(&format!("node:\n  {removed} \"square\"\nend\n")).unwrap();
            let error = parse_nodes_checked(&config).unwrap_err().to_string();
            assert!(error.contains(replacement), "{error}");
        }
    }
}
