use rune_cfg::{RuneConfig, RuneError};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::layout::RuntimeTuning;

use super::keybinds::{
    apply_explicit_keybind_overrides_entries, parse_inline_keybinds, strip_inline_keybind_block,
};
use super::rules::load_rules_section;
use super::sections::{
    load_animations_section, load_apogee_section, load_autostart_section, load_background_section,
    load_bearings_section, load_clusters_section, load_cursor_section, load_debug_section,
    load_decay_section, load_decorations_section, load_effects_section, load_env_section,
    load_field_section, load_focus_ring_section, load_font_section, load_gamescope_section,
    load_input_section, load_keybind_sections, load_nodes_section, load_overlays_section,
    load_physics_section, load_placement_section, load_screenshot_section, load_stacking_section,
    load_tile_section, load_trail_section, load_viewport_section,
};
use super::validate::validate_known_config_keys;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigLoadDiagnostic {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    pub hint: Option<String>,
    pub source_line: Option<String>,
}

impl RuntimeTuning {
    pub fn from_rune_file(path: &str) -> Option<Self> {
        Self::from_rune_file_diagnostic(path).ok()
    }

    pub fn from_rune_file_diagnostic(path: &str) -> Result<Self, ConfigLoadDiagnostic> {
        let raw = std::fs::read_to_string(path).map_err(|err| ConfigLoadDiagnostic {
            path: path.to_string(),
            line: None,
            column: None,
            message: format!("failed to read config: {err}"),
            hint: Some("Check that the file exists and is readable".to_string()),
            source_line: None,
        })?;
        let seed = Self::builtin_defaults();
        let inline_keybinds = parse_inline_keybinds(&raw)
            .map_err(|err| diagnostic_from_message(path, raw.as_str(), err))?;

        let cfg = parse_rune_file_with_keybind_fallback_diagnostic(path, &raw)
            .map_err(|err| diagnostic_from_rune_error(path, raw.as_str(), err))?;
        validate_known_config_keys(raw.as_str(), path)?;

        Self::from_parsed_rune_diagnostic(path, raw.as_str(), &cfg, inline_keybinds, seed)
    }

    pub(crate) fn from_rune_str_with_seed(raw: &str, seed: Self) -> Option<Self> {
        let inline_keybinds = match parse_inline_keybinds(raw) {
            Ok(bindings) => bindings,
            Err(err) => {
                eprintln!("halley config keybind parse error: {err}");
                return None;
            }
        };

        let cfg = RuneConfig::from_str(raw).or_else(|_| {
            let sanitized = strip_inline_keybind_block(raw);
            RuneConfig::from_str(sanitized.as_str())
        });
        let cfg = cfg.ok()?;

        Self::from_parsed_rune(raw, &cfg, inline_keybinds, seed)
    }

    pub fn from_rune_str(raw: &str) -> Option<Self> {
        Self::from_rune_str_with_seed(raw, Self::builtin_defaults())
    }

    fn from_parsed_rune(
        raw: &str,
        cfg: &RuneConfig,
        inline_keybinds: Vec<(String, String)>,
        seed: Self,
    ) -> Option<Self> {
        Self::from_parsed_rune_diagnostic("<config>", raw, cfg, inline_keybinds, seed)
            .map_err(|err| {
                eprintln!("halley config parse error: {}", err.message);
            })
            .ok()
    }

    fn from_parsed_rune_diagnostic(
        path: &str,
        raw: &str,
        cfg: &RuneConfig,
        inline_keybinds: Vec<(String, String)>,
        seed: Self,
    ) -> Result<Self, ConfigLoadDiagnostic> {
        let mut out = seed;

        load_autostart_section(raw, &mut out);
        load_rules_section(raw, &mut out).map_err(|err| {
            diagnostic_from_message(path, raw, format!("rules parse error: {err}"))
        })?;
        load_gamescope_section(raw, &mut out).map_err(|err| {
            diagnostic_from_message(path, raw, format!("gamescope parse error: {err}"))
        })?;
        load_config_sections(cfg, &mut out);
        load_keybind_sections(cfg, &mut out).map_err(|err| {
            diagnostic_from_message(path, raw, format!("keybind parse error: {err}"))
        })?;

        if !inline_keybinds.is_empty() {
            apply_explicit_keybind_overrides_entries(&inline_keybinds, &mut out).map_err(
                |err| diagnostic_from_message(path, raw, format!("keybind parse error: {err}")),
            )?;
        }

        Ok(out)
    }
}

fn load_config_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    load_env_section(cfg, out);
    load_input_section(cfg, out);
    load_cursor_section(cfg, out);
    load_font_section(cfg, out);
    load_debug_section(cfg, out);
    load_apogee_section(cfg, out);
    load_background_section(cfg, out);
    load_viewport_section(cfg, out);
    load_focus_ring_section(cfg, out);
    load_bearings_section(cfg, out);
    load_trail_section(cfg, out);
    load_nodes_section(cfg, out);
    load_clusters_section(cfg, out);
    load_tile_section(cfg, out);
    load_stacking_section(cfg, out);
    load_decay_section(cfg, out);
    load_field_section(cfg, out);
    load_placement_section(cfg, out);
    load_physics_section(cfg, out);
    load_decorations_section(cfg, out);
    load_effects_section(cfg, out);
    load_animations_section(cfg, out);
    load_overlays_section(cfg, out);
    load_screenshot_section(cfg, out);
}

pub fn from_rune_file(path: &str) -> Option<RuntimeTuning> {
    RuntimeTuning::from_rune_file(path)
}

pub fn gather_dependencies_for_file(path: &str) -> Vec<PathBuf> {
    let root = absolutize_config_path(Path::new(path));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect_gather_dependencies(&root, &mut seen, &mut out);
    out
}

fn collect_gather_dependencies(path: &Path, seen: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let key = absolutize_config_path(path);
    if !seen.insert(key.clone()) {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&key) else {
        return;
    };
    let base_dir = key.parent().unwrap_or_else(|| Path::new("."));
    for line in raw.lines() {
        let Some(dep) = gather_path_from_line(line, base_dir) else {
            continue;
        };
        if !dep.exists() || out.contains(&dep) {
            continue;
        }
        out.push(dep.clone());
        collect_gather_dependencies(dep.as_path(), seen, out);
    }
}

fn gather_path_from_line(line: &str, base_dir: &Path) -> Option<PathBuf> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("gather") {
        return None;
    }
    let after_gather = trimmed.strip_prefix("gather")?.trim_start();
    let quote = after_gather.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let close_relative = after_gather[1..].find(quote)?;
    let raw_path = &after_gather[1..close_relative + 1];
    Some(resolve_gather_path_for_halley(raw_path, base_dir))
}

fn diagnostic_from_rune_error(path: &str, raw: &str, err: RuneError) -> ConfigLoadDiagnostic {
    let (line, column, hint) = rune_error_location(&err);
    ConfigLoadDiagnostic {
        path: path.to_string(),
        line,
        column,
        message: err.to_string(),
        hint,
        source_line: line.and_then(|line| source_line(raw, line)),
    }
}

fn diagnostic_from_message(path: &str, raw: &str, message: String) -> ConfigLoadDiagnostic {
    let line = line_from_message(message.as_str());
    ConfigLoadDiagnostic {
        path: path.to_string(),
        line,
        column: None,
        message,
        hint: None,
        source_line: line.and_then(|line| source_line(raw, line)),
    }
}

fn rune_error_location(err: &RuneError) -> (Option<usize>, Option<usize>, Option<String>) {
    match err {
        RuneError::SyntaxError {
            line, column, hint, ..
        }
        | RuneError::InvalidToken {
            line, column, hint, ..
        }
        | RuneError::UnexpectedEof {
            line, column, hint, ..
        }
        | RuneError::TypeError {
            line, column, hint, ..
        }
        | RuneError::UnclosedString {
            line, column, hint, ..
        }
        | RuneError::UnexpectedCharacter {
            line, column, hint, ..
        }
        | RuneError::ValidationError {
            line, column, hint, ..
        } => (
            (*line > 0).then_some(*line),
            (*column > 0).then_some(*column),
            hint.clone(),
        ),
        RuneError::FileError { hint, .. } | RuneError::RuntimeError { hint, .. } => {
            (None, None, hint.clone())
        }
    }
}

fn source_line(raw: &str, line: usize) -> Option<String> {
    raw.lines()
        .nth(line.saturating_sub(1))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

fn line_from_message(message: &str) -> Option<usize> {
    let idx = message.find("line ")?;
    message[idx + 5..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

fn parse_rune_file_with_keybind_fallback_diagnostic(
    path: &str,
    raw: &str,
) -> Result<RuneConfig, RuneError> {
    RuneConfig::from_file(path)
        .ok()
        .or_else(|| {
            let sanitized = strip_inline_keybind_block(raw);
            parse_sanitized_rune_file(path, sanitized.as_str())
                .or_else(|| RuneConfig::from_str(sanitized.as_str()).ok())
        })
        .ok_or_else(|| {
            let sanitized = strip_inline_keybind_block(raw);
            RuneConfig::from_str(sanitized.as_str())
                .err()
                .unwrap_or_else(|| RuneError::RuntimeError {
                    message: "config parsing failed".to_string(),
                    hint: None,
                    code: None,
                })
        })
}

fn parse_sanitized_rune_file(original_path: &str, sanitized: &str) -> Option<RuneConfig> {
    let original_path = Path::new(original_path);
    let temp_dir = sanitized_config_temp_dir(original_path);
    std::fs::create_dir_all(&temp_dir).ok()?;

    let mut visited = HashMap::new();
    let temp_path =
        write_sanitized_config_tree(original_path, Some(sanitized), &temp_dir, &mut visited)?;
    let cfg = RuneConfig::from_file(temp_path.as_path()).ok();
    let _ = std::fs::remove_dir_all(&temp_dir);
    cfg
}

fn write_sanitized_config_tree(
    source_path: &Path,
    raw_override: Option<&str>,
    temp_dir: &Path,
    visited: &mut HashMap<PathBuf, PathBuf>,
) -> Option<PathBuf> {
    let source_key = absolutize_config_path(source_path);
    if let Some(existing) = visited.get(&source_key) {
        return Some(existing.clone());
    }

    let temp_path = sanitized_config_temp_path(&source_key, temp_dir, visited.len());
    visited.insert(source_key.clone(), temp_path.clone());

    let raw = match raw_override {
        Some(raw) => raw.to_string(),
        None => std::fs::read_to_string(&source_key).ok()?,
    };
    let sanitized = strip_inline_keybind_block(&raw);
    let rewritten = rewrite_gather_paths_to_sanitized_files(
        sanitized.as_str(),
        source_key.parent().unwrap_or_else(|| Path::new(".")),
        temp_dir,
        visited,
    );

    std::fs::write(&temp_path, rewritten).ok()?;
    Some(temp_path)
}

fn rewrite_gather_paths_to_sanitized_files(
    content: &str,
    base_dir: &Path,
    temp_dir: &Path,
    visited: &mut HashMap<PathBuf, PathBuf>,
) -> String {
    let mut out = String::with_capacity(content.len());

    for line in content.lines() {
        if let Some(rewritten) = rewrite_gather_line(line, base_dir, temp_dir, visited) {
            out.push_str(rewritten.as_str());
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    out
}

fn rewrite_gather_line(
    line: &str,
    base_dir: &Path,
    temp_dir: &Path,
    visited: &mut HashMap<PathBuf, PathBuf>,
) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("gather") {
        return None;
    }

    let indent_len = line.len() - trimmed.len();
    let after_gather = trimmed.strip_prefix("gather")?.trim_start();
    let quote = after_gather.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let close_relative = after_gather[1..].find(quote)?;
    let raw_path = &after_gather[1..close_relative + 1];
    let after_path = &after_gather[close_relative + 2..];
    let import_path = resolve_gather_path_for_halley(raw_path, base_dir);

    if !import_path.exists() {
        return None;
    }

    let sanitized_import = write_sanitized_config_tree(&import_path, None, temp_dir, visited)?;
    Some(format!(
        "{}gather \"{}\"{}",
        &line[..indent_len],
        sanitized_import.to_string_lossy(),
        after_path
    ))
}

fn resolve_gather_path_for_halley(raw_path: &str, base_dir: &Path) -> PathBuf {
    let mut path = if let Some(rest) = raw_path.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest)
    } else {
        PathBuf::from(raw_path)
    };

    if path.is_relative() {
        path = base_dir.join(path);
    }

    absolutize_config_path(&path)
}

fn absolutize_config_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn sanitized_config_temp_dir(original_path: &Path) -> PathBuf {
    let stem = original_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("halley");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    std::env::temp_dir().join(format!(
        "{stem}.sanitized.{}.{}",
        std::process::id(),
        unique
    ))
}

fn sanitized_config_temp_path(source_path: &Path, temp_dir: &Path, index: usize) -> PathBuf {
    let stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("halley");
    temp_dir.join(format!("{index}-{stem}.rune"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybinds::CompositorBindingAction;
    use crate::layout::{OverlayColorMode, PinBadgeCorner};

    #[test]
    fn from_rune_file_resolves_gather_when_inline_keybinds_require_sanitized_parse() {
        let dir = test_temp_dir("gather-inline-keybinds");
        let import_path = dir.join("colors.rune");
        let config_path = dir.join("halley.rune");

        std::fs::write(
            &import_path,
            r##"pywal_background "#123456"

keybinds:
  mod "super"
  "$var.mod+q" "close-focused"
end
"##,
        )
        .unwrap();
        std::fs::write(
            &config_path,
            r##"gather "colors.rune"

screenshot:
  background-colour pywal_background
end

keybinds:
  mod "super"
  "$var.mod+r" "reload"
end
"##,
        )
        .unwrap();

        let tuning = RuntimeTuning::from_rune_file(config_path.to_str().unwrap())
            .expect("config should parse with gathered colors and inline keybinds");

        assert_eq!(
            tuning.screenshot.background_color,
            OverlayColorMode::Fixed {
                r: 0x12 as f32 / 255.0,
                g: 0x34 as f32 / 255.0,
                b: 0x56 as f32 / 255.0,
                a: 1.0,
            }
        );
        assert!(tuning.keybinds.modifier.super_key);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn from_rune_file_deep_merges_unaliased_gather_sections() {
        let dir = test_temp_dir("gather-deep-merge");
        let import_path = dir.join("colors.rune");
        let config_path = dir.join("halley.rune");

        std::fs::write(
            &import_path,
            r##"field:
  pins:
    colour "#4a4768"
  end
end
"##,
        )
        .unwrap();
        std::fs::write(
            &config_path,
            r##"gather "colors.rune"

field:
  gap 20.0
  pins:
    corner "top-left"
    size 1.0
  end
end
"##,
        )
        .unwrap();

        let tuning = RuntimeTuning::from_rune_file(config_path.to_str().unwrap())
            .expect("config should parse with deep-merged gathered field settings");

        assert_eq!(tuning.non_overlap_gap_px, 20.0);
        assert_eq!(tuning.pins.corner, PinBadgeCorner::TopLeft);
        assert_eq!(tuning.pins.size, 1.0);
        assert_eq!(
            tuning.pins.color,
            OverlayColorMode::Fixed {
                r: 0x4a as f32 / 255.0,
                g: 0x47 as f32 / 255.0,
                b: 0x68 as f32 / 255.0,
                a: 1.0,
            }
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn from_rune_file_validates_and_loads_debug_booleans() {
        let dir = test_temp_dir("debug-booleans");
        let config_path = dir.join("halley.rune");

        std::fs::write(
            &config_path,
            r#"debug:
  overlay-fps true
  show-ring-when-resizing false
end
"#,
        )
        .unwrap();

        let tuning = RuntimeTuning::from_rune_file(config_path.to_str().unwrap())
            .expect("debug booleans should pass strict validation and load");

        assert!(tuning.debug.overlay_fps);
        assert!(!tuning.debug.show_ring_when_resizing);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn from_rune_file_validates_and_loads_apogee_section() {
        let dir = test_temp_dir("apogee-section");
        let config_path = dir.join("halley.rune");

        std::fs::write(
            &config_path,
            r#"apogee:
  enabled true
  live-previews false
  transition-ms 320
  gap 24.0
  max-rows 3
  background-dim 0.85
end

keybinds:
  mod "super"
  "$var.mod+a" "apogee"
end
"#,
        )
        .unwrap();

        let tuning = RuntimeTuning::from_rune_file(config_path.to_str().unwrap())
            .expect("apogee section should pass strict validation and load");

        assert!(tuning.apogee.enabled);
        assert!(!tuning.apogee.live_previews);
        assert_eq!(tuning.apogee.transition_ms, 320);
        assert_eq!(tuning.apogee.gap, 24.0);
        assert_eq!(tuning.apogee.max_rows, 3);
        assert_eq!(tuning.apogee.background_dim, 0.85);
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::Apogee)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gather_dependencies_for_file_collects_nested_imports() {
        let dir = test_temp_dir("gather-dependencies");
        let nested_path = dir.join("nested.rune");
        let import_path = dir.join("colors.rune");
        let config_path = dir.join("halley.rune");

        std::fs::write(&nested_path, "field:\n  gap 22\nend\n").unwrap();
        std::fs::write(
            &import_path,
            r##"gather "nested.rune"
nodes:
  icon-size 0.62
end
"##,
        )
        .unwrap();
        std::fs::write(&config_path, r##"gather "colors.rune""##).unwrap();

        let deps = gather_dependencies_for_file(config_path.to_str().unwrap());

        assert!(deps.contains(&import_path));
        assert!(deps.contains(&nested_path));

        let _ = std::fs::remove_dir_all(dir);
    }

    fn test_temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "halley-config-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
