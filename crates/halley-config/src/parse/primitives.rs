use std::collections::HashMap;

use rune_cfg::RuneConfig;

use crate::layout::{
    ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode, CloseRestorePanMode,
    ClusterBloomDirection, ClusterDefaultLayout, DecorationBorderColor, FocusRingConfig,
    NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy, OverlayColorMode,
    OverlayShape, PanToNewMode, ShapeStyle,
};

pub(crate) fn merge_env_map(cfg: &RuneConfig, out: &mut HashMap<String, String>, path: &str) {
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

pub(crate) fn pick_pan_to_new_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: PanToNewMode,
) -> PanToNewMode {
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

pub(crate) fn pick_close_restore_pan_mode(
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

pub(crate) fn pick_click_collapsed_outside_focus_mode(
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

pub(crate) fn pick_click_collapsed_pan_mode(
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

pub(crate) fn pick_cluster_bloom_direction(
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

pub(crate) fn pick_cluster_default_layout(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ClusterDefaultLayout,
) -> ClusterDefaultLayout {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "tiling" | "tile" => ClusterDefaultLayout::Tiling,
        "stacking" | "stack" => ClusterDefaultLayout::Stacking,
        _ => default,
    }
}

pub(crate) fn parse_viewport_focus_ring(
    cfg: &RuneConfig,
    root: &str,
    key: &str,
) -> Option<FocusRingConfig> {
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

pub(crate) fn pick_u64(cfg: &RuneConfig, paths: &[&str], default: u64) -> u64 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u64>(path) {
            return v;
        }
    }
    default
}

pub(crate) fn pick_f32(cfg: &RuneConfig, paths: &[&str], default: f32) -> f32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<f32>(path) {
            return v;
        }
    }
    default
}

pub(crate) fn pick_u32(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
    }
    default
}

pub(crate) fn pick_i32(cfg: &RuneConfig, paths: &[&str], default: i32) -> i32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<i32>(path) {
            return v;
        }
    }
    default
}

pub(crate) fn pick_bool(cfg: &RuneConfig, paths: &[&str], default: bool) -> bool {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return v;
        }
    }
    default
}

pub(crate) fn pick_optional_bool(cfg: &RuneConfig, paths: &[&str]) -> Option<bool> {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return Some(v);
        }
    }
    None
}

pub(crate) fn pick_string(cfg: &RuneConfig, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            return Some(v);
        }
    }
    None
}

pub(crate) fn pick_node_border_color_mode(
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

pub(crate) fn pick_node_display_policy(
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

pub(crate) fn pick_node_background_color_mode(
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

pub(crate) fn pick_shape_style(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ShapeStyle,
) -> ShapeStyle {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "square" => ShapeStyle::Square,
        "squircle" => ShapeStyle::Squircle,
        _ => default,
    }
}

pub(crate) fn pick_overlay_color_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: OverlayColorMode,
) -> OverlayColorMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    let value = raw.trim().trim_matches('"');
    if value.is_empty() {
        return default;
    }

    match value.to_ascii_lowercase().as_str() {
        "auto" => OverlayColorMode::Auto,
        "light" => OverlayColorMode::Light,
        "dark" => OverlayColorMode::Dark,
        _ => parse_hex_rgb(value)
            .map(|(r, g, b)| OverlayColorMode::Fixed { r, g, b })
            .unwrap_or(default),
    }
}

pub(crate) fn pick_overlay_shape(
    cfg: &RuneConfig,
    paths: &[&str],
    default: OverlayShape,
) -> OverlayShape {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "square" => OverlayShape::Square,
        "rounded" => OverlayShape::Rounded,
        _ => default,
    }
}

pub(crate) fn pick_decoration_border_color(
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

pub(crate) fn parse_hex_rgb(value: &str) -> Option<(f32, f32, f32)> {
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
