use std::collections::HashSet;

use super::ConfigLoadDiagnostic;

pub(crate) fn validate_known_config_keys(
    raw: &str,
    path: &str,
) -> Result<(), ConfigLoadDiagnostic> {
    let schema = ConfigSchema::new();
    let mut stack: Vec<String> = Vec::new();
    let mut ignored_depth: Option<usize> = None;

    for (line_idx, raw_line) in raw.lines().enumerate() {
        let line_no = line_idx + 1;
        let trimmed = strip_comment(raw_line).trim().to_string();
        if trimmed.is_empty() || trimmed.starts_with('@') || trimmed.starts_with("gather ") {
            continue;
        }

        if trimmed.eq_ignore_ascii_case("endif") {
            continue;
        }

        if trimmed.eq_ignore_ascii_case("end") {
            if ignored_depth == Some(stack.len()) {
                ignored_depth = None;
            }
            stack.pop();
            continue;
        }

        if ignored_depth.is_some() {
            if trimmed.ends_with(':') {
                stack.push(normalize_token(trimmed.trim_end_matches(':').trim()));
            }
            continue;
        }

        if trimmed.starts_with("if ")
            || trimmed.eq_ignore_ascii_case("else")
            || trimmed.starts_with("elseif ")
            || trimmed.starts_with("else-if ")
        {
            continue;
        }

        if trimmed.ends_with(':') {
            let section = normalize_section_name(trimmed.trim_end_matches(':').trim(), &stack);
            let next_path = path_with(&stack, section.as_str());
            let top = next_path.split('.').next().unwrap_or_default();

            if stack.is_empty() && !schema.known_top_sections.contains(top) {
                stack.push(section);
                ignored_depth = Some(stack.len());
                continue;
            }

            if !schema.section_allowed(next_path.as_str()) {
                if let Some(diag) =
                    deprecated_key_diagnostic(path, raw, line_no, next_path.as_str())
                {
                    return Err(diag);
                }
                return Err(unknown_key_diagnostic(
                    path,
                    raw,
                    line_no,
                    next_path.as_str(),
                    &schema,
                ));
            }

            stack.push(section);
            if schema.ignored_sections.contains(next_path.as_str()) {
                ignored_depth = Some(stack.len());
            }
            continue;
        }

        if stack.is_empty() {
            continue;
        }
        if schema.ignored_sections.contains(stack.join(".").as_str()) {
            continue;
        }

        let key = scalar_key(trimmed.as_str());
        if key.is_empty() {
            continue;
        }
        let full_path = path_with(&stack, key.as_str());
        if !schema.scalar_allowed(full_path.as_str()) {
            if let Some(diag) = deprecated_key_diagnostic(path, raw, line_no, full_path.as_str()) {
                return Err(diag);
            }
            return Err(unknown_key_diagnostic(
                path,
                raw,
                line_no,
                full_path.as_str(),
                &schema,
            ));
        }
        if let Some(message) = validate_scalar_value(full_path.as_str(), trimmed.as_str()) {
            return Err(ConfigLoadDiagnostic {
                path: path.to_string(),
                line: Some(line_no),
                column: None,
                message,
                hint: None,
                source_line: source_line(raw, line_no),
            });
        }
    }

    Ok(())
}

struct ConfigSchema {
    known_top_sections: HashSet<&'static str>,
    ignored_sections: HashSet<&'static str>,
    sections: HashSet<&'static str>,
    scalars: HashSet<&'static str>,
}

impl ConfigSchema {
    fn new() -> Self {
        let known_top_sections = HashSet::from([
            "animations",
            "apogee",
            "autostart",
            "bearings",
            "clusters",
            "cursor",
            "debug",
            "decay",
            "decorations",
            "effects",
            "env",
            "field",
            "focus-ring",
            "font",
            "gamescope",
            "input",
            "keybinds",
            "nodes",
            "overlays",
            "placement",
            "physics",
            "rules",
            "screenshot",
            "stacking",
            "tile",
            "trail",
            "viewport",
        ]);
        let ignored_sections =
            HashSet::from(["autostart", "env", "gamescope", "keybinds", "rules"]);
        let sections = HashSet::from([
            "animations",
            "animations.smooth-resize",
            "animations.maximize",
            "animations.fullscreen",
            "animations.window-close",
            "animations.window-open",
            "animations.tile",
            "animations.stack",
            "animations.raise",
            "apogee",
            "bearings",
            "clusters",
            "cursor",
            "debug",
            "decay",
            "decorations",
            "decorations.border",
            "decorations.secondary-border",
            "effects",
            "effects.blur",
            "effects.shadows",
            "effects.shadows.window",
            "effects.shadows.node",
            "effects.shadows.overlay",
            "field",
            "field.pins",
            "field.zoom",
            "focus-ring",
            "font",
            "input",
            "input.keyboard",
            "input.touchpad",
            "input.mouse",
            "input.devices",
            "nodes",
            "overlays",
            "placement",
            "placement.expanded",
            "placement.landmarks",
            "placement.reveal",
            "physics",
            "screenshot",
            "stacking",
            "tile",
            "trail",
            "viewport",
        ]);
        let scalars = HashSet::from([
            "animations.enabled",
            "animations.smooth-resize.enabled",
            "animations.smooth-resize.duration-ms",
            "animations.maximize.enabled",
            "animations.maximize.duration-ms",
            "animations.fullscreen.enabled",
            "animations.fullscreen.duration-ms",
            "animations.window-close.enabled",
            "animations.window-close.duration-ms",
            "animations.window-close.style",
            "animations.window-open.enabled",
            "animations.window-open.duration-ms",
            "animations.tile.enabled",
            "animations.tile.duration-ms",
            "animations.stack.enabled",
            "animations.stack.duration-ms",
            "animations.raise.enabled",
            "animations.raise.duration-ms",
            "animations.raise.scale",
            "animations.raise.shadow-boost",
            "animations.raise.trigger",
            "apogee.enabled",
            "apogee.live-previews",
            "apogee.live_previews",
            "apogee.transition-ms",
            "apogee.transition_ms",
            "apogee.gap",
            "apogee.gap-px",
            "apogee.max-rows",
            "apogee.max_rows",
            "apogee.rows",
            "apogee.show-collapsed-as-nodes",
            "apogee.show_collapsed_as_nodes",
            "apogee.background-dim",
            "apogee.background_dim",
            "bearings.show-distance",
            "bearings.show-icons",
            "bearings.show-pinned",
            "bearings.fade-distance",
            "bearings.blur",
            "clusters.distance-px",
            "clusters.cluster-dwell-ms",
            "clusters.dwell-ms",
            "clusters.show-icons",
            "clusters.bloom-direction",
            "clusters.default-layout",
            "cursor.theme",
            "cursor.size",
            "cursor.hide-while-typing",
            "cursor.hide-when-typing",
            "cursor.hide-after-ms",
            "cursor.hide-after-inactive-ms",
            "debug.overlay-fps",
            "debug.show-ring-when-resizing",
            "decay.active-delay",
            "decay.inactive-delay",
            "decay.docked-offscreen-delay",
            "decorations.border.size",
            "decorations.border.radius",
            "decorations.border.colour-focused",
            "decorations.border.color-focused",
            "decorations.border.colour-unfocused",
            "decorations.border.color-unfocused",
            "decorations.secondary-border.enabled",
            "decorations.secondary-border.size",
            "decorations.secondary-border.gap",
            "decorations.secondary-border.colour-focused",
            "decorations.secondary-border.color-focused",
            "decorations.secondary-border.colour-unfocused",
            "decorations.secondary-border.color-unfocused",
            "decorations.resize-using-border",
            "effects.blur.enabled",
            "effects.blur.overlays",
            "effects.blur.windows",
            "effects.blur.layer-shell",
            "effects.blur.layer_shell",
            "effects.blur.method",
            "effects.blur.radius",
            "effects.blur.passes",
            "effects.blur.saturation",
            "effects.blur.noise",
            "effects.shadows.window.enabled",
            "effects.shadows.window.blur-radius",
            "effects.shadows.window.spread",
            "effects.shadows.window.offset-x",
            "effects.shadows.window.offset-y",
            "effects.shadows.window.colour",
            "effects.shadows.window.color",
            "effects.shadows.node.enabled",
            "effects.shadows.node.blur-radius",
            "effects.shadows.node.spread",
            "effects.shadows.node.offset-x",
            "effects.shadows.node.offset-y",
            "effects.shadows.node.colour",
            "effects.shadows.node.color",
            "effects.shadows.overlay.enabled",
            "effects.shadows.overlay.blur-radius",
            "effects.shadows.overlay.spread",
            "effects.shadows.overlay.offset-x",
            "effects.shadows.overlay.offset-y",
            "effects.shadows.overlay.colour",
            "effects.shadows.overlay.color",
            "field.gap",
            "field.gap-px",
            "field.active-windows-allowed",
            "field.pan-to-new",
            "field.pins.corner",
            "field.pins.badge-corner",
            "field.pins.colour",
            "field.pins.color",
            "field.pins.pin-colour",
            "field.pins.pin-color",
            "field.pins.background-colour",
            "field.pins.background-color",
            "field.pins.bg-colour",
            "field.pins.bg-color",
            "field.pins.size",
            "field.close-restore-focus",
            "field.close-restore-pan",
            "field.zoom.enabled",
            "field.zoom.step",
            "field.zoom.min",
            "field.zoom.max",
            "field.zoom.smooth",
            "field.zoom.smooth-rate",
            "field.zoom-smooth-rate",
            "focus-ring.rx",
            "focus-ring.ry",
            "focus-ring.radius-x",
            "focus-ring.radius-y",
            "focus-ring.offset-x",
            "focus-ring.offset-y",
            "focus-ring.primary-rx",
            "focus-ring.primary-ry",
            "font.family",
            "font.size",
            "input.repeat-rate",
            "input.repeat-delay",
            "input.focus-mode",
            "input.raise-on-click",
            "input.keyboard.layout",
            "input.keyboard.variant",
            "input.keyboard.options",
            "input.keyboard.model",
            "nodes.primary-to-node-ms",
            "nodes.node-delay",
            "nodes.primary-to-preview-ms",
            "nodes.preview-delay",
            "nodes.primary-preview-to-node-ms",
            "nodes.preview-to-node-ms",
            "nodes.primary-hot-inner-frac",
            "nodes.hot-inner-frac",
            "nodes.show-labels",
            "nodes.show-app-icons",
            "nodes.show-icons",
            "nodes.node-shape",
            "nodes.shape",
            "nodes.node-label-shape",
            "nodes.label-shape",
            "nodes.icon-size",
            "nodes.opacity",
            "nodes.background-colour",
            "nodes.background-color",
            "nodes.border-colour-hover",
            "nodes.border-color-hover",
            "nodes.border-colour-inactive",
            "nodes.border-color-inactive",
            "nodes.click-collapsed-outside-focus",
            "nodes.click-collapsed-pan",
            "overlays.background-colour",
            "overlays.background-color",
            "overlays.text-colour",
            "overlays.text-color",
            "overlays.error-colour",
            "overlays.error-color",
            "overlays.shape",
            "overlays.borders",
            "overlays.border-source",
            "overlays.blur",
            "placement.expanded.strategy",
            "placement.expanded.fallback",
            "placement.expanded.find-empty-mode",
            "placement.landmarks.strategy",
            "placement.landmarks.normal-blocker",
            "placement.landmarks.pinned-blocker",
            "placement.reveal.enabled",
            "placement.reveal.max-pan-px",
            "placement.reveal.animation-ms",
            "placement.reveal.pan-to-new",
            "physics.enabled",
            "physics.damping",
            "screenshot.directory",
            "screenshot.output-directory",
            "screenshot.highlight-colour",
            "screenshot.highlight-color",
            "screenshot.background-colour",
            "screenshot.background-color",
            "stacking.max-visible",
            "stacking.visible-limit",
            "tile.gaps-inner",
            "tile.gap-inner",
            "tile.gaps-outer",
            "tile.gap-outer",
            "tile.new-on-top",
            "tile.queue-show-icons",
            "tile.show-queue-icons",
            "tile.max-stack",
            "tile.stack-limit",
            "trail.history-length",
            "trail.wrap",
            "trail.wrap-history",
            "viewport.center-x",
            "viewport.center-y",
            "viewport.size-w",
            "viewport.size-h",
        ]);

        Self {
            known_top_sections,
            ignored_sections,
            sections,
            scalars,
        }
    }

    fn section_allowed(&self, path: &str) -> bool {
        self.sections.contains(path)
            || self.ignored_sections.contains(path)
            || viewport_output_path(path)
                .is_some_and(|rest| rest.is_empty() || rest == "focus-ring")
            || input_device_override_section(path)
    }

    fn scalar_allowed(&self, path: &str) -> bool {
        self.scalars.contains(path)
            || viewport_output_path(path).is_some_and(|rest| viewport_output_scalar_allowed(rest))
            || input_device_setting_key(path).is_some_and(input_device_setting_key_allowed)
    }

    fn suggestions_for_parent(&self, parent: &str) -> Vec<&'static str> {
        self.sections
            .iter()
            .chain(self.scalars.iter())
            .copied()
            .filter(|candidate| path_parent(candidate) == parent)
            .collect()
    }
}

fn validate_scalar_value(path: &str, line: &str) -> Option<String> {
    let raw = line.split_once(char::is_whitespace)?.1.trim();
    let quoted = raw.starts_with('"') || raw.starts_with('\'');
    let value = raw
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase();
    if numeric_scalar(path) && value.parse::<f64>().is_err() {
        return Some(format!("Invalid number `{value}` for `{path}`"));
    }
    if bool_scalar(path) && !matches!(value.as_str(), "true" | "false") {
        return Some(format!(
            "Invalid boolean `{value}` for `{path}`; expected `true` or `false`"
        ));
    }
    if let Some(allowed) = enum_allowed_values(path)
        && !allowed.contains(&value.as_str())
    {
        return Some(format!(
            "Invalid value `{value}` for `{path}`; expected one of: {}",
            allowed.join(", ")
        ));
    }
    if color_scalar(path) && quoted && !valid_overlay_color_value(value.as_str()) {
        return Some(format!(
            "Invalid colour `{value}` for `{path}`; expected `auto`, `light`, `dark`, `#rrggbb`, or `#rrggbbaa`"
        ));
    }
    None
}

fn numeric_scalar(path: &str) -> bool {
    matches!(
        path,
        "animations.smooth-resize.duration-ms"
            | "animations.maximize.duration-ms"
            | "animations.fullscreen.duration-ms"
            | "animations.window-close.duration-ms"
            | "animations.window-open.duration-ms"
            | "animations.tile.duration-ms"
            | "animations.stack.duration-ms"
            | "animations.raise.duration-ms"
            | "animations.raise.scale"
            | "animations.raise.shadow-boost"
            | "apogee.transition-ms"
            | "apogee.transition_ms"
            | "apogee.gap"
            | "apogee.gap-px"
            | "apogee.max-rows"
            | "apogee.max_rows"
            | "apogee.rows"
            | "apogee.background-dim"
            | "apogee.background_dim"
            | "bearings.fade-distance"
            | "clusters.distance-px"
            | "clusters.cluster-dwell-ms"
            | "clusters.dwell-ms"
            | "cursor.size"
            | "cursor.hide-after-ms"
            | "cursor.hide-after-inactive-ms"
            | "decay.active-delay"
            | "decay.inactive-delay"
            | "decay.docked-offscreen-delay"
            | "decorations.border.size"
            | "decorations.border.radius"
            | "decorations.secondary-border.size"
            | "decorations.secondary-border.gap"
            | "effects.blur.radius"
            | "effects.blur.passes"
            | "effects.blur.saturation"
            | "effects.blur.noise"
            | "effects.shadows.window.blur-radius"
            | "effects.shadows.window.spread"
            | "effects.shadows.window.offset-x"
            | "effects.shadows.window.offset-y"
            | "effects.shadows.node.blur-radius"
            | "effects.shadows.node.spread"
            | "effects.shadows.node.offset-x"
            | "effects.shadows.node.offset-y"
            | "effects.shadows.overlay.blur-radius"
            | "effects.shadows.overlay.spread"
            | "effects.shadows.overlay.offset-x"
            | "effects.shadows.overlay.offset-y"
            | "field.gap"
            | "field.gap-px"
            | "field.active-windows-allowed"
            | "field.pins.size"
            | "field.zoom.step"
            | "field.zoom.min"
            | "field.zoom.max"
            | "field.zoom.smooth-rate"
            | "field.zoom-smooth-rate"
            | "focus-ring.rx"
            | "focus-ring.ry"
            | "focus-ring.radius-x"
            | "focus-ring.radius-y"
            | "focus-ring.offset-x"
            | "focus-ring.offset-y"
            | "focus-ring.primary-rx"
            | "focus-ring.primary-ry"
            | "font.size"
            | "input.repeat-rate"
            | "input.repeat-delay"
            | "nodes.primary-to-node-ms"
            | "nodes.node-delay"
            | "nodes.primary-to-preview-ms"
            | "nodes.preview-delay"
            | "nodes.primary-preview-to-node-ms"
            | "nodes.preview-to-node-ms"
            | "nodes.primary-hot-inner-frac"
            | "nodes.hot-inner-frac"
            | "nodes.icon-size"
            | "nodes.opacity"
            | "placement.reveal.max-pan-px"
            | "placement.reveal.animation-ms"
            | "physics.damping"
            | "stacking.max-visible"
            | "stacking.visible-limit"
            | "tile.gaps-inner"
            | "tile.gap-inner"
            | "tile.gaps-outer"
            | "tile.gap-outer"
            | "tile.max-stack"
            | "tile.stack-limit"
            | "trail.history-length"
            | "viewport.center-x"
            | "viewport.center-y"
            | "viewport.size-w"
            | "viewport.size-h"
    ) || viewport_output_path(path).is_some_and(|rest| {
        matches!(
            rest,
            "width"
                | "height"
                | "size-w"
                | "size-h"
                | "offset-x"
                | "offset-y"
                | "refresh-rate"
                | "rate"
                | "transform"
                | "rotation"
                | "focus-ring.rx"
                | "focus-ring.ry"
                | "focus-ring.radius-x"
                | "focus-ring.radius-y"
                | "focus-ring.primary-rx"
                | "focus-ring.primary-ry"
                | "focus-ring.offset-x"
                | "focus-ring.offset-y"
        )
    }) || input_device_setting_key(path)
        .is_some_and(|key| matches!(key, "accel-speed" | "scroll-button"))
}

fn bool_scalar(path: &str) -> bool {
    matches!(
        path,
        "animations.enabled"
            | "animations.smooth-resize.enabled"
            | "animations.maximize.enabled"
            | "animations.fullscreen.enabled"
            | "animations.window-close.enabled"
            | "animations.window-open.enabled"
            | "animations.tile.enabled"
            | "animations.stack.enabled"
            | "animations.raise.enabled"
            | "apogee.enabled"
            | "apogee.live-previews"
            | "apogee.live_previews"
            | "apogee.show-collapsed-as-nodes"
            | "apogee.show_collapsed_as_nodes"
            | "bearings.show-distance"
            | "bearings.show-icons"
            | "bearings.show-pinned"
            | "bearings.blur"
            | "clusters.show-icons"
            | "cursor.hide-while-typing"
            | "cursor.hide-when-typing"
            | "debug.overlay-fps"
            | "debug.show-ring-when-resizing"
            | "decorations.secondary-border.enabled"
            | "effects.blur.enabled"
            | "effects.blur.overlays"
            | "effects.shadows.window.enabled"
            | "effects.shadows.node.enabled"
            | "effects.shadows.overlay.enabled"
            | "decorations.resize-using-border"
            | "overlays.blur"
            | "field.close-restore-focus"
            | "field.zoom.enabled"
            | "field.zoom.smooth"
            | "input.raise-on-click"
            | "overlays.borders"
            | "placement.reveal.enabled"
            | "physics.enabled"
            | "tile.new-on-top"
            | "tile.queue-show-icons"
            | "tile.show-queue-icons"
            | "trail.wrap"
            | "trail.wrap-history"
    ) || viewport_output_path(path).is_some_and(|rest| matches!(rest, "enabled" | "active"))
        || input_device_setting_key(path).is_some_and(|key| {
            matches!(
                key,
                "tap"
                    | "tap-to-click"
                    | "natural-scroll"
                    | "dwt"
                    | "disable-while-typing"
                    | "left-handed"
                    | "middle-emulation"
                    | "drag"
                    | "drag-lock"
                    | "disabled-on-external-mouse"
                    | "enabled"
            )
        })
}

fn enum_allowed_values(path: &str) -> Option<&'static [&'static str]> {
    match path {
        "clusters.bloom-direction" => Some(&[
            "clockwise",
            "cw",
            "counterclockwise",
            "counter-clockwise",
            "counter_clockwise",
            "ccw",
        ]),
        "clusters.default-layout" => Some(&["tiling", "tile", "stacking", "stack"]),
        "field.close-restore-pan" => Some(&["never", "if-offscreen", "if_offscreen", "always"]),
        "field.pan-to-new" => Some(&["never", "if-needed", "if_needed", "always", "true", "false"]),
        "field.pins.corner" | "field.pins.badge-corner" => Some(&[
            "top-left",
            "top_left",
            "left",
            "top-right",
            "top_right",
            "right",
        ]),
        "input.focus-mode" => Some(&["click", "hover"]),
        "nodes.border-colour-hover"
        | "nodes.border-color-hover"
        | "nodes.border-colour-inactive"
        | "nodes.border-color-inactive" => Some(&[
            "use-window-active",
            "use-window-inactive",
            "use-window-secondary-active",
            "use-window-secondary-inactive",
        ]),
        "nodes.click-collapsed-outside-focus" => Some(&["ignore", "activate"]),
        "nodes.click-collapsed-pan" => Some(&["never", "if-offscreen", "if_offscreen", "always"]),
        "nodes.node-shape" | "nodes.shape" | "nodes.node-label-shape" | "nodes.label-shape" => {
            Some(&["square", "squircle"])
        }
        "nodes.show-labels" | "nodes.show-app-icons" | "nodes.show-icons" => {
            Some(&["off", "false", "hover", "always", "on", "true"])
        }
        "effects.blur.windows" | "effects.blur.layer-shell" | "effects.blur.layer_shell" => {
            Some(&["off", "auto", "always"])
        }
        "effects.blur.method" => Some(&["dual-kawase", "dual_kawase", "kawase"]),
        "overlays.shape" => Some(&["square", "rounded"]),
        "overlays.border-source" => Some(&["primary", "secondary"]),
        "placement.expanded.strategy" | "placement.expanded.fallback" => {
            Some(&["center", "find-empty", "find_empty"])
        }
        "placement.expanded.find-empty-mode" => Some(&["best-effort", "best_effort"]),
        "placement.landmarks.strategy" => Some(&["nearest-free", "nearest_free"]),
        "placement.landmarks.normal-blocker" => Some(&["relocate"]),
        "placement.landmarks.pinned-blocker" => Some(&["preserve"]),
        "placement.reveal.pan-to-new" => {
            Some(&["never", "if-needed", "if_needed", "always", "true", "false"])
        }
        "animations.window-close.style" => Some(&["shrink", "fade"]),
        "animations.raise.trigger" => Some(&["always", "overlap"]),
        path if viewport_output_path(path)
            .is_some_and(|rest| rest == "vrr" || rest == "variable-refresh-rate") =>
        {
            Some(&[
                "off",
                "false",
                "on",
                "true",
                "on-demand",
                "ondemand",
                "adaptive",
            ])
        }
        _ => match input_device_setting_key(path) {
            Some("accel-profile") => Some(&["adaptive", "flat"]),
            Some("scroll-method") => Some(&[
                "no-scroll",
                "none",
                "two-finger",
                "twofinger",
                "edge",
                "on-button-down",
                "button",
            ]),
            Some("click-method") => Some(&["button-areas", "areas", "clickfinger", "click-finger"]),
            Some("tap-button-map") => {
                Some(&["left-right-middle", "lrm", "left-middle-right", "lmr"])
            }
            _ => None,
        },
    }
}

fn color_scalar(path: &str) -> bool {
    matches!(
        path,
        "decorations.border.colour-focused"
            | "decorations.border.color-focused"
            | "decorations.border.colour-unfocused"
            | "decorations.border.color-unfocused"
            | "decorations.secondary-border.colour-focused"
            | "decorations.secondary-border.color-focused"
            | "decorations.secondary-border.colour-unfocused"
            | "decorations.secondary-border.color-unfocused"
            | "field.pins.colour"
            | "field.pins.color"
            | "field.pins.pin-colour"
            | "field.pins.pin-color"
            | "field.pins.background-colour"
            | "field.pins.background-color"
            | "field.pins.bg-colour"
            | "field.pins.bg-color"
            | "nodes.background-colour"
            | "nodes.background-color"
            | "overlays.background-colour"
            | "overlays.background-color"
            | "overlays.text-colour"
            | "overlays.text-color"
            | "overlays.error-colour"
            | "overlays.error-color"
            | "screenshot.highlight-colour"
            | "screenshot.highlight-color"
            | "screenshot.background-colour"
            | "screenshot.background-color"
    )
}

fn valid_overlay_color_value(value: &str) -> bool {
    matches!(value, "auto" | "light" | "dark")
        || value.strip_prefix('#').is_some_and(|hex| {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

/// True for a per-device override section header `input.devices.<name>` (a single segment
/// after the `input.devices.` prefix; the device name itself is not validated).
fn input_device_override_section(path: &str) -> bool {
    path.strip_prefix("input.devices.")
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('.'))
}

/// For a scalar under `input.touchpad`, `input.mouse`, or `input.devices.<name>`, return the
/// trailing device-setting key (e.g. `accel-speed`). Lets the touchpad/mouse type sections
/// and per-device override blocks share one allowlist and one set of value validators.
fn input_device_setting_key(path: &str) -> Option<&str> {
    if let Some(rest) = path.strip_prefix("input.touchpad.") {
        return Some(rest);
    }
    if let Some(rest) = path.strip_prefix("input.mouse.") {
        return Some(rest);
    }
    let rest = path.strip_prefix("input.devices.")?;
    // `input.devices.<name>.<key>` -> key; `input.devices.<name>` (section) -> None.
    rest.split_once('.').map(|(_name, key)| key)
}

fn input_device_setting_key_allowed(key: &str) -> bool {
    matches!(
        key,
        "tap"
            | "tap-to-click"
            | "natural-scroll"
            | "dwt"
            | "disable-while-typing"
            | "accel-speed"
            | "accel-profile"
            | "scroll-method"
            | "scroll-button"
            | "click-method"
            | "tap-button-map"
            | "middle-emulation"
            | "left-handed"
            | "disabled-on-external-mouse"
            | "enabled"
            | "drag"
            | "drag-lock"
    )
}

fn viewport_output_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("viewport.")?;
    let mut parts = rest.splitn(2, '.');
    let first = parts.next()?;
    if viewport_root_scalar_allowed(first) {
        return None;
    }
    Some(parts.next().unwrap_or_default())
}

fn viewport_root_scalar_allowed(key: &str) -> bool {
    matches!(key, "center-x" | "center-y" | "size-w" | "size-h")
}

fn viewport_output_scalar_allowed(rest: &str) -> bool {
    matches!(
        rest,
        "enabled"
            | "active"
            | "width"
            | "height"
            | "size-w"
            | "size-h"
            | "offset-x"
            | "offset-y"
            | "refresh-rate"
            | "rate"
            | "transform"
            | "rotation"
            | "vrr"
            | "variable-refresh-rate"
            | "focus-ring.rx"
            | "focus-ring.ry"
            | "focus-ring.radius-x"
            | "focus-ring.radius-y"
            | "focus-ring.primary-rx"
            | "focus-ring.primary-ry"
            | "focus-ring.offset-x"
            | "focus-ring.offset-y"
    )
}

/// Friendly migration errors for config keys that moved between sections.
///
/// `decorations.shadows` was relocated to `effects.shadows`. Rather than emit a
/// confusing Levenshtein "unknown key" suggestion, point users at the new home.
fn deprecated_key_diagnostic(
    path: &str,
    raw: &str,
    line: usize,
    key: &str,
) -> Option<ConfigLoadDiagnostic> {
    if key == "decorations.shadows" || key.starts_with("decorations.shadows.") {
        let moved = key.replacen("decorations.shadows", "effects.shadows", 1);
        return Some(ConfigLoadDiagnostic {
            path: path.to_string(),
            line: Some(line),
            column: None,
            message: "decorations.shadows has moved to effects.shadows".to_string(),
            hint: Some(format!("Use `{moved}` instead.")),
            source_line: source_line(raw, line),
        });
    }
    None
}

fn unknown_key_diagnostic(
    path: &str,
    raw: &str,
    line: usize,
    key: &str,
    schema: &ConfigSchema,
) -> ConfigLoadDiagnostic {
    let parent = path_parent(key);
    let suggestion = best_suggestion(key, &schema.suggestions_for_parent(parent));
    ConfigLoadDiagnostic {
        path: path.to_string(),
        line: Some(line),
        column: None,
        message: format!("Unknown config key `{key}`"),
        hint: suggestion.map(|candidate| format!("Did you mean `{candidate}`?")),
        source_line: source_line(raw, line),
    }
}

fn best_suggestion(key: &str, candidates: &[&'static str]) -> Option<String> {
    let key_leaf = path_leaf(key);
    candidates
        .iter()
        .map(|candidate| (*candidate, levenshtein(key_leaf, path_leaf(candidate))))
        .filter(|(_, distance)| *distance <= 3)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs = (0..=b.chars().count()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let temp = costs[j + 1];
            costs[j + 1] = if ca == cb {
                previous
            } else {
                1 + previous.min(costs[j]).min(costs[j + 1])
            };
            previous = temp;
        }
    }
    *costs.last().unwrap_or(&0)
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && in_quotes {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if ch == '#' && !in_quotes {
            return &line[..idx];
        }
    }
    line
}

fn normalize_section_name(name: &str, stack: &[String]) -> String {
    let name = normalize_token(name);
    if stack.is_empty() {
        match name.as_str() {
            "animation" => "animations".to_string(),
            "node" => "nodes".to_string(),
            "overlay" => "overlays".to_string(),
            "screenshots" => "screenshot".to_string(),
            _ => name,
        }
    } else {
        name
    }
}

fn normalize_token(token: &str) -> String {
    token.trim().to_ascii_lowercase().replace('_', "-")
}

fn scalar_key(line: &str) -> String {
    line.split_whitespace()
        .next()
        .map(normalize_token)
        .unwrap_or_default()
}

fn path_with(stack: &[String], child: &str) -> String {
    if stack.is_empty() {
        child.to_string()
    } else {
        format!("{}.{}", stack.join("."), child)
    }
}

fn path_parent(path: &str) -> &str {
    path.rsplit_once('.')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn path_leaf(path: &str) -> &str {
    path.rsplit_once('.').map(|(_, leaf)| leaf).unwrap_or(path)
}

fn source_line(raw: &str, line: usize) -> Option<String> {
    raw.lines()
        .nth(line.saturating_sub(1))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::validate_known_config_keys;

    #[test]
    fn validation_rejects_unknown_overlay_key_with_suggestion() {
        let err = validate_known_config_keys(
            r#"
overlays:
  shap "rounded"
end
"#,
            "halley.rune",
        )
        .expect_err("unknown overlay key should fail");

        assert_eq!(err.line, Some(3));
        assert!(err.message.contains("overlays.shap"));
        assert_eq!(err.hint.as_deref(), Some("Did you mean `overlays.shape`?"));
    }

    #[test]
    fn validation_reports_decorations_shadows_migration() {
        let err = validate_known_config_keys(
            r#"
decorations:
  shadows:
    window:
      enabled true
    end
  end
end
"#,
            "halley.rune",
        )
        .expect_err("decorations.shadows should be rejected");

        assert!(
            err.message
                .contains("decorations.shadows has moved to effects.shadows")
        );
        assert_eq!(err.hint.as_deref(), Some("Use `effects.shadows` instead."));
    }

    #[test]
    fn validation_accepts_effects_block() {
        validate_known_config_keys(
            r#"
effects:
  blur:
    enabled true
    windows "auto"
  end
  shadows:
    window:
      enabled true
      blur-radius 8
    end
  end
end
"#,
            "halley.rune",
        )
        .expect("effects block should validate");
    }

    #[test]
    fn validation_accepts_custom_top_level_values() {
        validate_known_config_keys(
            r##"
pywal_background "#211c20"
overlays:
  background-colour pywal_background
end
"##,
            "halley.rune",
        )
        .expect("custom globals should be allowed");
    }

    #[test]
    fn validation_rejects_invalid_numeric_literal() {
        let err = validate_known_config_keys(
            r#"
cursor:
  size d
end
"#,
            "halley.rune",
        )
        .expect_err("invalid numeric literal should fail");

        assert_eq!(err.line, Some(3));
        assert!(err.message.contains("Invalid number `d` for `cursor.size`"));
    }

    #[test]
    fn validation_accepts_input_device_sections() {
        validate_known_config_keys(
            r#"
input:
  keyboard:
    layout "us"
    model ""
  end
  touchpad:
    tap true
    natural-scroll true
    dwt true
    accel-speed 0.3
    accel-profile "adaptive"
    scroll-method "two-finger"
    click-method "clickfinger"
    tap-button-map "left-right-middle"
    middle-emulation false
    left-handed false
    disabled-on-external-mouse false
    enabled true
  end
  mouse:
    natural-scroll false
    accel-profile "flat"
    scroll-button 274
  end
  devices:
    "Logitech MX Master 3":
      accel-speed 0.6
      natural-scroll true
    end
  end
end
"#,
            "halley.rune",
        )
        .expect("input device sections should validate");
    }

    #[test]
    fn validation_rejects_unknown_touchpad_key() {
        let err = validate_known_config_keys(
            r#"
input:
  touchpad:
    nonsense true
  end
end
"#,
            "halley.rune",
        )
        .expect_err("unknown touchpad key should fail");
        assert!(err.message.contains("input.touchpad.nonsense"));
    }

    #[test]
    fn validation_accepts_rendered_template() {
        // The shipped internal template must always pass its own schema validator. This is
        // the guard that the type structs/parser/template and this allowlist stay in sync.
        let rendered = crate::layout::RuntimeTuning::render_fresh_config(&[]);
        validate_known_config_keys(rendered.as_str(), "halley.rune")
            .expect("internal template must satisfy the config schema");
    }
}
