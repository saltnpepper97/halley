use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use crate::keybinds::key_name_to_evdev;
use halley_core::decay::FocusRingDecayPolicy;
use halley_core::field::Vec2;
use halley_core::viewport::{FocusRing, Viewport};

use super::{
    CompositorBinding, CompositorBindingAction, DirectionalAction, KeyModifiers, Keybinds,
    LaunchBinding, PointerBinding, PointerBindingAction,
};

#[derive(Clone, Debug)]
pub struct RuntimeTuning {
    pub tick_ms: u64,
    pub debug_tick_dump: bool,
    pub debug_dump_every_ms: u64,

    pub viewport_center: Vec2,
    pub viewport_size: Vec2,

    pub focus_ring_rx: f32,
    pub focus_ring_ry: f32,
    pub focus_ring_offset_x: f32,
    pub focus_ring_offset_y: f32,

    pub primary_hot_inner_frac: f32,
    pub primary_to_node_ms: u64,

    pub dev_enabled: bool,
    pub dev_show_geometry_overlay: bool,
    pub dev_zoom_decay_enabled: bool,
    pub dev_zoom_decay_min_frac: f32,
    pub dev_anim_enabled: bool,
    pub dev_anim_state_change_ms: u64,
    pub dev_anim_bounce: f32,

    pub cluster_distance_px: f32,
    pub cluster_dwell_ms: u64,

    pub primary_outside_ring_delay_ms: u64,
    pub secondary_outside_ring_delay_ms: u64,
    pub docked_offscreen_delay_ms: u64,

    pub non_overlap_gap_px: f32,
    pub non_overlap_active_gap_scale: f32,
    pub non_overlap_bump_newer: bool,
    pub non_overlap_bump_damping: f32,
    pub drag_smoothing_boost: f32,
    pub center_window_to_mouse: bool,
    pub restore_last_active_on_pan_return: bool,
    pub physics_enabled: bool,

    pub keybinds: Keybinds,
    pub compositor_bindings: Vec<CompositorBinding>,
    pub launch_bindings: Vec<LaunchBinding>,
    pub pointer_bindings: Vec<PointerBinding>,
    pub scroll_zoom_enabled: bool,

    pub tty_viewports: Vec<ViewportOutputConfig>,
    pub autostart_once: Vec<String>,
    pub autostart_on_reload: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ViewportOutputConfig {
    pub connector: String,
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: Option<f64>,
}

impl Default for RuntimeTuning {
    fn default() -> Self {
        Self {
            tick_ms: 200,
            debug_tick_dump: false,
            debug_dump_every_ms: 1000,
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

            dev_enabled: false,
            dev_show_geometry_overlay: false,
            dev_zoom_decay_enabled: true,
            dev_zoom_decay_min_frac: 0.05,
            dev_anim_enabled: true,
            dev_anim_state_change_ms: 360,
            dev_anim_bounce: 1.45,

            cluster_distance_px: 280.0,
            cluster_dwell_ms: 900,

            primary_outside_ring_delay_ms: 120_000,
            secondary_outside_ring_delay_ms: 30_000,
            docked_offscreen_delay_ms: 300_000,

            non_overlap_gap_px: 20.0,
            non_overlap_active_gap_scale: 0.22,
            non_overlap_bump_newer: false,
            non_overlap_bump_damping: 0.35,
            drag_smoothing_boost: 6.0,
            center_window_to_mouse: false,
            restore_last_active_on_pan_return: true,
            physics_enabled: true,

            keybinds: Keybinds::default(),
            compositor_bindings: default_compositor_bindings(Keybinds::default().modifier),
            launch_bindings: Vec::new(),
            pointer_bindings: default_pointer_bindings(Keybinds::default().modifier),
            scroll_zoom_enabled: true,

            tty_viewports: Vec::new(),
            autostart_once: Vec::new(),
            autostart_on_reload: Vec::new(),
            env: HashMap::from([
                ("XCURSOR_THEME".to_string(), "Adwaita".to_string()),
                ("XCURSOR_SIZE".to_string(), "24".to_string()),
            ]),
        }
    }
}

impl RuntimeTuning {
    pub fn config_path() -> String {
        match env::var("HALLEY_WL_CONFIG") {
            Ok(path) => absolutize_path(&path).to_string_lossy().to_string(),
            Err(_) => default_config_path().to_string_lossy().to_string(),
        }
    }

    pub fn load() -> Self {
        Self::load_from_path(&Self::config_path())
    }

    pub fn load_from_path(path: &str) -> Self {
        let mut out = Self::from_rune_file(path).unwrap_or_default();
        out.clamp_values();
        out
    }

    pub fn apply_process_env(&self) {
        for (key, value) in &self.env {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            unsafe { env::set_var(key, value) };
        }
    }

    fn clamp_values(&mut self) {
        self.tick_ms = self.tick_ms.clamp(16, 5000);
        self.debug_dump_every_ms = self.debug_dump_every_ms.clamp(100, 60_000);

        self.viewport_center.x = self.viewport_center.x.clamp(-100_000.0, 100_000.0);
        self.viewport_center.y = self.viewport_center.y.clamp(-100_000.0, 100_000.0);
        self.viewport_size.x = self.viewport_size.x.clamp(320.0, 16_000.0);
        self.viewport_size.y = self.viewport_size.y.clamp(240.0, 16_000.0);

        self.focus_ring_rx = self.focus_ring_rx.clamp(8.0, 16_000.0);
        self.focus_ring_ry = self.focus_ring_ry.clamp(8.0, 16_000.0);
        self.focus_ring_offset_x = self.focus_ring_offset_x.clamp(-16_000.0, 16_000.0);
        self.focus_ring_offset_y = self.focus_ring_offset_y.clamp(-16_000.0, 16_000.0);

        self.primary_hot_inner_frac = self.primary_hot_inner_frac.clamp(0.1, 1.0);
        self.primary_to_node_ms = self.primary_to_node_ms.clamp(250, 7_200_000);

        self.dev_zoom_decay_min_frac = self.dev_zoom_decay_min_frac.clamp(0.005, 0.5);
        self.dev_anim_state_change_ms = self.dev_anim_state_change_ms.clamp(30, 3_000);
        self.dev_anim_bounce = self.dev_anim_bounce.clamp(0.0, 3.0);

        self.cluster_distance_px = self.cluster_distance_px.clamp(24.0, 4_000.0);
        self.cluster_dwell_ms = self.cluster_dwell_ms.clamp(0, 30_000);

        self.primary_outside_ring_delay_ms = self.primary_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.secondary_outside_ring_delay_ms =
            self.secondary_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.docked_offscreen_delay_ms = self.docked_offscreen_delay_ms.clamp(0, 7_200_000);

        self.non_overlap_gap_px = self.non_overlap_gap_px.clamp(0.0, 256.0);
        self.non_overlap_active_gap_scale = self.non_overlap_active_gap_scale.clamp(0.0, 1.2);
        self.non_overlap_bump_damping = self.non_overlap_bump_damping.clamp(0.05, 1.0);
        self.drag_smoothing_boost = self.drag_smoothing_boost.clamp(0.1, 20.0);
    }

    pub fn enforce_guards(&mut self) {
        self.clamp_values();
    }

    pub fn viewport(&self) -> Viewport {
        Viewport::new(self.viewport_center, self.viewport_size)
    }

    pub fn focus_ring(&self) -> FocusRing {
        FocusRing::new(
            self.focus_ring_rx,
            self.focus_ring_ry,
            self.focus_ring_offset_x,
            self.focus_ring_offset_y,
        )
    }

    pub fn focus_ring_decay_policy(&self) -> FocusRingDecayPolicy {
        let mut p = FocusRingDecayPolicy::new();
        p.inside_to_node_ms = self.primary_to_node_ms;
        p
    }

    pub fn keybinds_resolved_summary(&self) -> String {
        format!(
            "mod={} compositor_actions={} custom_launches={} pointer_actions={}",
            self.keybinds.modifier_name(),
            self.compositor_bindings.len(),
            self.launch_bindings.len(),
            self.pointer_bindings.len(),
        )
    }
}

pub(crate) fn default_pointer_bindings(modifier: KeyModifiers) -> Vec<PointerBinding> {
    vec![
        PointerBinding {
            modifiers: modifier,
            button: 272,
            action: PointerBindingAction::MoveWindow,
        },
        PointerBinding {
            modifiers: modifier,
            button: 273,
            action: PointerBindingAction::ResizeWindow,
        },
    ]
}

pub(crate) fn default_compositor_bindings(modifier: KeyModifiers) -> Vec<CompositorBinding> {
    let key = |name: &str| key_name_to_evdev(name).expect("default compositor key should exist");

    vec![
        CompositorBinding {
            modifiers: modifier,
            key: key("r"),
            action: CompositorBindingAction::Reload,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("n"),
            action: CompositorBindingAction::ToggleState,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("equal"),
            action: CompositorBindingAction::ZoomIn,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("minus"),
            action: CompositorBindingAction::ZoomOut,
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("0"),
            action: CompositorBindingAction::ZoomReset,
        },
        CompositorBinding {
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
            modifiers: modifier,
            key: key("h"),
            action: CompositorBindingAction::MoveNode(DirectionalAction::Left),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("k"),
            action: CompositorBindingAction::MoveNode(DirectionalAction::Up),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("l"),
            action: CompositorBindingAction::MoveNode(DirectionalAction::Right),
        },
        CompositorBinding {
            modifiers: modifier,
            key: key("j"),
            action: CompositorBindingAction::MoveNode(DirectionalAction::Down),
        },
    ]
}

fn default_config_path() -> PathBuf {
    if let Ok(home) = env::var("XDG_CONFIG_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join("halley/halley.rune");
        }
    }

    if let Ok(home) = env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".config/halley/halley.rune");
        }
    }

    PathBuf::from("halley.rune")
}

fn absolutize_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| PathBuf::from(path))
    }
}
