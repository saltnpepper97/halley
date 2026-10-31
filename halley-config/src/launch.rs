use std::collections::BTreeMap;
use std::fmt;

use rune_cfg::RuneConfig;
use rune_cfg::ast::{ObjectItem, Value};

/// Commands launched as part of the compositor session lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupCluster {
    pub name: String,
    pub members: Vec<String>,
    pub layout: Option<crate::ClusterLayout>,
    pub output: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Autostart {
    pub once: Vec<String>,
    pub on_reload: Vec<String>,
    pub clusters: Vec<StartupCluster>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchConfigError(String);

impl fmt::Display for LaunchConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LaunchConfigError {}

/// Parses optional environment overrides for applications launched by Halley.
///
/// Empty keys and values are ignored. When a key appears more than once, the
/// final declaration wins, matching the previous HashMap-based behavior.
pub fn parse_env(config: &RuneConfig) -> Result<BTreeMap<String, String>, LaunchConfigError> {
    let Some(fields) = section(config, "env")? else {
        return Ok(BTreeMap::new());
    };
    let mut env = BTreeMap::new();
    for item in fields {
        let ObjectItem::Assign(key, value) = item else {
            continue;
        };
        let Value::String(value) = value else {
            return Err(LaunchConfigError(format!("env.{key} must be a string")));
        };
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            env.insert(key.to_string(), value.to_string());
        }
    }
    Ok(env)
}

/// Parses ordered `once` and `on-reload` command declarations.
pub fn parse_autostart(config: &RuneConfig) -> Result<Autostart, LaunchConfigError> {
    let Some(fields) = section(config, "autostart")? else {
        return Ok(Autostart::default());
    };
    let mut autostart = Autostart::default();
    let mut cluster_counts = BTreeMap::<Option<String>, usize>::new();
    for item in fields {
        let ObjectItem::Assign(directive, value) = item else {
            continue;
        };
        match directive.as_str() {
            "once" | "on-reload" => {
                let Value::String(command) = value else {
                    return Err(LaunchConfigError(format!(
                        "autostart.{directive} must be a string"
                    )));
                };
                let command = command.trim();
                if command.is_empty() {
                    continue;
                }
                if directive == "once" {
                    autostart.once.push(command.to_string());
                } else {
                    autostart.on_reload.push(command.to_string());
                }
            }
            "cluster" => {
                let Value::Object(fields) = value else {
                    return Err(LaunchConfigError(
                        "autostart.cluster must be a block".to_string(),
                    ));
                };
                let cluster = parse_startup_cluster(&fields)?;
                let count = cluster_counts.entry(cluster.output.clone()).or_default();
                *count += 1;
                if *count > 10 {
                    let output = cluster.output.as_deref().unwrap_or("the primary output");
                    return Err(LaunchConfigError(format!(
                        "autostart configures more than 10 clusters for {output}"
                    )));
                }
                autostart.clusters.push(cluster);
            }
            _ => {
                return Err(LaunchConfigError(format!(
                    "autostart: unsupported directive {directive:?}"
                )));
            }
        }
    }
    Ok(autostart)
}

fn parse_startup_cluster(fields: &[ObjectItem]) -> Result<StartupCluster, LaunchConfigError> {
    let mut name = None;
    let mut members = None;
    let mut layout = None;
    let mut output = None;
    for item in fields {
        let ObjectItem::Assign(key, value) = item else {
            return Err(LaunchConfigError(
                "conditionals are not supported inside autostart.cluster".to_string(),
            ));
        };
        match key.as_str() {
            "name" => {
                let Value::String(value) = value else {
                    return Err(LaunchConfigError(
                        "autostart.cluster.name must be a string".to_string(),
                    ));
                };
                let value = value.trim();
                if value.is_empty() {
                    return Err(LaunchConfigError(
                        "autostart.cluster.name must not be empty".to_string(),
                    ));
                }
                name = Some(value.to_string());
            }
            "members" => {
                let Value::Array(values) = value else {
                    return Err(LaunchConfigError(
                        "autostart.cluster.members must be an array of command strings".to_string(),
                    ));
                };
                let mut commands = Vec::with_capacity(values.len());
                for value in values {
                    let Value::String(command) = value else {
                        return Err(LaunchConfigError(
                            "autostart.cluster.members accepts only command strings".to_string(),
                        ));
                    };
                    let command = command.trim();
                    if command.is_empty() {
                        return Err(LaunchConfigError(
                            "autostart.cluster.members commands must not be empty".to_string(),
                        ));
                    }
                    commands.push(command.to_string());
                }
                members = Some(commands);
            }
            "layout" => {
                let Value::String(value) = value else {
                    return Err(LaunchConfigError(
                        r#"autostart.cluster.layout must be "tiling" or "stacking""#.to_string(),
                    ));
                };
                layout = Some(match value.trim() {
                    "tiling" => crate::ClusterLayout::Tiling,
                    "stacking" => crate::ClusterLayout::Stacking,
                    _ => {
                        return Err(LaunchConfigError(
                            r#"autostart.cluster.layout must be "tiling" or "stacking""#
                                .to_string(),
                        ));
                    }
                });
            }
            "output" => {
                let Value::String(value) = value else {
                    return Err(LaunchConfigError(
                        "autostart.cluster.output must be a non-empty string".to_string(),
                    ));
                };
                let value = value.trim();
                if value.is_empty() {
                    return Err(LaunchConfigError(
                        "autostart.cluster.output must be a non-empty string".to_string(),
                    ));
                }
                output = Some(value.to_string());
            }
            _ => {
                return Err(LaunchConfigError(format!(
                    "autostart.cluster: unsupported field {key:?}"
                )));
            }
        }
    }
    Ok(StartupCluster {
        name: name
            .ok_or_else(|| LaunchConfigError("autostart.cluster requires name".to_string()))?,
        members: members.ok_or_else(|| {
            LaunchConfigError("autostart.cluster requires members array".to_string())
        })?,
        layout,
        output,
    })
}

fn section(config: &RuneConfig, name: &str) -> Result<Option<Vec<ObjectItem>>, LaunchConfigError> {
    let root = config
        .get_value("")
        .map_err(|err| LaunchConfigError(format!("launch config: {err}")))?;
    let Value::Object(fields) = root else {
        return Err(LaunchConfigError(
            "launch config root must be an object".to_string(),
        ));
    };
    let value = fields.into_iter().find_map(|item| match item {
        ObjectItem::Assign(key, value) if key == name => Some(value),
        _ => None,
    });
    match value {
        None => Ok(None),
        Some(Value::Object(fields)) => Ok(Some(fields)),
        Some(_) => Err(LaunchConfigError(format!("{name} must be a block"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> RuneConfig {
        RuneConfig::from_str(src).expect("valid Rune source")
    }

    #[test]
    fn environment_is_trimmed_and_last_value_wins() {
        let env = parse_env(&parse(
            r#"
env:
  QT_QPA_PLATFORM " wayland "
  EMPTY " "
  QT_QPA_PLATFORM "xcb"
end
"#,
        ))
        .unwrap();

        assert_eq!(
            env,
            BTreeMap::from([("QT_QPA_PLATFORM".to_string(), "xcb".to_string())])
        );
    }

    #[test]
    fn autostart_preserves_order_with_repeated_directives() {
        let autostart = parse_autostart(&parse(
            r#"
autostart:
  once "waybar"
  on-reload "notify-send reloaded"
  once "mako"
  on-reload "  "
end
"#,
        ))
        .unwrap();

        assert_eq!(autostart.once, ["waybar", "mako"]);
        assert_eq!(autostart.on_reload, ["notify-send reloaded"]);
    }

    #[test]
    fn startup_clusters_parse_compact_command_arrays() {
        let autostart = parse_autostart(&parse(
            r#"
autostart:
  cluster:
    name "Development"
    members ["foot" "firefox --new-window"]
  end
  cluster:
    name "Scratch"
    members []
    layout "stacking"
    output "DP-2"
  end
end
"#,
        ))
        .unwrap();

        assert_eq!(autostart.clusters.len(), 2);
        assert_eq!(
            autostart.clusters[0].members,
            ["foot", "firefox --new-window"]
        );
        assert_eq!(autostart.clusters[0].layout, None);
        assert!(autostart.clusters[1].members.is_empty());
        assert_eq!(
            autostart.clusters[1].layout,
            Some(crate::ClusterLayout::Stacking)
        );
        assert_eq!(autostart.clusters[1].output.as_deref(), Some("DP-2"));
    }
}
