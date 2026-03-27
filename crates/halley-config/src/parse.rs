use std::collections::HashMap;

use super::{
    BearingsBindingAction, CompositorBinding, CompositorBindingAction, DirectionalAction,
    KeyModifiers, LaunchBinding, MonitorBindingAction, MonitorBindingTarget, NodeBindingAction,
    PointerBinding, PointerBindingAction, TrailBindingAction,
};
use crate::keybinds::{is_pointer_button_code, parse_chord, parse_modifiers};
use crate::layout::FocusRingConfig;
use crate::layout::{
    ClusterBloomDirection,
    ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode, CloseRestorePanMode, PanToNewMode,
    ViewportOutputConfig, ViewportVrrMode, default_compositor_bindings, default_pointer_bindings,
};
use crate::{NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy, RuntimeTuning};

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
        load_bearings_section(&cfg, &mut out);
        load_trail_section(&cfg, &mut out);
        load_nodes_section(&cfg, &mut out);
        load_clusters_section(&cfg, &mut out);
        load_tile_section(&cfg, &mut out);
        load_decay_section(&cfg, &mut out);
        load_field_section(&cfg, &mut out);
        load_physics_section(&cfg, &mut out);
        load_decorations_section(&cfg, &mut out);
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

    if let Some(primary) = out.tty_viewports.iter().find(|viewport| viewport.enabled) {
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

fn load_bearings_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.bearings.show_distance = pick_bool(
        cfg,
        &["bearings.show-distance", "bearings.show_distance"],
        out.bearings.show_distance,
    );
    out.bearings.show_icons = pick_bool(
        cfg,
        &["bearings.show-icons", "bearings.show_icons"],
        out.bearings.show_icons,
    );
    out.bearings.fade_distance = pick_f32(
        cfg,
        &["bearings.fade-distance", "bearings.fade_distance"],
        out.bearings.fade_distance,
    );
}

fn load_nodes_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.primary_to_node_ms = pick_u64(
        cfg,
        &[
            "node.primary-to-node-ms",
            "node.primary_to_node_ms",
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
            "node.primary-to-preview-ms",
            "node.primary_to_preview_ms",
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
            "node.primary-preview-to-node-ms",
            "node.primary_preview_to_node_ms",
            "node.preview-to-node-ms",
            "node.preview_to_node_ms",
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
            "node.primary-hot-inner-frac",
            "node.primary_hot_inner_frac",
            "node.hot-inner-frac",
            "node.hot_inner_frac",
            "nodes.primary-hot-inner-frac",
            "nodes.primary_hot_inner_frac",
            "nodes.hot-inner-frac",
            "nodes.hot_inner_frac",
        ],
        out.primary_hot_inner_frac,
    );

    out.node_show_labels = pick_node_display_policy(
        cfg,
        &[
            "node.show-labels",
            "node.show_labels",
            "nodes.show-labels",
            "nodes.show_labels",
        ],
        out.node_show_labels,
    );
    out.node_show_app_icons = pick_node_display_policy(
        cfg,
        &[
            "node.show-app-icons",
            "node.show_app_icons",
            "node.show-icons",
            "node.show_icons",
            "nodes.show-app-icons",
            "nodes.show_app_icons",
            "nodes.show-icons",
            "nodes.show_icons",
        ],
        out.node_show_app_icons,
    );
    out.node_icon_size = pick_f32(
        cfg,
        &[
            "node.icon-size",
            "node.icon_size",
            "nodes.icon-size",
            "nodes.icon_size",
        ],
        out.node_icon_size,
    );
    out.node_background_color = pick_node_background_color_mode(
        cfg,
        &[
            "node.background-colour",
            "node.background_colour",
            "node.background-color",
            "node.background_color",
            "nodes.background-colour",
            "nodes.background_colour",
            "nodes.background-color",
            "nodes.background_color",
        ],
        out.node_background_color,
    );

    out.node_border_color_hover = pick_node_border_color_mode(
        cfg,
        &[
            "node.border-colour-hover",
            "node.border_colour_hover",
            "node.border-color-hover",
            "node.border_color_hover",
            "nodes.border-colour-hover",
            "nodes.border_colour_hover",
            "nodes.border-color-hover",
            "nodes.border_color_hover",
        ],
        out.node_border_color_hover,
    );
    out.node_border_color_inactive = pick_node_border_color_mode(
        cfg,
        &[
            "node.border-colour-inactive",
            "node.border_colour_inactive",
            "node.border-color-inactive",
            "node.border_color_inactive",
            "nodes.border-colour-inactive",
            "nodes.border_colour_inactive",
            "nodes.border-color-inactive",
            "nodes.border_color_inactive",
        ],
        out.node_border_color_inactive,
    );
    out.click_collapsed_outside_focus = pick_click_collapsed_outside_focus_mode(
        cfg,
        &[
            "node.click-collapsed-outside-focus",
            "node.click_collapsed_outside_focus",
            "nodes.click-collapsed-outside-focus",
            "nodes.click_collapsed_outside_focus",
        ],
        out.click_collapsed_outside_focus,
    );
    out.click_collapsed_pan = pick_click_collapsed_pan_mode(
        cfg,
        &[
            "node.click-collapsed-pan",
            "node.click_collapsed_pan",
            "nodes.click-collapsed-pan",
            "nodes.click_collapsed_pan",
        ],
        out.click_collapsed_pan,
    );
}

fn load_trail_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.trail_history_length = pick_u64(
        cfg,
        &["trail.history-length", "trail.history_length"],
        out.trail_history_length as u64,
    ) as usize;
    out.trail_wrap = pick_bool(cfg, &["trail.wrap", "trail.wrap-history"], out.trail_wrap);
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
    out.cluster_show_icons = pick_bool(
        cfg,
        &["clusters.show-icons", "clusters.show_icons"],
        out.cluster_show_icons,
    );
    out.cluster_bloom_direction = pick_cluster_bloom_direction(
        cfg,
        &["clusters.bloom-direction", "clusters.bloom_direction"],
        out.cluster_bloom_direction,
    );
}

fn load_tile_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.tile_gaps_inner_px = pick_f32(
        cfg,
        &["tile.gaps-inner", "tile.gaps_inner", "tile.gap-inner", "tile.gap_inner"],
        out.tile_gaps_inner_px,
    );
    out.tile_gaps_outer_px = pick_f32(
        cfg,
        &["tile.gaps-outer", "tile.gaps_outer", "tile.gap-outer", "tile.gap_outer"],
        out.tile_gaps_outer_px,
    );
}

fn load_decay_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let active_s = pick_u64(
        cfg,
        &["decay.active-delay", "decay.active_delay"],
        out.active_outside_ring_delay_ms / 1000,
    );
    let inactive_s = pick_u64(
        cfg,
        &["decay.inactive-delay", "decay.inactive_delay"],
        out.inactive_outside_ring_delay_ms / 1000,
    );
    let docked_s = pick_u64(
        cfg,
        &[
            "decay.docked-offscreen-delay",
            "decay.docked_offscreen_delay",
        ],
        out.docked_offscreen_delay_ms / 1000,
    );

    out.active_outside_ring_delay_ms = active_s.saturating_mul(1000);
    out.inactive_outside_ring_delay_ms = inactive_s.saturating_mul(1000);
    out.docked_offscreen_delay_ms = docked_s.saturating_mul(1000);
}

fn load_field_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px = pick_f32(cfg, &["field.gap", "field.gap-px"], out.non_overlap_gap_px);
    out.pan_to_new = pick_pan_to_new_mode(
        cfg,
        &["field.pan-to-new", "field.pan_to_new"],
        out.pan_to_new,
    );
    out.close_restore_focus = pick_bool(
        cfg,
        &["field.close-restore-focus", "field.close_restore_focus"],
        out.close_restore_focus,
    );
    out.close_restore_pan = pick_close_restore_pan_mode(
        cfg,
        &["field.close-restore-pan", "field.close_restore_pan"],
        out.close_restore_pan,
    );
    out.active_windows_allowed = pick_u64(
        cfg,
        &[
            "field.active-windows-allowed",
            "field.active_windows_allowed",
        ],
        out.active_windows_allowed as u64,
    ) as usize;
}

fn load_physics_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.physics_enabled = pick_bool(cfg, &["physics.enabled"], out.physics_enabled);

    out.non_overlap_bump_damping =
        pick_f32(cfg, &["physics.damping"], out.non_overlap_bump_damping);
}

fn load_decorations_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(no_csd) = pick_optional_bool(
        cfg,
        &[
            "decoration.no-csd",
            "decoration.no_csd",
            "decorations.no-csd",
            "decorations.no_csd",
        ],
    ) {
        out.no_csd = no_csd;
    }
}

fn load_keybind_sections(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.keybinds.modifier = pick_modifiers(cfg, &["keybinds.mod"], out.keybinds.modifier);
    out.compositor_bindings = default_compositor_bindings(out.keybinds.modifier);
    out.launch_bindings.clear();
    out.pointer_bindings = default_pointer_bindings(out.keybinds.modifier);
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
        let enabled = pick_bool(
            cfg,
            &[
                format!("{root}.{key}.enabled").as_str(),
                format!("{root}.{key}.active").as_str(),
            ],
            true,
        );

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

        let transform_degrees = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.transform").as_str(),
                format!("{root}.{key}.rotation").as_str(),
            ],
            0,
        );
        let transform_degrees = match transform_degrees {
            0 | 90 | 180 | 270 => transform_degrees as u16,
            1 => 90,
            2 => 180,
            3 => 270,
            _ => 0,
        };

        let vrr = pick_viewport_vrr_mode(
            cfg,
            &[
                format!("{root}.{key}.vrr").as_str(),
                format!("{root}.{key}.variable-refresh-rate").as_str(),
                format!("{root}.{key}.variable_refresh_rate").as_str(),
            ],
            ViewportVrrMode::Off,
        );
        let focus_ring = parse_viewport_focus_ring(cfg, root, &key);

        out.push(ViewportOutputConfig {
            connector: key,
            enabled,
            offset_x,
            offset_y,
            width,
            height,
            refresh_rate,
            transform_degrees,
            vrr,
            focus_ring,
        });
    }

    out
}

fn pick_viewport_vrr_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ViewportVrrMode,
) -> ViewportVrrMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "off" | "false" => ViewportVrrMode::Off,
        "on" | "true" => ViewportVrrMode::On,
        "on-demand" | "ondemand" | "adaptive" => ViewportVrrMode::OnDemand,
        _ => default,
    }
}

fn pick_pan_to_new_mode(cfg: &RuneConfig, paths: &[&str], default: PanToNewMode) -> PanToNewMode {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return if v {
                PanToNewMode::Always
            } else {
                PanToNewMode::Never
            };
        }
    }

    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "never" => PanToNewMode::Never,
        "if-needed" | "if_needed" => PanToNewMode::IfNeeded,
        "always" => PanToNewMode::Always,
        _ => default,
    }
}

fn pick_close_restore_pan_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: CloseRestorePanMode,
) -> CloseRestorePanMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "never" => CloseRestorePanMode::Never,
        "if-offscreen" | "if_offscreen" => CloseRestorePanMode::IfOffscreen,
        "always" => CloseRestorePanMode::Always,
        _ => default,
    }
}

fn pick_click_collapsed_outside_focus_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ClickCollapsedOutsideFocusMode,
) -> ClickCollapsedOutsideFocusMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "ignore" => ClickCollapsedOutsideFocusMode::Ignore,
        "activate" => ClickCollapsedOutsideFocusMode::Activate,
        _ => default,
    }
}

fn pick_click_collapsed_pan_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ClickCollapsedPanMode,
) -> ClickCollapsedPanMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "never" => ClickCollapsedPanMode::Never,
        "if-offscreen" | "if_offscreen" => ClickCollapsedPanMode::IfOffscreen,
        "always" => ClickCollapsedPanMode::Always,
        _ => default,
    }
}

fn pick_cluster_bloom_direction(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ClusterBloomDirection,
) -> ClusterBloomDirection {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "clockwise" | "cw" => ClusterBloomDirection::Clockwise,
        "counterclockwise" | "counter-clockwise" | "counter_clockwise" | "ccw" => {
            ClusterBloomDirection::CounterClockwise
        }
        _ => default,
    }
}

fn parse_viewport_focus_ring(cfg: &RuneConfig, root: &str, key: &str) -> Option<FocusRingConfig> {
    let ring_root = format!("{root}.{key}.focus-ring");
    let rx = pick_f32(
        cfg,
        &[
            format!("{ring_root}.rx").as_str(),
            format!("{ring_root}.radius-x").as_str(),
            format!("{ring_root}.radius_x").as_str(),
            format!("{ring_root}.primary-rx").as_str(),
            format!("{ring_root}.primary_rx").as_str(),
        ],
        0.0,
    );
    let ry = pick_f32(
        cfg,
        &[
            format!("{ring_root}.ry").as_str(),
            format!("{ring_root}.radius-y").as_str(),
            format!("{ring_root}.radius_y").as_str(),
            format!("{ring_root}.primary-ry").as_str(),
            format!("{ring_root}.primary_ry").as_str(),
        ],
        0.0,
    );
    let offset_x = pick_f32(
        cfg,
        &[
            format!("{ring_root}.offset-x").as_str(),
            format!("{ring_root}.offset_x").as_str(),
        ],
        0.0,
    );
    let offset_y = pick_f32(
        cfg,
        &[
            format!("{ring_root}.offset-y").as_str(),
            format!("{ring_root}.offset_y").as_str(),
        ],
        0.0,
    );

    ((rx > 0.0) || (ry > 0.0) || offset_x != 0.0 || offset_y != 0.0).then_some(FocusRingConfig {
        rx: if rx > 0.0 { rx } else { 820.0 },
        ry: if ry > 0.0 { ry } else { 420.0 },
        offset_x,
        offset_y,
    })
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

fn pick_optional_bool(cfg: &RuneConfig, paths: &[&str]) -> Option<bool> {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return Some(v);
        }
    }
    None
}

fn pick_string(cfg: &RuneConfig, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            return Some(v);
        }
    }
    None
}

fn pick_node_border_color_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: NodeBorderColorMode,
) -> NodeBorderColorMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"') {
        "use-window-active" => NodeBorderColorMode::UseWindowActive,
        "use-window-inactive" => NodeBorderColorMode::UseWindowInactive,
        _ => default,
    }
}

fn pick_node_display_policy(
    cfg: &RuneConfig,
    paths: &[&str],
    default: NodeDisplayPolicy,
) -> NodeDisplayPolicy {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return if v {
                NodeDisplayPolicy::Always
            } else {
                NodeDisplayPolicy::Off
            };
        }
    }

    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "off" | "false" => NodeDisplayPolicy::Off,
        "hover" => NodeDisplayPolicy::Hover,
        "always" | "on" | "true" => NodeDisplayPolicy::Always,
        _ => default,
    }
}

fn pick_node_background_color_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: NodeBackgroundColorMode,
) -> NodeBackgroundColorMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    let value = raw.trim().trim_matches('"');
    if value.is_empty() {
        return default;
    }

    match value.to_ascii_lowercase().as_str() {
        "auto" => NodeBackgroundColorMode::Auto,
        "theme" => NodeBackgroundColorMode::Theme,
        _ => parse_hex_rgb(value)
            .map(|(r, g, b)| NodeBackgroundColorMode::Fixed { r, g, b })
            .unwrap_or(default),
    }
}

fn parse_hex_rgb(value: &str) -> Option<(f32, f32, f32)> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    let expanded = match hex.len() {
        3 => {
            let mut out = String::with_capacity(6);
            for ch in hex.chars() {
                out.push(ch);
                out.push(ch);
            }
            out
        }
        6 => hex.to_string(),
        _ => return None,
    };

    let r = u8::from_str_radix(&expanded[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&expanded[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&expanded[4..6], 16).ok()? as f32 / 255.0;
    Some((r, g, b))
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
        "close_focused" | "close-focused" | "close_window" | "close-window" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::CloseFocusedWindow);
        }
        "cluster_mode" | "cluster-mode" => {
            upsert_compositor_binding(out, mods, key, CompositorBindingAction::ClusterMode);
        }
        "bearings_show" | "bearings-show" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Bearings(BearingsBindingAction::Show),
            );
        }
        "bearings_toggle" | "bearings-toggle" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Bearings(BearingsBindingAction::Toggle),
            );
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
        "move_left" | "move-left" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left)),
            );
        }
        "move_right" | "move-right" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Right)),
            );
        }
        "move_up" | "move-up" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Up)),
            );
        }
        "move_down" | "move-down" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Down)),
            );
        }
        "trail_prev" | "trail-prev" | "trail prev" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Trail(TrailBindingAction::Prev),
            );
        }
        "trail_next" | "trail-next" | "trail next" => {
            upsert_compositor_binding(
                out,
                mods,
                key,
                CompositorBindingAction::Trail(TrailBindingAction::Next),
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
        "field_jump" | "field-jump" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::FieldJump);
        }
        "resize_window" | "resize-window" if is_pointer_button_code(key) => {
            upsert_pointer_binding(out, mods, key, PointerBindingAction::ResizeWindow);
        }
        _ => {
            if let Some(action) = parse_parameterized_compositor_action(action_trimmed) {
                upsert_compositor_binding(out, mods, key, action);
            } else {
                upsert_launch_binding(out, mods, key, action_trimmed);
            }
        }
    }
}

fn parse_parameterized_compositor_action(action: &str) -> Option<CompositorBindingAction> {
    let mut parts = action.split_whitespace();
    let command = parts.next()?.to_ascii_lowercase();
    let arg = parts.collect::<Vec<_>>().join(" ");
    let arg = arg.trim();
    if arg.is_empty() {
        return None;
    }

    match command.as_str() {
        "node-move" | "node_move" => parse_directional_action(arg)
            .map(|direction| CompositorBindingAction::Node(NodeBindingAction::Move(direction))),
        "monitor-focus" | "monitor_focus" => Some(CompositorBindingAction::Monitor(
            MonitorBindingAction::Focus(
                parse_directional_action(arg)
                    .map(MonitorBindingTarget::Direction)
                    .unwrap_or_else(|| MonitorBindingTarget::Output(arg.to_string())),
            ),
        )),
        _ => None,
    }
}

fn parse_directional_action(text: &str) -> Option<DirectionalAction> {
    match text.trim().to_ascii_lowercase().as_str() {
        "left" => Some(DirectionalAction::Left),
        "right" => Some(DirectionalAction::Right),
        "up" => Some(DirectionalAction::Up),
        "down" => Some(DirectionalAction::Down),
        _ => None,
    }
}

fn upsert_compositor_binding(
    out: &mut RuntimeTuning,
    mods: KeyModifiers,
    key: u32,
    action: CompositorBindingAction,
) {
    // For actions that should only have one binding, remove any existing ones first.
    match &action {
        CompositorBindingAction::Quit { .. } => {
            out.compositor_bindings
                .retain(|b| !matches!(b.action, CompositorBindingAction::Quit { .. }));
        }
        _ => {}
    }
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
        BearingsBindingAction, ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode,
        ClusterBloomDirection,
        CloseRestorePanMode, CompositorBindingAction, DirectionalAction, MonitorBindingAction,
        MonitorBindingTarget, NodeBackgroundColorMode, NodeBindingAction, NodeDisplayPolicy,
        PanToNewMode, RuntimeTuning, WHEEL_DOWN_CODE, WHEEL_UP_CODE,
        keybinds::key_name_to_evdev,
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
    fn explicit_move_actions_become_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([("shift+h".to_string(), "move-left".to_string())]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action
                == CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left))
        }));
    }

    #[test]
    fn family_style_node_move_actions_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([("$mod+h".to_string(), "node-move left".to_string())]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action
                == CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left))
        }));
    }

    #[test]
    fn monitor_focus_family_actions_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings =
            HashMap::from([("$mod+o".to_string(), "monitor-focus HDMI-A-1".to_string())]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action
                == CompositorBindingAction::Monitor(MonitorBindingAction::Focus(
                    MonitorBindingTarget::Output("HDMI-A-1".to_string()),
                ))
        }));
    }

    #[test]
    fn bearings_family_actions_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("$mod+z".to_string(), "bearings-show".to_string()),
            ("$mod+shift+z".to_string(), "bearings-toggle".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::Bearings(BearingsBindingAction::Show)
        }));
        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::Bearings(BearingsBindingAction::Toggle)
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
    fn close_focused_aliases_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("$mod+q".to_string(), "close-focused".to_string()),
            ("$mod+w".to_string(), "close_window".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(
            tuning
                .compositor_bindings
                .iter()
                .any(|binding| { binding.action == CompositorBindingAction::CloseFocusedWindow })
        );
    }

    #[test]
    fn runtime_tuning_has_default_zoom_bindings() {
        let tuning = RuntimeTuning::default();

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::ZoomIn && binding.key == WHEEL_UP_CODE
        }));
        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::ZoomOut && binding.key == WHEEL_DOWN_CODE
        }));
        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.action == CompositorBindingAction::ZoomReset
                && binding.key == key_name_to_evdev("mousemiddle").expect("middle mouse")
        }));
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
        assert_eq!(zoom_in.key, WHEEL_UP_CODE);
    }

    #[test]
    fn wheel_zoom_aliases_parse_as_compositor_bindings() {
        let mut tuning = RuntimeTuning::default();
        let bindings = HashMap::from([
            ("$mod+mousewheelup".to_string(), "zoom_in".to_string()),
            ("$mod+mousewheeldown".to_string(), "zoom-out".to_string()),
        ]);

        apply_explicit_keybind_overrides_map(&bindings, &mut tuning);

        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.key == WHEEL_UP_CODE && binding.action == CompositorBindingAction::ZoomIn
        }));
        assert!(tuning.compositor_bindings.iter().any(|binding| {
            binding.key == WHEEL_DOWN_CODE && binding.action == CompositorBindingAction::ZoomOut
        }));
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
    fn field_pan_to_new_loads_from_field_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-field-pan-to-new-{unique}.rune"));
        fs::write(
            &path,
            r#"
field:
  pan-to-new "if-needed"
  close-restore-focus true
  close-restore-pan "if-offscreen"
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.pan_to_new, PanToNewMode::IfNeeded);
        assert!(tuning.close_restore_focus);
        assert_eq!(tuning.close_restore_pan, CloseRestorePanMode::IfOffscreen);
    }

    #[test]
    fn viewport_output_enabled_and_transform_parse() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-viewport-output-{unique}.rune"));
        fs::write(
            &path,
            r#"
viewport:
  DP-1:
    enabled false
    offset-x 0
    offset-y 0
    width 2560
    height 1440
    transform 180
  end
  DP-2:
    enabled true
    offset-x 2560
    offset-y 0
    width 1920
    height 1200
    transform 90
  end
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.tty_viewports.len(), 2);
        assert!(!tuning.tty_viewports[0].enabled);
        assert_eq!(tuning.tty_viewports[0].transform_degrees, 180);
        assert!(tuning.tty_viewports[1].enabled);
        assert_eq!(tuning.tty_viewports[1].transform_degrees, 90);
        assert_eq!(tuning.viewport_size.x, 1920.0);
        assert_eq!(tuning.viewport_size.y, 1200.0);
        assert_eq!(tuning.viewport_center.x, 3520.0);
        assert_eq!(tuning.viewport_center.y, 600.0);
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
    fn field_active_windows_allowed_parses_from_config() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-field-active-windows-{unique}.rune"));
        fs::write(
            &path,
            r#"
field:
  gap 24.0
  active-windows-allowed 5
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.non_overlap_gap_px, 24.0);
        assert_eq!(tuning.active_windows_allowed, 5);
    }

    #[test]
    fn physics_damping_loads_only_from_primary_key() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-physics-damping-{unique}.rune"));
        fs::write(
            &path,
            r#"
physics:
  damping 0.62
  bump-damping 0.91
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.non_overlap_bump_damping, 0.62);
    }

    #[test]
    fn node_display_policies_parse_from_strings() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-node-display-{unique}.rune"));
        fs::write(
            &path,
            r##"
node:
  show-labels "hover"
  show-app-icons "always"
  icon-size 0.72
  background-colour "#8fa4c7"
end
"##,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.node_show_labels, NodeDisplayPolicy::Hover);
        assert_eq!(tuning.node_show_app_icons, NodeDisplayPolicy::Always);
        assert_eq!(tuning.node_icon_size, 0.72);
        assert_eq!(
            tuning.click_collapsed_outside_focus,
            ClickCollapsedOutsideFocusMode::Activate
        );
        assert_eq!(
            tuning.click_collapsed_pan,
            ClickCollapsedPanMode::IfOffscreen
        );
        assert_eq!(
            tuning.node_background_color,
            NodeBackgroundColorMode::Fixed {
                r: 0x8f as f32 / 255.0,
                g: 0xa4 as f32 / 255.0,
                b: 0xc7 as f32 / 255.0,
            }
        );
    }

    #[test]
    fn legacy_boolean_node_label_setting_maps_to_always() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-node-label-bool-{unique}.rune"));
        fs::write(
            &path,
            r#"
node:
  show-labels true
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.node_show_labels, NodeDisplayPolicy::Always);
    }

    #[test]
    fn node_click_collapsed_settings_parse_from_strings() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-node-click-collapsed-{unique}.rune"));
        fs::write(
            &path,
            r#"
node:
  click-collapsed-outside-focus "ignore"
  click-collapsed-pan "always"
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(
            tuning.click_collapsed_outside_focus,
            ClickCollapsedOutsideFocusMode::Ignore
        );
        assert_eq!(tuning.click_collapsed_pan, ClickCollapsedPanMode::Always);
    }

    #[test]
    fn tile_gap_settings_parse_from_tile_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-tile-gaps-{unique}.rune"));
        fs::write(
            &path,
            r#"
tile:
  gaps-inner 18
  gaps-outer 26
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.tile_gaps_inner_px, 18.0);
        assert_eq!(tuning.tile_gaps_outer_px, 26.0);
    }

    #[test]
    fn cluster_bloom_settings_parse_from_cluster_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-cluster-bloom-{unique}.rune"));
        fs::write(
            &path,
            r#"
clusters:
  bloom-direction "counterclockwise"
  show-icons false
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(
            tuning.cluster_bloom_direction,
            ClusterBloomDirection::CounterClockwise
        );
        assert!(!tuning.cluster_show_icons);
    }

    #[test]
    fn decorations_no_csd_parses_from_plural_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-decorations-no-csd-{unique}.rune"));
        fs::write(
            &path,
            r#"
decorations:
  no-csd true
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert!(tuning.no_csd);
    }
}
