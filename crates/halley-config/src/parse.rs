use std::collections::HashMap;

use super::{
    BearingsBindingAction, CompositorBinding, CompositorBindingAction, DirectionalAction,
    KeyModifiers, LaunchBinding, MonitorBindingAction, MonitorBindingTarget, NodeBindingAction,
    PointerBinding, PointerBindingAction, TrailBindingAction,
};
use crate::keybinds::{is_pointer_button_code, parse_chord, parse_modifiers};
use crate::layout::FocusRingConfig;
use crate::layout::{
    ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode, CloseRestorePanMode,
    ClusterBloomDirection, InitialWindowClusterParticipation, InitialWindowOverlapPolicy,
    InitialWindowSpawnPlacement, PanToNewMode, ViewportOutputConfig, ViewportVrrMode, WindowRule,
    WindowRulePattern, default_compositor_bindings, default_pointer_bindings,
};
use crate::{
    DecorationBorderColor, NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy,
    RuntimeTuning,
};

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
        if let Err(err) = load_rules_section(raw.as_str(), &mut out) {
            eprintln!("halley config rules parse error: {err}");
            return None;
        }
        load_dev_section(&cfg, &mut out);
        load_env_section(&cfg, &mut out);
        load_cursor_section(&cfg, &mut out);
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

#[derive(Default)]
struct PartialWindowRule {
    app_ids: Vec<WindowRulePattern>,
    titles: Vec<WindowRulePattern>,
    overlap_policy: Option<InitialWindowOverlapPolicy>,
    spawn_placement: Option<InitialWindowSpawnPlacement>,
    cluster_participation: Option<InitialWindowClusterParticipation>,
}

fn load_rules_section(raw: &str, out: &mut RuntimeTuning) -> Result<(), String> {
    out.window_rules.clear();
    let mut in_rules = false;
    let mut current_rule: Option<PartialWindowRule> = None;

    for (line_no, raw_line) in raw.lines().enumerate() {
        let line_no = line_no + 1;
        let trimmed = strip_rule_comment(raw_line);
        if trimmed.is_empty() {
            continue;
        }

        if !in_rules {
            if trimmed == "rules:" {
                in_rules = true;
            }
            continue;
        }

        if let Some(rule) = current_rule.as_mut() {
            if trimmed == "end" {
                out.window_rules.push(finalize_window_rule(rule, line_no)?);
                current_rule = None;
                continue;
            }
            parse_rule_entry(rule, trimmed, line_no)?;
            continue;
        }

        if trimmed == "rule:" {
            current_rule = Some(PartialWindowRule::default());
            continue;
        }
        if trimmed == "end" {
            return Ok(());
        }
        return Err(format!(
            "line {line_no}: expected `rule:` or `end` inside `rules:` block, got `{trimmed}`"
        ));
    }

    if current_rule.is_some() {
        return Err("unterminated `rule:` block in `rules:` section".to_string());
    }

    Ok(())
}

fn strip_rule_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '#' && !in_quotes {
            return line[..idx].trim();
        }
    }
    line.trim()
}

fn finalize_window_rule(rule: &PartialWindowRule, line_no: usize) -> Result<WindowRule, String> {
    if rule.app_ids.is_empty() && rule.titles.is_empty() {
        return Err(format!(
            "line {line_no}: rule is missing required matcher; add `app-id` and/or `title`"
        ));
    }
    Ok(WindowRule {
        app_ids: rule.app_ids.clone(),
        titles: rule.titles.clone(),
        overlap_policy: rule
            .overlap_policy
            .unwrap_or(InitialWindowOverlapPolicy::None),
        spawn_placement: rule
            .spawn_placement
            .unwrap_or(InitialWindowSpawnPlacement::Adjacent),
        cluster_participation: rule
            .cluster_participation
            .unwrap_or(InitialWindowClusterParticipation::Layout),
    })
}

fn parse_rule_entry(
    rule: &mut PartialWindowRule,
    line: &str,
    line_no: usize,
) -> Result<(), String> {
    let Some((key, rest)) = line.split_once(char::is_whitespace) else {
        return Err(format!(
            "line {line_no}: expected `<key> <value>` inside rule"
        ));
    };
    let value = rest.trim();
    if value.is_empty() {
        return Err(format!("line {line_no}: missing value for `{key}`"));
    }

    match key {
        "app-id" | "app_id" => {
            rule.app_ids = parse_rule_app_ids(value, line_no)?;
        }
        "title" => {
            rule.titles = parse_rule_match_strings(value, line_no, "title")?;
        }
        "overlap-policy" | "overlap_policy" => {
            rule.overlap_policy = Some(parse_rule_overlap_policy(value, line_no)?);
        }
        "spawn-placement" | "spawn_placement" => {
            rule.spawn_placement = Some(parse_rule_spawn_placement(value, line_no)?);
        }
        "cluster-participation" | "cluster_participation" => {
            rule.cluster_participation = Some(parse_rule_cluster_participation(value, line_no)?);
        }
        _ => {
            return Err(format!("line {line_no}: unknown rule key `{key}`"));
        }
    }

    Ok(())
}

fn parse_rule_app_ids(value: &str, line_no: usize) -> Result<Vec<WindowRulePattern>, String> {
    parse_rule_match_strings(value, line_no, "app-id")
}

fn parse_rule_match_strings(
    value: &str,
    line_no: usize,
    field_name: &str,
) -> Result<Vec<WindowRulePattern>, String> {
    let trimmed = value.trim();
    if trimmed.starts_with('[') {
        return parse_string_array_literal(value, line_no, field_name);
    }
    Ok(vec![parse_rule_match_pattern(
        trimmed, line_no, field_name,
    )?])
}

fn parse_rule_overlap_policy(
    value: &str,
    line_no: usize,
) -> Result<InitialWindowOverlapPolicy, String> {
    match parse_quoted_string_literal(value, line_no)?.as_str() {
        "none" => Ok(InitialWindowOverlapPolicy::None),
        "parent-only" => Ok(InitialWindowOverlapPolicy::ParentOnly),
        "all" => Ok(InitialWindowOverlapPolicy::All),
        other => Err(format!(
            "line {line_no}: unknown overlap-policy `{other}`; expected `none`, `parent-only`, or `all`"
        )),
    }
}

fn parse_rule_spawn_placement(
    value: &str,
    line_no: usize,
) -> Result<InitialWindowSpawnPlacement, String> {
    match parse_quoted_string_literal(value, line_no)?.as_str() {
        "center" => Ok(InitialWindowSpawnPlacement::Center),
        "adjacent" => Ok(InitialWindowSpawnPlacement::Adjacent),
        "viewport-center" => Ok(InitialWindowSpawnPlacement::ViewportCenter),
        "cursor" => Ok(InitialWindowSpawnPlacement::Cursor),
        "app" => Ok(InitialWindowSpawnPlacement::App),
        other => Err(format!(
            "line {line_no}: unknown spawn-placement `{other}`; expected `center`, `adjacent`, `viewport-center`, `cursor`, or `app`"
        )),
    }
}

fn parse_rule_cluster_participation(
    value: &str,
    line_no: usize,
) -> Result<InitialWindowClusterParticipation, String> {
    match parse_quoted_string_literal(value, line_no)?.as_str() {
        "layout" => Ok(InitialWindowClusterParticipation::Layout),
        "float" => Ok(InitialWindowClusterParticipation::Float),
        other => Err(format!(
            "line {line_no}: unknown cluster-participation `{other}`; expected `layout` or `float`"
        )),
    }
}

fn parse_quoted_string_literal(value: &str, line_no: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if !trimmed.starts_with('"') || !trimmed.ends_with('"') || trimmed.len() < 2 {
        return Err(format!(
            "line {line_no}: expected quoted string, got `{trimmed}`"
        ));
    }
    Ok(trimmed[1..trimmed.len() - 1].to_string())
}

fn parse_regex_literal(value: &str, line_no: usize) -> Result<String, String> {
    let trimmed = value.trim();
    if !trimmed.starts_with("r\"") || !trimmed.ends_with('"') || trimmed.len() < 3 {
        return Err(format!(
            "line {line_no}: expected regex literal, got `{trimmed}`"
        ));
    }
    Ok(trimmed[2..trimmed.len() - 1].to_string())
}

fn parse_rule_match_pattern(
    value: &str,
    line_no: usize,
    field_name: &str,
) -> Result<WindowRulePattern, String> {
    let trimmed = value.trim();
    if trimmed.starts_with("r\"") {
        let raw = parse_regex_literal(trimmed, line_no)?;
        let compiled = regex::Regex::new(&raw)
            .map_err(|err| format!("line {line_no}: invalid {field_name} regex `{raw}`: {err}"))?;
        Ok(WindowRulePattern::Regex(compiled))
    } else {
        Ok(WindowRulePattern::Exact(parse_quoted_string_literal(
            trimmed, line_no,
        )?))
    }
}

fn parse_string_array_literal(
    value: &str,
    line_no: usize,
    field_name: &str,
) -> Result<Vec<WindowRulePattern>, String> {
    let trimmed = value.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(format!(
            "line {line_no}: expected string array literal, got `{trimmed}`"
        ));
    }
    let mut out = Vec::new();
    let mut rest = &trimmed[1..trimmed.len() - 1];
    while !rest.trim().is_empty() {
        rest = rest.trim_start();
        if !rest.starts_with('"') && !rest.starts_with("r\"") {
            return Err(format!(
                "line {line_no}: expected string or regex literal inside array, got `{rest}`"
            ));
        }
        let regex_prefix = rest.starts_with("r\"");
        let start = if regex_prefix { 2 } else { 1 };
        let mut escaped = false;
        let mut end_idx = None;
        for (idx, ch) in rest.char_indices().skip(start) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' && !regex_prefix {
                escaped = true;
                continue;
            }
            if ch == '"' {
                end_idx = Some(idx);
                break;
            }
        }
        let Some(end_idx) = end_idx else {
            return Err(format!(
                "line {line_no}: unterminated {field_name} matcher in array"
            ));
        };
        out.push(parse_rule_match_pattern(
            &rest[..=end_idx],
            line_no,
            field_name,
        )?);
        rest = rest[end_idx + 1..].trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(next) = rest.strip_prefix(',') {
            rest = next;
        } else {
            return Err(format!(
                "line {line_no}: expected `,` between {field_name} matchers, got `{rest}`"
            ));
        }
    }
    if out.is_empty() {
        return Err(format!(
            "line {line_no}: {field_name} array must not be empty"
        ));
    }
    Ok(out)
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

fn load_cursor_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(theme) = pick_string(cfg, &["cursor.theme"]) {
        let theme = theme.trim();
        if !theme.is_empty() {
            out.cursor.theme = theme.to_string();
        }
    }
    out.cursor.size = pick_u32(cfg, &["cursor.size"], out.cursor.size);
    out.cursor.hide_while_typing = pick_bool(
        cfg,
        &["cursor.hide-while-typing", "cursor.hide_while_typing"],
        out.cursor.hide_while_typing,
    );
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
        &[
            "tile.gaps-inner",
            "tile.gaps_inner",
            "tile.gap-inner",
            "tile.gap_inner",
        ],
        out.tile_gaps_inner_px,
    );
    out.tile_gaps_outer_px = pick_f32(
        cfg,
        &[
            "tile.gaps-outer",
            "tile.gaps_outer",
            "tile.gap-outer",
            "tile.gap_outer",
        ],
        out.tile_gaps_outer_px,
    );
    out.tile_new_on_top = pick_bool(
        cfg,
        &["tile.new-on-top", "tile.new_on_top"],
        out.tile_new_on_top,
    );
    out.tile_queue_show_icons = pick_bool(
        cfg,
        &[
            "tile.queue-show-icons",
            "tile.queue_show_icons",
            "tile.show-queue-icons",
            "tile.show_queue_icons",
        ],
        out.tile_queue_show_icons,
    );
    out.tile_max_stack = pick_u64(
        cfg,
        &[
            "tile.max-stack",
            "tile.max_stack",
            "tile.stack-limit",
            "field.active-windows-allowed",
            "field.active_windows_allowed",
        ],
        out.tile_max_stack as u64,
    ) as usize;
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
}

fn load_physics_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.physics_enabled = pick_bool(cfg, &["physics.enabled"], out.physics_enabled);

    out.non_overlap_bump_damping =
        pick_f32(cfg, &["physics.damping"], out.non_overlap_bump_damping);
}

fn load_decorations_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.border_size_px = pick_i32(
        cfg,
        &[
            "decoration.border-size",
            "decoration.border_size",
            "decorations.border-size",
            "decorations.border_size",
        ],
        out.border_size_px,
    );
    out.border_radius_px = pick_i32(
        cfg,
        &[
            "decoration.border-radius",
            "decoration.border_radius",
            "decorations.border-radius",
            "decorations.border_radius",
        ],
        out.border_radius_px,
    );
    out.border_color_focused = pick_decoration_border_color(
        cfg,
        &[
            "decoration.border-colour-focused",
            "decoration.border_colour_focused",
            "decoration.border-color-focused",
            "decoration.border_color_focused",
            "decorations.border-colour-focused",
            "decorations.border_colour_focused",
            "decorations.border-color-focused",
            "decorations.border_color_focused",
        ],
        out.border_color_focused,
    );
    out.border_color_unfocused = pick_decoration_border_color(
        cfg,
        &[
            "decoration.border-colour-unfocused",
            "decoration.border_colour_unfocused",
            "decoration.border-color-unfocused",
            "decoration.border_color_unfocused",
            "decorations.border-colour-unfocused",
            "decorations.border_colour_unfocused",
            "decorations.border-color-unfocused",
            "decorations.border_color_unfocused",
        ],
        out.border_color_unfocused,
    );
    out.resize_using_border = pick_bool(
        cfg,
        &[
            "decoration.resize-using-border",
            "decoration.resize_using_border",
            "decorations.resize-using-border",
            "decorations.resize_using_border",
        ],
        out.resize_using_border,
    );
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

fn pick_decoration_border_color(
    cfg: &RuneConfig,
    paths: &[&str],
    default: DecorationBorderColor,
) -> DecorationBorderColor {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    parse_hex_rgb(raw.trim().trim_matches('"'))
        .map(|(r, g, b)| DecorationBorderColor { r, g, b })
        .unwrap_or(default)
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
        CloseRestorePanMode, ClusterBloomDirection, CompositorBindingAction, CursorConfig,
        DirectionalAction, InitialWindowClusterParticipation, InitialWindowOverlapPolicy,
        InitialWindowSpawnPlacement, MonitorBindingAction, MonitorBindingTarget,
        NodeBackgroundColorMode, NodeBindingAction, NodeDisplayPolicy, PanToNewMode, RuntimeTuning,
        WHEEL_DOWN_CODE, WHEEL_UP_CODE, WindowRulePattern, keybinds::key_name_to_evdev,
    };

    fn write_temp_config(prefix: &str, content: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{unique}.rune"));
        fs::write(&path, content).expect("write temp config");
        path
    }

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
        let path = write_temp_config(
            "halley-autostart",
            r#"
autostart:
  once "waybar"
  once "mako"
  on-reload "thunderbird"
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.autostart_once, vec!["waybar", "mako"]);
        assert_eq!(tuning.autostart_on_reload, vec!["thunderbird"]);
    }

    #[test]
    fn cursor_section_parses_theme_size_and_hide_while_typing() {
        let path = write_temp_config(
            "halley-cursor",
            r#"
cursor:
  theme "Bibata-Modern-Ice"
  size 32
  hide-while-typing true
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(
            tuning.cursor,
            CursorConfig {
                theme: "Bibata-Modern-Ice".to_string(),
                size: 32,
                hide_while_typing: true,
            }
        );
    }

    #[test]
    fn default_env_no_longer_injects_cursor_theme_or_size() {
        let tuning = RuntimeTuning::default();

        assert!(!tuning.env.contains_key("XCURSOR_THEME"));
        assert!(!tuning.env.contains_key("XCURSOR_SIZE"));
        assert_eq!(tuning.cursor, CursorConfig::default());
    }

    #[test]
    fn rules_parse_single_app_id_string_and_defaults() {
        let path = write_temp_config(
            "halley-rules-single",
            r#"
rules:
  rule:
    app-id "firefox"
  end
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.window_rules.len(), 1);
        let rule = &tuning.window_rules[0];
        assert_eq!(rule.app_ids.len(), 1);
        assert!(matches!(&rule.app_ids[0], WindowRulePattern::Exact(value) if value == "firefox"));
        assert!(rule.titles.is_empty());
        assert_eq!(rule.overlap_policy, InitialWindowOverlapPolicy::None);
        assert_eq!(rule.spawn_placement, InitialWindowSpawnPlacement::Adjacent);
        assert_eq!(
            rule.cluster_participation,
            InitialWindowClusterParticipation::Layout
        );
    }

    #[test]
    fn rules_parse_app_id_array_in_source_order() {
        let path = write_temp_config(
            "halley-rules-array",
            r#"
rules:
  rule:
    app-id ["file-picker", "Picture-in-Picture"]
    overlap-policy "parent-only"
    spawn-placement "center"
    cluster-participation "float"
  end
  rule:
    app-id "firefox"
    overlap-policy "all"
  end
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.window_rules.len(), 2);
        assert_eq!(tuning.window_rules[0].app_ids.len(), 2);
        assert!(
            matches!(&tuning.window_rules[0].app_ids[0], WindowRulePattern::Exact(value) if value == "file-picker")
        );
        assert!(
            matches!(&tuning.window_rules[0].app_ids[1], WindowRulePattern::Exact(value) if value == "Picture-in-Picture")
        );
        assert!(tuning.window_rules[0].titles.is_empty());
        assert_eq!(
            tuning.window_rules[0].overlap_policy,
            InitialWindowOverlapPolicy::ParentOnly
        );
        assert_eq!(
            tuning.window_rules[0].spawn_placement,
            InitialWindowSpawnPlacement::Center
        );
        assert_eq!(
            tuning.window_rules[0].cluster_participation,
            InitialWindowClusterParticipation::Float
        );
        assert!(
            matches!(&tuning.window_rules[1].app_ids[0], WindowRulePattern::Exact(value) if value == "firefox")
        );
        assert!(tuning.window_rules[1].titles.is_empty());
    }

    #[test]
    fn rules_parse_title_string_and_array_matchers() {
        let path = write_temp_config(
            "halley-rules-title",
            r#"
rules:
  rule:
    title "Picture-in-Picture"
    spawn-placement "center"
  end
  rule:
    app-id "firefox"
    title ["Save As", "Open File"]
    cluster-participation "float"
  end
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.window_rules.len(), 2);
        assert!(tuning.window_rules[0].app_ids.is_empty());
        assert!(
            matches!(&tuning.window_rules[0].titles[0], WindowRulePattern::Exact(value) if value == "Picture-in-Picture")
        );
        assert_eq!(tuning.window_rules[1].titles.len(), 2);
        assert!(
            matches!(&tuning.window_rules[1].titles[0], WindowRulePattern::Exact(value) if value == "Save As")
        );
        assert!(
            matches!(&tuning.window_rules[1].titles[1], WindowRulePattern::Exact(value) if value == "Open File")
        );
    }

    #[test]
    fn rules_parse_regex_matchers() {
        let path = write_temp_config(
            "halley-rules-regex",
            r#"
rules:
  rule:
    title [r"File Upload.*", "Picture-in-Picture"]
  end
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert!(
            matches!(&tuning.window_rules[0].titles[0], WindowRulePattern::Regex(regex) if regex.as_str() == "File Upload.*")
        );
        assert!(
            matches!(&tuning.window_rules[0].titles[1], WindowRulePattern::Exact(value) if value == "Picture-in-Picture")
        );
    }

    #[test]
    fn rules_fail_strictly_on_unknown_enum_value() {
        let path = write_temp_config(
            "halley-rules-invalid",
            r#"
rules:
  rule:
    app-id "firefox"
    overlap-policy "weird"
  end
end
"#,
        );

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"));
        let _ = fs::remove_file(&path);

        assert!(tuning.is_none());
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
    fn tile_max_stack_parses_from_config() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-tile-max-stack-{unique}.rune"));
        fs::write(
            &path,
            r#"
tile:
  max-stack 5
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.tile_max_stack, 5);
    }

    #[test]
    fn legacy_field_active_windows_allowed_parses_as_fallback() {
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
        assert_eq!(tuning.tile_max_stack, 5);
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
    fn tile_new_on_top_setting_parses_from_tile_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-tile-new-on-top-{unique}.rune"));
        fs::write(
            &path,
            r#"
tile:
  new-on-top true
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert!(tuning.tile_new_on_top);
    }

    #[test]
    fn tile_queue_icon_setting_parses_from_tile_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-tile-queue-icons-{unique}.rune"));
        fs::write(
            &path,
            r#"
tile:
  queue-show-icons false
end
"#,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert!(!tuning.tile_queue_show_icons);
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

    #[test]
    fn decorations_border_settings_parse_from_plural_section() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-decorations-border-{unique}.rune"));
        fs::write(
            &path,
            r##"
decorations:
  border-size 3
  border-radius 6
  border-colour-focused "#fd5e53"
  border-colour-unfocused "#333333"
  resize-using-border true
end
"##,
        )
        .expect("write temp config");

        let tuning = RuntimeTuning::from_rune_file(path.to_str().expect("utf8 path"))
            .expect("config should parse");
        let _ = fs::remove_file(&path);

        assert_eq!(tuning.border_size_px, 3);
        assert_eq!(tuning.border_radius_px, 6);
        assert_eq!(tuning.border_color_focused.r, 253.0 / 255.0);
        assert_eq!(tuning.border_color_focused.g, 94.0 / 255.0);
        assert_eq!(tuning.border_color_focused.b, 83.0 / 255.0);
        assert_eq!(tuning.border_color_unfocused.r, 51.0 / 255.0);
        assert_eq!(tuning.border_color_unfocused.g, 51.0 / 255.0);
        assert_eq!(tuning.border_color_unfocused.b, 51.0 / 255.0);
        assert!(tuning.resize_using_border);
    }
}
