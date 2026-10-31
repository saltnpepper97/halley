use rune_cfg::RuneConfig;

pub const MIN_HISTORY_LENGTH: usize = 1;
pub const MAX_HISTORY_LENGTH: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trail {
    pub history_length: usize,
    pub wrap: bool,
}

impl Default for Trail {
    fn default() -> Self {
        Self {
            history_length: 32,
            wrap: true,
        }
    }
}

pub fn parse_trail(config: &RuneConfig) -> Trail {
    let defaults = Trail::default();
    let history_length = config
        .get_or("trail.history-length", defaults.history_length as u64)
        .clamp(MIN_HISTORY_LENGTH as u64, MAX_HISTORY_LENGTH as u64)
        as usize;
    Trail {
        history_length,
        wrap: config.get_or("trail.wrap", defaults.wrap),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_clamps_history_length() {
        let low = RuneConfig::from_str("trail:\n  history-length 0\n  wrap false\nend\n").unwrap();
        assert_eq!(parse_trail(&low).history_length, MIN_HISTORY_LENGTH);
        assert!(!parse_trail(&low).wrap);

        let high = RuneConfig::from_str("trail:\n  history-length 9999\nend\n").unwrap();
        assert_eq!(parse_trail(&high).history_length, MAX_HISTORY_LENGTH);
    }
}
