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

/// How an older action spelling is recognised.
#[derive(Clone, Copy)]
pub(crate) enum OldSpelling {
    /// The whole action string, exactly as written.
    Name(&'static str),
    /// Leading words followed by more, ignoring repeated spaces: `cluster
    /// focus` matches `cluster  focus left`.
    Words(&'static str),
    /// A leading word that must be followed by a direction: `move` matches
    /// `move left 10` but `move foo` stays a shell command.
    WordsThenDirection(&'static str),
    /// A prefix of the first word that must be followed by a direction:
    /// `move-` matches `move-left 10`.
    WordPrefixThenDirection(&'static str),
    /// A prefix of the action string, exactly as written.
    Prefix(&'static str),
    /// A prefix of the action string, exactly as written, that must be
    /// followed by a direction.
    PrefixThenDirection(&'static str),
}

/// Every older action spelling Halley still accepts, with the current
/// spelling it stands for. A current spelling ending in `-` is joined to the
/// rest of the action directly (`cluster slot 3` becomes `cluster-slot-3`);
/// otherwise with a space.
///
/// `snake_case` needs no entry: an action whose first word is written with
/// `_` reads as the same action written with `-`.
pub(crate) const ACTION_ALIASES: &[(OldSpelling, &str)] = {
    use OldSpelling::*;
    &[
        (Name("close-window"), "close-focused"),
        (Name("fullscreen"), "toggle-fullscreen"),
        (Name("toggle-maximize"), "maximize-focused"),
        (Name("toggle-pin"), "toggle-focused-pin"),
        (Name("pin-toggle"), "toggle-focused-pin"),
        (Name("overview"), "zoom-fit"),
        (Name("apogee"), "zoom-fit"),
        (Name("arrange-visible"), "tidy"),
        (Name("undo-arrange"), "undo-tidy"),
        (Name("trail-previous"), "trail-prev"),
        (Name("cluster-layout cycle"), "cluster-layout-cycle"),
        (Name("field-jump"), "drag-pan"),
        (Name("reload-config"), "reload"),
        (Name("open-terminal"), "default-terminal"),
        (Words("cluster slot"), "cluster-slot-"),
        (Words("cluster focus"), "cluster-focus-"),
        (Words("cluster-focus"), "cluster-focus-"),
        (Words("tile focus"), "cluster-focus-"),
        (Words("tile-focus"), "cluster-focus-"),
        (Prefix("cluster-tile-focus-"), "cluster-focus-"),
        (Words("focus"), "focus-"),
        (Name("node move"), "node-move"),
        (Words("node move"), "node-move"),
        (WordsThenDirection("move"), "node-move"),
        (WordPrefixThenDirection("move-"), "node-move"),
        (WordPrefixThenDirection("node-move-"), "node-move"),
        (Words("resize"), "resize-window-"),
        (Words("resize-window"), "resize-window-"),
        (Words("tile swap"), "cluster-tile-swap-"),
        (Words("tile-swap"), "cluster-tile-swap-"),
        (Words("monitor focus"), "monitor-focus"),
        (PrefixThenDirection("monitor-focus-"), "monitor-focus"),
    ]
};

/// `action` rewritten from an older spelling into the current one, if it is
/// written in one.
fn current_spelling(action: &str) -> Option<String> {
    let words = action.split_whitespace().collect::<Vec<_>>().join(" ");
    let join = |current: &str, rest: &str| {
        let rest = rest.trim_start();
        if current.ends_with('-') || rest.is_empty() {
            format!("{current}{rest}")
        } else {
            format!("{current} {rest}")
        }
    };
    let starts_with_direction = |rest: &str| {
        rest.split_whitespace()
            .next()
            .is_some_and(|word| parse_direction(word).is_some())
    };
    let leading_words = |old: &str| words.strip_prefix(old).filter(|rest| rest.starts_with(' '));
    ACTION_ALIASES.iter().find_map(|(old, current)| match *old {
        OldSpelling::Name(old) => (action == old).then(|| current.to_string()),
        OldSpelling::Words(old) => leading_words(old).map(|rest| join(current, rest)),
        OldSpelling::WordsThenDirection(old) => leading_words(old)
            .filter(|rest| starts_with_direction(rest))
            .map(|rest| join(current, rest)),
        OldSpelling::WordPrefixThenDirection(old) => words
            .strip_prefix(old)
            .filter(|rest| starts_with_direction(rest))
            .map(|rest| join(current, rest)),
        OldSpelling::Prefix(old) => action.strip_prefix(old).map(|rest| join(current, rest)),
        OldSpelling::PrefixThenDirection(old) => action
            .strip_prefix(old)
            .filter(|rest| starts_with_direction(rest))
            .map(|rest| join(current, rest)),
    })
}

/// `node-move DIRECTION [UNITS]`, or `None` for any other action.
fn movement_action(s: &str) -> Option<Result<Action, String>> {
    let (direction, arguments) = match s.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["node-move"] => {
            return Some(Err("node movement requires left, right, up, or down".into()));
        }
        ["node-move", direction, arguments @ ..] => (*direction, arguments.to_vec()),
        _ => return None,
    };
    let direction = match parse_direction(direction) {
        Some(direction) => direction,
        None => return Some(Err("node movement requires left, right, up, or down".into())),
    };
    Some(match arguments.as_slice() {
        [] => Ok(Action::MoveNode(direction)),
        [distance] => distance
            .parse::<i32>()
            .ok()
            .filter(|n| *n > 0)
            .map(|distance| Action::MoveNodeBy {
                direction,
                distance: distance as u32,
            })
            .ok_or_else(|| {
                "movement distance must be a whole number in 1..=2147483647 Field units".into()
            }),
        _ => Err("node movement accepts one optional distance in Field units".into()),
    })
}

/// Parse a keybind action. Anything that is not a built-in action, in its
/// current or an older spelling, is a shell command.
pub(crate) fn parse_action_checked(s: &str) -> Result<Action, String> {
    let parsed = parse_spelling(s);
    if matches!(parsed, Ok(Action::Spawn(_))) {
        // `toggle_state` reads as `toggle-state`, but a command that merely
        // contains `_`, like `my_script`, stays a command.
        let dashed = dash_first_word(s);
        if dashed != s
            && let Ok(action) = parse_spelling(&dashed)
            && !matches!(action, Action::Spawn(_))
        {
            return Ok(action);
        }
        return Ok(Action::Spawn(s.to_string()));
    }
    parsed
}

#[cfg(test)]
pub(crate) fn parse_action(s: &str) -> Action {
    parse_action_checked(s).unwrap_or_else(|_| Action::Spawn(s.to_string()))
}

fn parse_spelling(s: &str) -> Result<Action, String> {
    let current = current_spelling(s);
    let s = current.as_deref().unwrap_or(s);
    movement_action(s).unwrap_or_else(|| Ok(parse_current(s)))
}

fn dash_first_word(s: &str) -> String {
    let start = s.len() - s.trim_start().len();
    let end = s[start..]
        .find(char::is_whitespace)
        .map_or(s.len(), |end| start + end);
    format!(
        "{}{}{}",
        &s[..start],
        s[start..end].replace('_', "-"),
        &s[end..]
    )
}

/// A built-in action in its current spelling, or a shell command.
fn parse_current(s: &str) -> Action {
    let words = s.split_whitespace().collect::<Vec<_>>();
    if let Some(slot) = s
        .strip_prefix("cluster-slot-")
        .and_then(|slot| slot.parse::<u8>().ok())
        .filter(|slot| (1..=10).contains(slot))
    {
        return Action::ClusterSlot(slot);
    }
    let direction_after = |prefix: &str| s.strip_prefix(prefix).and_then(parse_direction);
    if let Some(direction) = direction_after("cluster-focus-") {
        return Action::ClusterTileFocus(direction);
    }
    if let Some(direction) = direction_after("focus-") {
        return Action::FocusDirection(direction);
    }
    if let Some(direction) = direction_after("resize-window-") {
        return Action::ResizeWindow(direction);
    }
    if let Some(direction) = direction_after("cluster-tile-swap-") {
        return Action::ClusterTileSwap(direction);
    }
    match words.as_slice() {
        ["window-transfer", direction] if let Some(direction) = parse_direction(direction) => {
            return Action::TransferWindow(direction);
        }
        ["pan-field", direction] if let Some(direction) = parse_direction(direction) => {
            return Action::PanField(direction);
        }
        ["monitor-focus", target] => {
            return Action::MonitorFocus(
                parse_direction(target)
                    .map(MonitorTarget::Direction)
                    .unwrap_or_else(|| MonitorTarget::Output((*target).to_string())),
            );
        }
        _ => {}
    }
    match s {
        "quit" => Action::Quit,
        "close-focused" => Action::CloseFocusedWindow,
        "toggle-fullscreen" => Action::ToggleFullscreen,
        "maximize-focused" => Action::ToggleFieldMaximize,
        "toggle-state" => Action::ToggleState,
        "toggle-focused-pin" => Action::ToggleFocusedPin,
        "zoom-fit" => Action::ZoomFit,
        "bearings-show" => Action::BearingsShow,
        "bearings-toggle" => Action::BearingsToggle,
        "cycle-focus" => Action::FocusCycle(crate::FocusCycleDirection::Forward),
        "cycle-focus-backward" => Action::FocusCycle(crate::FocusCycleDirection::Backward),
        "trail-prev" => Action::Trail(crate::TrailDirection::Previous),
        "trail-next" => Action::Trail(crate::TrailDirection::Next),
        "center-last-focused" => Action::CenterLastFocused,
        "cluster-mode" => Action::ClusterMode,
        "cluster-layout-cycle" => Action::ClusterLayoutCycle,
        "cluster-toggle-float" => Action::ClusterToggleFloat,
        "tidy" => Action::Tidy,
        "undo-tidy" => Action::UndoTidy,
        "move-window" => Action::PointerMoveWindow,
        "resize-window" => Action::PointerResizeWindow,
        "pan-field" => Action::PointerPanField,
        "pan-field-always" => Action::PointerPanFieldAlways,
        "drag-pan" => Action::PointerDragPan,
        "reload" => Action::Reload,
        "default-terminal" => Action::OpenTerminal,
        "zoom-in" => Action::ZoomIn,
        "zoom-out" => Action::ZoomOut,
        "zoom-reset" => Action::ZoomReset,
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
        return Ok((binding_action(chord, action)?, None, None));
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
        return Ok((binding_action(chord, action)?, repeat, scope));
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
    Ok((binding_action(chord, action)?, repeat, scope))
}

fn binding_action(chord: &str, action: &str) -> Result<Action, ParseError> {
    parse_action_checked(action).map_err(|message| ParseError::InvalidBinding {
        chord: chord.to_string(),
        message,
    })
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
    fn movement_distances_repeat_in_field_and_accept_legacy_aliases() {
        for action in [
            "node-move right 10",
            "node move right 10",
            "move right 10",
            "node-move-right 10",
            "node_move_right 10",
            "move-right 10",
            "move_right 10",
        ] {
            let kb = parse(&format!("keybinds:\n  \"super+right\" \"{action}\"\nend\n"));
            let bind = &kb.binds[0];
            assert_eq!(
                bind.action,
                Action::MoveNodeBy {
                    direction: crate::Direction::Right,
                    distance: 10
                }
            );
            assert!(bind.repeat);
            assert_eq!(bind.scope, BindingScope::Field);
        }
        let kb =
            parse("keybinds:\n  \"super+right\" \"node-move right 1\" with repeat false\nend\n");
        assert!(!kb.binds[0].repeat);
        assert_eq!(
            parse_action_checked("move files"),
            Ok(Action::Spawn("move files".into()))
        );
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
}

#[cfg(test)]
mod spelling_tests {
    use super::*;

    /// What every built-in action spelling, current or older, parses to. The
    /// fixture was recorded from the parser before the alias table existed,
    /// so a change here is a change in what users' configs mean.
    #[test]
    fn every_known_spelling_keeps_its_meaning() {
        let fixture = include_str!("../tests/fixtures/action_spellings.txt");
        let mut checked = 0;
        for line in fixture.lines() {
            let (input, expected) = line.split_once(" => ").expect("input => result");
            let input = unquote(input);
            assert_eq!(
                format!("{:?}", parse_action_checked(&input)),
                expected,
                "{input:?}"
            );
            checked += 1;
        }
        assert!(checked > 300, "only {checked} spellings checked");
    }

    /// The fixture stores inputs in Rust `Debug` form; only `"` is escaped.
    fn unquote(quoted: &str) -> String {
        quoted
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("quoted input")
            .replace(r#"\""#, r#"""#)
    }

    /// Every current spelling the table points at is a built-in action, never
    /// a shell command.
    #[test]
    fn every_alias_names_a_built_in_action() {
        for (old, current) in ACTION_ALIASES {
            let example = if current.ends_with('-') {
                let rest = if current.contains("slot") {
                    "3"
                } else {
                    "left"
                };
                format!("{current}{rest}")
            } else if matches!(old, OldSpelling::Name(_)) {
                current.to_string()
            } else {
                format!("{current} left")
            };
            assert!(
                !matches!(parse_spelling(&example), Ok(Action::Spawn(_))),
                "{current:?} is not a built-in action spelling"
            );
        }
    }

    /// The config Halley generates teaches the current spellings only.
    #[test]
    fn the_generated_config_uses_current_spellings() {
        let keybinds = crate::DEFAULT_CONFIG
            .split("\nkeybinds:\n")
            .nth(1)
            .and_then(|rest| rest.split("\nend\n").next())
            .expect("generated config has a keybinds section");
        let mut checked = 0;
        for line in keybinds.lines().map(str::trim) {
            let line = line.trim_start_matches('#').trim();
            let Some(action) = line
                .strip_suffix('"')
                .and_then(|line| line.rsplit_once(" \""))
                .map(|(_, action)| action)
            else {
                continue;
            };
            if !line.starts_with('"') {
                continue;
            }
            assert_eq!(
                current_spelling(action),
                None,
                "{action:?} is an old spelling"
            );
            assert!(
                !action.contains('_') || matches!(parse_spelling(action), Ok(Action::Spawn(_)))
            );
            checked += 1;
        }
        assert!(checked > 50, "only {checked} bindings checked");
    }

    #[test]
    fn snake_case_commands_stay_commands() {
        assert_eq!(
            parse_action_checked("my_script --flag"),
            Ok(Action::Spawn("my_script --flag".into()))
        );
        assert_eq!(
            parse_action_checked("toggle_state"),
            Ok(Action::ToggleState)
        );
    }
}
