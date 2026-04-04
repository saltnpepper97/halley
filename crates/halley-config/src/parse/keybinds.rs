use std::collections::HashMap;

use rune_cfg::RuneConfig;

use crate::keybinds::{
    BearingsBindingAction, CompositorBinding, CompositorBindingAction, CompositorBindingScope,
    DirectionalAction, KeyModifiers, LaunchBinding, MonitorBindingAction, MonitorBindingTarget,
    NodeBindingAction, PointerBinding, PointerBindingAction, StackBindingAction,
    StackCycleDirection, TrailBindingAction, is_pointer_button_code, parse_chord, parse_modifiers,
};
use crate::layout::{RuntimeTuning, default_compositor_bindings, default_pointer_bindings};

pub(crate) fn parse_inline_keybinds(content: &str) -> HashMap<String, String> {
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

pub(crate) fn strip_inline_keybind_block(content: &str) -> String {
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

pub(crate) fn apply_explicit_keybind_overrides(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let Ok(Some(bindings)) = cfg.get_optional::<HashMap<String, String>>("keybinds") else {
        return;
    };
    apply_explicit_keybind_overrides_map(&bindings, out);
}

pub(crate) fn apply_explicit_keybind_overrides_map(
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
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::Reload,
            );
        }
        "toggle_state" | "toggle-state" | "minimize_focused" | "minimize-focused" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::ToggleState,
            );
        }
        "close_focused" | "close-focused" | "close_window" | "close-window" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::CloseFocusedWindow,
            );
        }
        "cluster_mode" | "cluster-mode" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::ClusterMode,
            );
        }
        "bearings_show" | "bearings-show" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::Bearings(BearingsBindingAction::Show),
            );
        }
        "bearings_toggle" | "bearings-toggle" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::Bearings(BearingsBindingAction::Toggle),
            );
        }
        "quit" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
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
                CompositorBindingScope::Field,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left)),
            );
        }
        "move_right" | "move-right" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Field,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Right)),
            );
        }
        "move_up" | "move-up" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Field,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Up)),
            );
        }
        "move_down" | "move-down" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Field,
                mods,
                key,
                CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Down)),
            );
        }
        "trail_prev" | "trail-prev" | "trail prev" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::Trail(TrailBindingAction::Prev),
            );
        }
        "trail_next" | "trail-next" | "trail next" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::Trail(TrailBindingAction::Next),
            );
        }
        "zoom_in" | "zoom-in" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::ZoomIn,
            );
        }
        "zoom_out" | "zoom-out" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::ZoomOut,
            );
        }
        "zoom_reset" | "zoom-reset" => {
            upsert_compositor_binding(
                out,
                CompositorBindingScope::Global,
                mods,
                key,
                CompositorBindingAction::ZoomReset,
            );
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
            if let Some((scope, action)) = parse_parameterized_compositor_action(action_trimmed) {
                upsert_compositor_binding(out, scope, mods, key, action);
            } else {
                upsert_launch_binding(out, mods, key, action_trimmed);
            }
        }
    }
}

fn parse_parameterized_compositor_action(
    action: &str,
) -> Option<(CompositorBindingScope, CompositorBindingAction)> {
    let mut parts = action.split_whitespace();
    let command = parts.next()?.to_ascii_lowercase();
    let arg = parts.collect::<Vec<_>>().join(" ");
    let arg = arg.trim();
    if arg.is_empty() {
        return None;
    }

    match command.as_str() {
        "node-move" | "node_move" => parse_directional_action(arg).map(|direction| {
            (
                CompositorBindingScope::Field,
                CompositorBindingAction::Node(NodeBindingAction::Move(direction)),
            )
        }),
        "monitor-focus" | "monitor_focus" => Some((
            CompositorBindingScope::Global,
            CompositorBindingAction::Monitor(MonitorBindingAction::Focus(
                parse_directional_action(arg)
                    .map(MonitorBindingTarget::Direction)
                    .unwrap_or_else(|| MonitorBindingTarget::Output(arg.to_string())),
            )),
        )),
        "stack-cycle" | "stack_cycle" => parse_stack_cycle_direction(arg).map(|direction| {
            (
                CompositorBindingScope::Stack,
                CompositorBindingAction::Stack(StackBindingAction::Cycle(direction)),
            )
        }),
        _ => None,
    }
}

fn parse_stack_cycle_direction(text: &str) -> Option<StackCycleDirection> {
    match text.trim().to_ascii_lowercase().as_str() {
        "forward" | "next" => Some(StackCycleDirection::Forward),
        "backward" | "back" | "prev" | "previous" => Some(StackCycleDirection::Backward),
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
    scope: CompositorBindingScope,
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
        .find(|b| b.scope == scope && b.key == key && b.modifiers == mods)
    {
        existing.action = action;
        return;
    }

    out.compositor_bindings.push(CompositorBinding {
        scope,
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
