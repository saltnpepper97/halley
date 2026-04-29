use rune_cfg::RuneConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::layout::RuntimeTuning;

use super::keybinds::{
    apply_explicit_keybind_overrides_entries, parse_inline_keybinds, strip_inline_keybind_block,
};
use super::rules::load_rules_section;
use super::sections::{
    load_animations_section, load_autostart_section, load_bearings_section, load_clusters_section,
    load_cursor_section, load_decay_section, load_decorations_section, load_env_section,
    load_field_section, load_focus_ring_section, load_font_section, load_input_section,
    load_keybind_sections, load_nodes_section, load_overlays_section, load_physics_section,
    load_screenshot_section, load_stacking_section, load_tile_section, load_trail_section,
    load_viewport_section,
};

impl RuntimeTuning {
    pub fn from_rune_file(path: &str) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let seed = Self::builtin_defaults();
        let inline_keybinds = match parse_inline_keybinds(&raw) {
            Ok(bindings) => bindings,
            Err(err) => {
                eprintln!("halley config keybind parse error: {err}");
                return None;
            }
        };

        let cfg = parse_rune_file_with_keybind_fallback(path, &raw)?;

        Self::from_parsed_rune(raw.as_str(), &cfg, inline_keybinds, seed)
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
        let mut out = seed;

        load_autostart_section(raw, &mut out);
        if let Err(err) = load_rules_section(raw, &mut out) {
            eprintln!("halley config rules parse error: {err}");
            return None;
        }
        load_config_sections(cfg, &mut out);
        if let Err(err) = load_keybind_sections(cfg, &mut out) {
            eprintln!("halley config keybind parse error: {err}");
            return None;
        }

        if !inline_keybinds.is_empty() {
            if let Err(err) = apply_explicit_keybind_overrides_entries(&inline_keybinds, &mut out) {
                eprintln!("halley config keybind parse error: {err}");
                return None;
            }
        }

        Some(out)
    }
}

fn load_config_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    load_env_section(cfg, out);
    load_input_section(cfg, out);
    load_cursor_section(cfg, out);
    load_font_section(cfg, out);
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
    load_physics_section(cfg, out);
    load_decorations_section(cfg, out);
    load_animations_section(cfg, out);
    load_overlays_section(cfg, out);
    load_screenshot_section(cfg, out);
}

pub fn from_rune_file(path: &str) -> Option<RuntimeTuning> {
    RuntimeTuning::from_rune_file(path)
}

fn parse_rune_file_with_keybind_fallback(path: &str, raw: &str) -> Option<RuneConfig> {
    RuneConfig::from_file(path).ok().or_else(|| {
        let sanitized = strip_inline_keybind_block(raw);
        parse_sanitized_rune_file(path, sanitized.as_str())
            .or_else(|| RuneConfig::from_str(sanitized.as_str()).ok())
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
    use crate::layout::OverlayColorMode;

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
            }
        );
        assert!(tuning.keybinds.modifier.super_key);

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
