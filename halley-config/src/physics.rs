use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physics {
    pub enabled: bool,
    pub damping: f32,
}

impl Default for Physics {
    fn default() -> Self {
        Self {
            enabled: true,
            damping: 0.65,
        }
    }
}

pub fn parse_physics(config: &RuneConfig) -> Physics {
    let defaults = Physics::default();
    Physics {
        enabled: config.get_or("physics.enabled", defaults.enabled),
        damping: finite_clamp(
            config.get_or("physics.damping", defaults.damping),
            0.05,
            1.0,
            defaults.damping,
        ),
    }
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_clamps_the_old_section() {
        let config = RuneConfig::from_str(
            r#"
physics:
  enabled false
  damping 4.0
end
"#,
        )
        .unwrap();
        assert_eq!(
            parse_physics(&config),
            Physics {
                enabled: false,
                damping: 1.0,
            }
        );
    }
}
