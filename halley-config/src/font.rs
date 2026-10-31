use rune_cfg::RuneConfig;

/// Global typography used by compositor-owned UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    pub family: String,
    pub size: u16,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: "monospace".to_string(),
            size: 11,
        }
    }
}

pub fn parse_font(config: &RuneConfig) -> Font {
    let defaults = Font::default();
    let family = config
        .get_optional::<String>("font.family")
        .ok()
        .flatten()
        .map(|family| family.trim().to_string())
        .filter(|family| !family.is_empty())
        .unwrap_or(defaults.family);
    let size = config
        .get_or::<u64>("font.size", u64::from(defaults.size))
        .clamp(6, 96) as u16;
    Font { family, size }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_family_with_weight_suffix_and_size() {
        let config = RuneConfig::from_str(
            r#"
font:
  family "CommitMono Nerd Font Bold"
  size 12
end
"#,
        )
        .unwrap();

        assert_eq!(
            parse_font(&config),
            Font {
                family: "CommitMono Nerd Font Bold".to_string(),
                size: 12,
            }
        );
    }
}
