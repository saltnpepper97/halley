use std::collections::HashMap;

use halley_core::field::Vec2;

use crate::keybinds::{
    BearingsBindingAction, ClusterBindingAction, CompositorBinding, CompositorBindingAction,
    CompositorBindingScope, DirectionalAction, KeyModifiers, Keybinds, NodeBindingAction,
    PointerBinding, PointerBindingAction, TileBindingAction, TrailBindingAction, WHEEL_DOWN_CODE,
    WHEEL_UP_CODE, key_name_to_evdev,
};

use super::{
    BearingsConfig, ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode, CloseRestorePanMode,
    ClusterBloomDirection, ClusterDefaultLayout, CursorConfig, DecorationBorderColor, FontConfig,
    NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy, PanToNewMode, RuntimeTuning,
    ShapeStyle,
};

impl Default for RuntimeTuning {
    fn default() -> Self {
        Self {
            viewport_center: Vec2 { x: 0.0, y: 0.0 },
            viewport_size: Vec2 {
                x: 1920.0,
                y: 1080.0,
            },

            focus_ring_rx: 820.0,
            focus_ring_ry: 420.0,
            focus_ring_offset_x: 0.0,
            focus_ring_offset_y: 0.0,

            primary_hot_inner_frac: 0.88,
            primary_to_node_ms: 1_260_000,
            node_show_labels: NodeDisplayPolicy::Hover,
            node_show_app_icons: NodeDisplayPolicy::Always,
            node_shape: ShapeStyle::Squircle,
            node_label_shape: ShapeStyle::Squircle,
            node_icon_size: 0.72,
            node_background_color: NodeBackgroundColorMode::Auto,
            node_border_color_hover: NodeBorderColorMode::UseWindowActive,
            node_border_color_inactive: NodeBorderColorMode::UseWindowInactive,
            border_size_px: 3,
            border_radius_px: 0,
            border_color_focused: DecorationBorderColor {
                r: 0.22,
                g: 0.82,
                b: 0.92,
            },
            border_color_unfocused: DecorationBorderColor {
                r: 0.28,
                g: 0.30,
                b: 0.35,
            },
            resize_using_border: false,
            click_collapsed_outside_focus: ClickCollapsedOutsideFocusMode::Activate,
            click_collapsed_pan: ClickCollapsedPanMode::IfOffscreen,
            bearings: BearingsConfig {
                show_distance: true,
                show_icons: true,
                fade_distance: 1200.0,
            },

            cluster_distance_px: 280.0,
            cluster_dwell_ms: 900,
            cluster_show_icons: true,
            cluster_bloom_direction: ClusterBloomDirection::Clockwise,
            cluster_default_layout: ClusterDefaultLayout::Stacking,
            tile_gaps_inner_px: 20.0,
            tile_gaps_outer_px: 20.0,
            tile_new_on_top: false,
            tile_queue_show_icons: true,
            tile_max_stack: 3,
            stacking_max_visible: 5,
            trail_history_length: 32,
            trail_wrap: true,

            active_outside_ring_delay_ms: 120_000,
            inactive_outside_ring_delay_ms: 30_000,
            docked_offscreen_delay_ms: 300_000,

            non_overlap_gap_px: 20.0,
            pan_to_new: PanToNewMode::IfNeeded,
            close_restore_focus: true,
            close_restore_pan: CloseRestorePanMode::IfOffscreen,
            zoom_enabled: true,
            zoom_step: 1.10,
            zoom_min: 0.25,
            zoom_max: 1.35,
            zoom_smooth: true,
            zoom_smooth_rate: 12.5,
            non_overlap_active_gap_scale: 0.22,
            non_overlap_bump_newer: false,
            non_overlap_bump_damping: 0.65,
            drag_smoothing_boost: 6.0,
            center_window_to_mouse: false,
            restore_last_active_on_pan_return: true,
            physics_enabled: true,
            no_csd: false,
            window_rules: Vec::new(),

            keybinds: Keybinds::default(),
            compositor_bindings: default_compositor_bindings(Keybinds::default().modifier),
            launch_bindings: Vec::new(),
            pointer_bindings: default_pointer_bindings(Keybinds::default().modifier),

            tty_viewports: Vec::new(),
            autostart_once: Vec::new(),
            autostart_on_reload: Vec::new(),
            cursor: CursorConfig::default(),
            font: FontConfig::default(),
            env: HashMap::new(),
        }
    }
}

pub fn default_pointer_bindings(modifier: KeyModifiers) -> Vec<PointerBinding> {
    let mut transfer_modifier = modifier;
    transfer_modifier.shift = true;
    vec![
        PointerBinding {
            modifiers: modifier,
            button: 272,
            action: PointerBindingAction::MoveWindow,
        },
        PointerBinding {
            modifiers: transfer_modifier,
            button: 272,
            action: PointerBindingAction::FieldJump,
        },
        PointerBinding {
            modifiers: modifier,
            button: 273,
            action: PointerBindingAction::ResizeWindow,
        },
    ]
}

pub fn default_compositor_bindings(modifier: KeyModifiers) -> Vec<CompositorBinding> {
    let key = |name: &str| key_name_to_evdev(name).expect("default compositor key should exist");

    vec![
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: key("r"),
            action: CompositorBindingAction::Reload,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: key("n"),
            action: CompositorBindingAction::ToggleState,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("c"),
            action: CompositorBindingAction::ClusterMode,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: key("z"),
            action: CompositorBindingAction::Bearings(BearingsBindingAction::Show),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("z"),
            action: CompositorBindingAction::Bearings(BearingsBindingAction::Toggle),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: WHEEL_UP_CODE,
            action: CompositorBindingAction::ZoomIn,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: WHEEL_DOWN_CODE,
            action: CompositorBindingAction::ZoomOut,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: modifier,
            key: key("mousemiddle"),
            action: CompositorBindingAction::ZoomReset,
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("q"),
            action: CompositorBindingAction::Quit {
                requires_shift: true,
            },
        },
        CompositorBinding {
            scope: CompositorBindingScope::Field,
            modifiers: modifier,
            key: key("h"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Left)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Field,
            modifiers: modifier,
            key: key("k"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Up)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Field,
            modifiers: modifier,
            key: key("l"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(
                DirectionalAction::Right,
            )),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Field,
            modifiers: modifier,
            key: key("j"),
            action: CompositorBindingAction::Node(NodeBindingAction::Move(DirectionalAction::Down)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("comma"),
            action: CompositorBindingAction::Trail(TrailBindingAction::Prev),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Global,
            modifiers: KeyModifiers {
                shift: true,
                ..modifier
            },
            key: key("dot"),
            action: CompositorBindingAction::Trail(TrailBindingAction::Next),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Cluster,
            modifiers: modifier,
            key: key("l"),
            action: CompositorBindingAction::Cluster(ClusterBindingAction::LayoutCycle),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: modifier,
            key: key("left"),
            action: CompositorBindingAction::Tile(TileBindingAction::Focus(
                DirectionalAction::Left,
            )),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: modifier,
            key: key("right"),
            action: CompositorBindingAction::Tile(TileBindingAction::Focus(
                DirectionalAction::Right,
            )),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: modifier,
            key: key("up"),
            action: CompositorBindingAction::Tile(TileBindingAction::Focus(DirectionalAction::Up)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: modifier,
            key: key("down"),
            action: CompositorBindingAction::Tile(TileBindingAction::Focus(
                DirectionalAction::Down,
            )),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: KeyModifiers {
                ctrl: true,
                ..modifier
            },
            key: key("left"),
            action: CompositorBindingAction::Tile(TileBindingAction::Swap(DirectionalAction::Left)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: KeyModifiers {
                ctrl: true,
                ..modifier
            },
            key: key("right"),
            action: CompositorBindingAction::Tile(TileBindingAction::Swap(
                DirectionalAction::Right,
            )),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: KeyModifiers {
                ctrl: true,
                ..modifier
            },
            key: key("up"),
            action: CompositorBindingAction::Tile(TileBindingAction::Swap(DirectionalAction::Up)),
        },
        CompositorBinding {
            scope: CompositorBindingScope::Tile,
            modifiers: KeyModifiers {
                ctrl: true,
                ..modifier
            },
            key: key("down"),
            action: CompositorBindingAction::Tile(TileBindingAction::Swap(DirectionalAction::Down)),
        },
    ]
}
