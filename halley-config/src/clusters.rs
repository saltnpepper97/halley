use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClusterLayout {
    Tiling,
    #[default]
    Stacking,
}

impl ClusterLayout {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "tiling" => Some(Self::Tiling),
            "stacking" => Some(Self::Stacking),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClusterBloomDirection {
    #[default]
    Clockwise,
    CounterClockwise,
}

impl ClusterBloomDirection {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "clockwise" => Some(Self::Clockwise),
            "counter-clockwise" | "counter_clockwise" => Some(Self::CounterClockwise),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterTiling {
    pub new_on_top: bool,
    pub gaps_inner_px: f32,
    pub gaps_outer_px: f32,
    pub max_stack: usize,
    pub overflow_show_icons: bool,
}

impl Default for ClusterTiling {
    fn default() -> Self {
        Self {
            new_on_top: false,
            gaps_inner_px: 20.0,
            gaps_outer_px: 20.0,
            max_stack: 4,
            overflow_show_icons: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClusterStacking {
    pub max_visible: usize,
}

impl Default for ClusterStacking {
    fn default() -> Self {
        Self { max_visible: 5 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clusters {
    pub default_layout: ClusterLayout,
    pub join_dwell_ms: u64,
    /// Legacy compatibility setting. Manual bloom joining uses the real
    /// window/core bounds plus the Field landmark gap.
    pub join_distance_px: f32,
    pub show_icons: bool,
    pub bloom_direction: ClusterBloomDirection,
    pub tiling: ClusterTiling,
    pub stacking: ClusterStacking,
}

impl Default for Clusters {
    fn default() -> Self {
        Self {
            default_layout: ClusterLayout::Stacking,
            join_dwell_ms: 2_000,
            join_distance_px: 280.0,
            show_icons: true,
            bloom_direction: ClusterBloomDirection::Clockwise,
            tiling: ClusterTiling::default(),
            stacking: ClusterStacking::default(),
        }
    }
}

fn optional<T>(config: &RuneConfig, key: &str) -> Option<T>
where
    T: TryFrom<rune_cfg::Value, Error = rune_cfg::RuneError>,
{
    crate::aliases::spellings(key)
        .iter()
        .find_map(|path| config.get_optional::<T>(path).ok().flatten())
}

pub fn parse_clusters(config: &RuneConfig) -> Clusters {
    let defaults = Clusters::default();
    let default_layout = optional::<String>(config, "clusters.default-layout")
        .and_then(|value| ClusterLayout::parse(&value))
        .unwrap_or(defaults.default_layout);
    let bloom_direction = optional::<String>(config, "clusters.bloom-direction")
        .and_then(|value| ClusterBloomDirection::parse(&value))
        .unwrap_or(defaults.bloom_direction);

    Clusters {
        default_layout,
        join_dwell_ms: optional(config, "clusters.join-dwell-ms")
            .unwrap_or(defaults.join_dwell_ms)
            .clamp(50, 60_000),
        join_distance_px: optional(config, "clusters.join-distance-px")
            .unwrap_or(defaults.join_distance_px)
            .clamp(1.0, 4_096.0),
        show_icons: optional(config, "clusters.show-icons").unwrap_or(defaults.show_icons),
        bloom_direction,
        tiling: ClusterTiling {
            new_on_top: optional(config, "clusters.tiling.new-on-top")
                .unwrap_or(defaults.tiling.new_on_top),
            gaps_inner_px: optional(config, "clusters.tiling.gaps-inner")
                .unwrap_or(defaults.tiling.gaps_inner_px)
                .clamp(0.0, 256.0),
            gaps_outer_px: optional(config, "clusters.tiling.gaps-outer")
                .unwrap_or(defaults.tiling.gaps_outer_px)
                .clamp(0.0, 512.0),
            max_stack: optional(config, "clusters.tiling.max-stack")
                .unwrap_or(defaults.tiling.max_stack)
                .clamp(0, 64),
            overflow_show_icons: optional(config, "clusters.tiling.overflow-show-icons")
                .unwrap_or(defaults.tiling.overflow_show_icons),
        },
        stacking: ClusterStacking {
            max_visible: optional(config, "clusters.stacking.max-visible")
                .unwrap_or(defaults.stacking.max_visible)
                .clamp(0, 64),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_cluster_section_parses_and_clamps() {
        let config = RuneConfig::from_str(
            r#"
clusters:
  default-layout "tiling"
  join-dwell-ms 2500
  join-distance-px 320.0
  show-icons false
  bloom-direction "counter-clockwise"
  tiling:
    new-on-top true
    gaps-inner 12.0
    gaps-outer 30.0
    max-stack 6
    overflow-show-icons false
  end
  stacking:
    max-visible 7
  end
end
"#,
        )
        .unwrap();

        let parsed = parse_clusters(&config);
        assert_eq!(parsed.default_layout, ClusterLayout::Tiling);
        assert_eq!(parsed.join_dwell_ms, 2_500);
        assert_eq!(parsed.join_distance_px, 320.0);
        assert!(!parsed.show_icons);
        assert_eq!(
            parsed.bloom_direction,
            ClusterBloomDirection::CounterClockwise
        );
        assert!(parsed.tiling.new_on_top);
        assert_eq!(parsed.tiling.gaps_inner_px, 12.0);
        assert_eq!(parsed.tiling.gaps_outer_px, 30.0);
        assert_eq!(parsed.tiling.max_stack, 6);
        assert!(!parsed.tiling.overflow_show_icons);
        assert_eq!(parsed.stacking.max_visible, 7);
    }

    #[test]
    fn old_halley_keys_remain_compatible() {
        let config = RuneConfig::from_str(
            r#"
clusters:
  cluster-dwell-ms 1800
  distance-px 240.0
end
tile:
  max-stack 3
  queue-show-icons false
end
stacking:
  max-visible 4
end
"#,
        )
        .unwrap();

        let parsed = parse_clusters(&config);
        assert_eq!(parsed.join_dwell_ms, 1_800);
        assert_eq!(parsed.join_distance_px, 240.0);
        assert_eq!(parsed.tiling.max_stack, 3);
        assert!(!parsed.tiling.overflow_show_icons);
        assert_eq!(parsed.stacking.max_visible, 4);
    }
}
