use crate::layout::RuntimeTuning;

pub(crate) fn load_autostart_section(raw: &str, out: &mut RuntimeTuning) {
    let mut in_autostart = false;
    out.autostart_once.clear();
    out.autostart_on_reload.clear();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if !in_autostart {
            if trimmed == "autostart:" {
                in_autostart = true;
            }
            continue;
        }

        if trimmed == "end" {
            break;
        }

        if let Some(command) = parse_autostart_command(trimmed, "once") {
            out.autostart_once.push(command);
            continue;
        }

        if let Some(command) = parse_autostart_command(trimmed, "on-reload") {
            out.autostart_on_reload.push(command);
        }
    }
}

fn parse_autostart_command(line: &str, directive: &str) -> Option<String> {
    let rest = line.strip_prefix(directive)?.trim();
    if !rest.starts_with('"') {
        return None;
    }
    let rest = &rest[1..];
    let mut escaped = false;
    let mut command = String::new();
    for ch in rest.chars() {
        if escaped {
            command.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(command.trim().to_string()).filter(|value| !value.is_empty()),
            _ => command.push(ch),
        }
    }
    None
}

