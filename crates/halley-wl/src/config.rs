use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use halley_core::decay::RingDecayPolicy;
use halley_core::field::Vec2;
use halley_core::viewport::{EyeRing, FocusRings, Viewport};
use rune_cfg::RuneConfig;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub super_key: bool,
    pub left_super: bool,
    pub right_super: bool,
    pub alt: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub ctrl: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub shift: bool,
    pub left_shift: bool,
    pub right_shift: bool,
}

#[derive(Clone, Debug)]
pub struct Keybinds {
    pub modifier: KeyModifiers,
    pub reload_config: u32,
    pub minimize_focused: u32,
    pub overview_toggle: u32,
    pub quit_compositor: u32,
    pub launch_pavucontrol: u32,
    pub primary_left: u32,
    pub primary_right: u32,
    pub primary_up: u32,
    pub primary_down: u32,
    pub secondary_left: u32,
    pub secondary_right: u32,
    pub secondary_up: u32,
    pub secondary_down: u32,
    pub move_left: u32,
    pub move_right: u32,
    pub move_up: u32,
    pub move_down: u32,
}

#[derive(Clone, Debug)]
pub struct LaunchBinding {
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub command: String,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            modifier: KeyModifiers {
                left_alt: true,
                ..KeyModifiers::default()
            },
            reload_config: 19,      // r
            minimize_focused: 49,   // n
            overview_toggle: 24,    // o
            quit_compositor: 16,    // q (with mod+shift hard requirement in key handler)
            launch_pavucontrol: 25, // p
            primary_left: 105,      // left
            primary_right: 106,     // right
            primary_up: 103,        // up
            primary_down: 108,      // down
            secondary_left: 36,     // j
            secondary_right: 38,    // l
            secondary_up: 23,       // i
            secondary_down: 37,     // k
            move_left: 30,          // a
            move_right: 32,         // d
            move_up: 17,            // w
            move_down: 31,          // s
        }
    }
}

impl Keybinds {
    pub fn modifier_name(&self) -> String {
        let mut parts = Vec::new();
        if self.modifier.left_super {
            parts.push("lsuper");
        }
        if self.modifier.right_super {
            parts.push("rsuper");
        }
        if self.modifier.super_key {
            parts.push("super");
        }
        if self.modifier.left_ctrl {
            parts.push("lctrl");
        }
        if self.modifier.right_ctrl {
            parts.push("rctrl");
        }
        if self.modifier.ctrl {
            parts.push("ctrl");
        }
        if self.modifier.left_alt {
            parts.push("lalt");
        }
        if self.modifier.right_alt {
            parts.push("ralt");
        }
        if self.modifier.alt {
            parts.push("alt");
        }
        if self.modifier.left_shift {
            parts.push("lshift");
        }
        if self.modifier.right_shift {
            parts.push("rshift");
        }
        if self.modifier.shift {
            parts.push("shift");
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}

#[inline]
fn modifiers_empty(m: KeyModifiers) -> bool {
    !m.super_key
        && !m.left_super
        && !m.right_super
        && !m.alt
        && !m.left_alt
        && !m.right_alt
        && !m.ctrl
        && !m.left_ctrl
        && !m.right_ctrl
        && !m.shift
        && !m.left_shift
        && !m.right_shift
}

#[derive(Clone, Debug)]
pub struct RuntimeTuning {
    pub tick_ms: u64,
    pub debug_tick_dump: bool,
    pub debug_dump_every_ms: u64,

    pub viewport_center: Vec2,
    pub viewport_size: Vec2,

    pub ring_primary_rx: f32,
    pub ring_primary_ry: f32,
    pub ring_secondary_rx: f32,
    pub ring_secondary_ry: f32,
    pub ring_rotation_rad: f32,

    pub primary_hot_inner_frac: f32,
    pub primary_to_preview_ms: u64,
    pub primary_preview_to_node_ms: u64,
    pub secondary_to_node_ms: u64,
    pub dev_enabled: bool,
    pub dev_show_geometry_overlay: bool,
    pub dev_zoom_decay_enabled: bool,
    pub dev_zoom_decay_min_frac: f32,
    pub dev_anim_enabled: bool,
    pub dev_anim_state_change_ms: u64,
    pub dev_anim_bounce: f32,
    pub cluster_distance_px: f32,
    pub cluster_dwell_ms: u64,
    pub non_overlap_gap_px: f32,
    pub non_overlap_active_gap_scale: f32,
    pub new_window_on_top: bool,
    pub non_overlap_bump_newer: bool,
    pub non_overlap_bump_damping: f32,
    pub drag_smoothing_boost: f32,
    pub center_window_to_mouse: bool,
    pub restore_last_active_on_pan_return: bool,
    pub physics_enabled: bool,
    pub keybinds: Keybinds,
    pub keybind_launch_command: String,
    pub launch_bindings: Vec<LaunchBinding>,
    pub quit_requires_shift: bool,
    pub tty_viewports: Vec<ViewportOutputConfig>,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ViewportOutputConfig {
    pub connector: String,
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
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
            ring_primary_rx: 820.0,
            ring_primary_ry: 420.0,
            ring_secondary_rx: 920.0,
            ring_secondary_ry: 460.0,
            ring_rotation_rad: 0.0,
            primary_hot_inner_frac: 0.88,
            primary_to_preview_ms: 1_200_000,
            primary_preview_to_node_ms: 60_000,
            secondary_to_node_ms: 300_000,
            dev_enabled: false,
            dev_show_geometry_overlay: false,
            dev_zoom_decay_enabled: true,
            dev_zoom_decay_min_frac: 0.05,
            dev_anim_enabled: true,
            dev_anim_state_change_ms: 360,
            dev_anim_bounce: 1.45,
            cluster_distance_px: 280.0,
            cluster_dwell_ms: 900,
            non_overlap_gap_px: 20.0,
            non_overlap_active_gap_scale: 0.22,
            new_window_on_top: true,
            non_overlap_bump_newer: false,
            non_overlap_bump_damping: 0.35,
            drag_smoothing_boost: 6.0,
            center_window_to_mouse: false,
            restore_last_active_on_pan_return: true,
            physics_enabled: true,
            keybinds: Keybinds::default(),
            keybind_launch_command: "pavucontrol".to_string(),
            launch_bindings: vec![LaunchBinding {
                modifiers: Keybinds::default().modifier,
                key: Keybinds::default().launch_pavucontrol,
                command: "pavucontrol".to_string(),
            }],
            quit_requires_shift: true,
            tty_viewports: Vec::new(),
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
            // Runtime env controls are configured by trusted local config.
            unsafe { env::set_var(key, value) };
        }
    }

    fn from_rune_file(path: &str) -> Option<Self> {
        let raw = fs::read_to_string(path).ok()?;
        let legacy_keybinds = parse_legacy_keybinds(raw.as_str());
        let cfg = RuneConfig::from_file(path).or_else(|_| {
            let sanitized = strip_legacy_keybind_block(raw.as_str());
            RuneConfig::from_str(sanitized.as_str())
        });
        let cfg = cfg.ok()?;
        let mut out = Self::default();

        out.tick_ms = pick_u64(&cfg, &["halley.runtime.tick_ms"], out.tick_ms);
        out.debug_tick_dump = pick_bool(&cfg, &["dev.debug_tick_dump"], out.debug_tick_dump);
        out.debug_dump_every_ms =
            pick_u64(&cfg, &["dev.debug_dump_every_ms"], out.debug_dump_every_ms);
        out.dev_enabled = pick_bool(&cfg, &["dev.enabled"], out.dev_enabled);
        out.dev_show_geometry_overlay = pick_bool(
            &cfg,
            &["dev.show_geometry_overlay"],
            out.dev_show_geometry_overlay,
        );
        out.dev_zoom_decay_enabled = pick_bool(
            &cfg,
            &["dev.zoom_decay_enabled"],
            out.dev_zoom_decay_enabled,
        );
        out.dev_zoom_decay_min_frac = pick_f32(
            &cfg,
            &["dev.zoom_decay_min_frac"],
            out.dev_zoom_decay_min_frac,
        );
        out.dev_anim_enabled = pick_bool(&cfg, &["dev.anim.enabled"], out.dev_anim_enabled);
        out.dev_anim_state_change_ms = pick_u64(
            &cfg,
            &["dev.anim.state_change_ms"],
            out.dev_anim_state_change_ms,
        );
        out.dev_anim_bounce = pick_f32(&cfg, &["dev.anim.bounce"], out.dev_anim_bounce);
        out.cluster_distance_px = pick_f32(
            &cfg,
            &["halley.clusters.distance_px"],
            out.cluster_distance_px,
        );
        out.cluster_dwell_ms = pick_u64(&cfg, &["halley.clusters.dwell_ms"], out.cluster_dwell_ms);
        out.non_overlap_gap_px = pick_f32(
            &cfg,
            &["halley.layout.non_overlap_gap_px"],
            out.non_overlap_gap_px,
        );
        out.non_overlap_active_gap_scale = pick_f32(
            &cfg,
            &["halley.layout.non_overlap.active_gap_scale"],
            out.non_overlap_active_gap_scale,
        );
        out.new_window_on_top = pick_bool(
            &cfg,
            &["halley.layout.new_window_on_top"],
            out.new_window_on_top,
        );
        out.non_overlap_bump_newer = pick_bool(
            &cfg,
            &["halley.layout.non_overlap.bump_newer"],
            out.non_overlap_bump_newer,
        );
        out.non_overlap_bump_damping = pick_f32(
            &cfg,
            &["halley.layout.non_overlap.bump_damping"],
            out.non_overlap_bump_damping,
        );
        out.drag_smoothing_boost = pick_f32(
            &cfg,
            &["halley.layout.drag_smoothing_boost"],
            out.drag_smoothing_boost,
        );
        out.center_window_to_mouse = pick_bool(
            &cfg,
            &["halley.layout.center_window_to_mouse"],
            out.center_window_to_mouse,
        );
        out.restore_last_active_on_pan_return = pick_bool(
            &cfg,
            &["halley.layout.restore_last_active_on_pan_return"],
            out.restore_last_active_on_pan_return,
        );
        out.physics_enabled = pick_bool(
            &cfg,
            &["halley.layout.physics_enabled"],
            out.physics_enabled,
        );

        out.viewport_center.x =
            pick_f32(&cfg, &["halley.viewport.center_x"], out.viewport_center.x);
        out.viewport_center.y =
            pick_f32(&cfg, &["halley.viewport.center_y"], out.viewport_center.y);
        out.viewport_size.x = pick_f32(&cfg, &["halley.viewport.size_w"], out.viewport_size.x);
        out.viewport_size.y = pick_f32(&cfg, &["halley.viewport.size_h"], out.viewport_size.y);
        out.tty_viewports = parse_viewport_outputs(&cfg);
        if let Some(primary) = out.tty_viewports.first() {
            out.viewport_size.x = primary.width as f32;
            out.viewport_size.y = primary.height as f32;
        }

        out.ring_primary_rx = pick_f32(&cfg, &["halley.ring.primary_rx"], out.ring_primary_rx);
        out.ring_primary_ry = pick_f32(&cfg, &["halley.ring.primary_ry"], out.ring_primary_ry);
        out.ring_secondary_rx =
            pick_f32(&cfg, &["halley.ring.secondary_rx"], out.ring_secondary_rx);
        out.ring_secondary_ry =
            pick_f32(&cfg, &["halley.ring.secondary_ry"], out.ring_secondary_ry);
        out.ring_rotation_rad =
            pick_f32(&cfg, &["halley.ring.rotation_rad"], out.ring_rotation_rad);

        out.secondary_to_node_ms = pick_u64(
            &cfg,
            &["halley.decay.secondary_to_node_ms"],
            out.secondary_to_node_ms,
        );
        out.primary_to_preview_ms = pick_u64(
            &cfg,
            &["halley.decay.primary_to_preview_ms"],
            out.primary_to_preview_ms,
        );
        out.primary_preview_to_node_ms = pick_u64(
            &cfg,
            &["halley.decay.primary_preview_to_node_ms"],
            out.primary_preview_to_node_ms,
        );
        out.primary_hot_inner_frac = pick_f32(
            &cfg,
            &["halley.decay.primary_hot_inner_frac"],
            out.primary_hot_inner_frac,
        );
        out.keybinds.modifier =
            pick_modifiers(&cfg, &["dev.keybinds.modifier"], out.keybinds.modifier);
        out.keybinds.reload_config = pick_keycode(
            &cfg,
            &["dev.keybinds.reload_config"],
            out.keybinds.reload_config,
        );
        out.keybinds.minimize_focused = pick_keycode(
            &cfg,
            &["dev.keybinds.minimize_focused"],
            out.keybinds.minimize_focused,
        );
        out.keybinds.overview_toggle = pick_keycode(
            &cfg,
            &["dev.keybinds.overview_toggle"],
            out.keybinds.overview_toggle,
        );
        out.keybinds.quit_compositor = pick_keycode(
            &cfg,
            &["dev.keybinds.quit_compositor"],
            out.keybinds.quit_compositor,
        );
        out.keybinds.launch_pavucontrol = pick_keycode(
            &cfg,
            &["dev.keybinds.launch_pavucontrol"],
            out.keybinds.launch_pavucontrol,
        );
        out.keybind_launch_command = pick_string(
            &cfg,
            &["dev.keybinds.launch_command"],
            out.keybind_launch_command.as_str(),
        );
        out.launch_bindings.clear();
        out.launch_bindings.push(LaunchBinding {
            modifiers: out.keybinds.modifier,
            key: out.keybinds.launch_pavucontrol,
            command: out.keybind_launch_command.clone(),
        });
        out.quit_requires_shift = pick_bool(
            &cfg,
            &["dev.keybinds.quit_requires_shift"],
            out.quit_requires_shift,
        );
        out.keybinds.primary_left = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_left"],
            out.keybinds.primary_left,
        );
        out.keybinds.primary_right = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_right"],
            out.keybinds.primary_right,
        );
        out.keybinds.primary_up =
            pick_keycode(&cfg, &["dev.keybinds.primary_up"], out.keybinds.primary_up);
        out.keybinds.primary_down = pick_keycode(
            &cfg,
            &["dev.keybinds.primary_down"],
            out.keybinds.primary_down,
        );
        out.keybinds.secondary_left = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_left"],
            out.keybinds.secondary_left,
        );
        out.keybinds.secondary_right = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_right"],
            out.keybinds.secondary_right,
        );
        out.keybinds.secondary_up = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_up"],
            out.keybinds.secondary_up,
        );
        out.keybinds.secondary_down = pick_keycode(
            &cfg,
            &["dev.keybinds.secondary_down"],
            out.keybinds.secondary_down,
        );
        out.keybinds.move_left =
            pick_keycode(&cfg, &["dev.keybinds.move_left"], out.keybinds.move_left);
        out.keybinds.move_right =
            pick_keycode(&cfg, &["dev.keybinds.move_right"], out.keybinds.move_right);
        out.keybinds.move_up = pick_keycode(&cfg, &["dev.keybinds.move_up"], out.keybinds.move_up);
        out.keybinds.move_down =
            pick_keycode(&cfg, &["dev.keybinds.move_down"], out.keybinds.move_down);
        merge_env_map(&cfg, &mut out.env, "halley.env");
        apply_explicit_keybind_overrides(&cfg, &mut out);
        if !legacy_keybinds.is_empty() {
            apply_explicit_keybind_overrides_map(&legacy_keybinds, &mut out);
        }

        Some(out)
    }

    fn clamp_values(&mut self) {
        self.tick_ms = self.tick_ms.clamp(16, 5000);
        self.debug_dump_every_ms = self.debug_dump_every_ms.clamp(100, 60_000);

        self.viewport_center.x = self.viewport_center.x.clamp(-100_000.0, 100_000.0);
        self.viewport_center.y = self.viewport_center.y.clamp(-100_000.0, 100_000.0);
        self.viewport_size.x = self.viewport_size.x.clamp(320.0, 16_000.0);
        self.viewport_size.y = self.viewport_size.y.clamp(240.0, 16_000.0);

        self.ring_primary_rx = self.ring_primary_rx.clamp(8.0, 16_000.0);
        self.ring_primary_ry = self.ring_primary_ry.clamp(8.0, 16_000.0);
        self.ring_secondary_rx = self.ring_secondary_rx.clamp(16.0, 16_000.0);
        self.ring_secondary_ry = self.ring_secondary_ry.clamp(16.0, 16_000.0);
        self.ring_rotation_rad = self
            .ring_rotation_rad
            .clamp(-std::f32::consts::PI, std::f32::consts::PI);
        self.primary_hot_inner_frac = self.primary_hot_inner_frac.clamp(0.1, 1.0);
        self.primary_to_preview_ms = self.primary_to_preview_ms.clamp(250, 7_200_000);
        self.primary_preview_to_node_ms = self.primary_preview_to_node_ms.clamp(250, 7_200_000);
        self.secondary_to_node_ms = self.secondary_to_node_ms.clamp(250, 7_200_000);
        self.dev_zoom_decay_min_frac = self.dev_zoom_decay_min_frac.clamp(0.005, 0.5);
        self.dev_anim_state_change_ms = self.dev_anim_state_change_ms.clamp(30, 3_000);
        self.dev_anim_bounce = self.dev_anim_bounce.clamp(0.0, 3.0);
        self.cluster_distance_px = self.cluster_distance_px.clamp(24.0, 4_000.0);
        self.cluster_dwell_ms = self.cluster_dwell_ms.clamp(0, 30_000);
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

    pub fn rings(&self) -> FocusRings {
        FocusRings {
            primary: EyeRing::new(
                self.ring_primary_rx,
                self.ring_primary_ry,
                self.ring_rotation_rad,
            ),
            secondary: EyeRing::new(
                self.ring_secondary_rx,
                self.ring_secondary_ry,
                self.ring_rotation_rad,
            ),
        }
    }

    pub fn ring_decay_policy(&self) -> RingDecayPolicy {
        let mut p = RingDecayPolicy::new(self.secondary_to_node_ms);
        p.primary_to_preview_ms = self.primary_to_preview_ms;
        p.primary_preview_to_node_ms = self.primary_preview_to_node_ms;
        p
    }

    pub fn keybinds_resolved_summary(&self) -> String {
        let kb = &self.keybinds;
        format!(
            "mod={} reload={} minimize={} overview={} quit={} (requires_shift={}) launch={}=>`{}` custom_launches={} primary=[{},{},{},{}] secondary=[{},{},{},{}] move=[{},{},{},{}]",
            kb.modifier_name(),
            evdev_to_key_name(kb.reload_config),
            evdev_to_key_name(kb.minimize_focused),
            evdev_to_key_name(kb.overview_toggle),
            evdev_to_key_name(kb.quit_compositor),
            self.quit_requires_shift,
            evdev_to_key_name(kb.launch_pavucontrol),
            self.keybind_launch_command,
            self.launch_bindings.len(),
            evdev_to_key_name(kb.primary_left),
            evdev_to_key_name(kb.primary_right),
            evdev_to_key_name(kb.primary_up),
            evdev_to_key_name(kb.primary_down),
            evdev_to_key_name(kb.secondary_left),
            evdev_to_key_name(kb.secondary_right),
            evdev_to_key_name(kb.secondary_up),
            evdev_to_key_name(kb.secondary_down),
            evdev_to_key_name(kb.move_left),
            evdev_to_key_name(kb.move_right),
            evdev_to_key_name(kb.move_up),
            evdev_to_key_name(kb.move_down),
        )
    }
}

fn parse_viewport_outputs(cfg: &RuneConfig) -> Vec<ViewportOutputConfig> {
    let mut out = Vec::new();
    let Ok(keys) = cfg.get_keys("halley.viewport") else {
        return out;
    };
    for key in keys {
        let width = pick_u32(
            cfg,
            &[
                format!("halley.viewport.{}.width", key).as_str(),
                format!("halley.viewport.{}.size_w", key).as_str(),
            ],
            0,
        );
        let height = pick_u32(
            cfg,
            &[
                format!("halley.viewport.{}.height", key).as_str(),
                format!("halley.viewport.{}.size_h", key).as_str(),
            ],
            0,
        );
        if width == 0 || height == 0 {
            continue;
        }
        let offset_x = pick_i32(
            cfg,
            &[format!("halley.viewport.{}.offset_x", key).as_str()],
            0,
        );
        let offset_y = pick_i32(
            cfg,
            &[format!("halley.viewport.{}.offset_y", key).as_str()],
            0,
        );
        out.push(ViewportOutputConfig {
            connector: key,
            offset_x,
            offset_y,
            width,
            height,
        });
    }
    out
}

fn merge_env_map(cfg: &RuneConfig, out: &mut HashMap<String, String>, path: &str) {
    let Ok(Some(entries)) = cfg.get_optional::<HashMap<String, String>>(path) else {
        return;
    };
    for (key, value) in entries {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        out.insert(key.to_string(), value.to_string());
    }
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

fn pick_u64(cfg: &RuneConfig, paths: &[&str], default: u64) -> u64 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u64>(path) {
            return v;
        }
    }
    default
}

fn pick_f32(cfg: &RuneConfig, paths: &[&str], default: f32) -> f32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<f32>(path) {
            return v;
        }
    }
    default
}

fn pick_u32(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
    }
    default
}

fn pick_i32(cfg: &RuneConfig, paths: &[&str], default: i32) -> i32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<i32>(path) {
            return v;
        }
    }
    default
}

fn pick_bool(cfg: &RuneConfig, paths: &[&str], default: bool) -> bool {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<bool>(path) {
            return v;
        }
    }
    default
}

fn pick_string(cfg: &RuneConfig, paths: &[&str], default: &str) -> String {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    default.to_string()
}

fn pick_modifiers(cfg: &RuneConfig, paths: &[&str], default: KeyModifiers) -> KeyModifiers {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<String>(path) {
            if let Some(m) = parse_modifiers(v.as_str()) {
                return m;
            }
        }
    }
    default
}

fn parse_modifiers(text: &str) -> Option<KeyModifiers> {
    let mut out = KeyModifiers::default();
    let mut any = false;
    for raw in text.split('+') {
        let t = raw.trim().to_ascii_lowercase();
        match t.as_str() {
            "" => {}
            "none" => {}
            "super" | "win" | "windows" | "logo" => {
                out.super_key = true;
                any = true;
            }
            "lsuper" | "lwin" => {
                out.left_super = true;
                any = true;
            }
            "rsuper" | "rwin" => {
                out.right_super = true;
                any = true;
            }
            "alt" => {
                out.alt = true;
                any = true;
            }
            "lalt" => {
                out.left_alt = true;
                any = true;
            }
            "ralt" => {
                out.right_alt = true;
                any = true;
            }
            "ctrl" | "control" => {
                out.ctrl = true;
                any = true;
            }
            "lctrl" => {
                out.left_ctrl = true;
                any = true;
            }
            "rctrl" => {
                out.right_ctrl = true;
                any = true;
            }
            "shift" => {
                out.shift = true;
                any = true;
            }
            "lshift" => {
                out.left_shift = true;
                any = true;
            }
            "rshift" => {
                out.right_shift = true;
                any = true;
            }
            _ => return None,
        }
    }
    if any {
        Some(out)
    } else {
        Some(KeyModifiers::default())
    }
}

fn pick_keycode(cfg: &RuneConfig, paths: &[&str], default: u32) -> u32 {
    for path in paths {
        if let Ok(Some(v)) = cfg.get_optional::<u32>(path) {
            return v;
        }
        if let Ok(Some(name)) = cfg.get_optional::<String>(path) {
            if let Some(code) = key_name_to_evdev(name.as_str()) {
                return code;
            }
        }
    }
    default
}

fn key_name_to_evdev(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "none" => Some(0),
        "enter" | "return" => Some(28),
        "space" => Some(57),
        "escape" | "esc" => Some(1),
        "tab" => Some(15),
        "left" => Some(105),
        "right" => Some(106),
        "up" => Some(103),
        "down" => Some(108),
        "f5" => Some(63),
        "f6" => Some(64),
        "f7" => Some(65),
        "f8" => Some(66),
        "f9" => Some(67),
        "p" => Some(25),
        "q" => Some(16),
        "r" => Some(19),
        "w" => Some(17),
        "i" => Some(23),
        "a" => Some(30),
        "s" => Some(31),
        "d" => Some(32),
        "j" => Some(36),
        "k" => Some(37),
        "l" => Some(38),
        "m" => Some(50),
        "n" => Some(49),
        "o" => Some(24),
        "t" => Some(20),
        "u" => Some(22),
        "v" => Some(47),
        "x" => Some(45),
        "y" => Some(21),
        "z" => Some(44),
        _ => None,
    }
}

fn evdev_to_key_name(code: u32) -> &'static str {
    match code {
        0 => "none",
        1 => "Escape",
        15 => "Tab",
        16 => "Q",
        17 => "W",
        19 => "R",
        20 => "T",
        21 => "Y",
        22 => "U",
        23 => "I",
        24 => "O",
        25 => "P",
        28 => "Return",
        30 => "A",
        31 => "S",
        32 => "D",
        36 => "J",
        37 => "K",
        38 => "L",
        44 => "Z",
        45 => "X",
        47 => "V",
        49 => "N",
        50 => "M",
        57 => "Space",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        103 => "Up",
        105 => "Left",
        106 => "Right",
        108 => "Down",
        _ => "?",
    }
}

fn apply_explicit_keybind_overrides(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    let Ok(Some(bindings)) = cfg.get_optional::<HashMap<String, String>>("keybinds") else {
        return;
    };
    apply_explicit_keybind_overrides_map(&bindings, out);
}

fn apply_explicit_keybind_overrides_map(
    bindings: &HashMap<String, String>,
    out: &mut RuntimeTuning,
) {
    let mod_token = bindings
        .get("mod")
        .cloned()
        .unwrap_or_else(|| out.keybinds.modifier_name());
    if let Some(m) = parse_modifiers(mod_token.as_str()) {
        out.keybinds.modifier = m;
    }
    for (chord, action) in bindings {
        if chord.eq_ignore_ascii_case("mod") {
            continue;
        }
        apply_explicit_binding(
            out,
            mod_token.as_str(),
            out.keybinds.modifier,
            chord.as_str(),
            action.as_str(),
        );
    }
}

fn apply_explicit_binding(
    out: &mut RuntimeTuning,
    mod_token: &str,
    default_mods: KeyModifiers,
    chord: &str,
    action: &str,
) {
    let expanded = chord
        .replace("$var.mod", mod_token)
        .replace("$mod", mod_token);
    let Some((mods, key)) = parse_chord(expanded.as_str()) else {
        return;
    };
    let effective_mods = if modifiers_empty(mods) {
        default_mods
    } else {
        mods
    };
    let action_key = action.trim().to_ascii_lowercase();
    match action_key.as_str() {
        "reload_config" => out.keybinds.reload_config = key,
        "minimize_focused" => out.keybinds.minimize_focused = key,
        "overview_toggle" => out.keybinds.overview_toggle = key,
        "quit_halley" | "quit_compositor" => {
            out.keybinds.quit_compositor = key;
            out.quit_requires_shift = effective_mods.shift;
        }
        _ => {
            out.keybinds.launch_pavucontrol = key;
            out.keybind_launch_command = action.trim().to_string();
            upsert_launch_binding(out, effective_mods, key, action.trim());
        }
    }
}

fn upsert_launch_binding(out: &mut RuntimeTuning, mods: KeyModifiers, key: u32, command: &str) {
    if let Some(existing) = out
        .launch_bindings
        .iter_mut()
        .find(|b| b.key == key && b.modifiers == mods)
    {
        existing.command = command.to_string();
        return;
    }
    out.launch_bindings.push(LaunchBinding {
        modifiers: mods,
        key,
        command: command.to_string(),
    });
}

fn parse_chord(chord: &str) -> Option<(KeyModifiers, u32)> {
    let mut mods = KeyModifiers::default();
    let mut key: Option<u32> = None;
    for raw in chord.split('+') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if apply_modifier_token(&mut mods, t) {
            continue;
        }
        if key.is_some() {
            return None;
        }
        key = key_name_to_evdev(t);
    }
    key.map(|k| (mods, k))
}

fn apply_modifier_token(mods: &mut KeyModifiers, token: &str) -> bool {
    match token.trim().to_ascii_lowercase().as_str() {
        "lalt" => {
            mods.left_alt = true;
            true
        }
        "ralt" => {
            mods.right_alt = true;
            true
        }
        "alt" => {
            mods.alt = true;
            true
        }
        "lshift" => {
            mods.left_shift = true;
            true
        }
        "rshift" => {
            mods.right_shift = true;
            true
        }
        "shift" => {
            mods.shift = true;
            true
        }
        "lctrl" => {
            mods.left_ctrl = true;
            true
        }
        "rctrl" => {
            mods.right_ctrl = true;
            true
        }
        "ctrl" | "control" => {
            mods.ctrl = true;
            true
        }
        "lsuper" | "lwin" => {
            mods.left_super = true;
            true
        }
        "rsuper" | "rwin" => {
            mods.right_super = true;
            true
        }
        "super" | "win" | "windows" | "logo" | "meta" => {
            mods.super_key = true;
            true
        }
        _ => false,
    }
}

fn parse_legacy_keybinds(content: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut in_block = false;
    let mut depth = 0usize;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_block {
            if trimmed.eq_ignore_ascii_case("keybinds:") {
                in_block = true;
                depth = 1;
            }
            continue;
        }

        if trimmed.eq_ignore_ascii_case("end") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                in_block = false;
            }
            continue;
        }

        if trimmed.ends_with(':') {
            depth = depth.saturating_add(1);
            continue;
        }

        if depth != 1 {
            continue;
        }

        if let Some((k, v)) = parse_legacy_keybind_line(trimmed) {
            out.insert(k, v);
        }
    }

    out
}

fn parse_legacy_keybind_line(line: &str) -> Option<(String, String)> {
    let mut clean = String::with_capacity(line.len());
    let mut in_quotes = false;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            clean.push(ch);
            continue;
        }
        if ch == '#' && !in_quotes {
            break;
        }
        clean.push(ch);
    }
    let tokens = parse_quoted_tokens(clean.trim());
    if tokens.len() != 2 {
        return None;
    }
    Some((tokens[0].clone(), tokens[1].clone()))
}

fn parse_quoted_tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if in_quotes && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if !in_quotes && ch.is_ascii_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_legacy_keybind_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    let mut depth = 0usize;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if !in_block {
            if trimmed.eq_ignore_ascii_case("keybinds:") {
                in_block = true;
                depth = 1;
                continue;
            }
            out.push_str(raw);
            out.push('\n');
            continue;
        }

        if trimmed.eq_ignore_ascii_case("end") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                in_block = false;
            }
            continue;
        }
        if trimmed.ends_with(':') {
            depth = depth.saturating_add(1);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_legacy_keybind_block_pairs() {
        let src = r#"
keybinds:
  mod "LAlt"
  "$var.mod+r" "reload_config"
  "$var.mod+Return" "kitty"
end
"#;
        let binds = parse_legacy_keybinds(src);
        assert_eq!(binds.get("mod").map(String::as_str), Some("LAlt"));
        assert_eq!(
            binds.get("$var.mod+r").map(String::as_str),
            Some("reload_config")
        );
        assert_eq!(
            binds.get("$var.mod+Return").map(String::as_str),
            Some("kitty")
        );
    }

    #[test]
    fn loads_config_when_legacy_keybind_block_is_present() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-config-{ts}.rune"));
        let src = r#"
halley:
  runtime:
    tick_ms 321
  end
end

keybinds:
  mod "LAlt"
  "$var.mod+Return" "kitty"
end
"#;
        fs::write(path.as_path(), src).expect("write test config");
        let tuning = RuntimeTuning::load_from_path(path.to_string_lossy().as_ref());
        let _ = fs::remove_file(path.as_path());

        assert_eq!(tuning.tick_ms, 321);
        assert!(tuning.keybinds.modifier.left_alt);
        assert_eq!(tuning.keybind_launch_command, "kitty");
    }

    #[test]
    fn parse_modifiers_keeps_left_right_alt_distinct() {
        let mods = parse_modifiers("LAlt+Shift").expect("mod parse");
        assert!(mods.left_alt);
        assert!(!mods.right_alt);
        assert!(!mods.alt);
        assert!(mods.shift);
    }

    #[test]
    fn loads_env_section_and_overrides_cursor_env_defaults() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("halley-config-env-{ts}.rune"));
        let src = r#"
halley:
  env:
    XCURSOR_THEME "Bibata-Modern-Ice"
    XCURSOR_SIZE "32"
    MOZ_ENABLE_WAYLAND "1"
  end
end
"#;
        fs::write(path.as_path(), src).expect("write test config");
        let tuning = RuntimeTuning::load_from_path(path.to_string_lossy().as_ref());
        let _ = fs::remove_file(path.as_path());

        assert_eq!(
            tuning.env.get("XCURSOR_THEME").map(String::as_str),
            Some("Bibata-Modern-Ice")
        );
        assert_eq!(
            tuning.env.get("XCURSOR_SIZE").map(String::as_str),
            Some("32")
        );
        assert_eq!(
            tuning.env.get("MOZ_ENABLE_WAYLAND").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn explicit_keybinds_without_mod_use_configured_modifier() {
        let mut out = RuntimeTuning::default();
        out.keybinds.modifier = parse_modifiers("LSuper").expect("mod parse");
        let mut bindings = HashMap::new();
        bindings.insert("$var.mod+Return".to_string(), "kitty".to_string());

        apply_explicit_keybind_overrides_map(&bindings, &mut out);

        assert_eq!(out.keybind_launch_command, "kitty");
        let b = out
            .launch_bindings
            .iter()
            .find(|b| b.command == "kitty")
            .expect("missing kitty binding");
        assert!(b.modifiers.left_super);
        assert!(!b.modifiers.left_alt);
    }
}
