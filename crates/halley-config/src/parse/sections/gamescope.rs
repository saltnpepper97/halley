use crate::layout::{GamescopeConfig, GamescopeGameProfile, RuntimeTuning};

/// Parse the top-level `gaming:` section. It groups the `games` classifier list
/// (glob patterns for what Halley treats as a game) with the nested `gamescope:`
/// integration block. For backward compatibility a bare top-level `gamescope:`
/// (no `gaming:` wrapper) is still accepted and mapped into `gaming.gamescope`.
///
/// Modeled on `parse/rules.rs` (hand-rolled, line-based) because the repeated,
/// identically-named `game:` blocks and the block nesting do not map cleanly onto
/// the dotted RuneConfig key model.
pub(crate) fn load_gaming_section(raw: &str, out: &mut RuntimeTuning) -> Result<(), String> {
    if let Some(inner) = extract_block(raw, "gaming:") {
        // `games ["steam_app_*", "tf_linux64", ...]` — optional; keep the default
        // classifier list when absent.
        for raw_line in inner.lines() {
            let trimmed = strip_comment(raw_line);
            if let Some(rest) = trimmed.strip_prefix("games") {
                let rest = rest.trim_start();
                if rest.starts_with('[') || rest.starts_with('"') {
                    out.gaming.games = parse_string_list(rest);
                }
            }
        }
        // Nested `gamescope:` — optional; keep defaults when absent.
        if let Some(gs_inner) = extract_block(&inner, "gamescope:") {
            out.gaming.gamescope = parse_gamescope_block(&gs_inner)?;
        }
        return Ok(());
    }

    // Back-compat: a bare top-level `gamescope:` block (pre-`gaming:` configs).
    if let Some(gs_inner) = extract_block(raw, "gamescope:") {
        out.gaming.gamescope = parse_gamescope_block(&gs_inner)?;
    }
    Ok(())
}

/// Return the inner content of the first block named `header` (e.g. `"gaming:"`),
/// tracking nested `key:`/`end` pairs so the block's own `end` closes it rather
/// than a nested one. `None` if the block is absent.
fn extract_block(raw: &str, header: &str) -> Option<String> {
    let mut collecting = false;
    let mut depth = 0i32;
    let mut inner = String::new();
    for raw_line in raw.lines() {
        let trimmed = strip_comment(raw_line);
        if !collecting {
            if trimmed == header {
                collecting = true;
                depth = 1;
            }
            continue;
        }
        if trimmed == "end" {
            depth -= 1;
            if depth == 0 {
                return Some(inner);
            }
        } else if trimmed.ends_with(':') {
            depth += 1;
        }
        inner.push_str(raw_line);
        inner.push('\n');
    }
    // Unterminated block: hand back what we captured so the field parsers below
    // can raise a precise error (e.g. an unterminated `game:`).
    collecting.then_some(inner)
}

/// Parse the body of a `gamescope:` block (globals + repeated `game:` sub-blocks).
fn parse_gamescope_block(inner: &str) -> Result<GamescopeConfig, String> {
    let mut config = GamescopeConfig::default();
    let mut current_game: Option<GamescopeGameProfile> = None;

    for (line_no, raw_line) in inner.lines().enumerate() {
        let line_no = line_no + 1;
        let trimmed = strip_comment(raw_line);
        if trimmed.is_empty() {
            continue;
        }
        if let Some(game) = current_game.as_mut() {
            if trimmed == "end" {
                config.games.push(current_game.take().unwrap());
                continue;
            }
            parse_game_entry(game, trimmed, line_no)?;
            continue;
        }
        if trimmed == "game:" {
            current_game = Some(GamescopeGameProfile::default());
            continue;
        }
        parse_global_entry(&mut config, trimmed, line_no)?;
    }

    if current_game.is_some() {
        return Err("unterminated `game:` block in `gamescope:` section".to_string());
    }
    Ok(config)
}

/// Parse an inline `["a", "b"]` (or bare `"a", "b"`) list of quoted strings.
fn parse_string_list(value: &str) -> Vec<String> {
    let v = value.trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(v);
    inner
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            (!item.is_empty()).then(|| parse_value_string(item))
        })
        .collect()
}

fn strip_comment(line: &str) -> &str {
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

fn split_key_value(line: &str, line_no: usize) -> Result<(&str, &str), String> {
    let Some((key, rest)) = line.split_once(char::is_whitespace) else {
        return Err(format!(
            "line {line_no}: expected `<key> <value>` inside `gamescope:`"
        ));
    };
    let value = rest.trim();
    if value.is_empty() {
        return Err(format!("line {line_no}: missing value for `{key}`"));
    }
    Ok((key, value))
}

fn parse_bool(value: &str, line_no: usize, key: &str) -> Result<bool, String> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!(
            "line {line_no}: `{key}` expects `true` or `false`, got `{other}`"
        )),
    }
}

/// Accept either a quoted string (`"auto"`) or a bare token (`2560`).
fn parse_value_string(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_key(key: &str) -> String {
    key.replace('_', "-")
}

fn parse_global_entry(
    config: &mut GamescopeConfig,
    line: &str,
    line_no: usize,
) -> Result<(), String> {
    let (key, value) = split_key_value(line, line_no)?;
    match normalize_key(key).as_str() {
        "enabled" => config.enabled = parse_bool(value, line_no, key)?,
        "monitor" => config.monitor = parse_value_string(value),
        "output-width" => config.output_width = parse_value_string(value),
        "output-height" => config.output_height = parse_value_string(value),
        "game-width" => config.game_width = parse_value_string(value),
        "game-height" => config.game_height = parse_value_string(value),
        "refresh" => config.refresh = parse_value_string(value),
        "fullscreen" => config.fullscreen = parse_bool(value, line_no, key)?,
        "borderless" => config.borderless = parse_bool(value, line_no, key)?,
        "suppress-overlays" => config.suppress_overlays = parse_bool(value, line_no, key)?,
        "passthrough-pointer-lock" => {
            config.passthrough_pointer_lock = parse_bool(value, line_no, key)?
        }
        "bypass-spatial-camera" => config.bypass_spatial_camera = parse_bool(value, line_no, key)?,
        other => {
            return Err(format!(
                "line {line_no}: unknown `gamescope:` key `{other}`"
            ));
        }
    }
    Ok(())
}

fn parse_game_entry(
    game: &mut GamescopeGameProfile,
    line: &str,
    line_no: usize,
) -> Result<(), String> {
    let (key, value) = split_key_value(line, line_no)?;
    match normalize_key(key).as_str() {
        "name" => game.name = Some(parse_value_string(value)),
        "app-id" => game.app_id = Some(parse_value_string(value)),
        "enabled" => game.enabled = Some(parse_bool(value, line_no, key)?),
        "monitor" => game.monitor = Some(parse_value_string(value)),
        "output-width" => game.output_width = Some(parse_value_string(value)),
        "output-height" => game.output_height = Some(parse_value_string(value)),
        "game-width" => game.game_width = Some(parse_value_string(value)),
        "game-height" => game.game_height = Some(parse_value_string(value)),
        "refresh" => game.refresh = Some(parse_value_string(value)),
        "fullscreen" => game.fullscreen = Some(parse_bool(value, line_no, key)?),
        "borderless" => game.borderless = Some(parse_bool(value, line_no, key)?),
        "suppress-overlays" => game.suppress_overlays = Some(parse_bool(value, line_no, key)?),
        "passthrough-pointer-lock" => {
            game.passthrough_pointer_lock = Some(parse_bool(value, line_no, key)?)
        }
        "bypass-spatial-camera" => {
            game.bypass_spatial_camera = Some(parse_bool(value, line_no, key)?)
        }
        other => {
            return Err(format!(
                "line {line_no}: unknown `game:` key `{other}` inside `gamescope:`"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::layout::RuntimeTuning;

    #[test]
    fn defaults_are_sane() {
        let gaming = RuntimeTuning::default().gaming;
        assert_eq!(gaming.games, vec!["steam_app_*", "gamescope"]);
        let gs = gaming.gamescope;
        assert!(gs.enabled);
        assert_eq!(gs.monitor, "focused");
        assert_eq!(gs.output_width, "auto");
        assert!(gs.fullscreen);
        assert!(!gs.borderless);
        assert!(gs.bypass_spatial_camera);
        assert!(gs.games.is_empty());
    }

    #[test]
    fn parses_gaming_games_list_and_matches() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gaming:
  games ["steam_app_*", "tf_linux64", "gamescope"]
end
"#,
        )
        .expect("config should parse");
        let gaming = &tuning.gaming;
        assert_eq!(gaming.games, vec!["steam_app_*", "tf_linux64", "gamescope"]);
        assert!(gaming.matches_game("steam_app_548430"));
        assert!(gaming.matches_game("tf_linux64"));
        assert!(gaming.matches_game("gamescope"));
        assert!(!gaming.matches_game("firefox"));
    }

    #[test]
    fn parses_nested_gamescope_under_gaming() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gaming:
  games ["steam_app_*"]
  gamescope:
    enabled false
    monitor "primary"
    output-width 2560
    game:
      app-id "steam_app_548430"
      enabled true
    end
  end
end
"#,
        )
        .expect("config should parse");
        assert_eq!(tuning.gaming.games, vec!["steam_app_*"]);
        let gs = &tuning.gaming.gamescope;
        assert!(!gs.enabled);
        assert_eq!(gs.monitor, "primary");
        assert_eq!(gs.output_width, "2560");
        assert_eq!(gs.games.len(), 1);
        assert_eq!(gs.games[0].app_id.as_deref(), Some("steam_app_548430"));
        assert_eq!(gs.games[0].enabled, Some(true));
    }

    #[test]
    fn back_compat_top_level_gamescope() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gamescope:
  enabled false
  borderless true
  game:
    app-id "steam_app_1"
    fullscreen false
  end
end
"#,
        )
        .expect("config should parse");
        // Default classifier list preserved; gamescope mapped into gaming.gamescope.
        assert_eq!(tuning.gaming.games, vec!["steam_app_*", "gamescope"]);
        let gs = &tuning.gaming.gamescope;
        assert!(!gs.enabled);
        assert!(gs.borderless);
        assert_eq!(gs.games[0].app_id.as_deref(), Some("steam_app_1"));
        assert_eq!(gs.games[0].fullscreen, Some(false));
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(
            RuntimeTuning::from_rune_str(
                "gaming:\n  gamescope:\n    enabled true\n    bogus-key 1\n  end\nend\n"
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_unterminated_game_block() {
        assert!(
            RuntimeTuning::from_rune_str(
                "gaming:\n  gamescope:\n    game:\n      app-id \"x\"\n  end\nend\n"
            )
            .is_none()
        );
    }
}
