use std::collections::HashMap;

use super::{
    AutostartCommand, AutostartPhase, KeyModifiers, LaunchBinding, PointerBinding,
    PointerBindingAction,
};
use crate::RuntimeTuning;
use crate::keybinds::{is_pointer_button_code, key_name_to_evdev, parse_chord, parse_modifiers};
use crate::layout::{ViewportOutputConfig, default_pointer_bindings};
use crate::legacy::{parse_legacy_keybinds, strip_legacy_keybind_block};

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

        load_autostart_section(raw.as_str(), &mut out);
        load_dev_section(&cfg, &mut out);
        load_autostart_section(raw.as_str(), &mut out);
        load_env_section(&cfg, &mut out);
        load_viewport_section(&cfg, &mut out);
        load_focus_ring_section(&cfg, &mut out);
        load_nodes_section(&cfg, &mut out);
        load_clusters_section(&cfg, &mut out);
        load_decay_section(&cfg, &mut out);
        load_docking_section(&cfg, &mut out);
        load_physics_section(&cfg, &mut out);
        load_keybind_sections(&cfg, &mut out);

        if !legacy_keybinds.is_empty() {
            apply_explicit_keybind_overrides_map(&legacy_keybinds, &mut out);
        }

        Some(out)
    }
}

fn load_autostart_section(raw: &str, out: &mut RuntimeTuning) {
    out.autostart_commands = parse_autostart_commands(raw);
}

fn parse_autostart_commands(raw: &str) -> Vec<AutostartCommand> {
    let mut out = Vec::new();
    let mut in_autostart = false;
    let mut depth = 0usize;

    for raw_line in raw.lines() {
        let stripped = strip_rune_comment(raw_line);
        let line = stripped.trim();
        if line.is_empty() {
            continue;
        }

        if !in_autostart {
            if line == "autostart:" {
                in_autostart = true;
                depth = 1;
            }
            continue;
        }

        if line == "end" {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                break;
            }
            continue;
        }

        if depth == 1 {
            if let Some(command) = parse_autostart_command(line, "once") {
                out.push(AutostartCommand {
                    phase: AutostartPhase::Once,
                    command,
                });
            } else if let Some(command) = parse_autostart_command(line, "on-reload")
                .or_else(|| parse_autostart_command(line, "on_reload"))
            {
                out.push(AutostartCommand {
                    phase: AutostartPhase::OnReload,
                    command,
                });
            }
        }

        if line.ends_with(':') {
            depth = depth.saturating_add(1);
        }
    }

    out
}

fn strip_rune_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in line.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                out.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                out.push(ch);
            }
            '#' if !in_quotes => break,
            _ => out.push(ch),
        }
    }

    out
}

fn parse_autostart_command(line: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(keyword)?.trim();
    parse_quoted_string(rest)
}

fn parse_quoted_string(input: &str) -> Option<String> {
    let mut chars = input.chars();
    if chars.next()? != '"' {
        return None;
    }

    let mut out = String::new();
    let mut escaped = false;

    for ch in chars {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            _ => out.push(ch),
        }
    }

    None
}

fn load_dev_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.tick_ms = pick_u64(
        cfg,
        &["dev.runtime.tick-ms", "dev.runtime.tick_ms"],
        out.tick_ms,
    );

    out.debug_tick_dump = pick_bool(cfg, &["dev.debug_tick_dump"], out.debug_tick_dump);
    out.debug_dump_every_ms = pick_u64(cfg, &["dev.debug_dump_every_ms"], out.debug_dump_every_ms);

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

    out.keybinds.modifier = pick_modifiers(cfg, &["dev.keybinds.modifier"], out.keybinds.modifier);

    out.keybinds.reload = pick_keycode(cfg, &["dev.keybinds.reload"], out.keybinds.reload);
    out.keybinds.minimize_focused = pick_keycode(
        cfg,
        &[
            "dev.keybinds.minimize_focused",
            "dev.keybinds.minimize-focused",
        ],
        out.keybinds.minimize_focused,
    );
    out.keybinds.overview_toggle = pick_keycode(
        cfg,
        &[
            "dev.keybinds.overview_toggle",
            "dev.keybinds.overview-toggle",
        ],
        out.keybinds.overview_toggle,
    );
    out.keybinds.quit = pick_keycode(cfg, &["dev.keybinds.quit"], out.keybinds.quit);
    out.keybinds.docking = pick_keycode(cfg, &["dev.keybinds.docking"], out.keybinds.docking);

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
        &[
            "dev.keybinds.secondary_right",
            "dev.keybinds.secondary-right",
        ],
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

        out.viewport_center.x = primary.offset_x as f32 + primary.width as f32 / 2.0;
        out.viewport_center.y = primary.offset_y as f32 + primary.height as f32 / 2.0;
    }
}

fn load_focus_ring_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.focus_ring_rx = pick_f32(
        cfg,
        &[
            "focus-ring.rx",
            "focus-ring.radius-x",
            "focus-ring.radius_x",
        ],
        out.focus_ring_rx,
    );
    out.focus_ring_ry = pick_f32(
        cfg,
        &[
            "focus-ring.ry",
            "focus-ring.radius-y",
            "focus-ring.radius_y",
        ],
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
        &[
            "decay.primary-outside-ring-delay",
            "decay.primary_outside_ring_delay",
        ],
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
        &[
            "decay.docked-offscreen-delay",
            "decay.docked_offscreen_delay",
        ],
        out.docked_offscreen_delay_ms / 1000,
    );

    out.primary_outside_ring_delay_ms = primary_s.saturating_mul(1000);
    out.secondary_outside_ring_delay_ms = secondary_s.saturating_mul(1000);
    out.docked_offscreen_delay_ms = docked_s.saturating_mul(1000);
}

fn load_docking_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px = pick_f32(
        cfg,
        &["docking.gap", "docking.gap-px"],
        out.non_overlap_gap_px,
    );

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
        &[
            "physics.damping",
            "physics.bump-damping",
            "physics.bump_damping",
        ],
        out.non_overlap_bump_damping,
    );
}

fn load_keybind_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.keybinds.modifier = pick_modifiers(cfg, &["keybinds.mod"], out.keybinds.modifier);
    sync_action_modifiers_with_global(&mut out.keybinds);
    out.launch_bindings.clear();
    out.pointer_bindings = default_pointer_bindings(out.keybinds.modifier);
    apply_explicit_keybind_overrides(cfg, out);
}

fn sync_action_modifiers_with_global(keybinds: &mut crate::Keybinds) {
    let modifier = keybinds.modifier;
    keybinds.reload_modifiers = modifier;
    keybinds.minimize_focused_modifiers = modifier;
    keybinds.overview_toggle_modifiers = modifier;
    keybinds.quit_modifiers = modifier;
    keybinds.docking_modifiers = modifier;
    keybinds.move_left_modifiers = modifier;
    keybinds.move_right_modifiers = modifier;
    keybinds.move_up_modifiers = modifier;
    keybinds.move_down_modifiers = modifier;
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

        let refresh_rate = {
            let v = pick_f32(
                cfg,
                &[
                    format!("{root}.{key}.refresh-rate").as_str(),
                    format!("{root}.{key}.refresh_rate").as_str(),
                    format!("{root}.{key}.rate").as_str(),
                ],
                0.0,
            );
            if v > 0.0 { Some(v as f64) } else { None }
        };

        out.push(ViewportOutputConfig {
            connector: key,
            offset_x,
            offset_y,
            width,
            height,
            refresh_rate,
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
        if let Ok(Some(v)) = cfg.get_optional::<String>(path)
            && !v.trim().is_empty()
        {
            return v;
        }
    }
    default.to_string()
}

fn pick_modifiers(cfg: &RuneConfig, paths: &[&str], default: KeyModifiers) -> KeyModifiers {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path)
            && let Some(m) = parse_modifiers(v.as_str())
        {
            return m;
        }
    }
    default
}

fn pick_keycode(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
        if let Ok(Some(name)) = cfg.get_optional::<String>(path)
            && let Some(code) = key_name_to_evdev(name.as_str())
        {
            return code;
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
        out.pointer_bindings = default_pointer_bindings(out.keybinds.modifier);
    }

    for (chord, action) in bindings {
        if chord.eq_ignore_ascii_case("mod") {
            continue;
        }

        apply_explicit_binding(out, mod_token.as_str(), chord.as_str(), action.as_str());
    }
}

fn apply_explicit_binding(out: &mut RuntimeTuning, mod_token: &str, chord: &str, action: &str) {
    let expanded = chord
        .replace("$var.mod", mod_token)
        .replace("$mod", mod_token);

    let Some((mods, key)) = parse_chord(expanded.as_str()) else {
        return;
    };

    let effective_mods = mods;

    let action_trimmed = action.trim();
    let action_key = action_trimmed.to_ascii_lowercase();

    match action_key.as_str() {
        "reload" | "reload_config" | "reload-config" => {
            out.keybinds.reload = key;
            out.keybinds.reload_modifiers = effective_mods;
        }
        "minimize_focused" | "minimize-focused" => {
            out.keybinds.minimize_focused = key;
            out.keybinds.minimize_focused_modifiers = effective_mods;
        }
        "overview_toggle" | "overview-toggle" => {
            out.keybinds.overview_toggle = key;
            out.keybinds.overview_toggle_modifiers = effective_mods;
        }
        "quit" | "quit_halley" | "quit-halley" => {
            out.keybinds.quit = key;
            out.keybinds.quit_modifiers = effective_mods;
            out.quit_requires_shift = effective_mods.shift;
        }
        "docking" => {
            out.keybinds.docking = key;
            out.keybinds.docking_modifiers = effective_mods;
        }
        "move_left" | "move-left" => {
            out.keybinds.move_left = key;
            out.keybinds.move_left_modifiers = effective_mods;
        }
        "move_right" | "move-right" => {
            out.keybinds.move_right = key;
            out.keybinds.move_right_modifiers = effective_mods;
        }
        "move_up" | "move-up" => {
            out.keybinds.move_up = key;
            out.keybinds.move_up_modifiers = effective_mods;
        }
        "move_down" | "move-down" => {
            out.keybinds.move_down = key;
            out.keybinds.move_down_modifiers = effective_mods;
        }
        "move_window" | "move-window" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::MoveWindow);
        }
        "resize_window" | "resize-window" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::ResizeWindow);
        }
        _ => {
            out.keybind_launch_command = action_trimmed.to_string();
            upsert_launch_binding(out, mods, key, action_trimmed);
        }
    }
}

fn upsert_compositor_binding(
    out: &mut RuntimeTuning,
    mods: KeyModifiers,
    key: u32,
    action: CompositorBindingAction,
) {
    if let Some(existing) = out
        .compositor_bindings
        .iter_mut()
        .find(|b| b.key == key && b.modifiers == mods)
    {
        existing.action = action;
        return;
    }

    out.compositor_bindings.push(CompositorBinding {
        modifiers: mods,
        key,
        action,
    });
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

fn upsert_pointer_binding(
    out: &mut RuntimeTuning,
    mods: KeyModifiers,
    button: u32,
    action: PointerBindingAction,
) {
    if let Some(existing) = out
        .pointer_bindings
        .iter_mut()
        .find(|b| b.button == button && b.modifiers == mods)
    {
        existing.action = action;
        return;
    }

    out.pointer_bindings.push(PointerBinding {
        modifiers: mods,
        button,
        action,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn explicit_move_binding_keeps_shift_modifier() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("mod".to_string(), "super".to_string()),
            ("$var.mod+shift+left".to_string(), "move_left".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert_eq!(tuning.keybinds.move_left, 105);
        assert!(tuning.keybinds.move_left_modifiers.super_key);
        assert!(tuning.keybinds.move_left_modifiers.shift);
    }

    #[test]
    fn legacy_quit_halley_binding_maps_to_quit_keybind() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("mod".to_string(), "super".to_string()),
            ("$var.mod+shift+q".to_string(), "quit_halley".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert_eq!(tuning.keybinds.quit, 16);
        assert!(tuning.keybinds.quit_modifiers.super_key);
        assert!(tuning.keybinds.quit_modifiers.shift);
        assert!(tuning.quit_requires_shift);
    }

    #[test]
    fn parses_autostart_commands_from_rune_block() {
        let raw = r#"
autostart:
  once "waybar"
  # once "ignored"
  on-reload "makoctl reload"
end
"#;

        let commands = parse_autostart_commands(raw);

        assert_eq!(
            commands,
            vec![
                AutostartCommand {
                    phase: AutostartPhase::Once,
                    command: "waybar".to_string(),
                },
                AutostartCommand {
                    phase: AutostartPhase::OnReload,
                    command: "makoctl reload".to_string(),
                },
            ]
        );
    }

    #[test]
    fn ignores_hash_inside_autostart_quotes() {
        let raw = r#"
autostart:
  once "echo # still part of command"
end
"#;

        let commands = parse_autostart_commands(raw);

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command, "echo # still part of command");
    }
}
