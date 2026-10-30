use std::fmt;

use regex::Regex;
use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

#[derive(Clone, Debug)]
pub enum RulePattern {
    Exact(String),
    Regex(Regex),
}

impl PartialEq for RulePattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for RulePattern {}

impl RulePattern {
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(exact) => exact == value,
            Self::Regex(regex) => regex.is_match(value),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(exact) => exact,
            Self::Regex(regex) => regex.as_str(),
        }
    }
}

/// Backwards-compatible name for patterns used by managed-window rules.
pub type WindowRulePattern = RulePattern;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowSpawnPlacement {
    #[default]
    Default,
    Center,
    Adjacent,
    ViewportCenter,
    Cursor,
    App,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowClusterParticipation {
    #[default]
    Layout,
    Float,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowRule {
    pub app_ids: Vec<WindowRulePattern>,
    pub titles: Vec<WindowRulePattern>,
    pub initial_size: Option<(u32, u32)>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
    pub block_screencast: bool,
    pub game: Option<bool>,
    pub spawn_placement: WindowSpawnPlacement,
    pub cluster_participation: WindowClusterParticipation,
}

/// One protocol layer that a layer-shell rule may match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerShellLayer {
    Background,
    Bottom,
    Top,
    Overlay,
}

/// Visual policy for a layer-shell root and its popup tree.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRule {
    pub namespaces: Vec<RulePattern>,
    pub layers: Vec<LayerShellLayer>,
    pub blur: Option<bool>,
    pub block_screencast: bool,
}

impl LayerRule {
    pub fn matches(&self, namespace: &str, layer: LayerShellLayer) -> bool {
        (self.namespaces.is_empty()
            || self
                .namespaces
                .iter()
                .any(|pattern| pattern.matches(namespace)))
            && (self.layers.is_empty() || self.layers.contains(&layer))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub windows: Vec<WindowRule>,
    pub layers: Vec<LayerRule>,
}

impl WindowRule {
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        let app_matches = self.app_ids.is_empty()
            || app_id.is_some_and(|value| self.app_ids.iter().any(|item| item.matches(value)));
        let title_matches = self.titles.is_empty()
            || title.is_some_and(|value| self.titles.iter().any(|item| item.matches(value)));
        app_matches && title_matches
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRuleParseError(String);

impl fmt::Display for WindowRuleParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WindowRuleParseError {}

pub fn parse_rules(config: &RuneConfig) -> Result<Rules, WindowRuleParseError> {
    let Value::Object(root) = config
        .get_value("")
        .map_err(|error| WindowRuleParseError(format!("rules config: {error}")))?
    else {
        return Err(WindowRuleParseError(
            "rules config root must be an object".to_string(),
        ));
    };
    let mut rules = Rules::default();
    for section in root.iter().filter_map(|item| match item {
        ObjectItem::Assign(key, Value::Object(fields)) if key == "rules" => Some(fields),
        _ => None,
    }) {
        for item in section {
            let ObjectItem::Assign(key, value) = item else {
                return Err(WindowRuleParseError(
                    "conditionals are not supported directly inside rules".to_string(),
                ));
            };
            if key == "rule" {
                return Err(WindowRuleParseError(
                    "`rules.rule` has been renamed to `rules.window-rule`".to_string(),
                ));
            }
            if !matches!(key.as_str(), "window-rule" | "layer-rule") {
                return Err(WindowRuleParseError(format!(
                    "unknown entry {key:?} inside rules; expected `window-rule:` or `layer-rule:`"
                )));
            }
            let Value::Object(fields) = value else {
                return Err(WindowRuleParseError(format!(
                    "{key} entry must be an object"
                )));
            };
            match key.as_str() {
                "window-rule" => rules.windows.push(parse_rule(fields)?),
                "layer-rule" => rules.layers.push(parse_layer_rule(fields)?),
                _ => unreachable!("validated above"),
            }
        }
    }
    Ok(rules)
}

pub fn parse_window_rules(config: &RuneConfig) -> Result<Vec<WindowRule>, WindowRuleParseError> {
    Ok(parse_rules(config)?.windows)
}

fn parse_rule(fields: &[ObjectItem]) -> Result<WindowRule, WindowRuleParseError> {
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            return Err(WindowRuleParseError(
                "conditionals are not supported inside a window rule".to_string(),
            ));
        };
        if !matches!(
            key.as_str(),
            "app-id"
                | "app_id"
                | "title"
                | "width"
                | "height"
                | "opacity"
                | "blur"
                | "block-screencast"
                | "game"
                | "overlap-policy"
                | "overlap_policy"
                | "spawn-placement"
                | "spawn_placement"
                | "cluster-participation"
                | "cluster_participation"
        ) {
            return Err(WindowRuleParseError(format!(
                "unknown window rule key {key:?}"
            )));
        }
    }

    let app_ids = field(fields, &["app-id", "app_id"])
        .map(|value| patterns(value, "app-id"))
        .transpose()?
        .unwrap_or_default();
    let titles = field(fields, &["title"])
        .map(|value| patterns(value, "title"))
        .transpose()?
        .unwrap_or_default();
    if app_ids.is_empty() && titles.is_empty() {
        return Err(WindowRuleParseError(
            "window rule requires app-id and/or title".to_string(),
        ));
    }

    let width = field(fields, &["width"])
        .map(|value| dimension(value, "width"))
        .transpose()?;
    let height = field(fields, &["height"])
        .map(|value| dimension(value, "height"))
        .transpose()?;
    let initial_size = match (width, height) {
        (Some(width), Some(height)) => Some((width, height)),
        (Some(_), None) => {
            return Err(WindowRuleParseError(
                "window rule width requires a matching height".to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(WindowRuleParseError(
                "window rule height requires a matching width".to_string(),
            ));
        }
        (None, None) => None,
    };

    let opacity = field(fields, &["opacity"])
        .map(|value| number(value, "opacity").map(|value| value as f32))
        .transpose()?;
    if opacity.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(WindowRuleParseError(
            "window rule opacity must be finite and between 0.0 and 1.0".to_string(),
        ));
    }
    let blur = field(fields, &["blur"])
        .map(|value| boolean(value, "blur"))
        .transpose()?;
    let block_screencast = field(fields, &["block-screencast"])
        .map(|value| boolean(value, "block-screencast"))
        .transpose()?
        .unwrap_or(false);
    let game = field(fields, &["game"])
        .map(|value| boolean(value, "game"))
        .transpose()?;
    if let Some(value) = field(fields, &["overlap-policy", "overlap_policy"]) {
        let value = string(value, "overlap-policy")?;
        if !matches!(value.as_str(), "none" | "parent-only" | "all") {
            return Err(WindowRuleParseError(format!(
                "unknown overlap-policy {value:?}"
            )));
        }
    }
    let spawn_placement = match field(fields, &["spawn-placement", "spawn_placement"]) {
        None => WindowSpawnPlacement::Default,
        Some(value) => match string(value, "spawn-placement")?.as_str() {
            "center" => WindowSpawnPlacement::Center,
            "adjacent" => WindowSpawnPlacement::Adjacent,
            "viewport-center" => WindowSpawnPlacement::ViewportCenter,
            "cursor" => WindowSpawnPlacement::Cursor,
            "app" => WindowSpawnPlacement::App,
            other => {
                return Err(WindowRuleParseError(format!(
                    "unknown spawn-placement {other:?}"
                )));
            }
        },
    };
    let cluster_participation =
        match field(fields, &["cluster-participation", "cluster_participation"]) {
            None => WindowClusterParticipation::Layout,
            Some(value) => match string(value, "cluster-participation")?.as_str() {
                "layout" => WindowClusterParticipation::Layout,
                "float" => WindowClusterParticipation::Float,
                other => {
                    return Err(WindowRuleParseError(format!(
                        "unknown cluster-participation {other:?}"
                    )));
                }
            },
        };

    Ok(WindowRule {
        app_ids,
        titles,
        initial_size,
        opacity,
        blur,
        block_screencast,
        game,
        spawn_placement,
        cluster_participation,
    })
}

fn parse_layer_rule(fields: &[ObjectItem]) -> Result<LayerRule, WindowRuleParseError> {
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            return Err(WindowRuleParseError(
                "conditionals are not supported inside a layer rule".to_string(),
            ));
        };
        if !matches!(
            key.as_str(),
            "namespace" | "layer" | "blur" | "block-screencast"
        ) {
            return Err(WindowRuleParseError(format!(
                "unknown layer rule key {key:?}"
            )));
        }
    }

    let namespaces = field(fields, &["namespace"])
        .map(|value| patterns(value, "namespace"))
        .transpose()?
        .unwrap_or_default();
    let layers = field(fields, &["layer"])
        .map(layer_values)
        .transpose()?
        .unwrap_or_default();
    if namespaces.is_empty() && layers.is_empty() {
        return Err(WindowRuleParseError(
            "layer rule requires namespace and/or layer".to_string(),
        ));
    }
    let blur = field(fields, &["blur"])
        .map(|value| boolean(value, "blur"))
        .transpose()?;
    let block_screencast = field(fields, &["block-screencast"])
        .map(|value| boolean(value, "block-screencast"))
        .transpose()?;
    if blur.is_none() && block_screencast.is_none() {
        return Err(WindowRuleParseError(
            "layer rule requires blur or block-screencast".to_string(),
        ));
    }

    Ok(LayerRule {
        namespaces,
        layers,
        blur,
        block_screencast: block_screencast.unwrap_or(false),
    })
}

fn field<'a>(fields: &'a [ObjectItem], names: &[&str]) -> Option<&'a Value> {
    fields.iter().find_map(|item| match item {
        ObjectItem::Assign(key, value) if names.contains(&key.as_str()) => Some(value),
        _ => None,
    })
}

fn patterns(value: &Value, field_name: &str) -> Result<Vec<RulePattern>, WindowRuleParseError> {
    match value {
        Value::String(value) => Ok(vec![RulePattern::Exact(value.clone())]),
        Value::Regex(value) => Ok(vec![RulePattern::Regex(value.clone())]),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(RulePattern::Exact(value.clone())),
                Value::Regex(value) => Ok(RulePattern::Regex(value.clone())),
                _ => Err(WindowRuleParseError(format!(
                    "rule {field_name} array accepts only strings and regexes"
                ))),
            })
            .collect(),
        _ => Err(WindowRuleParseError(format!(
            "rule {field_name} must be a string, regex, or array"
        ))),
    }
}

fn layer_values(value: &Value) -> Result<Vec<LayerShellLayer>, WindowRuleParseError> {
    let values = match value {
        Value::String(value) => std::slice::from_ref(value),
        Value::Array(values) => {
            let mut parsed = Vec::with_capacity(values.len());
            for value in values {
                let Value::String(value) = value else {
                    return Err(WindowRuleParseError(
                        "layer rule layer array accepts only strings".to_string(),
                    ));
                };
                parsed.push(parse_layer(value)?);
            }
            return Ok(parsed);
        }
        _ => {
            return Err(WindowRuleParseError(
                "layer rule layer must be a string or array".to_string(),
            ));
        }
    };
    values.iter().map(|value| parse_layer(value)).collect()
}

fn parse_layer(value: &str) -> Result<LayerShellLayer, WindowRuleParseError> {
    match value {
        "background" => Ok(LayerShellLayer::Background),
        "bottom" => Ok(LayerShellLayer::Bottom),
        "top" => Ok(LayerShellLayer::Top),
        "overlay" => Ok(LayerShellLayer::Overlay),
        _ => Err(WindowRuleParseError(format!(
            "unknown layer rule layer {value:?}"
        ))),
    }
}

fn string(value: &Value, field_name: &str) -> Result<String, WindowRuleParseError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        _ => Err(WindowRuleParseError(format!(
            "window rule {field_name} must be a string"
        ))),
    }
}

fn number(value: &Value, field_name: &str) -> Result<f64, WindowRuleParseError> {
    match value {
        Value::Number(value) => Ok(*value),
        _ => Err(WindowRuleParseError(format!(
            "window rule {field_name} must be numeric"
        ))),
    }
}

fn dimension(value: &Value, field_name: &str) -> Result<u32, WindowRuleParseError> {
    let value = number(value, field_name)?;
    if !value.is_finite() || value.fract() != 0.0 || value < 1.0 || value > u32::MAX as f64 {
        return Err(WindowRuleParseError(format!(
            "window rule {field_name} must be a positive integer"
        )));
    }
    Ok(value as u32)
}

fn boolean(value: &Value, field_name: &str) -> Result<bool, WindowRuleParseError> {
    match value {
        Value::Bool(value) => Ok(*value),
        _ => Err(WindowRuleParseError(format!(
            "window rule {field_name} must be true or false"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeated_rules_and_matches_first_in_order() {
        let config = RuneConfig::from_str(
            r#"
rules:
  window-rule:
    app-id ["firefox", r"org\.mozilla\..*"]
    title r"Picture.*"
    width 720
    height 520
    opacity 0.86
    blur true
    spawn-placement "center"
    cluster-participation "float"
  end
  window-rule:
    title "Terminal"
  end
end
"#,
        )
        .unwrap();
        let rules = parse_window_rules(&config).unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules[0].matches(Some("firefox"), Some("Picture-in-Picture")));
        assert!(!rules[0].matches(Some("Firefox"), Some("Picture-in-Picture")));
        assert_eq!(rules[0].initial_size, Some((720, 520)));
        assert_eq!(rules[0].opacity, Some(0.86));
        assert_eq!(
            rules[0].cluster_participation,
            WindowClusterParticipation::Float
        );
    }

    #[test]
    fn rejects_incomplete_or_matcherless_rules() {
        let config =
            RuneConfig::from_str("rules:\n  window-rule:\n    width 100\n  end\nend\n").unwrap();
        assert!(parse_window_rules(&config).is_err());

        let config = RuneConfig::from_str(
            "rules:\n  window-rule:\n    app-id \"x\"\n    width 100\n  end\nend\n",
        )
        .unwrap();
        assert!(parse_window_rules(&config).is_err());
    }

    #[test]
    fn accepts_but_discards_deprecated_overlap_policy() {
        let config = RuneConfig::from_str(
            "rules:\n  window-rule:\n    app-id \"x\"\n    overlap-policy \"all\"\n  end\nend\n",
        )
        .unwrap();
        assert_eq!(parse_window_rules(&config).unwrap().len(), 1);
    }

    #[test]
    fn game_override_is_optional_and_requires_a_boolean() {
        for (value, expected) in [
            ("", None),
            ("game true", Some(true)),
            ("game false", Some(false)),
        ] {
            let config = RuneConfig::from_str(&format!(
                "rules:\n  window-rule:\n    app-id \"test\"\n    {value}\n  end\nend\n"
            ))
            .unwrap();
            assert_eq!(parse_window_rules(&config).unwrap()[0].game, expected);
        }
        let config = RuneConfig::from_str(
            "rules:\n  window-rule:\n    app-id \"test\"\n    game \"true\"\n  end\nend\n",
        )
        .unwrap();
        assert!(parse_window_rules(&config).is_err());
    }

    #[test]
    fn rejects_renamed_window_rule_entry() {
        let config =
            RuneConfig::from_str("rules:\n  rule:\n    app-id \"x\"\n  end\nend\n").unwrap();
        let error = parse_rules(&config).unwrap_err();
        assert!(error.to_string().contains("window-rule"));
    }

    #[test]
    fn parses_layer_rules_and_matches_namespace_and_layer() {
        let config = RuneConfig::from_str(
            r#"
rules:
  layer-rule:
    namespace ["waybar", r"^fuzzel$"]
    layer ["top", "overlay"]
    blur true
  end
  layer-rule:
    layer "bottom"
    blur false
  end
end
"#,
        )
        .unwrap();
        let rules = parse_rules(&config).unwrap();
        assert!(rules.windows.is_empty());
        assert_eq!(rules.layers.len(), 2);
        assert!(rules.layers[0].matches("waybar", LayerShellLayer::Top));
        assert!(rules.layers[0].matches("fuzzel", LayerShellLayer::Overlay));
        assert!(!rules.layers[0].matches("waybar", LayerShellLayer::Bottom));
        assert!(rules.layers[1].matches("anything", LayerShellLayer::Bottom));
        assert_eq!(rules.layers[1].blur, Some(false));
    }

    #[test]
    fn parses_screencast_privacy_for_windows_and_layers() {
        for value in ["true", "false"] {
            let config = RuneConfig::from_str(&format!(
                "rules:\n  window-rule:\n    app-id \"secret\"\n    block-screencast {value}\n  end\n  layer-rule:\n    namespace \"notifications\"\n    block-screencast {value}\n  end\nend\n"
            )).unwrap();
            let rules = parse_rules(&config).unwrap();
            assert_eq!(rules.windows[0].block_screencast, value == "true");
            assert_eq!(rules.layers[0].block_screencast, value == "true");
            assert_eq!(rules.layers[0].blur, None);
        }
        let config = RuneConfig::from_str(
            "rules:\n  layer-rule:\n    namespace \"notifications\"\n    block-screencast \"true\"\n  end\nend\n"
        ).unwrap();
        assert!(parse_rules(&config).is_err());
    }

    #[test]
    fn rejects_incomplete_or_invalid_layer_rules() {
        for source in [
            "rules:\n  layer-rule:\n    blur true\n  end\nend\n",
            "rules:\n  layer-rule:\n    namespace \"waybar\"\n  end\nend\n",
            "rules:\n  layer-rule:\n    layer \"middle\"\n    blur true\n  end\nend\n",
        ] {
            let config = RuneConfig::from_str(source).unwrap();
            assert!(parse_rules(&config).is_err());
        }
    }
}
