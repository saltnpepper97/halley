use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimationCurve {
    #[default]
    Linear,
    EaseInOutCubic,
    EaseOutQuad,
    EaseOutCubic,
    EaseOutExpo,
    Elastic,
}

impl AnimationCurve {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "linear" => Some(Self::Linear),
            "ease-in-out-cubic" => Some(Self::EaseInOutCubic),
            "ease-out-quad" => Some(Self::EaseOutQuad),
            "ease-out-cubic" => Some(Self::EaseOutCubic),
            "ease-out-expo" => Some(Self::EaseOutExpo),
            "elastic" => Some(Self::Elastic),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EasingMotion {
    pub duration_ms: u32,
    pub curve: AnimationCurve,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringMotion {
    pub damping_ratio: f64,
    pub stiffness: f64,
}

impl Default for SpringMotion {
    fn default() -> Self {
        Self {
            damping_ratio: 1.0,
            stiffness: 800.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimationMotion {
    Easing(EasingMotion),
    Spring(SpringMotion),
}

impl AnimationMotion {
    fn parse(config: &RuneConfig, path: &str, default: Self) -> Self {
        let kind = config
            .get_optional::<String>(&format!("{path}.motion"))
            .ok()
            .flatten();

        match kind.as_deref() {
            Some("spring") => {
                let defaults = match default {
                    Self::Spring(defaults) => defaults,
                    Self::Easing(_) => SpringMotion::default(),
                };
                Self::Spring(SpringMotion {
                    damping_ratio: finite_clamp(
                        config.get_or(&format!("{path}.damping-ratio"), defaults.damping_ratio),
                        0.1,
                        10.0,
                        defaults.damping_ratio,
                    ),
                    stiffness: finite_clamp(
                        config.get_or(&format!("{path}.stiffness"), defaults.stiffness),
                        1.0,
                        100_000.0,
                        defaults.stiffness,
                    ),
                })
            }
            Some("easing") => {
                let defaults = match default {
                    Self::Easing(defaults) => defaults,
                    Self::Spring(_) => EasingMotion {
                        duration_ms: 250,
                        curve: AnimationCurve::EaseOutCubic,
                    },
                };
                Self::Easing(EasingMotion {
                    duration_ms: config
                        .get_or(&format!("{path}.duration-ms"), defaults.duration_ms),
                    curve: config
                        .get_optional::<String>(&format!("{path}.curve"))
                        .ok()
                        .flatten()
                        .and_then(|curve| AnimationCurve::parse(&curve))
                        .unwrap_or(defaults.curve),
                })
            }
            _ => default,
        }
    }
}

fn finite_clamp(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowOpenAnimationType {
    #[default]
    CenterOut,
    Fade,
    Launch,
}

impl WindowOpenAnimationType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "center-out" => Some(Self::CenterOut),
            "fade" => Some(Self::Fade),
            "launch" => Some(Self::Launch),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowOpenAnimation {
    pub enabled: bool,
    pub animation_type: WindowOpenAnimationType,
    pub motion: AnimationMotion,
    /// Optional fragment-shader path. Relative paths resolve from the
    /// directory that contains `halley.rune`. Empty or omitted means the
    /// configured `type` draws the pixels.
    pub custom_shader: Option<String>,
}

impl Default for WindowOpenAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            animation_type: WindowOpenAnimationType::default(),
            motion: AnimationMotion::Easing(EasingMotion {
                duration_ms: 300,
                curve: AnimationCurve::Linear,
            }),
            custom_shader: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowCloseAnimationType {
    #[default]
    Shrink,
    Fade,
    Retract,
}

impl WindowCloseAnimationType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "shrink" => Some(Self::Shrink),
            "fade" => Some(Self::Fade),
            "retract" => Some(Self::Retract),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowCloseAnimation {
    pub enabled: bool,
    pub animation_type: WindowCloseAnimationType,
    pub duration_ms: u32,
    /// Optional fragment-shader path. Relative paths resolve from the
    /// directory that contains `halley.rune`. Empty or omitted means the
    /// configured `type` draws the pixels.
    pub custom_shader: Option<String>,
}

impl Default for WindowCloseAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            animation_type: WindowCloseAnimationType::default(),
            duration_ms: 270,
            custom_shader: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FullscreenAnimation {
    pub enabled: bool,
    pub motion: AnimationMotion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeAnimation {
    pub enabled: bool,
    pub duration_ms: u32,
    pub collapse_duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmoothResizeAnimation {
    pub enabled: bool,
    pub duration_ms: u32,
}

impl Default for SmoothResizeAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_ms: 90,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClusterTilingAnimation {
    pub open_duration_ms: u32,
    pub close_duration_ms: u32,
    pub reflow_duration_ms: u32,
    pub stagger_ms: u32,
}

impl Default for ClusterTilingAnimation {
    fn default() -> Self {
        Self {
            open_duration_ms: 300,
            close_duration_ms: 420,
            reflow_duration_ms: 240,
            stagger_ms: 55,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClusterStackingAnimation {
    pub open_duration_ms: u32,
    pub close_duration_ms: u32,
    pub cycle_duration_ms: u32,
}

impl Default for ClusterStackingAnimation {
    fn default() -> Self {
        Self {
            open_duration_ms: 240,
            close_duration_ms: 360,
            cycle_duration_ms: 220,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClusterAnimation {
    pub enabled: bool,
    pub tiling: ClusterTilingAnimation,
    pub stacking: ClusterStackingAnimation,
}

impl Default for ClusterAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            tiling: ClusterTilingAnimation::default(),
            stacking: ClusterStackingAnimation::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaximizeAnimation {
    pub enabled: bool,
    pub motion: AnimationMotion,
}

impl Default for MaximizeAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            motion: AnimationMotion::Easing(EasingMotion {
                duration_ms: 240,
                curve: AnimationCurve::EaseInOutCubic,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrangeAnimation {
    pub enabled: bool,
    pub motion: AnimationMotion,
}

impl Default for ArrangeAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            motion: AnimationMotion::Easing(EasingMotion {
                duration_ms: 360,
                curve: AnimationCurve::EaseInOutCubic,
            }),
        }
    }
}

impl Default for NodeAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_ms: 280,
            collapse_duration_ms: 280,
        }
    }
}

impl Default for FullscreenAnimation {
    fn default() -> Self {
        Self {
            enabled: true,
            motion: AnimationMotion::Spring(SpringMotion::default()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Animations {
    pub enabled: bool,
    pub window_open: WindowOpenAnimation,
    pub window_close: WindowCloseAnimation,
    pub fullscreen: FullscreenAnimation,
    pub maximize: MaximizeAnimation,
    pub arrange: ArrangeAnimation,
    pub smooth_resize: SmoothResizeAnimation,
    pub node: NodeAnimation,
    pub cluster: ClusterAnimation,
}

impl Default for Animations {
    fn default() -> Self {
        Self {
            enabled: true,
            window_open: WindowOpenAnimation::default(),
            window_close: WindowCloseAnimation::default(),
            fullscreen: FullscreenAnimation::default(),
            maximize: MaximizeAnimation::default(),
            arrange: ArrangeAnimation::default(),
            smooth_resize: SmoothResizeAnimation::default(),
            node: NodeAnimation::default(),
            cluster: ClusterAnimation::default(),
        }
    }
}

pub fn parse_animations(config: &RuneConfig) -> Animations {
    let defaults = Animations::default();
    let animation_type = config
        .get_optional::<String>("animations.window-open.type")
        .ok()
        .flatten()
        .and_then(|value| WindowOpenAnimationType::parse(&value))
        .unwrap_or(defaults.window_open.animation_type);
    let default_easing = match defaults.window_open.motion {
        AnimationMotion::Easing(easing) => easing,
        AnimationMotion::Spring(_) => unreachable!("window-open defaults use easing motion"),
    };
    let curve = config
        .get_optional::<String>("animations.window-open.curve")
        .ok()
        .flatten()
        .and_then(|curve| AnimationCurve::parse(&curve))
        .unwrap_or(default_easing.curve);
    let configured_easing = AnimationMotion::Easing(EasingMotion {
        duration_ms: config.get_or(
            "animations.window-open.duration-ms",
            default_easing.duration_ms,
        ),
        curve,
    });
    let window_open_motion =
        AnimationMotion::parse(config, "animations.window-open", configured_easing);

    Animations {
        enabled: config.get_or("animations.enabled", defaults.enabled),
        window_open: WindowOpenAnimation {
            enabled: config.get_or(
                "animations.window-open.enabled",
                defaults.window_open.enabled,
            ),
            animation_type,
            motion: window_open_motion,
            custom_shader: optional_shader_path(config, "animations.window-open.custom-shader"),
        },
        window_close: WindowCloseAnimation {
            enabled: config.get_or(
                "animations.window-close.enabled",
                defaults.window_close.enabled,
            ),
            animation_type: config
                .get_optional::<String>("animations.window-close.type")
                .ok()
                .flatten()
                .and_then(|value| WindowCloseAnimationType::parse(&value))
                .unwrap_or(defaults.window_close.animation_type),
            duration_ms: config.get_or(
                "animations.window-close.duration-ms",
                defaults.window_close.duration_ms,
            ),
            custom_shader: optional_shader_path(config, "animations.window-close.custom-shader"),
        },
        fullscreen: FullscreenAnimation {
            enabled: config.get_or("animations.fullscreen.enabled", defaults.fullscreen.enabled),
            motion: AnimationMotion::parse(
                config,
                "animations.fullscreen",
                defaults.fullscreen.motion,
            ),
        },
        maximize: {
            let default_easing = match defaults.maximize.motion {
                AnimationMotion::Easing(easing) => easing,
                AnimationMotion::Spring(_) => unreachable!("maximize defaults use easing motion"),
            };
            let configured_easing = AnimationMotion::Easing(EasingMotion {
                duration_ms: config.get_or(
                    "animations.maximize.duration-ms",
                    default_easing.duration_ms,
                ),
                curve: config
                    .get_optional::<String>("animations.maximize.curve")
                    .ok()
                    .flatten()
                    .and_then(|curve| AnimationCurve::parse(&curve))
                    .unwrap_or(default_easing.curve),
            });
            MaximizeAnimation {
                enabled: config.get_or("animations.maximize.enabled", defaults.maximize.enabled),
                motion: AnimationMotion::parse(config, "animations.maximize", configured_easing),
            }
        },
        arrange: {
            let default_easing = match defaults.arrange.motion {
                AnimationMotion::Easing(easing) => easing,
                AnimationMotion::Spring(_) => unreachable!("arrange defaults use easing motion"),
            };
            let configured_easing = AnimationMotion::Easing(EasingMotion {
                duration_ms: config
                    .get_or("animations.arrange.duration-ms", default_easing.duration_ms),
                curve: config
                    .get_optional::<String>("animations.arrange.curve")
                    .ok()
                    .flatten()
                    .and_then(|curve| AnimationCurve::parse(&curve))
                    .unwrap_or(default_easing.curve),
            });
            ArrangeAnimation {
                enabled: config.get_or("animations.arrange.enabled", defaults.arrange.enabled),
                motion: AnimationMotion::parse(config, "animations.arrange", configured_easing),
            }
        },
        smooth_resize: SmoothResizeAnimation {
            enabled: config.get_or(
                "animations.smooth-resize.enabled",
                defaults.smooth_resize.enabled,
            ),
            duration_ms: config
                .get_or(
                    "animations.smooth-resize.duration-ms",
                    defaults.smooth_resize.duration_ms,
                )
                .clamp(1, 2_000),
        },
        node: NodeAnimation {
            enabled: config.get_or("animations.node.enabled", defaults.node.enabled),
            duration_ms: config.get_or("animations.node.duration-ms", defaults.node.duration_ms),
            collapse_duration_ms: config.get_or(
                "animations.node.collapse-duration-ms",
                defaults.node.collapse_duration_ms,
            ),
        },
        cluster: ClusterAnimation {
            enabled: config.get_or("animations.cluster.enabled", defaults.cluster.enabled),
            tiling: ClusterTilingAnimation {
                open_duration_ms: config.get_or(
                    "animations.cluster.tiling.open-duration-ms",
                    defaults.cluster.tiling.open_duration_ms,
                ),
                close_duration_ms: config.get_or(
                    "animations.cluster.tiling.close-duration-ms",
                    defaults.cluster.tiling.close_duration_ms,
                ),
                reflow_duration_ms: config.get_or(
                    "animations.cluster.tiling.reflow-duration-ms",
                    defaults.cluster.tiling.reflow_duration_ms,
                ),
                stagger_ms: config.get_or(
                    "animations.cluster.tiling.stagger-ms",
                    defaults.cluster.tiling.stagger_ms,
                ),
            },
            stacking: ClusterStackingAnimation {
                open_duration_ms: config.get_or(
                    "animations.cluster.stacking.open-duration-ms",
                    defaults.cluster.stacking.open_duration_ms,
                ),
                close_duration_ms: config.get_or(
                    "animations.cluster.stacking.close-duration-ms",
                    defaults.cluster.stacking.close_duration_ms,
                ),
                cycle_duration_ms: config.get_or(
                    "animations.cluster.stacking.cycle-duration-ms",
                    defaults.cluster.stacking.cycle_duration_ms,
                ),
            },
        },
    }
}

fn optional_shader_path(config: &RuneConfig, path: &str) -> Option<String> {
    config
        .get_optional::<String>(path)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn load_animations() -> Animations {
    let Some(path) = crate::config_path() else {
        eprintln!("animations: no config path resolvable, using defaults");
        return Animations::default();
    };

    if let Err(err) = crate::bootstrap_default_config_at(&path) {
        eprintln!("animations: failed to bootstrap default config: {err}");
    }

    match RuneConfig::from_file(&path) {
        Ok(config) => parse_animations(&config),
        Err(err) => {
            eprintln!("animations: failed to load {path:?}, using defaults: {err}");
            Animations::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_collapse_defaults_independently_of_existing_durations() {
        let config = RuneConfig::from_str("animations:\n  node:\n    duration-ms 910\n  end\n  window-close:\n    duration-ms 1800\n  end\nend\n").unwrap();
        let animations = parse_animations(&config);
        assert_eq!(animations.node.duration_ms, 910);
        assert_eq!(animations.node.collapse_duration_ms, 280);
        assert_eq!(animations.window_close.duration_ms, 1800);
        assert_eq!(Animations::default().node.collapse_duration_ms, 280);
    }

    #[test]
    fn parses_independent_node_collapse_duration_including_zero() {
        for duration in [0, 470] {
            let config = RuneConfig::from_str(&format!("animations:\n  node:\n    duration-ms 910\n    collapse-duration-ms {duration}\n  end\nend\n")).unwrap();
            let animations = parse_animations(&config);
            assert_eq!(animations.node.duration_ms, 910);
            assert_eq!(animations.node.collapse_duration_ms, duration);
        }
    }

    #[test]
    fn parses_window_open_animation() {
        let config = RuneConfig::from_str(
            r#"
animations:
  enabled true
  window-open:
    enabled false
    type "fade"
    duration-ms 450
    curve "elastic"
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config),
            Animations {
                enabled: true,
                window_open: WindowOpenAnimation {
                    enabled: false,
                    animation_type: WindowOpenAnimationType::Fade,
                    motion: AnimationMotion::Easing(EasingMotion {
                        duration_ms: 450,
                        curve: AnimationCurve::Elastic,
                    }),
                    custom_shader: None,
                },
                window_close: WindowCloseAnimation::default(),
                fullscreen: FullscreenAnimation::default(),
                maximize: MaximizeAnimation::default(),
                arrange: ArrangeAnimation::default(),
                smooth_resize: SmoothResizeAnimation::default(),
                node: NodeAnimation::default(),
                cluster: ClusterAnimation::default(),
            }
        );
    }

    #[test]
    fn parses_window_close_animation() {
        let config = RuneConfig::from_str(
            r#"
animations:
  window-close:
    enabled false
    type "fade"
    duration-ms 410
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).window_close,
            WindowCloseAnimation {
                enabled: false,
                animation_type: WindowCloseAnimationType::Fade,
                duration_ms: 410,
                custom_shader: None,
            }
        );
    }

    #[test]
    fn parses_smooth_resize_animation_and_clamps_duration() {
        let config = RuneConfig::from_str(
            r#"
animations:
  smooth-resize:
    enabled false
    duration-ms 5000
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).smooth_resize,
            SmoothResizeAnimation {
                enabled: false,
                duration_ms: 2_000,
            }
        );
    }

    #[test]
    fn parses_launch_counterpart_for_window_close() {
        let config = RuneConfig::from_str(
            r#"
animations:
  window-close:
    type "retract"
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).window_close.animation_type,
            WindowCloseAnimationType::Retract
        );
    }

    #[test]
    fn missing_section_uses_center_out_defaults() {
        let config = RuneConfig::from_str("keybinds:\n  mod \"super\"\nend\n")
            .expect("valid rune-cfg source");

        assert_eq!(parse_animations(&config), Animations::default());
        assert!(Animations::default().enabled);
        assert!(Animations::default().window_open.enabled);
        assert_eq!(
            Animations::default().window_close,
            WindowCloseAnimation {
                enabled: true,
                animation_type: WindowCloseAnimationType::Shrink,
                duration_ms: 270,
                custom_shader: None,
            }
        );
        assert_eq!(
            Animations::default().window_open.animation_type,
            WindowOpenAnimationType::CenterOut
        );
        assert_eq!(
            Animations::default().window_open.motion,
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 300,
                curve: AnimationCurve::Linear,
            })
        );
    }

    #[test]
    fn window_open_style_does_not_select_motion_defaults() {
        let parse = |animation_type| {
            let config = RuneConfig::from_str(&format!(
                r#"
animations:
  window-open:
    type "{animation_type}"
  end
end
"#
            ))
            .expect("valid rune-cfg source");
            parse_animations(&config).window_open
        };

        let center_out = parse("center-out");
        let fade = parse("fade");
        let launch = parse("launch");

        assert_eq!(
            center_out.animation_type,
            WindowOpenAnimationType::CenterOut
        );
        assert_eq!(fade.animation_type, WindowOpenAnimationType::Fade);
        assert_eq!(launch.animation_type, WindowOpenAnimationType::Launch);
        assert_eq!(center_out.motion, fade.motion);
        assert_eq!(center_out.motion, launch.motion);
    }

    #[test]
    fn removed_animation_names_have_no_compatibility_aliases() {
        assert_eq!(WindowOpenAnimationType::parse("elastic"), None);
        assert_eq!(AnimationCurve::parse("ease-out-back"), None);
        assert_eq!(
            WindowOpenAnimationType::parse("fade"),
            Some(WindowOpenAnimationType::Fade)
        );
        assert_eq!(
            WindowOpenAnimationType::parse("launch"),
            Some(WindowOpenAnimationType::Launch)
        );
        assert_eq!(
            AnimationCurve::parse("elastic"),
            Some(AnimationCurve::Elastic)
        );
        assert_eq!(
            AnimationCurve::parse("ease-in-out-cubic"),
            Some(AnimationCurve::EaseInOutCubic)
        );
    }

    #[test]
    fn invalid_values_fall_back_to_center_out_defaults() {
        let config = RuneConfig::from_str(
            r#"
animations:
  window-open:
    type "stretchy"
    curve "wobbly"
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        let animation = parse_animations(&config).window_open;
        assert_eq!(animation.animation_type, WindowOpenAnimationType::CenterOut);
        assert_eq!(
            animation.motion,
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 300,
                curve: AnimationCurve::Linear,
            })
        );

        let config = RuneConfig::from_str(
            r#"
animations:
  window-close:
    type "vanish"
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        assert_eq!(
            parse_animations(&config).window_close.animation_type,
            WindowCloseAnimationType::Shrink
        );
    }

    #[test]
    fn fullscreen_defaults_to_critical_spring() {
        let config = RuneConfig::from_str(
            r#"
animations:
  fullscreen:
    enabled false
    motion "spring"
    damping-ratio 0.8
    stiffness 600.0
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        let fullscreen = parse_animations(&config).fullscreen;
        assert!(!fullscreen.enabled);
        assert_eq!(
            fullscreen.motion,
            AnimationMotion::Spring(SpringMotion {
                damping_ratio: 0.8,
                stiffness: 600.0,
            })
        );
    }

    #[test]
    fn fullscreen_can_use_easing_motion() {
        let config = RuneConfig::from_str(
            r#"
animations:
  fullscreen:
    motion "easing"
    duration-ms 180
    curve "ease-out-expo"
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).fullscreen.motion,
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 180,
                curve: AnimationCurve::EaseOutExpo,
            })
        );
    }

    #[test]
    fn maximize_keeps_legacy_duration_and_ease_in_out_default() {
        let config = RuneConfig::from_str(
            r#"
animations:
  maximize:
    duration-ms 360
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).maximize.motion,
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 360,
                curve: AnimationCurve::EaseInOutCubic,
            })
        );
    }

    #[test]
    fn arrange_supports_smooth_easing_and_spring_motion() {
        let easing = RuneConfig::from_str(
            r#"
animations:
  arrange:
    enabled false
    duration-ms 420
    curve "ease-out-cubic"
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        assert_eq!(
            parse_animations(&easing).arrange,
            ArrangeAnimation {
                enabled: false,
                motion: AnimationMotion::Easing(EasingMotion {
                    duration_ms: 420,
                    curve: AnimationCurve::EaseOutCubic,
                }),
            }
        );

        let spring = RuneConfig::from_str(
            r#"
animations:
  arrange:
    motion "spring"
    damping-ratio 0.9
    stiffness 500.0
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        assert_eq!(
            parse_animations(&spring).arrange.motion,
            AnimationMotion::Spring(SpringMotion {
                damping_ratio: 0.9,
                stiffness: 500.0,
            })
        );
    }

    #[test]
    fn maximize_supports_fullscreen_motion_knobs() {
        let spring = RuneConfig::from_str(
            r#"
animations:
  maximize:
    motion "spring"
    damping-ratio 0.7
    stiffness 650.0
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        assert_eq!(
            parse_animations(&spring).maximize.motion,
            AnimationMotion::Spring(SpringMotion {
                damping_ratio: 0.7,
                stiffness: 650.0,
            })
        );

        let easing = RuneConfig::from_str(
            r#"
animations:
  maximize:
    motion "easing"
    duration-ms 180
    curve "ease-out-expo"
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        assert_eq!(
            parse_animations(&easing).maximize.motion,
            AnimationMotion::Easing(EasingMotion {
                duration_ms: 180,
                curve: AnimationCurve::EaseOutExpo,
            })
        );
    }

    #[test]
    fn spring_values_are_constrained_to_stable_ranges() {
        let config = RuneConfig::from_str(
            r#"
animations:
  fullscreen:
    motion "spring"
    damping-ratio 0.0
    stiffness 999999.0
  end
end
"#,
        )
        .expect("valid rune-cfg source");

        assert_eq!(
            parse_animations(&config).fullscreen.motion,
            AnimationMotion::Spring(SpringMotion {
                damping_ratio: 0.1,
                stiffness: 100_000.0,
            })
        );
    }

    #[test]
    fn parses_custom_shader_paths_and_treats_blank_as_unset() {
        let config = RuneConfig::from_str(
            r#"
animations:
  window-open:
    custom-shader "shaders/open.frag"
  end
  window-close:
    custom-shader "  shaders/close.frag  "
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        let animations = parse_animations(&config);
        assert_eq!(
            animations.window_open.custom_shader.as_deref(),
            Some("shaders/open.frag")
        );
        assert_eq!(
            animations.window_close.custom_shader.as_deref(),
            Some("shaders/close.frag")
        );

        let blank = RuneConfig::from_str(
            r#"
animations:
  window-open:
    custom-shader "   "
  end
  window-close:
    custom-shader ""
  end
end
"#,
        )
        .expect("valid rune-cfg source");
        let animations = parse_animations(&blank);
        assert_eq!(animations.window_open.custom_shader, None);
        assert_eq!(animations.window_close.custom_shader, None);
    }
}
