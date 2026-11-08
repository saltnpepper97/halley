use std::fmt;

use rune_cfg::ast::{ObjectItem, Value};

/// Variable refresh rate mode for one output. `On` keeps hardware VRR enabled;
/// `Auto` enables it only for a committed, settled fullscreen window while no
/// compositor or layer-shell overlay is visible. Backends may leave `Auto`
/// disabled when changing VRR would require a modeset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vrr {
    #[default]
    Off,
    On,
    Auto,
}

/// One monitor's mode, logical position, transform, and display scale.
/// Unconfigured connectors keep their preferred mode at scale 1.0.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputConfig {
    /// Connector name, e.g. "DP-1" - matched against real connector names
    /// at startup.
    pub name: String,
    /// Omit both dimensions to keep the connector's preferred mode.
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Physical pixels per logical pixel, quantized to the Wayland 1/120 unit.
    pub scale: f64,
    pub offset_x: i32,
    pub offset_y: i32,
    /// `None` means "use the highest advertised refresh at this exact
    /// resolution" - when `Some`, the backend requires an exact
    /// integer-millihertz match (see `backend/tty.rs`), not a closest/fuzzy
    /// match.
    pub rate: Option<f64>,
    /// Raw degrees - 0/90/180/270. Any other value falls back to 0, same
    /// as old halley's own clamp (its legacy 1/2/3 shorthand isn't ported -
    /// no reason to keep an alternate spelling this project never used).
    pub transform: u16,
    pub vrr: Vrr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputParseError(String);

impl fmt::Display for OutputParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OutputParseError {}

pub(crate) fn is_hardware_field(key: &str) -> bool {
    matches!(
        key,
        "width"
            | "height"
            | "offset-x"
            | "offset_x"
            | "offset-y"
            | "offset_y"
            | "rate"
            | "refresh-rate"
            | "refresh_rate"
            | "transform"
            | "rotation"
            | "vrr"
            | "scale"
    )
}

/// Parses only the hardware portion of a `view.output` entry. `None` means
/// the entry intentionally contains policy (such as a decay zone) without a
/// hardware override, so applying it must not trigger a modeset.
pub(crate) fn parse_hardware_output(
    name: &str,
    fields: &[ObjectItem],
) -> Result<Option<OutputConfig>, OutputParseError> {
    for aliases in [
        &["width"][..],
        &["height"][..],
        &["offset-x", "offset_x"][..],
        &["offset-y", "offset_y"][..],
        &["rate", "refresh-rate", "refresh_rate"][..],
        &["transform", "rotation"][..],
        &["vrr"][..],
        &["scale"][..],
    ] {
        if assigned_keys(fields)
            .filter(|key| aliases.contains(key))
            .count()
            > 1
        {
            return Err(OutputParseError(format!(
                "output {name:?}: {} may only be specified once",
                aliases[0]
            )));
        }
    }

    let width = field(fields, &["width"]);
    let height = field(fields, &["height"]);
    match (width, height) {
        (None, None) => {
            if let Some(key) = assigned_keys(fields)
                .find(|key| is_hardware_field(key) && !matches!(*key, "width" | "height" | "scale"))
            {
                return Err(OutputParseError(format!(
                    "output {name:?}: {key} requires width and height"
                )));
            }
            if field(fields, &["scale"]).is_none() {
                return Ok(None);
            }
        }
        (Some(_), None) => {
            return Err(OutputParseError(format!(
                "output {name:?}: width and height must be specified together"
            )));
        }
        (None, Some(_)) => {
            return Err(OutputParseError(format!(
                "output {name:?}: width and height must be specified together"
            )));
        }
        (Some(_), Some(_)) => {}
    }

    let dimension = |value: Option<&Value>, key: &str| {
        value
            .map(|value| {
                as_i32(value).ok_or_else(|| {
                    OutputParseError(format!("output {name:?}: {key} must be a whole number"))
                })
            })
            .transpose()
    };
    let width = dimension(width, "width")?;
    let height = dimension(height, "height")?;
    if width.is_some_and(|value| value <= 0) || height.is_some_and(|value| value <= 0) {
        return Err(OutputParseError(format!(
            "output {name:?}: width/height must be positive"
        )));
    }

    let scale = match field(fields, &["scale"]) {
        None => 1.0,
        Some(value) => as_f64(value).and_then(normalize_scale).ok_or_else(|| {
            OutputParseError(format!(
                "output {name:?}: scale must be a number from 0.25 through 10.0"
            ))
        })?,
    };

    for keys in [&["offset-x", "offset_x"][..], &["offset-y", "offset_y"][..]] {
        if field(fields, keys).is_some_and(|value| as_i32(value).is_none()) {
            return Err(OutputParseError(format!(
                "output {name:?}: {} must be numeric",
                keys[0]
            )));
        }
    }

    if let Some(value) = field(fields, &["rate", "refresh-rate", "refresh_rate"]) {
        let Some(rate) = as_f64(value) else {
            return Err(OutputParseError(format!(
                "output {name:?}: rate must be numeric"
            )));
        };
        if !rate.is_finite() || rate <= 0.0 {
            return Err(OutputParseError(format!(
                "output {name:?}: rate must be positive and finite"
            )));
        }
    }

    if let Some(value) = field(fields, &["transform", "rotation"]) {
        let Some(transform) = as_u16(value) else {
            return Err(OutputParseError(format!(
                "output {name:?}: transform must be numeric"
            )));
        };
        if !matches!(transform, 0 | 90 | 180 | 270) {
            return Err(OutputParseError(format!(
                "output {name:?}: transform must be 0, 90, 180, or 270"
            )));
        }
    }

    if let Some(value) = field(fields, &["vrr"]) {
        let Some(vrr) = as_str(value) else {
            return Err(OutputParseError(format!(
                "output {name:?}: vrr must be a string"
            )));
        };
        if !matches!(
            vrr.to_ascii_lowercase().as_str(),
            "on" | "true" | "off" | "false" | "auto" | "on-demand" | "on_demand" | "ondemand"
        ) {
            return Err(OutputParseError(format!(
                "output {name:?}: unknown vrr mode {vrr:?}"
            )));
        }
    }

    let offset_x = field(fields, &["offset-x", "offset_x"])
        .and_then(as_i32)
        .unwrap_or(0);
    let offset_y = field(fields, &["offset-y", "offset_y"])
        .and_then(as_i32)
        .unwrap_or(0);
    let rate = field(fields, &["rate", "refresh-rate", "refresh_rate"]).and_then(as_f64);
    let transform = field(fields, &["transform", "rotation"])
        .and_then(as_u16)
        .unwrap_or(0);
    let vrr = field(fields, &["vrr"])
        .and_then(as_str)
        .map(|raw| parse_vrr(&raw))
        .unwrap_or_default();

    Ok(Some(OutputConfig {
        name: name.to_string(),
        width,
        height,
        scale,
        offset_x,
        offset_y,
        rate,
        transform,
        vrr,
    }))
}

/// Share validation and protocol precision between persistent config and
/// output-management requests so clients and compositor use the same scale.
pub fn normalize_scale(scale: f64) -> Option<f64> {
    (scale.is_finite() && (0.25..=10.0).contains(&scale)).then(|| (scale * 120.0).round() / 120.0)
}

fn parse_vrr(raw: &str) -> Vrr {
    match raw.to_ascii_lowercase().as_str() {
        "on" | "true" => Vrr::On,
        "auto" | "on-demand" | "on_demand" | "ondemand" => Vrr::Auto,
        _ => Vrr::Off,
    }
}

fn field<'a>(fields: &'a [ObjectItem], keys: &[&str]) -> Option<&'a Value> {
    fields.iter().find_map(|item| match item {
        ObjectItem::Assign(key, value) if keys.contains(&key.as_str()) => Some(value),
        _ => None,
    })
}

fn assigned_keys(fields: &[ObjectItem]) -> impl Iterator<Item = &str> {
    fields.iter().filter_map(|item| match item {
        ObjectItem::Assign(key, _) => Some(key.as_str()),
        _ => None,
    })
}

fn as_str(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

fn as_i32(value: &Value) -> Option<i32> {
    let number = as_f64(value)?;
    (number.is_finite()
        && number.fract() == 0.0
        && number >= i32::MIN as f64
        && number <= i32::MAX as f64)
        .then_some(number as i32)
}

fn as_u16(value: &Value) -> Option<u16> {
    let number = as_f64(value)?;
    (number.is_finite()
        && number.fract() == 0.0
        && number >= u16::MIN as f64
        && number <= u16::MAX as f64)
        .then_some(number as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrr_accepts_on_and_auto_spellings() {
        for (raw, expected) in [
            ("on", Vrr::On),
            ("true", Vrr::On),
            ("off", Vrr::Off),
            ("auto", Vrr::Auto),
            ("on-demand", Vrr::Auto),
        ] {
            assert_eq!(parse_vrr(raw), expected, "vrr {raw:?}");
        }
    }

    #[test]
    fn numeric_conversions_require_whole_finite_values() {
        assert_eq!(as_i32(&Value::Number(42.0)), Some(42));
        assert_eq!(as_i32(&Value::Number(42.5)), None);
        assert_eq!(as_i32(&Value::Number(f64::INFINITY)), None);
    }
}
