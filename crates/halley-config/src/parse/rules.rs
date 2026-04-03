use crate::layout::{
    InitialWindowClusterParticipation, InitialWindowOverlapPolicy,
    InitialWindowSpawnPlacement, RuntimeTuning, WindowRule, WindowRulePattern,
};

#[derive(Default)]
struct PartialWindowRule {
    app_ids: Vec<WindowRulePattern>,
    titles: Vec<WindowRulePattern>,
    overlap_policy: Option<InitialWindowOverlapPolicy>,
    spawn_placement: Option<InitialWindowSpawnPlacement>,
    cluster_participation: Option<InitialWindowClusterParticipation>,
}

pub(crate) fn load_rules_section(raw: &str, out: &mut RuntimeTuning) -> Result<(), String> {
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

