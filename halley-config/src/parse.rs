use std::collections::HashMap;
use std::fmt;

use rune_cfg::ast::ObjectItem;
use rune_cfg::{RuneConfig, Value};

use crate::chord::parse_chord;
use crate::keybinds::{Action, BindingScope, Keybind, Keybinds, ModifierKey, MonitorTarget};

const KEY_MOD: &str = "mod";
const KEY_DEFAULT_TERMINAL_KEBAB: &str = "default-terminal";
const KEY_DEFAULT_TERMINAL_SNAKE: &str = "default_terminal";

#[derive(Debug)]
pub enum ParseError {
    Rune(rune_cfg::RuneError),
    UnknownModifier(String),
    InvalidChord(String),
    InvalidBinding { chord: String, message: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Rune(e) => write!(f, "config error: {e}"),
            ParseError::UnknownModifier(m) => write!(f, "unknown modifier key: {m:?}"),
            ParseError::InvalidChord(c) => write!(f, "invalid keybind chord: {c:?}"),
            ParseError::InvalidBinding { chord, message } => {
                write!(f, "invalid keybind {chord:?}: {message}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<rune_cfg::RuneError> for ParseError {
    fn from(e: rune_cfg::RuneError) -> Self {
        ParseError::Rune(e)
    }
}

fn parse_modifier_key(s: &str) -> Option<ModifierKey> {
    match s.to_lowercase().as_str() {
        "super" | "logo" | "mod4" => Some(ModifierKey::Super),
        "lsuper" | "left-super" | "lwin" | "left-logo" => Some(ModifierKey::LeftSuper),
        "rsuper" | "right-super" | "rwin" | "right-logo" => Some(ModifierKey::RightSuper),
        "alt" => Some(ModifierKey::Alt),
        "lalt" | "left-alt" => Some(ModifierKey::LeftAlt),
        "ralt" | "right-alt" => Some(ModifierKey::RightAlt),
        "ctrl" | "control" => Some(ModifierKey::Ctrl),
        "lctrl" | "left-ctrl" | "left-control" => Some(ModifierKey::LeftCtrl),
        "rctrl" | "right-ctrl" | "right-control" => Some(ModifierKey::RightCtrl),
        "shift" => Some(ModifierKey::Shift),
        "lshift" | "left-shift" => Some(ModifierKey::LeftShift),
        "rshift" | "right-shift" => Some(ModifierKey::RightShift),
        _ => None,
    }
}

fn parse_direction(value: &str) -> Option<crate::Direction> {
    match value {
        "left" => Some(crate::Direction::Left),
        "right" => Some(crate::Direction::Right),
        "up" => Some(crate::Direction::Up),
        "down" => Some(crate::Direction::Down),
        _ => None,
    }
}

pub(crate) fn parse_action(s: &str) -> Action {
    let words = s.split_whitespace().collect::<Vec<_>>();
    if let ["cluster", "slot", slot] = words.as_slice()
        && let Ok(slot) = slot.parse::<u8>()
        && (1..=10).contains(&slot)
    {
        return Action::ClusterSlot(slot);
    }
    if let Some(slot) = s
        .strip_prefix("cluster-slot-")
        .or_else(|| s.strip_prefix("cluster_slot_"))
        .and_then(|slot| slot.parse::<u8>().ok())
        .filter(|slot| (1..=10).contains(slot))
    {
        return Action::ClusterSlot(slot);
    }
    if let ["cluster", "focus", direction]
    | ["cluster-focus", direction]
    | ["tile", "focus", direction]
    | ["tile-focus", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::ClusterTileFocus(direction);
    }
    if let Some(direction) = s
        .strip_prefix("cluster-focus-")
        .or_else(|| s.strip_prefix("cluster_focus_"))
        .or_else(|| s.strip_prefix("cluster-tile-focus-"))
        .or_else(|| s.strip_prefix("cluster_tile_focus_"))
        .and_then(parse_direction)
    {
        return Action::ClusterTileFocus(direction);
    }
    if let ["focus", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::FocusDirection(direction);
    }
    if let Some(direction) = s
        .strip_prefix("focus-")
        .or_else(|| s.strip_prefix("focus_"))
        .and_then(parse_direction)
    {
        return Action::FocusDirection(direction);
    }
    if let ["window-transfer", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::TransferWindow(direction);
    }
    if let ["pan-field", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::PanField(direction);
    }
    if let ["node", "move", direction] | ["node-move", direction] | ["move", direction] =
        words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::MoveNode(direction);
    }
    if let ["resize", direction] | ["resize-window", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::ResizeWindow(direction);
    }
    if let Some(direction) = s
        .strip_prefix("resize-window-")
        .or_else(|| s.strip_prefix("resize_window_"))
        .and_then(parse_direction)
    {
        return Action::ResizeWindow(direction);
    }
    if let Some(direction) = s
        .strip_prefix("node-move-")
        .or_else(|| s.strip_prefix("node_move_"))
        .or_else(|| s.strip_prefix("move-"))
        .or_else(|| s.strip_prefix("move_"))
        .and_then(parse_direction)
    {
        return Action::MoveNode(direction);
    }
    if let ["tile", "swap", direction] | ["tile-swap", direction] = words.as_slice()
        && let Some(direction) = parse_direction(direction)
    {
        return Action::ClusterTileSwap(direction);
    }
    if let ["monitor", "focus", target] | ["monitor-focus", target] = words.as_slice() {
        return Action::MonitorFocus(
            parse_direction(target)
                .map(MonitorTarget::Direction)
                .unwrap_or_else(|| MonitorTarget::Output((*target).to_string())),
        );
    }
    if let Some(direction) = s
        .strip_prefix("monitor-focus-")
        .or_else(|| s.strip_prefix("monitor_focus_"))
        .and_then(parse_direction)
    {
        return Action::MonitorFocus(MonitorTarget::Direction(direction));
    }
    if let Some(direction) = s
        .strip_prefix("cluster-tile-swap-")
        .or_else(|| s.strip_prefix("cluster_tile_swap_"))
        .and_then(parse_direction)
    {
        return Action::ClusterTileSwap(direction);
    }
    match s {
        "quit" => Action::Quit,
        "close-focused" | "close_focused" | "close-window" | "close_window" => {
            Action::CloseFocusedWindow
        }
        "toggle-fullscreen" | "toggle_fullscreen" | "fullscreen" => Action::ToggleFullscreen,
        "maximize-focused" | "maximize_focused" | "toggle-maximize" | "toggle_maximize" => {
            Action::ToggleFieldMaximize
        }
        "toggle-state" | "toggle_state" => Action::ToggleState,
        "toggle-pin" | "toggle_pin" | "pin-toggle" | "pin_toggle" | "toggle-focused-pin"
        | "toggle_focused_pin" => Action::ToggleFocusedPin,
        "apogee" | "overview" => Action::Apogee,
        "bearings-show" | "bearings_show" => Action::BearingsShow,
        "bearings-toggle" | "bearings_toggle" => Action::BearingsToggle,
        "cycle-focus" | "cycle_focus" => Action::FocusCycle(crate::FocusCycleDirection::Forward),
        "cycle-focus-backward" | "cycle_focus_backward" => {
            Action::FocusCycle(crate::FocusCycleDirection::Backward)
        }
        "trail-prev" | "trail_prev" | "trail-previous" | "trail_previous" => {
            Action::Trail(crate::TrailDirection::Previous)
        }
        "trail-next" | "trail_next" => Action::Trail(crate::TrailDirection::Next),
        "center-last-focused" | "center_last_focused" => Action::CenterLastFocused,
        "cluster-mode" | "cluster_mode" => Action::ClusterMode,
        "cluster-layout cycle" | "cluster-layout-cycle" | "cluster_layout_cycle" => {
            Action::ClusterLayoutCycle
        }
        "cluster-toggle-float" | "cluster_toggle_float" => Action::ClusterToggleFloat,
        "arrange-visible" | "arrange_visible" => Action::ArrangeVisible,
        "undo-arrange" | "undo_arrange" => Action::UndoArrange,
        "move-window" | "move_window" => Action::PointerMoveWindow,
        "resize-window" | "resize_window" => Action::PointerResizeWindow,
        "pan-field" | "pan_field" => Action::PointerPanField,
        "pan-field-always" | "pan_field_always" => Action::PointerPanFieldAlways,
        "drag-pan" | "drag_pan" | "field-jump" | "field_jump" => Action::PointerDragPan,
        "reload" | "reload-config" | "reload_config" => Action::Reload,
        "open-terminal" | "open_terminal" | "default-terminal" | "default_terminal" => {
            Action::OpenTerminal
        }
        "zoom-in" | "zoom_in" => Action::ZoomIn,
        "zoom-out" | "zoom_out" => Action::ZoomOut,
        "zoom-reset" | "zoom_reset" => Action::ZoomReset,
        "screenshot" => Action::Screenshot,
        command => Action::Spawn(command.to_string()),
    }
}

/// Parse the `keybinds:` section of a loaded `RuneConfig` into a `Keybinds`
/// value.
///
/// Values are compact action strings with an optional inline `repeat`
/// attribute. The older one-line object form remains accepted for config
/// compatibility.
///
/// Chord keys reference the modifier via a literal `$var.mod` substring
/// (e.g. `"$var.mod+shift+e"`) - rune-cfg's `$var` resolution only applies
/// to *values*, never object *keys* (confirmed by reading its resolver:
/// keys are plain, unprocessed `String`s in the AST), so this substitutes
/// `$var.mod` with the already-parsed modifier string itself before
/// chord-parsing, rather than relying on rune-cfg to do it.
pub fn parse_keybinds(config: &RuneConfig) -> Result<Keybinds, ParseError> {
    let root = config.get_value("")?;
    let Value::Object(root) = root else {
        return Err(ParseError::InvalidBinding {
            chord: "keybinds".to_string(),
            message: "configuration root must be an object".to_string(),
        });
    };
    let section = root.into_iter().find_map(|item| match item {
        ObjectItem::Assign(key, Value::Object(fields)) if key == "keybinds" => Some(fields),
        _ => None,
    });
    let fields = section.ok_or_else(|| ParseError::InvalidBinding {
        chord: "keybinds".to_string(),
        message: "missing required keybinds section".to_string(),
    })?;

    let modifier_str = fields
        .iter()
        .filter_map(|item| match item {
            ObjectItem::Assign(key, value) if key == KEY_MOD => Some(value),
            _ => None,
        })
        .next_back()
        .map(|value| binding_string(KEY_MOD, value))
        .transpose()?
        .map(str::to_owned)
        .unwrap_or_else(|| "super".to_string());
    let modifier = parse_modifier_key(&modifier_str)
        .ok_or_else(|| ParseError::UnknownModifier(modifier_str.clone()))?;
    if fields.iter().any(|item| {
        matches!(
            item,
            ObjectItem::Assign(key, _)
                if key == KEY_DEFAULT_TERMINAL_KEBAB || key == KEY_DEFAULT_TERMINAL_SNAKE
        )
    }) {
        return Err(ParseError::InvalidBinding {
            chord: KEY_DEFAULT_TERMINAL_KEBAB.to_string(),
            message: "default-terminal is now an action, not a setting; bind it to a chord or replace that chord with the command you want to run".to_string(),
        });
    }

    let entries = fields.iter().filter_map(|item| match item {
        ObjectItem::Assign(key, value) if key != KEY_MOD => Some((key, value)),
        _ => None,
    });
    let mut binds = Vec::new();
    for (chord_key, value) in entries {
        let resolved_chord = chord_key.replace("$var.mod", &modifier_str);
        let (modifiers, key) = parse_chord(&resolved_chord)
            .ok_or_else(|| ParseError::InvalidChord(chord_key.clone()))?;
        let (action, repeat_override, scope_override) = parse_binding(chord_key, value)?;
        if repeat_override == Some(true) && is_pointer_trigger(&key) {
            return Err(ParseError::InvalidBinding {
                chord: chord_key.clone(),
                message: "repeat true is only valid for keyboard triggers".to_string(),
            });
        }
        let repeat = repeat_override.unwrap_or_else(|| action.repeats_by_default());
        binds.push(Keybind {
            scope: scope_override.unwrap_or_else(|| action.default_scope()),
            modifiers,
            key,
            action,
            repeat,
        });
    }

    Ok(Keybinds { modifier, binds })
}

fn binding_string<'a>(field: &str, value: &'a Value) -> Result<&'a str, ParseError> {
    let Value::String(value) = value else {
        return Err(ParseError::InvalidBinding {
            chord: field.to_string(),
            message: "expected a string".to_string(),
        });
    };
    Ok(value)
}

fn parse_binding(
    chord: &str,
    value: &Value,
) -> Result<(Action, Option<bool>, Option<BindingScope>), ParseError> {
    if let Value::String(action) = value {
        return Ok((parse_action(action), None, None));
    }
    if let Value::Annotated(binding) = value {
        let action = binding_string(chord, &binding.value)?;
        if let Some((field, _)) = binding
            .attributes
            .iter()
            .find(|(field, _)| field != "repeat" && field != "scope")
        {
            return Err(ParseError::InvalidBinding {
                chord: chord.to_string(),
                message: format!("unsupported inline attribute {field:?}"),
            });
        }
        let repeat_fields = binding
            .attributes
            .iter()
            .filter(|(field, _)| field == "repeat")
            .collect::<Vec<_>>();
        if repeat_fields.len() > 1 {
            return Err(ParseError::InvalidBinding {
                chord: chord.to_string(),
                message: "duplicate inline attribute \"repeat\"".to_string(),
            });
        }
        let repeat = match repeat_fields.first().map(|(_, value)| value) {
            None => None,
            Some(Value::Bool(repeat)) => Some(*repeat),
            Some(_) => {
                return Err(ParseError::InvalidBinding {
                    chord: chord.to_string(),
                    message: "repeat must be a boolean".to_string(),
                });
            }
        };
        let scope = parse_inline_scope(chord, &binding.attributes)?;
        return Ok((parse_action(action), repeat, scope));
    }
    let Value::Object(items) = value else {
        return Err(ParseError::InvalidBinding {
            chord: chord.to_string(),
            message: "expected an action string optionally followed by `with repeat true|false`"
                .to_string(),
        });
    };
    let fields: HashMap<String, Value> = Value::Object(items.clone()).try_into()?;
    if let Some(field) = fields
        .keys()
        .find(|field| !matches!(field.as_str(), "action" | "repeat" | "scope"))
    {
        return Err(ParseError::InvalidBinding {
            chord: chord.to_string(),
            message: format!("unsupported field {field:?}"),
        });
    }
    let action = fields
        .get("action")
        .ok_or_else(|| ParseError::InvalidBinding {
            chord: chord.to_string(),
            message: "missing required action field".to_string(),
        })
        .and_then(|value| binding_string(chord, value))?;
    let repeat = match fields.get("repeat") {
        None => None,
        Some(Value::Bool(repeat)) => Some(*repeat),
        Some(_) => {
            return Err(ParseError::InvalidBinding {
                chord: chord.to_string(),
                message: "repeat must be a boolean".to_string(),
            });
        }
    };
    let scope = fields
        .get("scope")
        .map(|value| binding_string(chord, value).and_then(|scope| parse_scope(chord, scope)))
        .transpose()?;
    Ok((parse_action(action), repeat, scope))
}

fn parse_inline_scope(
    chord: &str,
    attributes: &[(String, Value)],
) -> Result<Option<BindingScope>, ParseError> {
    let scopes = attributes
        .iter()
        .filter(|(field, _)| field == "scope")
        .collect::<Vec<_>>();
    if scopes.len() > 1 {
        return Err(ParseError::InvalidBinding {
            chord: chord.to_string(),
            message: "duplicate inline attribute \"scope\"".to_string(),
        });
    }
    scopes
        .first()
        .map(|(_, value)| binding_string(chord, value).and_then(|scope| parse_scope(chord, scope)))
        .transpose()
}

fn parse_scope(chord: &str, scope: &str) -> Result<BindingScope, ParseError> {
    match scope {
        "global" => Ok(BindingScope::Global),
        "field" => Ok(BindingScope::Field),
        "cluster" => Ok(BindingScope::Cluster),
        "tile" | "tiling" => Ok(BindingScope::Tile),
        "stack" | "stacking" => Ok(BindingScope::Stack),
        _ => Err(ParseError::InvalidBinding {
            chord: chord.to_string(),
            message: format!("scope must be global, field, cluster, tile, or stack, got {scope:?}"),
        }),
    }
}

fn is_pointer_trigger(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("click-") || key.starts_with("button-") || key.starts_with("scroll-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Keybinds {
        let config = RuneConfig::from_str(src).expect("valid rune-cfg source");
        parse_keybinds(&config).expect("valid keybinds section")
    }

    #[test]
    fn parses_mod_and_three_binds() {
        let kb = parse(
            r#"
keybinds:
  mod "super"

  "$var.mod+shift+e" "quit"
  "$var.mod+c" "close-focused"
  "$var.mod+t" "open-terminal"
end
"#,
        );

        assert_eq!(kb.modifier, ModifierKey::Super);
        assert_eq!(kb.binds.len(), 3);

        let quit = kb.binds.iter().find(|b| b.action == Action::Quit).unwrap();
        assert_eq!(quit.key, "e");
        assert!(quit.modifiers.super_key);
        assert!(quit.modifiers.shift);
    }

    #[test]
    fn accepts_close_window_alias() {
        let kb = parse(
            r#"
keybinds:
  mod "alt"
  "$var.mod+q" "close-window"
end
"#,
        );
        assert_eq!(kb.modifier, ModifierKey::Alt);
        let close = kb
            .binds
            .iter()
            .find(|b| b.action == Action::CloseFocusedWindow)
            .unwrap();
        assert_eq!(close.key, "q");
        assert!(close.modifiers.alt);
        assert!(!close.modifiers.super_key);
    }

    #[test]
    fn accepts_fullscreen_action_aliases() {
        for action in ["toggle-fullscreen", "toggle_fullscreen", "fullscreen"] {
            let kb = parse(&format!(
                "keybinds:\n  mod \"super\"\n  \"$var.mod+f\" \"{action}\"\nend\n"
            ));
            assert!(kb.binds.iter().any(|bind| {
                bind.key == "f"
                    && bind.modifiers.super_key
                    && bind.action == Action::ToggleFullscreen
            }));
        }
    }

    #[test]
    fn accepts_field_maximize_action_aliases() {
        for action in [
            "maximize-focused",
            "maximize_focused",
            "toggle-maximize",
            "toggle_maximize",
        ] {
            let kb = parse(&format!(
                "keybinds:\n  mod \"super\"\n  \"$var.mod+m\" \"{action}\"\nend\n"
            ));
            assert!(kb.binds.iter().any(|bind| {
                bind.key == "m"
                    && bind.modifiers.super_key
                    && bind.action == Action::ToggleFieldMaximize
            }));
        }
    }

    #[test]
    fn accepts_old_halley_pin_action_aliases() {
        for action in [
            "toggle-pin",
            "toggle_pin",
            "pin-toggle",
            "pin_toggle",
            "toggle-focused-pin",
            "toggle_focused_pin",
        ] {
            let kb = parse(&format!(
                "keybinds:\n  mod \"super\"\n  \"$var.mod+p\" \"{action}\"\nend\n"
            ));
            let pin = kb
                .binds
                .iter()
                .find(|bind| bind.action == Action::ToggleFocusedPin)
                .expect("pin action parsed");
            assert_eq!(pin.key, "p");
            assert!(!pin.repeat);
        }
    }

    #[test]
    fn accepts_zoom_in_zoom_out_and_zoom_reset_actions() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+equal" "zoom-in"
  "$var.mod+minus" "zoom-out"
  "$var.mod+click-middle" "zoom_reset"
end
"#,
        );
        let zoom_in = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomIn)
            .unwrap();
        assert_eq!(zoom_in.key, "equal");
        let zoom_out = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomOut)
            .unwrap();
        assert_eq!(zoom_out.key, "minus");
        let zoom_reset = kb
            .binds
            .iter()
            .find(|b| b.action == Action::ZoomReset)
            .unwrap();
        assert_eq!(zoom_reset.key, "click-middle");
        assert!(zoom_reset.modifiers.super_key);
    }

    #[test]
    fn accepts_old_field_node_move_spellings() {
        for (source, direction) in [
            ("node-move left", crate::Direction::Left),
            ("node move right", crate::Direction::Right),
            ("node-move-up", crate::Direction::Up),
            ("move-down", crate::Direction::Down),
        ] {
            let kb = parse(&format!(
                "keybinds:\n  mod \"super\"\n  \"$var.mod+alt+x\" \"{source}\"\nend\n"
            ));
            assert_eq!(kb.binds[0].action, Action::MoveNode(direction));
        }
    }

    #[test]
    fn compact_bindings_use_action_aware_repeat_defaults() {
        let kb = parse(
            r#"keybinds:
  mod "super"
  "$var.mod+left" "node-move left"
  "$var.mod+t" "open-terminal"
  "$var.mod+v" "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"
end
"#,
        );
        assert!(
            kb.binds
                .iter()
                .find(|bind| bind.key == "left")
                .unwrap()
                .repeat
        );
        assert!(!kb.binds.iter().find(|bind| bind.key == "t").unwrap().repeat);
        assert!(!kb.binds.iter().find(|bind| bind.key == "v").unwrap().repeat);
    }

    #[test]
    fn inline_with_attributes_override_repeat() {
        let kb = parse(
            r#"keybinds:
  mod "super"
  "$var.mod+left" "node-move left" with repeat false
  "$var.mod+t" "open-terminal" with repeat true
end
"#,
        );
        assert!(
            !kb.binds
                .iter()
                .find(|bind| bind.key == "left")
                .unwrap()
                .repeat
        );
        assert!(kb.binds.iter().find(|bind| bind.key == "t").unwrap().repeat);
    }

    #[test]
    fn legacy_one_line_binding_objects_still_override_repeat() {
        let kb = parse(
            r#"keybinds:
  mod "super"
  "$var.mod+left": action "node-move left" repeat false end
end
"#,
        );
        assert!(!kb.binds[0].repeat);
    }

    #[test]
    fn inline_attributes_validate_fields_and_keyboard_only_repeat() {
        for (binding, expected) in [
            (
                r#""$var.mod+x" "zoom-in" with repeat "yes""#,
                "repeat must be a boolean",
            ),
            (
                r#""$var.mod+x" "zoom-in" with cooldown 20"#,
                "unsupported inline attribute",
            ),
            (
                r#""$var.mod+x" "zoom-in" with repeat true repeat false"#,
                "duplicate inline attribute",
            ),
            (
                r#""$var.mod+scroll-up" "zoom-in" with repeat true"#,
                "only valid for keyboard triggers",
            ),
        ] {
            let config =
                RuneConfig::from_str(&format!("keybinds:\n  mod \"super\"\n  {binding}\nend\n"))
                    .expect("valid Rune syntax");
            let error = parse_keybinds(&config).unwrap_err().to_string();
            assert!(error.contains(expected), "{error:?}");
        }
    }

    #[test]
    fn parses_cluster_actions() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+shift+c" "cluster-mode"
  "$var.mod+l" "cluster-layout cycle"
  "$var.mod+v" "cluster-toggle-float"
  "$var.mod+1" "cluster slot 1"
  "$var.mod+left" "cluster-focus-left"
  "$var.mod+ctrl+right" "tile swap right"
  "$var.mod+shift+up" "monitor-focus up"
end
"#,
        );

        for expected in [
            Action::ClusterMode,
            Action::ClusterLayoutCycle,
            Action::ClusterToggleFloat,
            Action::ClusterSlot(1),
            Action::ClusterTileFocus(crate::ClusterDirection::Left),
            Action::ClusterTileSwap(crate::ClusterDirection::Right),
            Action::MonitorFocus(crate::MonitorTarget::Direction(crate::Direction::Up)),
        ] {
            assert!(kb.binds.iter().any(|bind| bind.action == expected));
        }
    }

    #[test]
    fn parses_contextual_direction_and_center_actions() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+left" "focus-left"
  "$var.mod+right" "focus right"
  "$var.mod+h" "center-last-focused"
end
"#,
        );

        for expected in [
            Action::FocusDirection(crate::Direction::Left),
            Action::FocusDirection(crate::Direction::Right),
            Action::CenterLastFocused,
        ] {
            assert!(kb.binds.iter().any(|bind| bind.action == expected));
        }
    }

    #[test]
    fn duplicate_chords_are_retained_for_context_scopes() {
        let kb = parse(
            r#"
keybinds:
  mod "lsuper"
  "$var.mod+ctrl+left" "resize-window-left"
  "$var.mod+ctrl+left" "cluster-tile-swap-left"
  "$var.mod+x" "quit" with scope "stack"
end
"#,
        );
        assert_eq!(kb.modifier, ModifierKey::LeftSuper);
        assert_eq!(kb.binds.len(), 3);
        assert_eq!(kb.binds[0].scope, BindingScope::Field);
        assert_eq!(
            kb.binds[0].action,
            Action::ResizeWindow(crate::Direction::Left)
        );
        assert!(kb.binds[0].modifiers.left_super);
        assert_eq!(kb.binds[1].scope, BindingScope::Tile);
        assert_eq!(
            kb.binds[1].action,
            Action::ClusterTileSwap(crate::Direction::Left)
        );
        assert_eq!(kb.binds[2].scope, BindingScope::Stack);
    }

    #[test]
    fn parses_named_monitor_focus_and_reload() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+1" "monitor focus DP-1"
  "$var.mod+shift+r" "reload"
end
"#,
        );
        assert!(kb.binds.iter().any(|bind| {
            bind.action == Action::MonitorFocus(MonitorTarget::Output("DP-1".into()))
        }));
        assert!(kb.binds.iter().any(|bind| bind.action == Action::Reload));
    }

    #[test]
    fn parses_arrange_and_undo_actions() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+a" "arrange-visible"
  "$var.mod+shift+a" "undo_arrange"
end
"#,
        );
        assert_eq!(kb.binds[0].action, Action::ArrangeVisible);
        assert_eq!(kb.binds[0].scope, BindingScope::Field);
        assert!(!kb.binds[0].repeat);
        assert_eq!(kb.binds[1].action, Action::UndoArrange);
        assert_eq!(kb.binds[1].scope, BindingScope::Field);
        assert!(!kb.binds[1].repeat);
    }

    #[test]
    fn parses_remappable_compositor_pointer_actions() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+click-left" "move-window"
  "$var.mod+click-right" "resize-window"
  "$var.mod+shift+click-left" "drag-pan"
  "click-left" "pan-field"
end
"#,
        );
        assert_eq!(kb.binds[0].action, Action::PointerMoveWindow);
        assert_eq!(kb.binds[1].action, Action::PointerResizeWindow);
        assert_eq!(kb.binds[2].action, Action::PointerDragPan);
        assert_eq!(kb.binds[2].scope, BindingScope::Field);
        assert_eq!(kb.binds[3].action, Action::PointerPanField);
        assert_eq!(kb.binds[3].scope, BindingScope::Field);

        for alias in ["drag_pan", "field-jump", "field_jump"] {
            assert_eq!(parse_action(alias), Action::PointerDragPan);
        }
    }

    #[test]
    fn parses_pan_field_always_as_a_distinct_pointer_action() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+click-middle" "pan-field-always"
  "$var.mod+shift+click-back" "pan_field_always"
  "click-left" "pan-field"
  "$var.mod+shift+h" "pan-field left"
end
"#,
        );
        assert_eq!(kb.binds[0].action, Action::PointerPanFieldAlways);
        assert_eq!(kb.binds[0].key, "click-middle");
        assert!(kb.binds[0].modifiers.super_key);
        assert_eq!(kb.binds[0].scope, BindingScope::Field);
        assert!(!kb.binds[0].repeat);
        assert_eq!(kb.binds[1].action, Action::PointerPanFieldAlways);
        assert_eq!(kb.binds[1].key, "click-back");
        assert!(kb.binds[1].modifiers.super_key && kb.binds[1].modifiers.shift);
        // The new name never shadows the existing empty-background drag or the
        // directional keyboard form.
        assert_eq!(kb.binds[2].action, Action::PointerPanField);
        assert_eq!(kb.binds[3].action, Action::PanField(crate::Direction::Left));
        assert_ne!(
            parse_action("pan-field-always"),
            parse_action("pan-field"),
            "always-pan must stay a separate, explicitly requested action"
        );
    }

    #[test]
    fn accepts_screenshot_action() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "Print" "screenshot"
end
"#,
        );
        assert!(kb.binds.iter().any(|bind| {
            bind.key == "Print"
                && bind.modifiers == crate::Modifiers::default()
                && bind.action == Action::Screenshot
        }));
    }

    #[test]
    fn legacy_default_terminal_reports_the_keybind_migration() {
        let config = RuneConfig::from_str(
            r#"
keybinds:
  mod "super"
  default-terminal "foot"
end
"#,
        )
        .unwrap();
        let error = parse_keybinds(&config).unwrap_err().to_string();
        assert!(error.contains("default-terminal is now an action"));
        assert!(error.contains("bind it to a chord"));
    }

    #[test]
    fn unknown_modifier_name_errors() {
        let config = RuneConfig::from_str(
            r#"
keybinds:
  mod "windows"
end
"#,
        )
        .unwrap();
        assert!(matches!(
            parse_keybinds(&config),
            Err(ParseError::UnknownModifier(_))
        ));
    }

    #[test]
    fn non_builtin_action_is_a_command_line() {
        let kb = parse(
            r#"
keybinds:
  mod "super"
  "$var.mod+d" "fuzzel"
  "$var.mod+shift+s" "grim -g \"$(slurp)\" ~/shot.png"
end
"#,
        );

        assert!(
            kb.binds.iter().any(|bind| {
                bind.key == "d" && bind.action == Action::Spawn("fuzzel".to_string())
            })
        );
        assert!(kb.binds.iter().any(|bind| {
            bind.key == "s"
                && bind.action == Action::Spawn("grim -g \"$(slurp)\" ~/shot.png".to_string())
        }));
    }

    #[test]
    fn missing_keybinds_section_errors() {
        let config = RuneConfig::from_str("other: \n foo \"bar\"\nend\n").unwrap();
        assert!(parse_keybinds(&config).is_err());
    }
}
