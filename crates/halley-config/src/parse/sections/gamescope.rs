use crate::layout::{GamescopeConfig, GamescopeGameProfile, RuntimeTuning};

/// Parse the top-level `gamescope:` section: scalar global defaults plus repeated
/// `game:` / `end` sub-blocks. Modeled on `parse/rules.rs` because the repeated,
/// identically-named `game:` blocks do not map cleanly onto the dotted RuneConfig
/// key model.
pub(crate) fn load_gamescope_section(raw: &str, out: &mut RuntimeTuning) -> Result<(), String> {
    let mut in_section = false;
    let mut started = false;
    let mut current_game: Option<GamescopeGameProfile> = None;
    let mut config = GamescopeConfig::default();

    for (line_no, raw_line) in raw.lines().enumerate() {
        let line_no = line_no + 1;
        let trimmed = strip_comment(raw_line);
        if trimmed.is_empty() {
            continue;
        }

        if !in_section {
            if trimmed == "gamescope:" {
                in_section = true;
                started = true;
            }
            continue;
        }

        if let Some(game) = current_game.as_mut() {
            if trimmed == "end" {
                out_push_game(&mut config, current_game.take().unwrap());
                continue;
            }
            parse_game_entry(game, trimmed, line_no)?;
            continue;
        }

        if trimmed == "game:" {
            current_game = Some(GamescopeGameProfile::default());
            continue;
        }
        if trimmed == "end" {
            in_section = false;
            continue;
        }
        parse_global_entry(&mut config, trimmed, line_no)?;
    }

    if current_game.is_some() {
        return Err("unterminated `game:` block in `gamescope:` section".to_string());
    }

    if started {
        out.gamescope = config;
    }
    Ok(())
}

fn out_push_game(config: &mut GamescopeConfig, game: GamescopeGameProfile) {
    config.games.push(game);
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
        let gs = RuntimeTuning::default().gamescope;
        assert!(gs.enabled);
        assert_eq!(gs.monitor, "focused");
        assert_eq!(gs.output_width, "auto");
        assert!(gs.fullscreen);
        assert!(!gs.borderless);
        assert!(gs.bypass_spatial_camera);
        assert!(gs.games.is_empty());
    }

    #[test]
    fn parses_global_scalars() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gamescope:
  enabled false
  monitor "primary"
  output-width 2560
  output-height "auto"
  fullscreen false
  borderless true
  suppress-overlays false
end
"#,
        )
        .expect("config should parse");
        let gs = &tuning.gamescope;
        assert!(!gs.enabled);
        assert_eq!(gs.monitor, "primary");
        assert_eq!(gs.output_width, "2560");
        assert_eq!(gs.output_height, "auto");
        assert!(!gs.fullscreen);
        assert!(gs.borderless);
        assert!(!gs.suppress_overlays);
    }

    #[test]
    fn parses_multiple_game_blocks_with_opt_out() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gamescope:
  enabled true
  game:
    name "Deep Rock Galactic"
    app-id "steam_app_548430"
    enabled true
  end
  game:
    name "Example Opt Out"
    app-id "steam_app_000000"
    enabled false
  end
end
"#,
        )
        .expect("config should parse");
        let gs = &tuning.gamescope;
        assert_eq!(gs.games.len(), 2);
        assert_eq!(gs.games[0].app_id.as_deref(), Some("steam_app_548430"));
        assert_eq!(gs.games[0].enabled, Some(true));
        assert_eq!(gs.games[1].app_id.as_deref(), Some("steam_app_000000"));
        assert_eq!(gs.games[1].enabled, Some(false));
    }

    #[test]
    fn per_game_overrides_global_key() {
        let tuning = RuntimeTuning::from_rune_str(
            r#"
gamescope:
  fullscreen true
  game:
    app-id "steam_app_1"
    fullscreen false
    borderless true
    game-width 1920
  end
end
"#,
        )
        .expect("config should parse");
        let game = &tuning.gamescope.games[0];
        assert_eq!(game.fullscreen, Some(false));
        assert_eq!(game.borderless, Some(true));
        assert_eq!(game.game_width.as_deref(), Some("1920"));
        // Unset per-game keys stay None (inherit at resolve time).
        assert_eq!(game.refresh, None);
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(
            RuntimeTuning::from_rune_str("gamescope:\n  enabled true\n  bogus-key 1\nend\n")
                .is_none()
        );
    }

    #[test]
    fn rejects_unterminated_game_block() {
        // `game:` opened but never closed before end-of-input.
        assert!(RuntimeTuning::from_rune_str("gamescope:\n  game:\n    app-id \"x\"\n").is_none());
    }
}
