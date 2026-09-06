use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rune_cfg::RuneConfig;

use crate::{Action, BindingScope, DEFAULT_CONFIG, Keybind, ModifierKey};

const LEGACY_VERSION_PREFIX: &str = "# halley-config-version:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationStatus {
    UpToDate,
    WouldUpdate,
    Updated,
    /// A pre-0.6 file was backed up and replaced with the current default.
    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub status: MigrationStatus,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub reason: Option<String>,
    pub backup: Option<PathBuf>,
}

impl MigrationReport {
    fn up_to_date(skipped: Vec<String>) -> Self {
        Self {
            status: MigrationStatus::UpToDate,
            applied: Vec::new(),
            skipped,
            reason: None,
            backup: None,
        }
    }
}

#[derive(Debug)]
pub enum MigrationError {
    Io(io::Error),
    Invalid(String),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for MigrationError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy)]
struct BindingCandidate {
    name: &'static str,
    line: &'static str,
}

const PRE_06_REPLACE: &str = "pre-0.6 configuration replaced with the current default";

/// Top-level sections that exist in released pre-0.6 Halley configs and never
/// appear in the 0.6 schema. `viewport` was also rewritten on first tty launch.
const PRE_06_TOP_LEVEL: &[&str] = &["viewport", "gaming", "tile", "stacking"];

/// Nested keys that 0.6 renamed or removed. Any one of these is enough to
/// treat an unversioned file as a generation boundary rather than a backfill.
const PRE_06_ASSIGNMENTS: &[&str] = &[
    "show-ring-when-resizing",
    "node-shape",
    "node-label-shape",
    "border-colour-hover",
    "border-colour-inactive",
    "cluster-dwell-ms",
    "active-windows-allowed",
    "layer-shell",
];

/// Bindings added while the 0.6 compositor surface was being completed.
///
/// These are deliberately exact and finite. Config migration must never turn
/// into a template merge that floods a customized file with every new option.
const VERSION_1_BINDINGS: &[BindingCandidate] = &[
    BindingCandidate {
        name: "pin focused Field node",
        line: r#""$var.mod+p" "toggle-focused-pin""#,
    },
    BindingCandidate {
        name: "focus left",
        line: r#""$var.mod+left" "focus-left""#,
    },
    BindingCandidate {
        name: "focus right",
        line: r#""$var.mod+right" "focus-right""#,
    },
    BindingCandidate {
        name: "focus up",
        line: r#""$var.mod+up" "focus-up""#,
    },
    BindingCandidate {
        name: "focus down",
        line: r#""$var.mod+down" "focus-down""#,
    },
    BindingCandidate {
        name: "Trail previous",
        line: r#""$var.mod+comma" "trail-prev""#,
    },
    BindingCandidate {
        name: "Trail next",
        line: r#""$var.mod+period" "trail-next""#,
    },
    BindingCandidate {
        name: "move Field node left",
        line: r#""$var.mod+alt+left" "node-move left""#,
    },
    BindingCandidate {
        name: "move Field node right",
        line: r#""$var.mod+alt+right" "node-move right""#,
    },
    BindingCandidate {
        name: "move Field node up",
        line: r#""$var.mod+alt+up" "node-move up""#,
    },
    BindingCandidate {
        name: "move Field node down",
        line: r#""$var.mod+alt+down" "node-move down""#,
    },
    BindingCandidate {
        name: "swap cluster tile left",
        line: r#""$var.mod+ctrl+left" "cluster-tile-swap-left" with scope "tile""#,
    },
    BindingCandidate {
        name: "swap cluster tile right",
        line: r#""$var.mod+ctrl+right" "cluster-tile-swap-right" with scope "tile""#,
    },
    BindingCandidate {
        name: "swap cluster tile up",
        line: r#""$var.mod+ctrl+up" "cluster-tile-swap-up" with scope "tile""#,
    },
    BindingCandidate {
        name: "swap cluster tile down",
        line: r#""$var.mod+ctrl+down" "cluster-tile-swap-down" with scope "tile""#,
    },
    BindingCandidate {
        name: "resize Field window left",
        line: r#""$var.mod+ctrl+left" "resize-window-left" with scope "field""#,
    },
    BindingCandidate {
        name: "resize Field window right",
        line: r#""$var.mod+ctrl+right" "resize-window-right" with scope "field""#,
    },
    BindingCandidate {
        name: "resize Field window up",
        line: r#""$var.mod+ctrl+up" "resize-window-up" with scope "field""#,
    },
    BindingCandidate {
        name: "resize Field window down",
        line: r#""$var.mod+ctrl+down" "resize-window-down" with scope "field""#,
    },
    BindingCandidate {
        name: "focus monitor left",
        line: r#""$var.mod+shift+left" "monitor-focus left""#,
    },
    BindingCandidate {
        name: "focus monitor right",
        line: r#""$var.mod+shift+right" "monitor-focus right""#,
    },
    BindingCandidate {
        name: "focus monitor up",
        line: r#""$var.mod+shift+up" "monitor-focus up""#,
    },
    BindingCandidate {
        name: "focus monitor down",
        line: r#""$var.mod+shift+down" "monitor-focus down""#,
    },
    BindingCandidate {
        name: "arrange visible Field windows",
        line: r#""$var.mod+a" "arrange-visible""#,
    },
    BindingCandidate {
        name: "manual config reload",
        line: r#""$var.mod+shift+r" "reload""#,
    },
    BindingCandidate {
        name: "pointer window move",
        line: r#""$var.mod+click-left" "move-window""#,
    },
    BindingCandidate {
        name: "pointer window resize",
        line: r#""$var.mod+click-right" "resize-window""#,
    },
    BindingCandidate {
        name: "grabbed-window Field pan",
        line: r#""$var.mod+shift+click-left" "drag-pan""#,
    },
    BindingCandidate {
        name: "pointer Field pan",
        line: r#""click-left" "pan-field""#,
    },
];

pub fn migrate_config_at(path: &Path, dry_run: bool) -> Result<MigrationReport, MigrationError> {
    let source = fs::read_to_string(path)?;
    if looks_like_pre_06_tree(path, &source) {
        return replace_pre_06_config(path, dry_run);
    }
    if contains_gather(&source) {
        return Err(MigrationError::Invalid(
            "the selected root file uses gather; migrate the file that owns the affected section explicitly"
                .to_string(),
        ));
    }

    let runtime = crate::load_runtime_config_at(path).map_err(|error| {
        MigrationError::Invalid(format!(
            "existing configuration is invalid; leaving it unchanged: {error}"
        ))
    })?;
    let mut known_binds = runtime.keybinds.binds;
    let modifier = runtime.keybinds.modifier;
    let mut accepted = Vec::new();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    for candidate in VERSION_1_BINDINGS {
        let binding = parse_candidate(modifier, candidate.line)?;
        if known_binds
            .iter()
            .any(|existing| existing.action == binding.action)
        {
            continue;
        }
        if let Some(conflict) = known_binds
            .iter()
            .find(|existing| bindings_conflict(existing, &binding))
        {
            skipped.push(format!(
                "{}: chord is already occupied in {:?} scope",
                candidate.name, conflict.scope
            ));
            continue;
        }
        known_binds.push(binding);
        accepted.push(*candidate);
        applied.push(candidate.name.to_string());
    }

    let mut updated = source.clone();
    if !accepted.is_empty() {
        updated = insert_bindings(&updated, &accepted)?;
    }
    let (without_retired, split_removed, undo_chord_removed) =
        remove_retired_default_bindings(&updated);
    updated = without_retired;
    if split_removed {
        applied.push("retired pointer Field split binding".to_string());
    }
    if undo_chord_removed {
        applied.push("retired separate Field arrange undo binding".to_string());
    }
    let (backfilled, arrange_changed) = backfill_arrange_animation(&updated)?;
    updated = backfilled;
    if arrange_changed {
        applied.push("arrange animation".to_string());
    }
    let (backfilled, node_changed) = backfill_node_collapse_animation(&updated);
    updated = backfilled;
    if node_changed {
        applied.push("node collapse animation duration".to_string());
    }
    let (backfilled, zoom_changed) = backfill_zoom_indicator(&updated)?;
    updated = backfilled;
    if zoom_changed {
        applied.push("zoom indicator overlay".to_string());
    }
    let (without_marker, marker_removed) = remove_legacy_version_markers(&updated);
    updated = without_marker;
    if marker_removed {
        applied.push("obsolete config version marker".to_string());
    }

    if updated == source {
        return Ok(MigrationReport::up_to_date(skipped));
    }

    validate_candidate(path, &updated)?;

    let status = if dry_run {
        MigrationStatus::WouldUpdate
    } else {
        MigrationStatus::Updated
    };
    let backup = if dry_run {
        None
    } else {
        let backup = backup_path(path)?;
        fs::copy(path, &backup)?;
        if let Err(error) = atomic_replace(path, updated.as_bytes()) {
            return Err(MigrationError::Invalid(format!(
                "failed to install migrated config (backup retained at {}): {error}",
                backup.display()
            )));
        }
        Some(backup)
    };

    Ok(MigrationReport {
        status,
        applied,
        skipped,
        reason: None,
        backup,
    })
}

fn remove_retired_default_bindings(source: &str) -> (String, bool, bool) {
    const SPLIT: &str = r#""$var.mod+ctrl+click-left" "split-window""#;
    const ARRANGE_UNDO: &str = r#""$var.mod+shift+a" "undo-arrange""#;
    let mut updated = String::with_capacity(source.len());
    let mut split_removed = false;
    let mut undo_chord_removed = false;
    for line in source.split_inclusive('\n') {
        match line.trim() {
            SPLIT => split_removed = true,
            ARRANGE_UNDO => undo_chord_removed = true,
            _ => updated.push_str(line),
        }
    }
    (updated, split_removed, undo_chord_removed)
}

fn remove_legacy_version_markers(source: &str) -> (String, bool) {
    let mut updated = String::with_capacity(source.len());
    let mut removed = false;
    for line in source.split_inclusive('\n') {
        if line.trim().starts_with(LEGACY_VERSION_PREFIX) {
            removed = true;
        } else {
            updated.push_str(line);
        }
    }
    (updated, removed)
}

fn contains_gather(source: &str) -> bool {
    source.lines().any(|line| gather_path(line).is_some())
}

fn replace_pre_06_config(path: &Path, dry_run: bool) -> Result<MigrationReport, MigrationError> {
    let applied = vec![PRE_06_REPLACE.to_string()];
    if dry_run {
        return Ok(MigrationReport {
            status: MigrationStatus::WouldUpdate,
            applied,
            skipped: Vec::new(),
            reason: Some("pre-0.6 configuration is not compatible with Halley 0.6".to_string()),
            backup: None,
        });
    }

    let backup = backup_path_with_label(path, "pre-0.6.bak")?;
    fs::copy(path, &backup)?;
    if let Err(error) = atomic_replace(path, DEFAULT_CONFIG.as_bytes()) {
        return Err(MigrationError::Invalid(format!(
            "failed to install the 0.6 default config (backup retained at {}): {error}",
            backup.display()
        )));
    }

    Ok(MigrationReport {
        status: MigrationStatus::Replaced,
        applied,
        skipped: Vec::new(),
        reason: Some("pre-0.6 configuration is not compatible with Halley 0.6".to_string()),
        backup: Some(backup),
    })
}

fn looks_like_pre_06_tree(path: &Path, source: &str) -> bool {
    if looks_like_pre_06(source) {
        return true;
    }
    let mut visited = HashSet::new();
    visited.insert(path.to_path_buf());
    scan_gather_tree(path, source, &mut visited)
}

fn scan_gather_tree(path: &Path, source: &str, visited: &mut HashSet<PathBuf>) -> bool {
    let Some(base) = path.parent() else {
        return false;
    };
    for raw in source.lines().filter_map(gather_path) {
        let Some(child) = resolve_gather_path(raw, base) else {
            continue;
        };
        if !visited.insert(child.clone()) {
            continue;
        }
        let Ok(child_source) = fs::read_to_string(&child) else {
            continue;
        };
        if looks_like_pre_06(&child_source) || scan_gather_tree(&child, &child_source, visited) {
            return true;
        }
    }
    false
}

fn gather_path(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("gather")?.trim();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let after_quote = &rest[quote.len_utf8()..];
    let end = after_quote.find(quote)?;
    Some(&after_quote[..end])
}

fn resolve_gather_path(raw: &str, base: &Path) -> Option<PathBuf> {
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else {
        PathBuf::from(raw)
    };
    Some(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn looks_like_pre_06(source: &str) -> bool {
    source.lines().any(|line| {
        let Some(trimmed) = uncommented_code(line) else {
            return false;
        };
        if !line.starts_with(char::is_whitespace)
            && PRE_06_TOP_LEVEL.iter().any(|name| {
                trimmed
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with(':'))
            })
        {
            return true;
        }
        if trimmed == "rule:" {
            return true;
        }
        PRE_06_ASSIGNMENTS.iter().any(|key| {
            trimmed
                .strip_prefix(key)
                .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace))
        })
    })
}

fn uncommented_code(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('@') {
        None
    } else {
        Some(trimmed)
    }
}

fn parse_candidate(modifier: ModifierKey, line: &str) -> Result<Keybind, MigrationError> {
    let source = format!(
        "keybinds:\n  mod \"{}\"\n  {line}\nend\n",
        modifier_name(modifier)
    );
    let config = RuneConfig::from_str(&source).map_err(|error| {
        MigrationError::Invalid(format!("internal migration binding is invalid: {error}"))
    })?;
    let mut keybinds = crate::parse_keybinds(&config).map_err(|error| {
        MigrationError::Invalid(format!("internal migration binding is invalid: {error}"))
    })?;
    keybinds.binds.pop().ok_or_else(|| {
        MigrationError::Invalid("internal migration binding produced no action".to_string())
    })
}

fn modifier_name(modifier: ModifierKey) -> &'static str {
    match modifier {
        ModifierKey::Super => "super",
        ModifierKey::LeftSuper => "left-super",
        ModifierKey::RightSuper => "right-super",
        ModifierKey::Alt => "alt",
        ModifierKey::LeftAlt => "left-alt",
        ModifierKey::RightAlt => "right-alt",
        ModifierKey::Ctrl => "ctrl",
        ModifierKey::LeftCtrl => "left-ctrl",
        ModifierKey::RightCtrl => "right-ctrl",
        ModifierKey::Shift => "shift",
        ModifierKey::LeftShift => "left-shift",
        ModifierKey::RightShift => "right-shift",
    }
}

fn bindings_conflict(existing: &Keybind, candidate: &Keybind) -> bool {
    if existing.modifiers != candidate.modifiers
        || !existing.key.eq_ignore_ascii_case(&candidate.key)
        || !scopes_overlap(existing.scope, candidate.scope)
    {
        return false;
    }

    !matches!(
        (&existing.action, &candidate.action),
        (Action::ClusterTileFocus(old), Action::FocusDirection(new)) if old == new
    )
}

fn scopes_overlap(left: BindingScope, right: BindingScope) -> bool {
    use BindingScope::{Cluster, Global, Stack, Tile};
    matches!(left, Global)
        || matches!(right, Global)
        || left == right
        || matches!(
            (left, right),
            (Cluster, Tile | Stack) | (Tile | Stack, Cluster)
        )
}

fn insert_bindings(source: &str, bindings: &[BindingCandidate]) -> Result<String, MigrationError> {
    let Some(offset) = keybind_end_offset(source) else {
        return Err(MigrationError::Invalid(
            "the selected root file does not own a keybinds section; migrate the gathered keybind file explicitly"
                .to_string(),
        ));
    };
    let mut block = String::new();
    if !source[..offset].ends_with("\n\n") {
        block.push('\n');
    }
    block.push_str(
        "  # Added by Halley config migration 1. These remain ordinary, editable bindings.\n",
    );
    for binding in bindings {
        block.push_str("  ");
        block.push_str(binding.line);
        block.push('\n');
    }
    let mut updated = String::with_capacity(source.len() + block.len());
    updated.push_str(&source[..offset]);
    updated.push_str(&block);
    updated.push_str(&source[offset..]);
    Ok(updated)
}

fn keybind_end_offset(source: &str) -> Option<usize> {
    block_offsets(source, &["keybinds"]).map(|(_, end)| end)
}

fn block_offsets(source: &str, path: &[&str]) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    let mut stack = Vec::<&str>::new();
    let mut body_start = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            if trimmed == "end" {
                if stack.as_slice() == path {
                    return body_start.map(|start| (start, offset));
                }
                stack.pop();
            } else if let Some(name) = trimmed.strip_suffix(':') {
                stack.push(name);
                if stack.as_slice() == path {
                    body_start = Some(offset + line.len());
                }
            }
        }
        offset += line.len();
    }
    None
}

fn backfill_node_collapse_animation(source: &str) -> (String, bool) {
    if let Some((body_start, end)) = block_offsets(source, &["animations", "node"]) {
        if has_assignment(&source[body_start..end], "collapse-duration-ms") {
            return (source.to_string(), false);
        }
        let mut updated = source.to_string();
        updated.insert_str(end, "    collapse-duration-ms 280\n");
        return (updated, true);
    }
    let node_block = "  node:\n    collapse-duration-ms 280\n  end\n";
    let mut updated = source.to_string();
    if let Some((_, end)) = block_offsets(source, &["animations"]) {
        updated.insert_str(end, node_block);
    } else {
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("animations:\n");
        updated.push_str(node_block);
        updated.push_str("end\n");
    }
    (updated, true)
}

fn backfill_arrange_animation(source: &str) -> Result<(String, bool), MigrationError> {
    if block_offsets(source, &["animations", "arrange"]).is_some() {
        return Ok((source.to_string(), false));
    }

    let arrange_block = concat!(
        "  arrange:\n",
        "    enabled true\n",
        "    motion \"easing\"\n",
        "    duration-ms 360\n",
        "    curve \"ease-in-out-cubic\"\n",
        "  end\n",
    );
    if let Some((_, end)) = block_offsets(source, &["animations"]) {
        let mut insertion = String::new();
        if !source[..end].ends_with("\n\n") {
            insertion.push('\n');
        }
        insertion.push_str("  # Added by Halley config migration.\n");
        insertion.push_str(arrange_block);
        let mut updated = String::with_capacity(source.len() + insertion.len());
        updated.push_str(&source[..end]);
        updated.push_str(&insertion);
        updated.push_str(&source[end..]);
        return Ok((updated, true));
    }

    let mut updated = source.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.is_empty() && !updated.ends_with("\n\n") {
        updated.push('\n');
    }
    updated.push_str("# Added by Halley config migration.\n");
    updated.push_str("animations:\n");
    updated.push_str(arrange_block);
    updated.push_str("end\n");
    Ok((updated, true))
}

fn backfill_zoom_indicator(source: &str) -> Result<(String, bool), MigrationError> {
    if let Some((body_start, end)) = block_offsets(source, &["overlays", "zoom-indicator"]) {
        let body = &source[body_start..end];
        let mut additions = String::new();
        if !has_assignment(body, "background") {
            additions.push_str("    background true\n");
        }
        if !has_assignment(body, "opacity") {
            additions.push_str("    opacity 1.0\n");
        }
        if additions.is_empty() {
            return Ok((source.to_string(), false));
        }
        let mut updated = String::with_capacity(source.len() + additions.len());
        updated.push_str(&source[..end]);
        updated.push_str(&additions);
        updated.push_str(&source[end..]);
        return Ok((updated, true));
    }

    let zoom_block = concat!(
        "  zoom-indicator:\n",
        "    enabled true\n",
        "    position \"bottom-center\"\n",
        "    hold-duration-ms 750\n",
        "    fade-duration-ms 180\n",
        "    background true\n",
        "    opacity 1.0\n",
        "  end\n",
    );
    if let Some((_, end)) = block_offsets(source, &["overlays"]) {
        let mut insertion = String::new();
        if !source[..end].ends_with("\n\n") {
            insertion.push('\n');
        }
        insertion.push_str("  # Added by Halley config migration 2.\n");
        insertion.push_str(zoom_block);
        let mut updated = String::with_capacity(source.len() + insertion.len());
        updated.push_str(&source[..end]);
        updated.push_str(&insertion);
        updated.push_str(&source[end..]);
        return Ok((updated, true));
    }

    let mut updated = source.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.is_empty() && !updated.ends_with("\n\n") {
        updated.push('\n');
    }
    updated.push_str("# Added by Halley config migration 2.\n");
    updated.push_str("overlays:\n");
    updated.push_str(zoom_block);
    updated.push_str("end\n");
    Ok((updated, true))
}

fn has_assignment(block: &str, key: &str) -> bool {
    block.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#')
            && line
                .strip_prefix(key)
                .is_some_and(|rest| rest.chars().next().is_some_and(char::is_whitespace))
    })
}

fn validate_candidate(path: &Path, source: &str) -> Result<(), MigrationError> {
    let temp = temporary_path(path, "validate")?;
    let result = (|| {
        write_new_file(&temp, source.as_bytes(), path)?;
        crate::load_runtime_config_at(&temp).map_err(|error| {
            MigrationError::Invalid(format!(
                "migrated configuration did not validate; leaving the original unchanged: {error}"
            ))
        })?;
        Ok(())
    })();
    let _ = fs::remove_file(&temp);
    result
}

fn backup_path(path: &Path) -> Result<PathBuf, MigrationError> {
    backup_path_with_label(path, "bak")
}

fn backup_path_with_label(path: &Path, label: &str) -> Result<PathBuf, MigrationError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MigrationError::Invalid(format!("system clock error: {error}")))?
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MigrationError::Invalid("config path has no UTF-8 file name".to_string()))?;
    Ok(path.with_file_name(format!("{file_name}.{label}-{timestamp}")))
}

fn temporary_path(path: &Path, purpose: &str) -> Result<PathBuf, MigrationError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MigrationError::Invalid(format!("system clock error: {error}")))?
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MigrationError::Invalid("config path has no UTF-8 file name".to_string()))?;
    Ok(path.with_file_name(format!(
        ".{file_name}.{purpose}-{}-{timestamp}",
        std::process::id()
    )))
}

fn atomic_replace(path: &Path, contents: &[u8]) -> Result<(), MigrationError> {
    let temp = temporary_path(path, "migrate")?;
    if let Err(error) = write_new_file(&temp, contents, path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(MigrationError::Io(error));
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn write_new_file(
    path: &Path,
    contents: &[u8],
    permissions_from: &Path,
) -> Result<(), MigrationError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Ok(metadata) = fs::metadata(permissions_from) {
        file.set_permissions(metadata.permissions())?;
    }
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halley-migrate-{}-{name}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("halley.rune")
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn minimal(bindings: &str) -> String {
        format!("keybinds:\n  mod \"super\"\n{bindings}end\n")
    }

    fn pre_06_root() -> String {
        concat!(
            "@author \"Dustin Pilgrim\"\n",
            "viewport:\n",
            "  DP-1:\n",
            "    enabled true\n",
            "    width 2560\n",
            "    height 1440\n",
            "  end\n",
            "end\n\n",
            "node:\n",
            "  node-shape \"square\"\n",
            "  border-colour-hover \"use-window-active\"\n",
            "end\n\n",
            "tile:\n",
            "  gaps-inner 20\n",
            "end\n\n",
            "gaming:\n",
            "  games [\"steam_app_*\"]\n",
            "end\n\n",
            "effects:\n",
            "  blur:\n",
            "    enabled false\n",
            "    windows \"auto\"\n",
            "    layer-shell \"off\"\n",
            "  end\n",
            "end\n\n",
            "keybinds:\n",
            "  mod \"super\"\n",
            "  \"$var.mod+return\" \"open-terminal\"\n",
            "  \"$var.mod+leftmouse\" \"move-window\"\n",
            "end\n",
        )
        .to_string()
    }

    #[test]
    fn current_markerless_default_needs_no_migration() {
        assert!(!looks_like_pre_06(DEFAULT_CONFIG));
        assert!(!DEFAULT_CONFIG.contains(LEGACY_VERSION_PREFIX));

        let scratch = ScratchDir::new("current");
        let path = scratch.config();
        fs::write(&path, DEFAULT_CONFIG).unwrap();

        let report = migrate_config_at(&path, false).unwrap();

        assert_eq!(report.status, MigrationStatus::UpToDate);
        assert!(report.applied.is_empty());
        assert!(report.backup.is_none());
        assert_eq!(fs::read_to_string(path).unwrap(), DEFAULT_CONFIG);
    }

    #[test]
    fn dry_run_replaces_pre_06_config_without_writing() {
        let scratch = ScratchDir::new("pre06-dry");
        let path = scratch.config();
        let original = pre_06_root();
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, true).unwrap();

        assert_eq!(report.status, MigrationStatus::WouldUpdate);
        assert!(report.applied.iter().any(|item| item == PRE_06_REPLACE));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(report.backup.is_none());
    }

    #[test]
    fn pre_06_config_is_backed_up_replaced_and_then_current() {
        let scratch = ScratchDir::new("pre06-write");
        let path = scratch.config();
        let original = pre_06_root();
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, false).unwrap();

        assert_eq!(report.status, MigrationStatus::Replaced);
        let backup = report.backup.expect("backup path");
        assert!(
            backup
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("pre-0.6.bak-")),
            "backup name should mark the generation boundary: {}",
            backup.display()
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        crate::load_runtime_config_at(&path).expect("installed default validates");
        assert_eq!(
            migrate_config_at(&path, false).unwrap().status,
            MigrationStatus::UpToDate
        );
    }

    #[test]
    fn commented_pre_06_syntax_does_not_trigger_replacement() {
        let scratch = ScratchDir::new("pre06-comment");
        let path = scratch.config();
        fs::write(
            &path,
            concat!(
                "# viewport:\n",
                "#   gaming is unused\n",
                "# tile:\n",
                "keybinds:\n",
                "  mod \"super\"\n",
                "  \"$var.mod+q\" \"close-focused\"\n",
                "end\n",
            ),
        )
        .unwrap();

        let report = migrate_config_at(&path, false).unwrap();

        assert_eq!(report.status, MigrationStatus::Updated);
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# viewport:"));
        assert!(updated.contains("# tile:"));
        assert_ne!(updated, DEFAULT_CONFIG);
    }

    #[test]
    fn dry_run_backfills_missing_sections_without_writing() {
        let scratch = ScratchDir::new("dry-run");
        let path = scratch.config();
        let original = minimal("  \"$var.mod+q\" \"close-focused\"\n");
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, true).unwrap();

        assert_eq!(report.status, MigrationStatus::WouldUpdate);
        assert!(report.applied.iter().any(|name| name == "Trail previous"));
        assert!(
            report
                .applied
                .iter()
                .any(|name| name == "zoom indicator overlay")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(report.backup.is_none());
    }

    #[test]
    fn migration_is_atomic_validated_backed_up_and_idempotent() {
        let scratch = ScratchDir::new("write");
        let path = scratch.config();
        let original = minimal("  \"$var.mod+q\" \"close-focused\"\n");
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, false).unwrap();

        assert_eq!(report.status, MigrationStatus::Updated);
        let backup = report.backup.expect("backup path");
        assert_eq!(fs::read_to_string(backup).unwrap(), original);
        let updated = fs::read_to_string(&path).unwrap();
        assert!(!updated.contains(LEGACY_VERSION_PREFIX));
        assert!(updated.contains("\"$var.mod+comma\" \"trail-prev\""));
        assert!(updated.contains("\"$var.mod+shift+click-left\" \"drag-pan\""));
        assert!(updated.contains("zoom-indicator:"));
        assert!(updated.contains("background true"));
        assert!(updated.contains("opacity 1.0"));
        crate::load_runtime_config_at(&path).expect("migrated config validates");

        assert_eq!(
            migrate_config_at(&path, false).unwrap().status,
            MigrationStatus::UpToDate
        );
    }

    #[test]
    fn retired_split_and_separate_undo_bindings_are_replaced_by_arrange_toggle() {
        let scratch = ScratchDir::new("replace-arrange-defaults");
        let path = scratch.config();
        let original = minimal(
            "  \"$var.mod+ctrl+click-left\" \"split-window\"\n  # \"$var.mod+ctrl+click-left\" \"split-window\"\n  \"$var.mod+shift+a\" \"undo-arrange\"\n  # \"$var.mod+shift+a\" \"undo-arrange\"\n",
        );
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert_eq!(report.status, MigrationStatus::Updated);
        assert!(
            report
                .applied
                .iter()
                .any(|item| item == "retired pointer Field split binding")
        );
        assert!(
            report
                .applied
                .iter()
                .any(|item| item == "retired separate Field arrange undo binding")
        );
        assert!(updated.contains("\"$var.mod+a\" \"arrange-visible\""));
        assert!(updated.contains("  arrange:\n"));
        assert!(updated.contains("    duration-ms 360\n"));
        assert!(!updated.contains("  \"$var.mod+shift+a\" \"undo-arrange\""));
        assert!(updated.contains("# \"$var.mod+shift+a\" \"undo-arrange\""));
        assert!(!updated.contains("  \"$var.mod+ctrl+click-left\" \"split-window\""));
        assert!(updated.contains("# \"$var.mod+ctrl+click-left\" \"split-window\""));
        crate::load_runtime_config_at(&path).expect("migrated config validates");
        assert_eq!(
            migrate_config_at(&path, false).unwrap().status,
            MigrationStatus::UpToDate
        );
    }

    #[test]
    fn a_custom_overlapping_chord_is_not_overwritten() {
        let scratch = ScratchDir::new("conflict");
        let path = scratch.config();
        fs::write(&path, minimal("  \"$var.mod+p\" \"notify-send custom\"\n")).unwrap();

        let report = migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert!(
            report
                .skipped
                .iter()
                .any(|item| item.contains("pin focused"))
        );
        assert!(updated.contains("\"$var.mod+p\" \"notify-send custom\""));
        assert!(!updated.contains("\"$var.mod+p\" \"toggle-focused-pin\""));
    }

    #[test]
    fn grabbed_window_pan_migration_preserves_an_occupied_chord() {
        let scratch = ScratchDir::new("drag-pan-conflict");
        let path = scratch.config();
        fs::write(
            &path,
            minimal("  \"$var.mod+shift+click-left\" \"notify-send custom\"\n"),
        )
        .unwrap();

        let report = migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert!(
            report
                .skipped
                .iter()
                .any(|item| item.contains("grabbed-window Field pan"))
        );
        assert!(updated.contains("\"$var.mod+shift+click-left\" \"notify-send custom\""));
        assert!(!updated.contains("\"$var.mod+shift+click-left\" \"drag-pan\""));
    }

    #[test]
    fn scoped_resize_can_share_a_chord_with_tile_swap() {
        let scratch = ScratchDir::new("scopes");
        let path = scratch.config();
        fs::write(
            &path,
            minimal("  \"$var.mod+ctrl+left\" \"cluster-tile-swap-left\" with scope \"tile\"\n"),
        )
        .unwrap();

        migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert!(
            updated.contains("\"$var.mod+ctrl+left\" \"resize-window-left\" with scope \"field\"")
        );
    }

    #[test]
    fn node_collapse_backfill_preserves_customization_and_is_idempotent() {
        for existing in [
            "",
            "animations:\nend\n",
            "animations:\n  node:\n    duration-ms 910\n    enabled false\n  end\nend\n",
        ] {
            let (updated, changed) = backfill_node_collapse_animation(existing);
            assert!(changed);
            let parsed =
                crate::parse_animations(&rune_cfg::RuneConfig::from_str(&updated).unwrap());
            assert_eq!(parsed.node.collapse_duration_ms, 280);
            if existing.contains("910") {
                assert_eq!(parsed.node.duration_ms, 910);
                assert!(!parsed.node.enabled);
            }
            assert_eq!(backfill_node_collapse_animation(&updated), (updated, false));
        }
        for duration in [0, 630] {
            let existing =
                format!("animations:\n  node:\n    collapse-duration-ms {duration}\n  end\nend\n");
            assert_eq!(
                backfill_node_collapse_animation(&existing),
                (existing, false)
            );
        }
    }

    #[test]
    fn migration_writes_missing_node_collapse_duration() {
        let scratch = ScratchDir::new("node-collapse");
        let path = scratch.config();
        fs::write(&path, minimal("")).unwrap();
        let report = migrate_config_at(&path, false).unwrap();
        assert!(
            report
                .applied
                .iter()
                .any(|item| item == "node collapse animation duration")
        );
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("collapse-duration-ms 280"));
        migrate_config_at(&path, false).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), updated);
    }

    #[test]
    fn structural_backfill_preserves_existing_zoom_customization() {
        let scratch = ScratchDir::new("zoom-existing");
        let path = scratch.config();
        fs::write(
            &path,
            concat!(
                "keybinds:\n",
                "  mod \"super\"\n",
                "end\n\n",
                "overlays:\n",
                "  zoom-indicator:\n",
                "    enabled false\n",
                "    background false\n",
                "    text-size 18\n",
                "  end\n",
                "end\n",
            ),
        )
        .unwrap();

        migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert_eq!(updated.matches("background false").count(), 1);
        assert_eq!(updated.matches("opacity 1.0").count(), 1);
        assert!(updated.contains("text-size 18"));
        assert!(updated.contains("enabled false"));
        crate::load_runtime_config_at(&path).expect("migrated config validates");
    }

    #[test]
    fn explicit_migration_reports_ambiguous_gather_owner() {
        let scratch = ScratchDir::new("gather");
        let path = scratch.config();
        let keys = scratch.0.join("keys.rune");
        fs::write(&path, "gather \"keys.rune\"\n").unwrap();
        fs::write(&keys, minimal("  \"$var.mod+q\" \"close-focused\"\n")).unwrap();

        let error = migrate_config_at(&path, true).unwrap_err();

        assert!(error.to_string().contains("uses gather"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "gather \"keys.rune\"\n");
    }

    #[test]
    fn explicit_migration_removes_obsolete_version_markers_regardless_of_value() {
        let scratch = ScratchDir::new("legacy-marker");
        let path = scratch.config();
        let original = format!("@author \"Dustin\"\n# halley-config-version: 99\n{DEFAULT_CONFIG}");
        fs::write(&path, &original).unwrap();

        let report = migrate_config_at(&path, false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();

        assert_eq!(report.status, MigrationStatus::Updated);
        assert_eq!(
            report.applied,
            vec!["obsolete config version marker".to_string()]
        );
        assert!(updated.starts_with("@author \"Dustin\"\n"));
        assert!(!updated.contains(LEGACY_VERSION_PREFIX));
        assert_eq!(
            migrate_config_at(&path, false).unwrap().status,
            MigrationStatus::UpToDate
        );
    }

    #[test]
    fn marker_removal_handles_multiple_legacy_comments_without_touching_other_text() {
        let source = concat!(
            "@author \"Dustin\"\n",
            "# halley-config-version: 1\n",
            "# keep this comment\n",
            "keybinds:\n",
            "  # halley-config-version: invalid\n",
            "end\n",
        );

        let (updated, removed) = remove_legacy_version_markers(source);

        assert!(removed);
        assert_eq!(
            updated,
            "@author \"Dustin\"\n# keep this comment\nkeybinds:\nend\n"
        );
    }
}
