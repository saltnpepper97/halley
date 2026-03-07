use std::collections::HashMap;

use super::{KeyModifiers, LaunchBinding};
use crate::keybinds::{modifiers_empty, parse_chord, parse_modifiers, key_name_to_evdev};
use crate::legacy::{parse_legacy_keybinds, strip_legacy_keybind_block};
use crate::layout::ViewportOutputConfig;
use crate::RuntimeTuning;

use rune_cfg::RuneConfig;

impl RuntimeTuning {
    pub fn from_rune_file(path: &str) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let legacy_keybinds = parse_legacy_keybinds(raw.as_str());
        let cfg = RuneConfig::from_file(path).or_else(|_| {
            let sanitized = strip_legacy_keybind_block(raw.as_str());
            RuneConfig::from_str(sanitized.as_str())
        });
        let cfg = cfg.ok()?;
        let mut out = Self::default();

        out.tick_ms = pick_u64(&cfg, &["halley.runtime.tick_ms"], out.tick_ms);
        out.debug_tick_dump = pick_bool(&cfg, &["dev.debug_tick_dump"], out.debug_tick_dump);
        out.debug_dump_every_ms =
            pick_u64(&cfg, &["dev.debug_dump_every_ms"], out.debug_dump_every_ms);
        out.dev_enabled = pick_bool(&cfg, &["dev.enabled"], out.dev_enabled);
        out.dev_show_geometry_overlay = pick_bool(
            &cfg,
            &["dev.show_geometry_overlay"],
            out.dev_show_geometry_overlay,
        );
        out.dev_zoom_decay_enabled = pick_bool(
            &cfg,
            &["dev.zoom_decay_enabled"],
            out.dev_zoom_decay_enabled,
        );
        out.dev_zoom_decay_min_frac = pick_f32(
            &cfg,
            &["dev.zoom_decay_min_frac"],
            out.dev_zoom_decay_min_frac,
        );
        out.dev_anim_enabled = pick_bool(&cfg, &["dev.anim.enabled"], out.dev_anim_enabled);
        out.dev_anim_state_change_ms = pick_u64(
            &cfg,
            &["dev.anim.state_change_ms"],
            out.dev_anim_state_change_ms,
        );
        out.dev_anim_bounce = pick_f32(&cfg, &["dev.anim.bounce"], out.dev_anim_bounce);
        out.cluster_distance_px = pick_f32(
            &cfg,
            &["halley.clusters.distance_px"],
            out.cluster_distance_px,
        );
        out.cluster_dwell_ms = pick_u64(&cfg, &["halley.clusters.dwell_ms"], out.cluster_dwell_ms);
        out.non_overlap_gap_px = pick_f32(
            &cfg,
            &["halley.layout.non_overlap_gap_px"],
            out.non_overlap_gap_px,
        );
        out.non_overlap_active_gap_scale = pick_f32(
            &cfg,
            &["halley.layout.non_overlap.active_gap_scale"],
            out.non_overlap_active_gap_scale,
        );
        out.new_window_on_top = pick_bool(
            &cfg,
            &["halley.layout.new_window_on_top"],
            out.new_window_on_top,
        );
        out.non_overlap_bump_newer = pick_bool(
            &cfg,
            &["halley.layout.non_overlap.bump_newer"],
            out.non_overlap_bump_newer,
        );
        out.non_overlap_bump_damping = pick_f32(
            &cfg,
            &["halley.layout.non_overlap.bump_damping"],
            out.non_overlap_bump_damping,
        );
        out.drag_smoothing_boost = pick_f32(
            &cfg,
            &["halley.layout.drag_smoothing_boost"],
            out.drag_smoothing_boost,
        );
        out.center_window_to_mouse = pick_bool(
            &cfg,
            &["halley.layout.center_window_to_mouse"],
            out.center_window_to_mouse,
        );
        out.restore_last_active_on_pan_return = pick_bool(
            &cfg,
            &["halley.layout.restore_last_active_on_pan_return"],
            out.restore_last_active_on_pan_return,
        );
        out.physics_enabled = pick_bool(
            &cfg,
            &["halley.layout.physics_enabled"],
            out.physics_enabled,
        );

        out.viewport_center.x =
            pick_f32(&cfg, &["halley.viewport.center_x"], out.viewport_center.x);
        out.viewport_center.y =
            pick_f32(&cfg, &["halley.viewport.center_y"], out.viewport_center.y);
        out.viewport_size.x = pick_f32(&cfg, &["halley.viewport.size_w"], out.viewport_size.x);
        out.viewport_size.y = pick_f32(&cfg, &["halley.viewport.size_h"], out.viewport_size.y);
        out.tty_viewports = parse_viewport_outputs(&cfg);
        if let Some(primary) = out.tty_viewports.first() {
            out.viewport_size.x = primary.width as f32;
            out.viewport_size.y = primary.height as f32;
        }

        out.ring_primary_rx = pick_f32(&cfg, &["halley.ring.primary_rx"], out.ring_primary_rx);
        out.ring_primary_ry = pick_f32(&cfg, &["halley.ring.primary_ry"], out.ring_primary_ry);
        out.ring_secondary_rx =
            pick_f32(&cfg, &["halley.ring.secondary_rx"], out.ring_secondary_rx);
        out.ring_secondary_ry =
            pick_f32(&cfg, &["halley.ring.secondary_ry"], out.ring_secondary_ry);
        out.ring_rotation_rad =
            pick_f32(&cfg, &["halley.ring.rotation_rad"], out.ring_rotation_rad);

        out.secondary_to_node_ms = pick_u64(
            &cfg,
            &["halley.decay.secondary_to_node_ms"],
            out.secondary_to_node_ms,
        );
        out.primary_to_preview_ms = pick_u64(
            &cfg,
            &["halley.decay.primary_to_preview_ms"],
            out.primary_to_preview_ms,
        );
        out.primary_preview_to_node_ms = pick_u64(
            &cfg,
            &["halley.decay.primary_preview_to_node_ms"],
            out.primary_preview_to_node_ms,
        );
        out.primary_hot_inner_frac = pick_f32(
            &cfg,
            &["halley.decay.primary_hot_inner_frac"],
            out.primary_hot_inner_frac,
        );
        out.keybinds.modifier =
            pick_modifiers(&cfg, &["dev.keybinds.modifier"], out.keybinds.modifier);
        out.keybinds.reload_config = pick_keycode(
            &cfg,
            &["dev.keybinds.reload_config"],
            out.keybinds.reload_config,
        );
        out.keybinds.minimize_focused = pick_keycode(
            &cfg,
            &["dev.keybinds.minimize_focused"],
            out.keybinds.minimize_focused,
        );
        out.keybinds.overview_toggle = pick_keycode(
            &cfg,
            &["dev.keybinds.overview_toggle"],
            out.keybinds.overview_toggle,
        );
        out.keybinds.quit_compositor = pick_keycode(
            &cfg,
            &["dev.keybinds.quit_compositor"],
            out.keybinds.quit_compositor,
        );
        out.keybind_launch_command = pick_string(
            &cfg,
            &["dev.keybinds.launch_command"],
            out.keybind_launch_command.as_str(),
        );
        out.launch_bindings.clear();
        out.quit_requires_shift = pick_bool(
            &cfg,
            &["dev.keybinds.quit_requires_shift"],
            out.quit_requires_shift,
        );
        out.keybinds.primary_left = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_left"],
            out.keybinds.primary_left,
        );
        out.keybinds.primary_right = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_right"],
            out.keybinds.primary_right,
        );
        out.keybinds.primary_up =
            pick_keycode(&cfg, &["dev.keybinds.primary_up"], out.keybinds.primary_up);
        out.keybinds.primary_down = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_down"],
            out.keybinds.primary_down,
        );
        out.keybinds.secondary_left = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_left"],
            out.keybinds.secondary_left,
        );
        out.keybinds.secondary_right = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_right"],
            out.keybinds.secondary_right,
        );
        out.keybinds.secondary_up = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_up"],
            out.keybinds.secondary_up,
        );
        out.keybinds.secondary_down = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_down"],
            out.keybinds.secondary_down,
        );
        out.keybinds.move_left =
            pick_keycode(&cfg, &["dev.keybinds.move_left"], out.keybinds.move_left);
        out.keybinds.move_right =
            pick_keycode(&cfg, &["dev.keybinds.move_right"], out.keybinds.move_right);
        out.keybinds.move_up = pick_keycode(&cfg, &["dev.keybinds.move_up"], out.keybinds.move_up);
        out.keybinds.move_down =
            pick_keycode(&cfg, &["dev.keybinds.move_down"], out.keybinds.move_down);
        merge_env_map(&cfg, &mut out.env, "halley.env");
        apply_explicit_keybind_overrides(&cfg, &mut out);
        if !legacy_keybinds.is_empty() {
            apply_explicit_keybind_overrides_map(&legacy_keybinds, &mut out);
        }

        Some(out)
    }
}

fn merge_env_map(cfg: &RuneConfig, out: &mut HashMap<String, String>, path: &str) {
    let Ok(Some(entries)) = cfg.get_optional::<HashMap<String, String>>(path) else {
        return;
    };
    for (key, value) in entries {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        out.insert(key.to_string(), value.to_string());
    }
}

fn parse_viewport_outputs(cfg: &RuneConfig) -> Vec<ViewportOutputConfig> {
    let mut out = Vec::new();
    let Ok(keys) = cfg.get_keys("halley.viewport") else {
        return out;
    };
    for key in keys {
        let width = pick_u32(
            cfg,
            &[
                format!("halley.viewport.{}.width", key).as_str(),
                format!("halley.viewport.{}.size_w", key).as_str(),
            ],
            0,
        );
        let height = pick_u32(
            cfg,
            &[
                format!("halley.viewport.{}.height", key).as_str(),
                format!("halley.viewport.{}.size_h", key).as_str(),
            ],
            0,
        );
        if width == 0 || height == 0 {
            continue;
        }
        let offset_x = pick_i32(
            cfg,
            &[format!("halley.viewport.{}.offset_x", key).as_str()],
            0,
        );
        let offset_y = pick_i32(
            cfg,
            &[format!("halley.viewport.{}.offset_y", key).as_str()],
            0,
        );
        out.push(ViewportOutputConfig {
            connector: key,
            offset_x,
            offset_y,
            width,
            height,
        });
    }
    out
}


fn pick_u64(cfg: &RuneConfig, paths: &[&str], default: u64) -> u64 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u64>(path) {
            return v;
        }
    }
    default
}

fn pick_f32(cfg: &RuneConfig, paths: &[&str], default: f32) -> f32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<f32>(path) {
            return v;
        }
    }
    default
}

fn pick_u32(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
    }
    default
}

fn pick_i32(cfg: &RuneConfig, paths: &[&str], default: i32) -> i32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<i32>(path) {
            return v;
        }
    }
    default
}

fn pick_bool(cfg: &RuneConfig, paths: &[&str], default: bool) -> bool {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return v;
        }
    }
    default
}

fn pick_string(cfg: &RuneConfig, paths: &[&str], default: &str) -> String {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    default.to_string()
}

fn pick_modifiers(cfg: &RuneConfig, paths: &[&str], default: KeyModifiers) -> KeyModifiers {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            if let Some(m) = parse_modifiers(v.as_str()) {
                return m;
            }
        }
    }
    default
}

fn pick_keycode(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
        if let Ok(Some(name)) = cfg.get_optional::<String>(path) {
            if let Some(code) = key_name_to_evdev(name.as_str()) {
                return code;
            }
        }
    }
    default
}

fn apply_explicit_keybind_overrides(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let Ok(Some(bindings)) = cfg.get_optional::<HashMap<String, String>>("keybinds") else {
        return;
    };
    apply_explicit_keybind_overrides_map(&bindings, out);
}

fn apply_explicit_keybind_overrides_map(
    bindings: &HashMap<String, String>,
    out: &mut RuntimeTuning,
) {
    let mod_token = bindings
        .get("mod")
        .cloned()
        .unwrap_or_else(|| out.keybinds.modifier_name());
    if let Some(m) = parse_modifiers(mod_token.as_str()) {
        out.keybinds.modifier = m;
    }
    for (chord, action) in bindings {
        if chord.eq_ignore_ascii_case("mod") {
            continue;
        }
        apply_explicit_binding(
            out,
            mod_token.as_str(),
            out.keybinds.modifier,
            chord.as_str(),
            action.as_str(),
        );
    }
}

fn apply_explicit_binding(
    out: &mut RuntimeTuning,
    mod_token: &str,
    default_mods: KeyModifiers,
    chord: &str,
    action: &str,
) {
    let expanded = chord
        .replace("$var.mod", mod_token)
        .replace("$mod", mod_token);
    let Some((mods, key)) = parse_chord(expanded.as_str()) else {
        return;
    };
    let effective_mods = if modifiers_empty(mods) {
        default_mods
    } else {
        mods
    };
    let action_key = action.trim().to_ascii_lowercase();
    match action_key.as_str() {
        "reload_config" => out.keybinds.reload_config = key,
        "minimize_focused" => out.keybinds.minimize_focused = key,
        "overview_toggle" => out.keybinds.overview_toggle = key,
        "quit_halley" | "quit_compositor" => {
            out.keybinds.quit_compositor = key;
            out.quit_requires_shift = effective_mods.shift;
        }
        _ => {
            out.keybind_launch_command = action.trim().to_string();
            upsert_launch_binding(out, effective_mods, key, action.trim());
        }
    }
}

fn upsert_launch_binding(out: &mut RuntimeTuning, mods: KeyModifiers, key: u32, command: &str) {
    if let Some(existing) = out
        .launch_bindings
        .iter_mut()
        .find(|b| b.key == key && b.modifiers == mods)
    {
        existing.command = command.to_string();
        return;
    }
    out.launch_bindings.push(LaunchBinding {
        modifiers: mods,
        key,
        command: command.to_string(),
    });
}
