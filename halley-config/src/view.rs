use std::collections::HashSet;
use std::fmt;

use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

use crate::nodes::{DecayZones, parse_decay_zone_fields};
use crate::output::{OutputConfig, is_hardware_field, parse_hardware_output};

/// The config-facing aggregate for output-specific view policy. Runtime
/// consumers continue to receive hardware outputs and decay zones separately.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewConfig {
    pub outputs: Vec<OutputConfig>,
    pub decay_zones: DecayZones,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewParseError(String);

impl fmt::Display for ViewParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ViewParseError {}

pub fn parse_view(config: &RuneConfig) -> ViewConfig {
    parse_view_checked(config).unwrap_or_default()
}

/// Parses the single canonical `view` section. Repeated `output` entries are
/// preserved in file order even though rune-cfg dotted paths resolve only the
/// first matching key.
pub fn parse_view_checked(config: &RuneConfig) -> Result<ViewConfig, ViewParseError> {
    let Value::Object(root) = config
        .get_value("")
        .map_err(|error| ViewParseError(format!("view config: {error}")))?
    else {
        return Err(ViewParseError(
            "view config root must be an object".to_string(),
        ));
    };

    let mut view = None;
    for item in &root {
        let ObjectItem::Assign(key, value) = item else {
            continue;
        };
        match key.as_str() {
            "output" => {
                return Err(ViewParseError(
                    "top-level output blocks were removed; nest them under view:".to_string(),
                ));
            }
            "decay-zone" | "focus-ring" => {
                return Err(ViewParseError(format!(
                    "top-level {key} blocks were removed; nest decay-zone inside view.output"
                )));
            }
            "view" if view.is_some() => {
                return Err(ViewParseError(
                    "only one top-level view block is allowed".to_string(),
                ));
            }
            "view" => view = Some(value),
            _ => {}
        }
    }

    let Some(view) = view else {
        return Ok(ViewConfig::default());
    };
    let Value::Object(entries) = view else {
        return Err(ViewParseError("view must be an object".to_string()));
    };

    let mut parsed = ViewConfig::default();
    let mut output_names = HashSet::new();
    for item in entries {
        let ObjectItem::Assign(key, value) = item else {
            continue;
        };
        if key != "output" {
            return Err(ViewParseError(format!(
                "view: unknown setting {key:?}; expected an output block"
            )));
        }
        let Value::Object(fields) = value else {
            return Err(ViewParseError("view.output must be an object".to_string()));
        };
        let name = parse_output_name(fields)?;
        if !output_names.insert(name.clone()) {
            return Err(ViewParseError(format!(
                "duplicate view.output block for {name:?}"
            )));
        }

        validate_output_keys(fields, &name)?;
        let decay_zone = parse_decay_zone_block(fields, &name)?;
        let hardware = parse_hardware_output(&name, fields)
            .map_err(|error| ViewParseError(error.to_string()))?;
        if hardware.is_none() && decay_zone.is_none() {
            return Err(ViewParseError(format!(
                "view.output {name:?} must configure hardware or a decay-zone"
            )));
        }
        if let Some(output) = hardware {
            parsed.outputs.push(output);
        }
        if let Some(zone) = decay_zone {
            parsed.decay_zones.by_output.insert(name, zone);
        }
    }
    Ok(parsed)
}

fn parse_output_name(fields: &[ObjectItem]) -> Result<String, ViewParseError> {
    let names = fields
        .iter()
        .filter_map(|item| match item {
            ObjectItem::Assign(key, value) if key == "name" => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    if names.len() != 1 {
        return Err(ViewParseError(
            "each view.output block requires exactly one name".to_string(),
        ));
    }
    let Value::String(name) = names[0] else {
        return Err(ViewParseError(
            "view.output name must be a non-empty string".to_string(),
        ));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(ViewParseError(
            "view.output name must be a non-empty string".to_string(),
        ));
    }
    Ok(name.to_string())
}

fn validate_output_keys(fields: &[ObjectItem], output: &str) -> Result<(), ViewParseError> {
    for item in fields {
        let ObjectItem::Assign(key, _) = item else {
            continue;
        };
        if key != "name" && !is_decay_zone_key(key) && !is_hardware_field(key) {
            return Err(ViewParseError(format!(
                "view.output {output:?}: unknown setting {key:?}"
            )));
        }
    }
    Ok(())
}

/// `focus-ring` is the older spelling of `decay-zone`.
fn is_decay_zone_key(key: &str) -> bool {
    matches!(key, "decay-zone" | "focus-ring")
}

fn parse_decay_zone_block(
    fields: &[ObjectItem],
    output: &str,
) -> Result<Option<crate::DecayZone>, ViewParseError> {
    let blocks = fields
        .iter()
        .filter_map(|item| match item {
            ObjectItem::Assign(key, value) if is_decay_zone_key(key) => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    if blocks.len() > 1 {
        return Err(ViewParseError(format!(
            "view.output {output:?} may contain only one decay-zone"
        )));
    }
    let Some(block) = blocks.first() else {
        return Ok(None);
    };
    let Value::Object(zone_fields) = block else {
        return Err(ViewParseError(format!(
            "decay-zone for output {output:?} must be an object"
        )));
    };
    parse_decay_zone_fields(zone_fields, output)
        .map(Some)
        .map_err(|error| ViewParseError(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vrr;

    fn parse(source: &str) -> Result<ViewConfig, ViewParseError> {
        parse_view_checked(&RuneConfig::from_str(source).expect("valid rune-cfg source"))
    }

    #[test]
    fn parses_hardware_and_focus_policy_from_one_output() {
        let view = parse(
            r#"
view:
  output:
    name "DP-1"
    width 2560
    height 1440
    offset-x 40
    rate 179.998
    transform 90
    vrr "auto"
    decay-zone:
      radius-x 900
      radius-y 500
      offset-x 20
    end
  end
end
"#,
        )
        .unwrap();

        assert_eq!(view.outputs.len(), 1);
        assert_eq!(view.outputs[0].name, "DP-1");
        assert_eq!(view.outputs[0].offset_x, 40);
        assert_eq!(view.outputs[0].rate, Some(179.998));
        assert_eq!(view.outputs[0].transform, 90);
        assert_eq!(view.outputs[0].vrr, Vrr::Auto);
        assert_eq!(view.decay_zones.for_output("DP-1").unwrap().radius_x, 900.0);
        assert_eq!(view.decay_zones.for_output("DP-2"), None);
    }

    #[test]
    fn decay_zone_is_optional_and_accepts_the_focus_ring_spelling() {
        let none = parse(
            "view:\n  output:\n    name \"DP-1\"\n    width 1920\n    height 1080\n  end\nend\n",
        )
        .unwrap();
        assert_eq!(none.decay_zones.for_output("DP-1"), None);

        let old = parse(
            "view:\n  output:\n    name \"DP-1\"\n    focus-ring:\n      radius-x 700\n    end\n  end\nend\n",
        )
        .unwrap();
        assert_eq!(old.decay_zones.for_output("DP-1").unwrap().radius_x, 700.0);

        let both = parse(
            "view:\n  output:\n    name \"DP-1\"\n    focus-ring:\n      radius-x 700\n    end\n    decay-zone:\n      radius-x 700\n    end\n  end\nend\n",
        );
        assert!(
            both.unwrap_err()
                .to_string()
                .contains("only one decay-zone")
        );
    }

    #[test]
    fn display_scale_can_be_configured_without_overriding_the_mode() {
        let view =
            parse("view:\n  output:\n    name \"eDP-1\"\n    scale 1.5\n  end\nend\n").unwrap();
        assert_eq!(view.outputs[0].scale, 1.5);
        assert_eq!(view.outputs[0].width, None);
        assert_eq!(view.outputs[0].height, None);
        let mode = parse(
            "view:\n  output:\n    name \"DP-1\"\n    width 2560\n    height 1440\n  end\nend\n",
        )
        .unwrap();
        assert_eq!(mode.outputs[0].scale, 1.0);
    }
}
