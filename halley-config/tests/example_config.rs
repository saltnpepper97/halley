use halley_config::{
    Action, Direction, ModifierKey, MonitorTarget, OverlayColorMode, TrailDirection, parse_keybinds,
};
use rune_cfg::RuneConfig;

const EXAMPLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/halley.rune");
const SPLIT_EXAMPLE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../examples/split-config/halley.rune"
);

/// Real end-to-end check: parse the actual shipped example file through
/// real rune-cfg, not just hand-constructed strings like the unit tests
/// elsewhere in this crate use.
#[test]
fn example_config_parses_end_to_end() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let keybinds = parse_keybinds(&config).expect("example keybinds section parses");

    assert_eq!(keybinds.modifier, ModifierKey::Super);
    assert_eq!(keybinds.binds.len(), 62);

    let arrange = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::ArrangeVisible)
        .expect("arrange-visible bind present");
    assert_eq!(arrange.key, "a");
    assert!(arrange.modifiers.super_key);
    assert!(!arrange.modifiers.shift);

    let drag_pan = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::PointerDragPan)
        .expect("grabbed-window Field pan bind present");
    assert_eq!(drag_pan.key, "click-left");
    assert!(drag_pan.modifiers.super_key);
    assert!(drag_pan.modifiers.shift);

    let launcher = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::Spawn("fuzzel".into()))
        .expect("Fuzzel launcher bind present");
    assert_eq!(launcher.key, "d");
    assert!(launcher.modifiers.super_key);
    assert!(
        keybinds
            .binds
            .iter()
            .all(|bind| bind.action != Action::Spawn("halley-lift".into())),
        "the commented Halley Lift alternative must not become active"
    );

    let quit = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::Quit)
        .expect("quit bind present");
    assert_eq!(quit.key, "e");
    assert!(quit.modifiers.super_key);
    assert!(quit.modifiers.shift);

    let close = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::CloseFocusedWindow)
        .expect("close-focused bind present");
    assert_eq!(close.key, "q");
    assert!(close.modifiers.super_key);
    assert!(!close.modifiers.shift);

    let fullscreen = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ToggleFullscreen)
        .expect("toggle-fullscreen bind present");
    assert_eq!(fullscreen.key, "f");
    assert!(fullscreen.modifiers.super_key);
    assert!(!fullscreen.modifiers.shift);

    let maximize = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ToggleFieldMaximize)
        .expect("maximize-focused bind present");
    assert_eq!(maximize.key, "m");
    assert!(maximize.modifiers.super_key);

    let toggle_state = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ToggleState)
        .expect("toggle-state bind present");
    assert_eq!(toggle_state.key, "n");
    assert!(toggle_state.modifiers.super_key);

    let toggle_pin = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ToggleFocusedPin)
        .expect("toggle-focused-pin bind present");
    assert_eq!(toggle_pin.key, "p");
    assert!(toggle_pin.modifiers.super_key);
    assert!(!toggle_pin.repeat);

    let cluster_float = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ClusterToggleFloat)
        .expect("cluster-toggle-float bind present");
    assert_eq!(cluster_float.key, "v");
    assert!(cluster_float.modifiers.super_key);

    let bearings_show = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::BearingsShow)
        .expect("bearings-show bind present");
    assert_eq!(bearings_show.key, "z");
    assert!(bearings_show.modifiers.super_key);
    assert!(!bearings_show.modifiers.shift);

    let bearings_toggle = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::BearingsToggle)
        .expect("bearings-toggle bind present");
    assert_eq!(bearings_toggle.key, "z");
    assert!(bearings_toggle.modifiers.super_key);
    assert!(bearings_toggle.modifiers.shift);

    let field_left = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::FocusDirection(Direction::Left))
        .expect("contextual left-focus bind present");
    assert_eq!(field_left.key, "left");
    assert!(field_left.modifiers.super_key);

    let center = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::CenterLastFocused)
        .expect("center-last-focused bind present");
    assert_eq!(center.key, "h");
    assert!(center.modifiers.super_key);

    let tile_swap_down = keybinds
        .binds
        .iter()
        .find(|bind| bind.action == Action::ClusterTileSwap(Direction::Down))
        .expect("cluster down-swap bind present");
    assert_eq!(tile_swap_down.key, "down");
    assert!(tile_swap_down.modifiers.super_key);
    assert!(tile_swap_down.modifiers.ctrl);

    let monitor_right = keybinds
        .binds
        .iter()
        .find(|bind| {
            bind.action == Action::MonitorFocus(MonitorTarget::Direction(Direction::Right))
        })
        .expect("right monitor-focus bind present");
    assert_eq!(monitor_right.key, "right");
    assert!(monitor_right.modifiers.super_key);
    assert!(monitor_right.modifiers.shift);

    let terminal = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::OpenTerminal)
        .expect("open-terminal bind present");
    assert_eq!(terminal.key, "t");
    assert!(terminal.modifiers.super_key);

    for (key, command) in [
        (
            "XF86AudioRaiseVolume",
            "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+ --limit 1.0",
        ),
        (
            "XF86AudioLowerVolume",
            "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-",
        ),
        (
            "XF86AudioMute",
            "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
        ),
    ] {
        let binding = keybinds
            .binds
            .iter()
            .find(|binding| binding.key == key)
            .expect("media key bind present");
        assert_eq!(binding.modifiers, halley_config::Modifiers::default());
        assert_eq!(&binding.action, &Action::Spawn(command.to_string()));
        assert_eq!(binding.repeat, key != "XF86AudioMute");
    }

    let zoom_out = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ZoomOut)
        .expect("zoom-out bind present");
    assert_eq!(zoom_out.key, "minus");
    assert!(zoom_out.modifiers.super_key);

    let zoom_in = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ZoomIn)
        .expect("zoom-in bind present");
    assert_eq!(zoom_in.key, "equal");
    assert!(zoom_in.modifiers.super_key);

    let zoom_reset = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::ZoomReset)
        .expect("zoom-reset bind present");
    assert_eq!(zoom_reset.key, "0");
    assert!(zoom_reset.modifiers.super_key);
    assert!(zoom_reset.modifiers.shift);

    let screenshot = keybinds
        .binds
        .iter()
        .find(|b| b.action == Action::Screenshot)
        .expect("screenshot bind present");
    assert_eq!(screenshot.key, "Print");
    assert_eq!(screenshot.modifiers, halley_config::Modifiers::default());
}

#[test]
fn split_example_config_parses_end_to_end() {
    let runtime = halley_config::load_runtime_config_at(std::path::Path::new(SPLIT_EXAMPLE_PATH))
        .expect("split example and its gathered files parse");

    assert_eq!(runtime.keybinds.modifier, ModifierKey::Super);
    assert!(
        runtime
            .keybinds
            .binds
            .iter()
            .any(|bind| bind.action == Action::PointerDragPan),
        "split example includes grabbed-window Field panning"
    );
    assert_default_startup_workspaces(&runtime.autostart);
    assert_eq!(
        runtime.decorations.titlebars.button_position,
        halley_config::TitlebarButtonPosition::Right
    );
    assert_eq!(
        runtime.decorations.border_color_focused,
        halley_config::BorderColor {
            r: 0xf4 as f32 / 255.0,
            g: 0xf5 as f32 / 255.0,
            b: 0xf7 as f32 / 255.0,
        }
    );
    assert_eq!(
        runtime.decorations.titlebars.color_focused,
        halley_config::BorderColor {
            r: 0xd6 as f32 / 255.0,
            g: 0x5d as f32 / 255.0,
            b: 0x26 as f32 / 255.0,
        }
    );
    assert_eq!(runtime.cursor.theme, "Adwaita");
    assert!(runtime.cursor.hide_on_keyboard_nav);
    assert_eq!(
        runtime.overlays.zoom_indicator,
        halley_config::ZoomIndicator::default()
    );
    assert_eq!(
        runtime.field.pins.color,
        OverlayColorMode::Fixed {
            r: 0xd6 as f32 / 255.0,
            g: 0x5d as f32 / 255.0,
            b: 0x26 as f32 / 255.0,
            a: 1.0,
        }
    );
    assert_eq!(
        runtime.field.pins.background_color,
        OverlayColorMode::Fixed {
            r: 0x1d as f32 / 255.0,
            g: 0x20 as f32 / 255.0,
            b: 0x21 as f32 / 255.0,
            a: 1.0,
        }
    );
    assert!(runtime.animations.smooth_resize.enabled);
    assert!(
        runtime
            .keybinds
            .binds
            .iter()
            .any(|binding| binding.action == Action::Trail(TrailDirection::Previous))
    );
    assert!(
        runtime
            .keybinds
            .binds
            .iter()
            .any(|binding| binding.action == Action::Reload)
    );
}

#[test]
fn example_config_cluster_sections_parse() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let runtime = halley_config::parse_runtime_config(&config).expect("runtime config parses");

    assert_eq!(
        runtime.clusters.default_layout,
        halley_config::ClusterLayout::Stacking
    );
    assert_eq!(runtime.clusters.tiling.max_stack, 4);
    assert_eq!(runtime.clusters.stacking.max_visible, 5);
    assert!(runtime.animations.cluster.enabled);
    assert_eq!(
        runtime.decorations.titlebars.title_position,
        halley_config::TitlebarContentPosition::Center
    );
    assert_eq!(
        runtime.decorations.titlebars.button_position,
        halley_config::TitlebarButtonPosition::Right
    );
    assert_eq!(
        runtime.decorations.border_color_focused,
        halley_config::BorderColor {
            r: 0xf4 as f32 / 255.0,
            g: 0xf5 as f32 / 255.0,
            b: 0xf7 as f32 / 255.0,
        }
    );
    assert_eq!(
        runtime.decorations.titlebars.color_focused,
        halley_config::BorderColor {
            r: 0xd6 as f32 / 255.0,
            g: 0x5d as f32 / 255.0,
            b: 0x26 as f32 / 255.0,
        }
    );
    assert!(runtime.decorations.resize_using_border);
    assert_eq!(runtime.animations.cluster.tiling.open_duration_ms, 300);
    assert_eq!(runtime.animations.cluster.stacking.cycle_duration_ms, 220);
    assert!(
        runtime
            .keybinds
            .binds
            .iter()
            .any(|binding| binding.action == Action::ClusterSlot(10))
    );
}

#[test]
fn example_config_keeps_fps_overlay_disabled() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let runtime = halley_config::parse_runtime_config(&config).expect("runtime config parses");

    assert!(!runtime.debug.overlay_fps);
}

fn assert_default_startup_workspaces(autostart: &halley_config::Autostart) {
    assert!(autostart.once.is_empty());
    assert!(autostart.on_reload.is_empty());
    assert_eq!(autostart.clusters.len(), 12);

    for (index, cluster) in autostart.clusters.iter().enumerate() {
        let number = index + 1;
        assert_eq!(cluster.name, number.to_string());
        assert!(cluster.members.is_empty());
        assert_eq!(cluster.layout, None);
        assert_eq!(
            cluster.output.as_deref(),
            Some(if number <= 6 { "DP-1" } else { "DP-2" })
        );
    }
}

#[test]
fn example_config_keeps_environment_and_application_autostart_inactive() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let runtime = halley_config::parse_runtime_config(&config).expect("runtime config parses");

    assert!(runtime.env.is_empty());
    assert_default_startup_workspaces(&runtime.autostart);
}

#[test]
fn example_config_keeps_wallpaper_and_window_rules_inert() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let runtime = halley_config::parse_runtime_config(&config).expect("runtime config parses");

    assert_eq!(runtime.background, halley_config::Background::default());
    assert!(runtime.window_rules.is_empty());
}

/// Confirms the shipped example's nested field zoom parses to the documented
/// values.
#[test]
fn example_config_zoom_section_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let zoom = halley_config::parse_field_checked(&config).unwrap().zoom;

    assert!(zoom.enabled);
    assert_eq!(zoom.min, 0.35);
    assert_eq!(zoom.step, 1.10);
    assert_eq!(zoom.smooth_rate, 12.5);
}

#[test]
fn example_config_cursor_section_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let cursor = halley_config::parse_cursor(&config);

    assert_eq!(cursor, halley_config::Cursor::default());
}

#[test]
fn example_config_apogee_section_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let apogee = halley_config::parse_apogee(&config);

    assert!(apogee.enabled);
    assert!(apogee.live_previews);
    assert_eq!(apogee.preview_max_fps, 30);
    assert_eq!(apogee.max_rows, 3);
}

#[test]
fn example_config_overlay_section_is_the_bootstrap_style() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let overlays = halley_config::parse_overlays_checked(&config).expect("overlays parse");

    assert_eq!(overlays.radius_px, 8);
    assert!(overlays.borders);
    assert_eq!(overlays.border_size_px, 3);
    assert_eq!(
        overlays.notifications.position,
        halley_config::NotificationPosition::TopCenter
    );
    assert_eq!(overlays.notifications.success_duration_ms, 4_000);
    assert_eq!(overlays.notifications.error_duration_ms, 9_000);
    assert_eq!(
        overlays.zoom_indicator,
        halley_config::ZoomIndicator::default()
    );
}

#[test]
fn example_config_has_per_output_rings_font_and_landmarks() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let runtime = halley_config::parse_runtime_config(&config).expect("runtime config parses");

    assert_eq!(runtime.font.family, "monospace");
    assert_eq!(runtime.font.size, 11);
    assert_eq!(runtime.bearings, halley_config::Bearings::default());
    assert_eq!(runtime.focus_rings.for_output("DP-1").radius_x, 820.0);
    assert_eq!(runtime.focus_rings.for_output("DP-2").radius_y, 420.0);
    assert_eq!(runtime.field.gap, 20.0);
    assert!(runtime.field.close_restore_focus);
    assert!(!runtime.field.close_restore_nodes);
    assert_eq!(
        runtime.field.close_restore_pan,
        halley_config::CloseRestorePan::IfOffscreen
    );
    assert!(runtime.physics.enabled);
    assert_eq!(runtime.physics.damping, 0.45);
    assert_eq!(
        runtime.nodes.restore_centering,
        halley_config::RestoreCentering::Never
    );
}

#[test]
fn example_config_input_section_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let input = halley_config::parse_input(&config).expect("example input section parses");

    assert_eq!(input.repeat_rate, 30);
    assert_eq!(input.repeat_delay, 500);
    assert_eq!(input.focus_mode, halley_config::FocusMode::Click);
    assert!(input.raise_on_click);
    assert_eq!(input.keyboard.layout, "us");
    assert_eq!(input.gestures, halley_config::GestureSettings::default());
    assert_eq!(input.touchpad, halley_config::DeviceSettings::default());
    assert_eq!(input.mouse, halley_config::MouseSettings::default());
    assert_eq!(input.trackpoint, halley_config::DeviceSettings::default());
    assert_eq!(input.trackball, halley_config::DeviceSettings::default());
    assert_eq!(input.touchscreen, halley_config::DeviceSettings::default());
    assert!(input.devices.is_empty());
}

#[test]
fn example_config_window_open_animation_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let animations = halley_config::parse_animations(&config);

    assert!(animations.enabled);
    assert!(animations.window_open.enabled);
    assert_eq!(
        animations.window_open.animation_type,
        halley_config::WindowOpenAnimationType::CenterOut
    );
    assert_eq!(
        animations.window_open.motion,
        halley_config::AnimationMotion::Easing(halley_config::EasingMotion {
            duration_ms: 300,
            curve: halley_config::AnimationCurve::Linear,
        })
    );
}

#[test]
fn example_config_window_close_animation_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let close = halley_config::parse_animations(&config).window_close;

    assert!(close.enabled);
    assert_eq!(
        close.animation_type,
        halley_config::WindowCloseAnimationType::Shrink
    );
    assert_eq!(close.duration_ms, 270);
}

#[test]
fn example_config_fullscreen_animation_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let fullscreen = halley_config::parse_animations(&config).fullscreen;

    assert!(fullscreen.enabled);
    assert_eq!(
        fullscreen.motion,
        halley_config::AnimationMotion::Spring(halley_config::SpringMotion {
            damping_ratio: 1.0,
            stiffness: 800.0,
        })
    );
}

#[test]
fn example_config_maximize_animation_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let maximize = halley_config::parse_animations(&config).maximize;

    assert!(maximize.enabled);
    assert_eq!(
        maximize.motion,
        halley_config::AnimationMotion::Easing(halley_config::EasingMotion {
            duration_ms: 240,
            curve: halley_config::AnimationCurve::EaseInOutCubic,
        })
    );
}

#[test]
fn example_config_arrange_animation_parses() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let arrange = halley_config::parse_animations(&config).arrange;

    assert!(arrange.enabled);
    assert_eq!(
        arrange.motion,
        halley_config::AnimationMotion::Easing(halley_config::EasingMotion {
            duration_ms: 360,
            curve: halley_config::AnimationCurve::EaseInOutCubic,
        })
    );
}

/// The shipped example uses ring-only view entries by default. A connector
/// name matching real hardware must not be enough to create modeset work.
#[test]
fn example_config_view_has_no_hardware_overrides_by_default() {
    let config = RuneConfig::from_file(EXAMPLE_PATH).expect("example config parses");
    let view = halley_config::parse_view_checked(&config).expect("example view parses");
    assert_eq!(view.outputs, Vec::new());
    assert_eq!(view.focus_rings.by_output.len(), 2);
}

#[test]
fn all_templates_ship_explicit_node_collapse_duration() {
    for path in [EXAMPLE_PATH, SPLIT_EXAMPLE_PATH] {
        let config = RuneConfig::from_file(path).unwrap();
        assert_eq!(
            config
                .get_optional::<u32>("animations.node.collapse-duration-ms")
                .unwrap(),
            Some(280)
        );
        assert_eq!(
            halley_config::parse_animations(&config).node.duration_ms,
            280
        );
    }
    let config = RuneConfig::from_str(halley_config::DEFAULT_CONFIG).unwrap();
    assert_eq!(
        config
            .get_optional::<u32>("animations.node.collapse-duration-ms")
            .unwrap(),
        Some(280)
    );
}
