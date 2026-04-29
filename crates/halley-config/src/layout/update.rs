use std::mem;

use crate::layout::{RuntimeTuning, ViewportOutputConfig};
use crate::parse::keybinds::{apply_explicit_keybind_overrides_entries, parse_inline_keybinds};

#[derive(Clone, Debug)]
struct ParsedScope {
    items: Vec<ScopeItem>,
    suffix: String,
}

#[derive(Clone, Debug)]
struct ScopeItem {
    leading: String,
    kind: ScopeItemKind,
}

#[derive(Clone, Debug)]
enum ScopeItemKind {
    Scalar(ScalarItem),
    Section(SectionItem),
}

#[derive(Clone, Debug)]
struct ScalarItem {
    key: String,
    raw_line: String,
}

#[derive(Clone, Debug)]
struct SectionItem {
    name: String,
    header_line: String,
    body: ParsedScope,
    end_line: String,
}

impl ParsedScope {
    fn render(&self) -> String {
        let mut out = String::new();
        for item in &self.items {
            out.push_str(item.leading.as_str());
            out.push_str(item.kind.render().as_str());
        }
        out.push_str(self.suffix.as_str());
        out
    }
}

impl ScopeItemKind {
    fn render(&self) -> String {
        match self {
            ScopeItemKind::Scalar(item) => format!("{}\n", item.raw_line),
            ScopeItemKind::Section(item) => {
                let mut out = String::new();
                out.push_str(item.header_line.as_str());
                out.push('\n');
                out.push_str(item.body.render().as_str());
                out.push_str(item.end_line.as_str());
                out.push('\n');
                out
            }
        }
    }
}

impl RuntimeTuning {
    pub fn update_user_config_text(
        raw: &str,
        tty_viewports: &[ViewportOutputConfig],
    ) -> Result<Option<String>, String> {
        if Self::from_rune_str(raw).is_none() {
            return Err("config parse failed; leaving file unchanged".to_string());
        }

        let template = Self::render_fresh_config(tty_viewports);
        let mut existing_doc = parse_scope(raw);
        let template_doc = parse_scope(template.as_str());
        let mut changed = merge_non_keybind_sections(&mut existing_doc, &template_doc);
        changed |= merge_keybinds(&mut existing_doc, &template_doc, raw)?;

        if !changed {
            return Ok(None);
        }

        Ok(Some(existing_doc.render()))
    }
}

fn merge_non_keybind_sections(existing: &mut ParsedScope, template: &ParsedScope) -> bool {
    let mut changed = false;

    for template_item in &template.items {
        let ScopeItemKind::Section(template_section) = &template_item.kind else {
            continue;
        };

        if template_section.name == "keybinds" {
            continue;
        }
        if !should_merge_top_level_section(template_section.name.as_str()) {
            continue;
        }

        if let Some(existing_section) = find_section_mut(existing, template_section.name.as_str()) {
            changed |= merge_section_body(existing_section, template_section);
            continue;
        }

        existing.items.push(template_item.clone());
        changed = true;
    }

    changed
}

fn merge_section_body(existing: &mut SectionItem, template: &SectionItem) -> bool {
    let mut changed = false;

    for template_item in &template.body.items {
        match &template_item.kind {
            ScopeItemKind::Scalar(template_scalar) => {
                if has_scalar_key(&existing.body, template_scalar.key.as_str()) {
                    continue;
                }
                existing.body.items.push(template_item.clone());
                changed = true;
            }
            ScopeItemKind::Section(template_section) => {
                if let Some(existing_section) =
                    find_section_mut(&mut existing.body, template_section.name.as_str())
                {
                    changed |= merge_section_body(existing_section, template_section);
                    continue;
                }
                existing.body.items.push(template_item.clone());
                changed = true;
            }
        }
    }

    changed
}

fn merge_keybinds(
    existing: &mut ParsedScope,
    template: &ParsedScope,
    raw: &str,
) -> Result<bool, String> {
    let Some(template_keybinds) = find_section(template, "keybinds") else {
        return Ok(false);
    };

    let Some(existing_keybinds) = find_section_mut(existing, "keybinds") else {
        existing.items.push(ScopeItem {
            leading: if existing.items.is_empty() && existing.suffix.is_empty() {
                String::new()
            } else {
                String::from("\n")
            },
            kind: ScopeItemKind::Section(template_keybinds.clone()),
        });
        return Ok(true);
    };

    let existing_entries = parse_inline_keybinds(raw)
        .map_err(|err| format!("config keybind parse failed; leaving file unchanged: {err}"))?;
    let mut resolved = resolve_explicit_keybinds(&existing_entries)?;
    let mod_token = existing_entries
        .iter()
        .rev()
        .find_map(|entry| entry.0.eq_ignore_ascii_case("mod").then(|| entry.1.clone()))
        .unwrap_or_else(|| resolved.keybinds.modifier_name());

    let mut additions = Vec::new();
    for candidate in keybind_candidates() {
        let candidate_entries = candidate_entries(*candidate, mod_token.as_str());
        let candidate_tuning = resolve_explicit_keybinds(&candidate_entries)?;
        if compositor_or_launch_conflict(&resolved, &candidate_tuning) {
            continue;
        }
        merge_resolved_bindings(&mut resolved, candidate_tuning);
        additions.push(make_keybind_item(
            *candidate,
            additions.is_empty() && !existing_keybinds.body.items.is_empty(),
        ));
    }

    if additions.is_empty() {
        return Ok(false);
    }

    existing_keybinds.body.items.extend(additions);
    Ok(true)
}

fn find_section<'a>(scope: &'a ParsedScope, name: &str) -> Option<&'a SectionItem> {
    scope.items.iter().find_map(|item| match &item.kind {
        ScopeItemKind::Section(section) if section.name == name => Some(section),
        _ => None,
    })
}

fn find_section_mut<'a>(scope: &'a mut ParsedScope, name: &str) -> Option<&'a mut SectionItem> {
    scope
        .items
        .iter_mut()
        .find_map(|item| match &mut item.kind {
            ScopeItemKind::Section(section) if section.name == name => Some(section),
            _ => None,
        })
}

fn has_scalar_key(scope: &ParsedScope, key: &str) -> bool {
    scope.items.iter().any(|item| match &item.kind {
        ScopeItemKind::Scalar(scalar) => scalar.key == key,
        ScopeItemKind::Section(_) => false,
    })
}

fn parse_scope(raw: &str) -> ParsedScope {
    let lines = raw.lines().map(str::to_string).collect::<Vec<_>>();
    let mut idx = 0usize;
    parse_scope_lines(&lines, &mut idx, false, 0)
}

fn parse_scope_lines(
    lines: &[String],
    idx: &mut usize,
    stop_at_end: bool,
    depth: usize,
) -> ParsedScope {
    let mut items = Vec::new();
    let mut pending = String::new();

    while *idx < lines.len() {
        let raw = lines[*idx].as_str();
        let trimmed = raw.trim();

        if stop_at_end && trimmed.eq_ignore_ascii_case("end") {
            break;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            pending.push_str(raw);
            pending.push('\n');
            *idx += 1;
            continue;
        }

        if trimmed.ends_with(':') {
            let header_line = raw.to_string();
            let name = normalize_section_name(trimmed.trim_end_matches(':').trim(), depth);
            *idx += 1;
            let body = parse_scope_lines(lines, idx, true, depth + 1);
            let end_line = if *idx < lines.len() && lines[*idx].trim().eq_ignore_ascii_case("end") {
                let line = lines[*idx].clone();
                *idx += 1;
                line
            } else {
                String::from("end")
            };
            items.push(ScopeItem {
                leading: mem::take(&mut pending),
                kind: ScopeItemKind::Section(SectionItem {
                    name,
                    header_line,
                    body,
                    end_line,
                }),
            });
            continue;
        }

        items.push(ScopeItem {
            leading: mem::take(&mut pending),
            kind: ScopeItemKind::Scalar(ScalarItem {
                key: scalar_key(trimmed),
                raw_line: raw.to_string(),
            }),
        });
        *idx += 1;
    }

    ParsedScope {
        items,
        suffix: pending,
    }
}

fn scalar_key(line: &str) -> String {
    line.split_whitespace()
        .next()
        .map(normalize_token)
        .unwrap_or_default()
}

fn normalize_token(token: &str) -> String {
    token.trim().to_ascii_lowercase().replace('_', "-")
}

fn normalize_section_name(name: &str, depth: usize) -> String {
    let normalized = normalize_token(name);
    if depth > 0 {
        return normalized;
    }

    canonical_top_level_section_name(normalized.as_str()).to_string()
}

fn canonical_top_level_section_name(name: &str) -> &str {
    match name {
        "animation" | "animations" => "animations",
        "node" | "nodes" => "nodes",
        "overlay" | "overlays" => "overlays",
        "screenshot" | "screenshots" => "screenshot",
        _ => name,
    }
}

fn should_merge_top_level_section(name: &str) -> bool {
    !matches!(name, "autostart" | "env" | "rules")
}

fn resolve_explicit_keybinds(entries: &[(String, String)]) -> Result<RuntimeTuning, String> {
    let mut tuning = RuntimeTuning::default();
    tuning.compositor_bindings.clear();
    tuning.launch_bindings.clear();
    tuning.pointer_bindings.clear();
    apply_explicit_keybind_overrides_entries(entries, &mut tuning)?;
    Ok(tuning)
}

fn compositor_or_launch_conflict(existing: &RuntimeTuning, candidate: &RuntimeTuning) -> bool {
    candidate.compositor_bindings.iter().any(|binding| {
        existing.compositor_bindings.iter().any(|existing_binding| {
            existing_binding.modifiers == binding.modifiers && existing_binding.key == binding.key
        }) || existing.launch_bindings.iter().any(|existing_binding| {
            existing_binding.modifiers == binding.modifiers && existing_binding.key == binding.key
        })
    })
}

fn merge_resolved_bindings(existing: &mut RuntimeTuning, candidate: RuntimeTuning) {
    existing
        .compositor_bindings
        .extend(candidate.compositor_bindings);
    existing.launch_bindings.extend(candidate.launch_bindings);
    existing.pointer_bindings.extend(candidate.pointer_bindings);
}

fn keybind_candidates() -> &'static [(&'static str, &'static str)] {
    &[
        ("alt+tab", "cycle-focus"),
        ("alt+shift+tab", "cycle-focus-backward"),
        ("$var.mod+m", "maximize-focused"),
        ("$var.mod+p", "toggle-focused-pin"),
        ("$var.mod+1", "cluster slot 1"),
        ("$var.mod+2", "cluster slot 2"),
        ("$var.mod+3", "cluster slot 3"),
        ("$var.mod+4", "cluster slot 4"),
        ("$var.mod+5", "cluster slot 5"),
        ("$var.mod+6", "cluster slot 6"),
        ("$var.mod+7", "cluster slot 7"),
        ("$var.mod+8", "cluster slot 8"),
        ("$var.mod+9", "cluster slot 9"),
        ("$var.mod+0", "cluster slot 10"),
    ]
}

fn candidate_entries(candidate: (&str, &str), mod_token: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if candidate.0.contains("$var.mod") {
        out.push(("mod".to_string(), mod_token.to_string()));
    }
    out.push((candidate.0.to_string(), candidate.1.to_string()));
    out
}

fn make_keybind_item(candidate: (&str, &str), needs_blank_line: bool) -> ScopeItem {
    ScopeItem {
        leading: if needs_blank_line {
            String::from("\n")
        } else {
            String::new()
        },
        kind: ScopeItemKind::Scalar(ScalarItem {
            key: normalize_token(candidate.0),
            raw_line: format!("  \"{}\" \"{}\"", candidate.0, candidate.1),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_adds_missing_animation_block() {
        let raw = r#"
animations:
  enabled true
  smooth-resize:
    enabled true
    duration-ms 90
  end
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(updated.contains("  maximize:\n    enabled true\n    duration-ms 240\n  end"));
        assert!(updated.contains("smooth-resize:\n    enabled true\n    duration-ms 90"));
    }

    #[test]
    fn updater_adds_missing_input_keyboard_block() {
        let raw = r#"
input:
  repeat-rate 30
  repeat-delay 500
  focus-mode "click"
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(
            updated
                .contains("input:\n  repeat-rate 30\n  repeat-delay 500\n  focus-mode \"click\"")
        );
        assert!(
            updated.contains(
                "  keyboard:\n    layout \"us\"\n    variant \"\"\n    options \"\"\n  end"
            )
        );
    }

    #[test]
    fn updater_adds_missing_decoration_shadow_defaults() {
        let raw = r##"
decorations:
  border:
    size 3
    radius 0
    colour-focused "#d65d26"
    colour-unfocused "#333333"
  end

  resize-using-border true
end
"##;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(updated.contains("  shadows:\n    window:"));
        assert!(updated.contains("      blur-radius 8"));
        assert!(updated.contains("      colour \"#05030530\""));
        assert!(updated.contains("    node:\n      enabled true\n      blur-radius 14"));
        assert!(updated.contains("    overlay:\n      enabled true\n      blur-radius 24"));
        assert!(updated.contains("      colour \"#05030538\""));
    }

    #[test]
    fn updater_respects_node_section_aliases() {
        let raw = r#"
node:
  show-labels "always"
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(updated.contains("node:\n  show-labels \"always\""));
        assert!(!updated.contains("\nnodes:\n"));
        assert!(updated.contains("  shape \"square\""));
    }

    #[test]
    fn updater_respects_animation_section_aliases() {
        let raw = r#"
animation:
  enabled true
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(updated.contains("animation:\n  enabled true"));
        assert!(!updated.contains("\nanimations:\n"));
        assert!(updated.contains("  maximize:\n    enabled true\n    duration-ms 240\n  end"));
    }

    #[test]
    fn updater_adds_missing_keybind_candidates_without_conflicts() {
        let raw = r#"
keybinds:
  mod "super"
  "$var.mod+shift+r" "reload"
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(updated.contains("  \"alt+tab\" \"cycle-focus\""));
        assert!(updated.contains("  \"alt+shift+tab\" \"cycle-focus-backward\""));
        assert!(updated.contains("  \"$var.mod+m\" \"maximize-focused\""));
        assert!(updated.contains("  \"$var.mod+0\" \"cluster slot 10\""));
    }

    #[test]
    fn updater_skips_conflicting_keybind_candidates() {
        let raw = r#"
keybinds:
  mod "super"
  "alt+tab" "open-terminal"
  "$var.mod+m" "fuzzel"
  "$var.mod+1" "cluster slot 1"
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(!updated.contains("\"alt+tab\" \"cycle-focus\""));
        assert!(!updated.contains("\"$var.mod+m\" \"maximize-focused\""));
        assert_eq!(
            updated.matches("\"$var.mod+1\" \"cluster slot 1\"").count(),
            1
        );
        assert!(updated.contains("\"$var.mod+2\" \"cluster slot 2\""));
    }

    #[test]
    fn updater_is_idempotent() {
        let raw = r#"
animations:
  enabled true
end

keybinds:
  mod "super"
end
"#;

        let updated = RuntimeTuning::update_user_config_text(raw, &[])
            .expect("config should update")
            .expect("config should change");

        assert!(
            RuntimeTuning::update_user_config_text(updated.as_str(), &[])
                .expect("second pass should succeed")
                .is_none()
        );
    }

    #[test]
    fn updater_rejects_invalid_config_text() {
        let raw = "keybinds:\n  \"mod+return\"\n";

        let err = RuntimeTuning::update_user_config_text(raw, &[])
            .expect_err("invalid config should fail");

        assert!(err.contains("leaving file unchanged"));
    }
}
