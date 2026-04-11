use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::keybinds::{
    apply_explicit_keybind_overrides_entries, parse_inline_keybinds, strip_inline_keybind_block,
};
use super::rules::load_rules_section;
use super::sections::{
    load_animations_section, load_autostart_section, load_bearings_section, load_clusters_section,
    load_cursor_section, load_decay_section, load_decorations_section, load_env_section,
    load_field_section, load_focus_ring_section, load_font_section, load_keybind_sections,
    load_nodes_section, load_overlays_section, load_physics_section, load_screenshot_section,
    load_stacking_section, load_tile_section, load_trail_section, load_viewport_section,
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

        let cfg = RuneConfig::from_file(path).or_else(|_| {
            let sanitized = strip_inline_keybind_block(&raw);
            RuneConfig::from_str(sanitized.as_str())
        });
        let cfg = cfg.ok()?;

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
        load_env_section(cfg, &mut out);
        load_cursor_section(cfg, &mut out);
        load_font_section(cfg, &mut out);
        load_viewport_section(cfg, &mut out);
        load_focus_ring_section(cfg, &mut out);
        load_bearings_section(cfg, &mut out);
        load_trail_section(cfg, &mut out);
        load_nodes_section(cfg, &mut out);
        load_clusters_section(cfg, &mut out);
        load_tile_section(cfg, &mut out);
        load_stacking_section(cfg, &mut out);
        load_decay_section(cfg, &mut out);
        load_field_section(cfg, &mut out);
        load_physics_section(cfg, &mut out);
        load_decorations_section(cfg, &mut out);
        load_animations_section(cfg, &mut out);
        load_overlays_section(cfg, &mut out);
        load_screenshot_section(cfg, &mut out);
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

pub fn from_rune_file(path: &str) -> Option<RuntimeTuning> {
    RuntimeTuning::from_rune_file(path)
}
