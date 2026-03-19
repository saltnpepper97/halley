use std::collections::HashMap;

use super::{
    CompositorBinding, CompositorBindingAction, DirectionalAction, KeyModifiers, LaunchBinding,
    PointerBinding, PointerBindingAction,
};
use crate::RuntimeTuning;
use crate::keybinds::{is_pointer_button_code, parse_chord, parse_modifiers};
use crate::layout::{ViewportOutputConfig, default_compositor_bindings, default_pointer_bindings};

use rune_cfg::RuneConfig;

impl RuntimeTuning {
    pub fn from_rune_file(path: &str) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let inline_keybinds = parse_inline_keybinds(raw.as_str());

        let cfg = RuneConfig::from_file(path).or_else(|_| {
            let sanitized = strip_inline_keybind_block(raw.as_str());
            RuneConfig::from_str(sanitized.as_str())
        });
        let cfg = cfg.ok()?;

        let mut out = Self::default();

        load_autostart_section(raw.as_str(), &mut out);
        load_dev_section(&cfg, &mut out);
        load_env_section(&cfg, &mut out);
        load_viewport_section(&cfg, &mut out);
        load_focus_ring_section(&cfg, &mut out);
        load_nodes_section(&cfg, &mut out);
        load_clusters_section(&cfg, &mut out);
        load_decay_section(&cfg, &mut out);
        load_field_section(&cfg, &mut out);
        load_physics_section(&cfg, &mut out);
        load_keybind_sections(&cfg, &mut out);

        if !inline_keybinds.is_empty() {
            apply_explicit_keybind_overrides_map(&inline_keybinds, &mut out);
        }

        Some(out)
    }
}

fn load_autostart_section(raw: &str, out: &mut RuntimeTuning) {
    let mut in_autostart = false;
    out.autostart_once.clear();
    out.autostart_on_reload.clear();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if !in_autostart {
            if trimmed == "autostart:" {
                in_autostart = true;
            }
            continue;
        }

        if trimmed == "end" {
            break;
        }

        if let Some(command) = parse_autostart_command(trimmed, "once") {
            out.autostart_once.push(command);
            continue;
        }

        if let Some(command) = parse_autostart_command(trimmed, "on-reload") {
            out.autostart_on_reload.push(command);
        }
    }
}

fn parse_autostart_command(line: &str, directive: &str) -> Option<String> {
    let rest = line.strip_prefix(directive)?.trim();
    if !rest.starts_with('"') {
        return None;
    }
    let rest = &rest[1..];
    let mut escaped = false;
    let mut command = String::new();
    for ch in rest.chars() {
        if escaped {
            command.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(command.trim().to_string()).filter(|value| !value.is_empty()),
            _ => command.push(ch),
        }
    }
    None
}

fn parse_inline_keybinds(content: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut in_block = false;
    let mut depth = 0usize;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_block {
            if trimmed.eq_ignore_ascii_case("keybinds:") {
                in_block = true;
                depth = 1;
            }
            continue;
        }

        if trimmed.eq_ignore_ascii_case("end") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                in_block = false;
            }
            continue;
        }

        if trimmed.ends_with(':') {
            depth = depth.saturating_add(1);
            continue;
        }

        if depth != 1 {
            continue;
        }

        if let Some((k, v)) = parse_inline_keybind_line(trimmed) {
            out.insert(k, v);
        }
    }

    out
}

fn parse_inline_keybind_line(line: &str) -> Option<(String, String)> {
    let mut clean = String::with_capacity(line.len());
    let mut in_quotes = false;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            clean.push(ch);
            continue;
        }
        if ch == '#' && !in_quotes {
            break;
        }
        clean.push(ch);
    }
    let tokens = parse_quoted_tokens(clean.trim());
    if tokens.len() != 2 {
        return None;
    }
    Some((tokens[0].clone(), tokens[1].clone()))
}

fn parse_quoted_tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if in_quotes && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes && ch.is_ascii_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_inline_keybind_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    let mut depth = 0usize;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if !in_block {
            if trimmed.eq_ignore_ascii_case("keybinds:") {
                in_block = true;
                depth = 1;
                continue;
            }
            out.push_str(raw);
            out.push('\n');
            continue;
        }

        if trimmed.eq_ignore_ascii_case("end") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                in_block = false;
            }
            continue;
        }
        if trimmed.ends_with(':') {
            depth = depth.saturating_add(1);
        }
    }

    out
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

fn load_field_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px = pick_f32(cfg, &["field.gap", "field.gap-px"], out.non_overlap_gap_px);
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
    out.compositor_bindings = default_compositor_bindings(out.keybinds.modifier);
    out.launch_bindings.clear();
    out.pointer_bindings = default_pointer_bindings(out.keybinds.modifier);
    out.scroll_zoom_enabled = pick_bool(
        cfg,
        &["keybinds.scroll-zoom", "keybinds.scroll_zoom"],
        out.scroll_zoom_enabled,
    );
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
        out.compositor_bindings = default_compositor_bindings(out.keybinds.modifier);
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

    let action_trimmed = action.trim();
    let action_key = action_trimmed.to_ascii_lowercase();

    match action_key.as_str() {
        "reload" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::Reload);
        }
        "toggle_state" | "toggle-state" | "minimize_focused" | "minimize-focused" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::ToggleState);
        }
        "quit" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Quit {
                    requires_shift: mods.shift,
                },
            );
        }
        "docking" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::Docking);
        }
        "move_left" | "move-left" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::MoveNode(DirectionalAction::Left),
            );
        }
        "move_right" | "move-right" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::MoveNode(DirectionalAction::Right),
            );
        }
        "move_up" | "move-up" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::MoveNode(DirectionalAction::Up),
            );
        }
        "move_down" | "move-down" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::MoveNode(DirectionalAction::Down),
            );
        }
        "zoom_in" | "zoom-in" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::ZoomIn);
        }
        "zoom_out" | "zoom-out" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::ZoomOut);
        }
        "zoom_reset" | "zoom-reset" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::ZoomReset);
        }
        "move_window" | "move-window" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::MoveWindow);
        }
        "resize_window" | "resize-window" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::ResizeWindow);
        }
        _ => upsert_launch_binding(out, mods, key, action_trimmed),
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
    use std::collections::HashMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::apply_explicit_keybind_overrides_map;
    use crate::{
        CompositorBindingAction, DirectionalAction, RuntimeTuning, keybinds::key_name_to_evdev,
    };

    #[test]
    fn explicit_binding_without_modifiers_stays_modless() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("mod".to_string(), "lsuper".to_string()),
            (
                "XF86AudioRaiseVolume".to_string(),
                "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 5%+".to_string(),
            ),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        let binding = tuning
            .launch_bindings
            .iter()
            .find(|binding| binding.command.contains("set-volume"))
            .expect("launch binding");
        assert_eq!(
            binding.key,
            key_name_to_evdev("XF86AudioRaiseVolume").expect("media key")
        );
        assert!(!binding.modifiers.left_super);
        assert!(!binding.modifiers.super_key);
    }

    #[test]
    fn explicit_docking_and_move_actions_become_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("$mod+x".to_string(), "docking".to_string()),
            ("shift+h".to_string(), "move-left".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::Docking)
        );
        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::MoveNode(DirectionalAction::Left)
        }));
    }

    #[test]
    fn toggle_state_and_zoom_aliases_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("$mod+n".to_string(), "toggle-state".to_string()),
            ("$mod+equal".to_string(), "zoom_in".to_string()),
            ("$mod+minus".to_string(), "zoom-out".to_string()),
            ("$mod+0".to_string(), "zoom-reset".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ToggleState)
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomIn)
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomOut)
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomReset)
        );
    }

    #[test]
    fn runtime_tuning_has_default_zoom_bindings() {
        let tuning = RuntimeTuning::default();

        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomIn)
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomOut)
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomReset)
        );
    }

    #[test]
    fn changing_mod_reseeds_default_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([("mod".to_string(), "super".to_string())]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        let zoom_in = tuning
            .compositor_bindings
            .iter()
            .find(|binding| binding.action == CompositorBindingAction::ZoomIn)
            .expect("zoom-in binding");
        assert!(zoom_in.modifiers.super_key);
        assert!(!zoom_in.modifiers.left_alt);
    }

    #[test]
    fn autostart_section_loads_once_and_on_reload_commands() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-autostart-{unique}.rune"));
        fs::write(
            &path,
            r#"
autostart:
  once "waybar"
  once "mako"
  on-reload "thunderbird"
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.autostart_once, vec!["waybar", "mako"]);
        assert_eq!(tuning.autostart_on_reload, vec!["thunderbird"]);
    }

    #[test]
    fn field_gap_loads_from_field_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-field-gap-{unique}.rune"));
        fs::write(
            &path,
            r#"
field:
  gap 24.0
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.non_overlap_gap_px, 24.0);
    }

    #[test]
    fn inline_keybind_block_does_not_break_full_config_loading() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-inline-keybinds-{unique}.rune"));
        fs::write(
            &path,
            r#"
autostart:
  once "waybar"
end

field:
  gap 24.0
end

keybinds:
  mod "super"
  "$var.mod+return" "kitty"
  "$var.mod+shift+q" "quit"
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.autostart_once, vec!["waybar"]);
        assert_eq!(tuning.non_overlap_gap_px, 24.0);
        assert_eq!(tuning.launch_bindings.len(), 1);
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| matches!(binding.action, CompositorBindingAction::Quit { .. }))
        );
        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| binding.action == CompositorBindingAction::ZoomIn)
        );
    }

    #[test]
    fn scroll_zoom_can_be_disabled_in_config() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-scroll-zoom-{unique}.rune"));
        fs::write(
            &path,
            r#"
keybinds:
  mod "super"
  scroll-zoom false
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert!(!tuning.scroll_zoom_enabled);
    }
}
