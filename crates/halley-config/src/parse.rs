use std::collections::HashMap;

use super::{KeyModifiers, LaunchBinding};
use crate::keybinds::{key_name_to_evdev, modifiers_empty, parse_chord, parse_modifiers};
use crate::layout::ViewportOutputConfig;
use crate::legacy::{parse_legacy_keybinds, strip_legacy_keybind_block};
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

        load_dev_section(&cfg, &mut out);
        load_env_section(&cfg, &mut out);
        load_viewport_section(&cfg, &mut out);
        load_focus_ring_section(&cfg, &mut out);
        load_nodes_section(&cfg, &mut out);
        load_clusters_section(&cfg, &mut out);
        load_decay_section(&cfg, &mut out);
        load_tile_section(&cfg, &mut out);
        load_docking_section(&cfg, &mut out);
        load_physics_section(&cfg, &mut out);
        load_keybind_sections(&cfg, &mut out);

        if !legacy_keybinds.is_empty() {
            apply_explicit_keybind_overrides_map(&legacy_keybinds, &mut out);
        }

        Some(out)
    }
}

fn load_dev_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.tick_ms = pick_u64(cfg, &["dev.runtime.tick-ms", "dev.runtime.tick_ms"], out.tick_ms);

    out.debug_tick_dump = pick_bool(cfg, &["dev.debug_tick_dump"], out.debug_tick_dump);
    out.debug_dump_every_ms =
        pick_u64(cfg, &["dev.debug_dump_every_ms"], out.debug_dump_every_ms);

    out.dev_enabled = pick_bool(cfg, &["dev.enabled"], out.dev_enabled);
    out.dev_show_geometry_overlay = pick_bool(
        cfg,
        &["dev.show_geometry_overlay"],
        out.dev_show_geometry_overlay,
    );
    out.dev_zoom_decay_enabled =
        pick_bool(cfg, &["dev.zoom_decay_enabled"], out.dev_zoom_decay_enabled);
    out.dev_zoom_decay_min_frac = pick_f32(
        cfg,
        &["dev.zoom_decay_min_frac"],
        out.dev_zoom_decay_min_frac,
    );

    out.dev_anim_enabled = pick_bool(cfg, &["dev.anim.enabled"], out.dev_anim_enabled);
    out.dev_anim_state_change_ms = pick_u64(
        cfg,
        &["dev.anim.state_change_ms"],
        out.dev_anim_state_change_ms,
    );
    out.dev_anim_bounce = pick_f32(cfg, &["dev.anim.bounce"], out.dev_anim_bounce);

    out.keybinds.modifier =
        pick_modifiers(cfg, &["dev.keybinds.modifier"], out.keybinds.modifier);

    out.keybinds.reload = pick_keycode(cfg, &["dev.keybinds.reload"], out.keybinds.reload);
    out.keybinds.minimize_focused = pick_keycode(
        cfg,
        &["dev.keybinds.minimize_focused", "dev.keybinds.minimize-focused"],
        out.keybinds.minimize_focused,
    );
    out.keybinds.overview_toggle = pick_keycode(
        cfg,
        &["dev.keybinds.overview_toggle", "dev.keybinds.overview-toggle"],
        out.keybinds.overview_toggle,
    );
    out.keybinds.quit = pick_keycode(cfg, &["dev.keybinds.quit"], out.keybinds.quit);

    out.keybind_launch_command = pick_string(
        cfg,
        &["dev.keybinds.launch_command", "dev.keybinds.launch-command"],
        out.keybind_launch_command.as_str(),
    );

    out.quit_requires_shift = pick_bool(
        cfg,
        &[
            "dev.keybinds.quit_requires_shift",
            "dev.keybinds.quit-requires-shift",
        ],
        out.quit_requires_shift,
    );

    out.keybinds.primary_left = pick_keycode(
        cfg,
        &["dev.keybinds.primary_left", "dev.keybinds.primary-left"],
        out.keybinds.primary_left,
    );
    out.keybinds.primary_right = pick_keycode(
        cfg,
        &["dev.keybinds.primary_right", "dev.keybinds.primary-right"],
        out.keybinds.primary_right,
    );
    out.keybinds.primary_up = pick_keycode(
        cfg,
        &["dev.keybinds.primary_up", "dev.keybinds.primary-up"],
        out.keybinds.primary_up,
    );
    out.keybinds.primary_down = pick_keycode(
        cfg,
        &["dev.keybinds.primary_down", "dev.keybinds.primary-down"],
        out.keybinds.primary_down,
    );

    out.keybinds.secondary_left = pick_keycode(
        cfg,
        &["dev.keybinds.secondary_left", "dev.keybinds.secondary-left"],
        out.keybinds.secondary_left,
    );
    out.keybinds.secondary_right = pick_keycode(
        cfg,
        &["dev.keybinds.secondary_right", "dev.keybinds.secondary-right"],
        out.keybinds.secondary_right,
    );
    out.keybinds.secondary_up = pick_keycode(
        cfg,
        &["dev.keybinds.secondary_up", "dev.keybinds.secondary-up"],
        out.keybinds.secondary_up,
    );
    out.keybinds.secondary_down = pick_keycode(
        cfg,
        &["dev.keybinds.secondary_down", "dev.keybinds.secondary-down"],
        out.keybinds.secondary_down,
    );

    out.keybinds.move_left = pick_keycode(
        cfg,
        &["dev.keybinds.move_left", "dev.keybinds.move-left"],
        out.keybinds.move_left,
    );
    out.keybinds.move_right = pick_keycode(
        cfg,
        &["dev.keybinds.move_right", "dev.keybinds.move-right"],
        out.keybinds.move_right,
    );
    out.keybinds.move_up = pick_keycode(
        cfg,
        &["dev.keybinds.move_up", "dev.keybinds.move-up"],
        out.keybinds.move_up,
    );
    out.keybinds.move_down = pick_keycode(
        cfg,
        &["dev.keybinds.move_down", "dev.keybinds.move-down"],
        out.keybinds.move_down,
    );
}

fn load_env_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    merge_env_map(cfg, &mut out.env, "env");
}

fn load_viewport_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.viewport_center.x = pick_f32(
        cfg,
        &["viewport.center-x", "viewport.center_x"],
        out.viewport_center.x,
    );
    out.viewport_center.y = pick_f32(
        cfg,
        &["viewport.center-y", "viewport.center_y"],
        out.viewport_center.y,
    );

    out.viewport_size.x = pick_f32(
        cfg,
        &["viewport.size-w", "viewport.size_w"],
        out.viewport_size.x,
    );
    out.viewport_size.y = pick_f32(
        cfg,
        &["viewport.size-h", "viewport.size_h"],
        out.viewport_size.y,
    );

    out.tty_viewports = parse_viewport_outputs(cfg, "viewport");

    if let Some(primary) = out.tty_viewports.first() {
        out.viewport_size.x = primary.width as f32;
        out.viewport_size.y = primary.height as f32;
    }
}

fn load_focus_ring_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.focus_ring_rx = pick_f32(
        cfg,
        &["focus-ring.rx", "focus-ring.radius-x", "focus-ring.radius_x"],
        out.focus_ring_rx,
    );
    out.focus_ring_ry = pick_f32(
        cfg,
        &["focus-ring.ry", "focus-ring.radius-y", "focus-ring.radius_y"],
        out.focus_ring_ry,
    );

    out.focus_ring_offset_x = pick_f32(
        cfg,
        &["focus-ring.offset-x", "focus-ring.offset_x"],
        out.focus_ring_offset_x,
    );
    out.focus_ring_offset_y = pick_f32(
        cfg,
        &["focus-ring.offset-y", "focus-ring.offset_y"],
        out.focus_ring_offset_y,
    );

    out.focus_ring_rx = pick_f32(
        cfg,
        &["focus-ring.primary-rx", "focus-ring.primary_rx"],
        out.focus_ring_rx,
    );
    out.focus_ring_ry = pick_f32(
        cfg,
        &["focus-ring.primary-ry", "focus-ring.primary_ry"],
        out.focus_ring_ry,
    );
}

fn load_nodes_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.primary_to_node_ms = pick_u64(
        cfg,
        &[
            "nodes.primary-to-node-ms",
            "nodes.primary_to_node_ms",
            "nodes.node-delay",
            "nodes.node_delay",
        ],
        out.primary_to_node_ms,
    );

    let legacy_preview = pick_u64(
        cfg,
        &[
            "nodes.primary-to-preview-ms",
            "nodes.primary_to_preview_ms",
            "nodes.preview-delay",
            "nodes.preview_delay",
        ],
        0,
    );
    let legacy_preview_to_node = pick_u64(
        cfg,
        &[
            "nodes.primary-preview-to-node-ms",
            "nodes.primary_preview_to_node_ms",
            "nodes.preview-to-node-ms",
            "nodes.preview_to_node_ms",
        ],
        0,
    );

    if legacy_preview > 0 || legacy_preview_to_node > 0 {
        let combined = legacy_preview.saturating_add(legacy_preview_to_node);
        if combined > 0 {
            out.primary_to_node_ms = combined;
        }
    }

    out.primary_hot_inner_frac = pick_f32(
        cfg,
        &[
            "nodes.primary-hot-inner-frac",
            "nodes.primary_hot_inner_frac",
            "nodes.hot-inner-frac",
            "nodes.hot_inner_frac",
        ],
        out.primary_hot_inner_frac,
    );
}

fn load_clusters_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.cluster_distance_px = pick_f32(
        cfg,
        &["clusters.distance-px", "clusters.distance_px"],
        out.cluster_distance_px,
    );
    out.cluster_dwell_ms = pick_u64(
        cfg,
        &["clusters.dwell-ms", "clusters.dwell_ms"],
        out.cluster_dwell_ms,
    );
}

fn load_decay_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let primary_s = pick_u64(
        cfg,
        &["decay.primary-outside-ring-delay", "decay.primary_outside_ring_delay"],
        out.primary_outside_ring_delay_ms / 1000,
    );
    let secondary_s = pick_u64(
        cfg,
        &[
            "decay.secondary-outside-ring-delay",
            "decay.secondary_outside_ring_delay",
        ],
        out.secondary_outside_ring_delay_ms / 1000,
    );
    let docked_s = pick_u64(
        cfg,
        &["decay.docked-offscreen-delay", "decay.docked_offscreen_delay"],
        out.docked_offscreen_delay_ms / 1000,
    );

    out.primary_outside_ring_delay_ms = primary_s.saturating_mul(1000);
    out.secondary_outside_ring_delay_ms = secondary_s.saturating_mul(1000);
    out.docked_offscreen_delay_ms = docked_s.saturating_mul(1000);
}

fn load_tile_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.new_window_on_top = pick_bool(
        cfg,
        &["tile.new-on-top", "tile.new_on_top"],
        out.new_window_on_top,
    );
}

fn load_docking_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px =
        pick_f32(cfg, &["docking.gap", "docking.gap-px"], out.non_overlap_gap_px);

    out.restore_last_active_on_pan_return = pick_bool(
        cfg,
        &[
            "docking.restore-last-active-on-pan-return",
            "docking.restore_last_active_on_pan_return",
            "layout.restore-last-active-on-pan-return",
            "layout.restore_last_active_on_pan_return",
        ],
        out.restore_last_active_on_pan_return,
    );
}

fn load_physics_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.physics_enabled = pick_bool(cfg, &["physics.enabled"], out.physics_enabled);

    out.non_overlap_bump_damping = pick_f32(
        cfg,
        &["physics.damping", "physics.bump-damping", "physics.bump_damping"],
        out.non_overlap_bump_damping,
    );
}

fn load_keybind_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.keybinds.modifier = pick_modifiers(cfg, &["keybinds.mod"], out.keybinds.modifier);
    out.launch_bindings.clear();
    apply_explicit_keybind_overrides(cfg, out);
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

fn parse_viewport_outputs(cfg: &RuneConfig, root: &str) -> Vec<ViewportOutputConfig> {
    let mut out = Vec::new();

    let Ok(keys) = cfg.get_keys(root) else {
        return out;
    };

    for key in keys {
        let width = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.width").as_str(),
                format!("{root}.{key}.size-w").as_str(),
                format!("{root}.{key}.size_w").as_str(),
            ],
            0,
        );

        let height = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.height").as_str(),
                format!("{root}.{key}.size-h").as_str(),
                format!("{root}.{key}.size_h").as_str(),
            ],
            0,
        );

        if width == 0 || height == 0 {
            continue;
        }

        let offset_x = pick_i32(
            cfg,
            &[
                format!("{root}.{key}.offset-x").as_str(),
                format!("{root}.{key}.offset_x").as_str(),
            ],
            0,
        );

        let offset_y = pick_i32(
            cfg,
            &[
                format!("{root}.{key}.offset-y").as_str(),
                format!("{root}.{key}.offset_y").as_str(),
            ],
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

    let action_trimmed = action.trim();
    let action_key = action_trimmed.to_ascii_lowercase();

    match action_key.as_str() {
        "reload" => {
            out.keybinds.reload = key;
        }
        "minimize_focused" | "minimize-focused" => {
            out.keybinds.minimize_focused = key;
        }
        "overview_toggle" | "overview-toggle" => {
            out.keybinds.overview_toggle = key;
        }
        "quit" => {
            out.keybinds.quit = key;
            out.quit_requires_shift = effective_mods.shift;
        }
        _ => {
            out.keybind_launch_command = action_trimmed.to_string();
            upsert_launch_binding(out, effective_mods, key, action_trimmed);
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
