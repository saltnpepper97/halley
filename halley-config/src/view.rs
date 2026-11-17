use std::collections::HashSet;
use std::fmt;

use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

use crate::output::{OutputConfig, is_hardware_field, parse_hardware_output};

/// The config-facing aggregate for output-specific view policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewConfig {
    pub outputs: Vec<OutputConfig>,
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
        let hardware = parse_hardware_output(&name, fields)
            .map_err(|error| ViewParseError(error.to_string()))?;
        match hardware {
            Some(output) => parsed.outputs.push(output),
            // An entry that held only a retired decay zone configures
            // nothing now, but configs written for it keep loading.
            None if has_retired_decay_zone(fields) => {}
            None => {
                return Err(ViewParseError(format!(
                    "view.output {name:?} must configure hardware"
                )));
            }
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
        if key != "name" && !is_retired_decay_zone_key(key) && !is_hardware_field(key) {
            return Err(ViewParseError(format!(
                "view.output {output:?}: unknown setting {key:?}"
            )));
        }
    }
    Ok(())
}

/// The decay zone (once spelled `focus-ring`) was removed when decay became
/// off-screen only. Existing blocks are accepted and ignored.
fn is_retired_decay_zone_key(key: &str) -> bool {
    matches!(key, "decay-zone" | "focus-ring")
}

fn has_retired_decay_zone(fields: &[ObjectItem]) -> bool {
    fields
        .iter()
        .any(|item| matches!(item, ObjectItem::Assign(key, _) if is_retired_decay_zone_key(key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vrr;

    fn parse(source: &str) -> Result<ViewConfig, ViewParseError> {
        parse_view_checked(&RuneConfig::from_str(source).expect("valid rune-cfg source"))
    }

    #[test]
    fn parses_hardware_and_ignores_a_retired_decay_zone() {
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
    }

    #[test]
    fn retired_decay_zone_blocks_still_load() {
        for key in ["decay-zone", "focus-ring"] {
            let only_zone = parse(&format!(
                "view:\n  output:\n    name \"DP-1\"\n    {key}:\n      radius-x 700\n    end\n  end\nend\n"
            ))
            .unwrap();
            assert!(only_zone.outputs.is_empty(), "{key}");
        }
        let top_level = parse("decay-zone:\n  radius-x 700\nend\n").unwrap();
        assert!(top_level.outputs.is_empty());

        let empty = parse("view:\n  output:\n    name \"DP-1\"\n  end\nend\n");
        assert!(
            empty
                .unwrap_err()
                .to_string()
                .contains("must configure hardware")
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
