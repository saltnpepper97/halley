use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::OnceLock;

use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::decay::FocusRingDecayPolicy;
use halley_core::field::Vec2;
use halley_core::viewport::{FocusRing, Viewport};

use crate::keybinds::{CompositorBinding, Keybinds, LaunchBinding, PointerBinding};

use super::paths::{absolutize_path, default_config_path, global_config_path};
use super::{
    AnimationsConfig, BearingsConfig, ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode,
    CloseRestorePanMode, ClusterBloomDirection, ClusterDefaultLayout, CursorConfig,
    DecorationBorderColor, FocusRingConfig, FontConfig, NodeBackgroundColorMode,
    NodeBorderColorMode, NodeDisplayPolicy, OverlayStyleConfig, PanToNewMode, ScreenshotConfig,
    ShapeStyle, ViewportOutputConfig, WindowCloseAnimationStyle, WindowRule,
};

#[derive(Clone, Debug)]
pub struct RuntimeTuning {
    pub viewport_center: Vec2,
    pub viewport_size: Vec2,

    pub focus_ring_rx: f32,
    pub focus_ring_ry: f32,
    pub focus_ring_offset_x: f32,
    pub focus_ring_offset_y: f32,

    pub primary_hot_inner_frac: f32,
    pub primary_to_node_ms: u64,
    pub node_show_labels: NodeDisplayPolicy,
    pub node_show_app_icons: NodeDisplayPolicy,
    pub node_shape: ShapeStyle,
    pub node_label_shape: ShapeStyle,
    pub node_icon_size: f32,
    pub node_background_color: NodeBackgroundColorMode,
    pub node_border_color_hover: NodeBorderColorMode,
    pub node_border_color_inactive: NodeBorderColorMode,
    pub border_size_px: i32,
    pub border_radius_px: i32,
    pub border_color_focused: DecorationBorderColor,
    pub border_color_unfocused: DecorationBorderColor,
    pub resize_using_border: bool,
    pub click_collapsed_outside_focus: ClickCollapsedOutsideFocusMode,
    pub click_collapsed_pan: ClickCollapsedPanMode,
    pub bearings: BearingsConfig,

    pub cluster_distance_px: f32,
    pub cluster_dwell_ms: u64,
    pub cluster_show_icons: bool,
    pub cluster_bloom_direction: ClusterBloomDirection,
    pub cluster_default_layout: ClusterDefaultLayout,
    pub tile_gaps_inner_px: f32,
    pub tile_gaps_outer_px: f32,
    pub tile_new_on_top: bool,
    pub tile_queue_show_icons: bool,
    pub tile_max_stack: usize,
    pub stacking_max_visible: usize,
    pub trail_history_length: usize,
    pub trail_wrap: bool,

    pub active_outside_ring_delay_ms: u64,
    pub inactive_outside_ring_delay_ms: u64,
    pub docked_offscreen_delay_ms: u64,

    pub non_overlap_gap_px: f32,
    pub pan_to_new: PanToNewMode,
    pub close_restore_focus: bool,
    pub close_restore_pan: CloseRestorePanMode,
    pub zoom_enabled: bool,
    pub zoom_step: f32,
    pub zoom_min: f32,
    pub zoom_max: f32,
    pub zoom_smooth: bool,
    pub zoom_smooth_rate: f32,
    pub non_overlap_active_gap_scale: f32,
    pub non_overlap_bump_newer: bool,
    pub non_overlap_bump_damping: f32,
    pub drag_smoothing_boost: f32,
    pub center_window_to_mouse: bool,
    pub restore_last_active_on_pan_return: bool,
    pub physics_enabled: bool,
    pub no_csd: bool,
    pub window_rules: Vec<WindowRule>,

    pub keybinds: Keybinds,
    pub compositor_bindings: Vec<CompositorBinding>,
    pub launch_bindings: Vec<LaunchBinding>,
    pub pointer_bindings: Vec<PointerBinding>,

    pub tty_viewports: Vec<ViewportOutputConfig>,
    pub autostart_once: Vec<String>,
    pub autostart_on_reload: Vec<String>,
    pub cursor: CursorConfig,
    pub font: FontConfig,
    pub animations: AnimationsConfig,
    pub overlay_style: OverlayStyleConfig,
    pub screenshot: ScreenshotConfig,
    pub env: HashMap<String, String>,
}
impl RuntimeTuning {
    pub fn default_home_config_path() -> String {
        default_config_path().to_string_lossy().to_string()
    }

    pub fn global_config_path() -> String {
        global_config_path().to_string_lossy().to_string()
    }

    pub fn internal_config_template() -> String {
        Self::render_fresh_config(&[])
    }

    pub fn builtin_defaults() -> Self {
        static BUILTIN_DEFAULTS: OnceLock<RuntimeTuning> = OnceLock::new();

        BUILTIN_DEFAULTS
            .get_or_init(|| {
                let template = RuntimeTuning::internal_config_template();
                RuntimeTuning::from_rune_str_with_seed(
                    &template,
                    RuntimeTuning::default(),
                )
                .unwrap_or_default()
            })
            .clone()
    }

    pub fn render_fresh_config(tty_viewports: &[ViewportOutputConfig]) -> String {
        let viewport_block = render_viewport_section(tty_viewports);
        let mut rendered = String::with_capacity(
            INTERNAL_CONFIG_PREFIX.len() + viewport_block.len() + INTERNAL_CONFIG_SUFFIX.len(),
        );
        rendered.push_str(INTERNAL_CONFIG_PREFIX);
        rendered.push_str(viewport_block.as_str());
        rendered.push_str(INTERNAL_CONFIG_SUFFIX);
        rendered
    }

    pub fn effective_no_csd(&self) -> bool {
        self.no_csd || self.border_radius_px > 0
    }

    pub fn cluster_layout_kind(&self) -> ClusterWorkspaceLayoutKind {
        self.cluster_default_layout.to_workspace_layout_kind()
    }

    pub fn active_cluster_visible_limit(&self) -> usize {
        match self.cluster_layout_kind() {
            ClusterWorkspaceLayoutKind::Tiling => self.tile_max_stack,
            ClusterWorkspaceLayoutKind::Stacking => self.stacking_max_visible,
        }
    }

    pub fn animations_enabled(&self) -> bool {
        self.animations.enabled
    }

    pub fn smooth_resize_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.smooth_resize.enabled
    }

    pub fn smooth_resize_duration_ms(&self) -> u64 {
        self.animations.smooth_resize.duration_ms.max(1)
    }

    pub fn window_close_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.window_close.enabled
    }

    pub fn window_close_duration_ms(&self) -> u64 {
        self.animations.window_close.duration_ms.max(1)
    }

    pub fn window_close_style(&self) -> WindowCloseAnimationStyle {
        self.animations.window_close.style
    }

    pub fn window_open_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.window_open.enabled
    }

    pub fn window_open_duration_ms(&self) -> u64 {
        self.animations.window_open.duration_ms.max(1)
    }

    pub fn tile_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.tile.enabled
    }

    pub fn tile_animation_duration_ms(&self) -> u64 {
        self.animations.tile.duration_ms.max(1)
    }

    pub fn stack_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.stack.enabled
    }

    pub fn stack_animation_duration_ms(&self) -> u64 {
        self.animations.stack.duration_ms.max(1)
    }

    pub fn config_path() -> String {
        match env::var("HALLEY_WL_CONFIG") {
            Ok(path) => absolutize_path(&path).to_string_lossy().to_string(),
            Err(_) => {
                let home = default_config_path();
                if Path::new(&home).exists() {
                    home.to_string_lossy().to_string()
                } else {
                    let global = global_config_path();
                    if Path::new(&global).exists() {
                        global.to_string_lossy().to_string()
                    } else {
                        home.to_string_lossy().to_string()
                    }
                }
            }
        }
    }

    pub fn load() -> Self {
        Self::load_from_path(&Self::config_path())
    }

    pub fn load_from_path(path: &str) -> Self {
        let mut out = Self::try_load_from_path(path).unwrap_or_else(Self::builtin_defaults);
        out.clamp_values();
        out
    }

    pub fn try_load_from_path(path: &str) -> Option<Self> {
        let mut out = Self::from_rune_file(path)?;
        out.clamp_values();
        Some(out)
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

        let theme = self.cursor.theme.trim();
        if !theme.is_empty() {
            unsafe { env::set_var("XCURSOR_THEME", theme) };
        }
        unsafe { env::set_var("XCURSOR_SIZE", self.cursor.size.to_string()) };
    }

    pub fn viewport(&self) -> Viewport {
        Viewport::new(self.viewport_center, self.viewport_size)
    }

    pub fn focus_ring(&self) -> FocusRing {
        FocusRingConfig {
            rx: self.focus_ring_rx,
            ry: self.focus_ring_ry,
            offset_x: self.focus_ring_offset_x,
            offset_y: self.focus_ring_offset_y,
        }
        .to_focus_ring()
    }

    pub fn focus_ring_for_output(&self, output_name: &str) -> FocusRing {
        self.tty_viewports
            .iter()
            .find(|viewport| viewport.connector == output_name)
            .and_then(|viewport| viewport.focus_ring)
            .unwrap_or(FocusRingConfig {
                rx: self.focus_ring_rx,
                ry: self.focus_ring_ry,
                offset_x: self.focus_ring_offset_x,
                offset_y: self.focus_ring_offset_y,
            })
            .to_focus_ring()
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

    pub fn zoom_resolved_summary(&self) -> String {
        format!(
            "enabled={} step={:.3} min={:.3} max={:.3} smooth={} smooth_rate={:.3}",
            self.zoom_enabled,
            self.zoom_step,
            self.zoom_min,
            self.zoom_max,
            self.zoom_smooth,
            self.zoom_smooth_rate,
        )
    }
}

const INTERNAL_CONFIG_PREFIX: &str = r##"@author "Dustin Pilgrim"
@description "Spatial Wayland compositor built around infinite workspace navigation"

# Halley is a spatial compositor.
# Instead of fixed workspaces, each monitor has a navigable field where
# windows live in space. You move through that space with panning, zooming,
# clusters, and focus-aware behavior.

# Optional environment variables for apps launched by Halley.
# Uncomment these if you want to prefer Wayland for Qt apps and use qt6ct.
#env:
#  QT_QPA_PLATFORM "wayland"
#  QT_QPA_PLATFORMTHEME "qt6ct"
#end

# Autostart lets Halley launch bars, notifiers, and background helpers.
# `once` runs only on compositor startup. `on-reload` runs after a config reload.
autostart:
  # Common examples you may want later:
  #once "waybar"

  #once "mako"
  #once "gessod"
  #once "stasis"

  # Example:
  #on-reload "thunderbird"
end

# Cursor settings apply to the compositor itself and child apps started by Halley.
# `hide-when-typing` is useful when you mostly drive the field with the keyboard.
cursor:
  theme "Adwaita"
  size 24
  hide-when-typing true
  hide-after-ms 2000
end

# Default font used for compositor UI like labels and overlays.
font:
  family "monospace"
  size 11
end

# Where screenshots taken through Halley are saved.
# Use an absolute path or an env-expanded path like `$env.HOME/...`.
screenshot:
  directory "$env.HOME/Pictures/Screenshots/"
end

"##;

const INTERNAL_CONFIG_SUFFIX: &str = r##"
# The field is Halley's spatial world for a monitor.
# Windows live on this field instead of being arranged into fixed desktops.
field:
  # Gap in pixels between windows and layout elements.
  gap 20.0
  # Maximum number of fully active windows before decay takes over.
  active-windows-allowed 5
  # How aggressively the camera pans to newly opened windows.
  pan-to-new "if-needed"
  close-restore-focus true
  close-restore-pan "if-offscreen"

  zoom:
    enabled true
    step 1.10
    min 0.35
    max 1.35
    smooth true
    smooth-rate 12.5
  end
end

# A node is Halley's collapsed representation of a window.
# When a window is no longer active enough to stay expanded,
# it can decay into a compact node that still exists on the field.
node:
  # Keep nodes recognizable without making the field too noisy.
  show-labels "hover"
  # `always`, `hover`, or `off` for real app icons. Halley falls back to
  # the app-id initial when an icon is unavailable or intentionally hidden.
  show-app-icons "always"

  node-shape "square"
  node-label-shape "square"

  # Size is a fraction of the node diameter.
  icon-size 0.72

  # Auto tints the node fill from its border colour.
  background-colour "auto"

  border-colour-hover "use-window-active"
  border-colour-inactive "use-window-inactive"

  click-collapsed-outside-focus "activate"
  click-collapsed-pan "if-offscreen"
end

# Decay controls how windows transition between active, inactive,
# and collapsed states.
# Lower values make Halley condense inactive work more quickly.
decay:
  active-delay 240
  inactive-delay 120
end

# Trail is Halley's navigation history.
# Think back/forward through previously focused places or windows.
trail:
  history-length 25
  wrap true
end

# Bearings are directional indicators for offscreen things.
# They can show both labels and distance to help you re-orient quickly.
bearings:
  show-distance true
  show-icons true
  fade-distance 1200
end

# Clusters are Halley's workspace-like grouping system.
# Unlike traditional workspaces, clusters live in the field.
clusters:
  cluster-dwell-ms 2000
  distance-px 280.0
  bloom-direction "clockwise"
  show-icons true
  default-layout "stacking"
end

# Settings for tiled layout inside a cluster.
tile:
  new-on-top false
  gaps-inner 20
  gaps-outer 20
  max-stack 4
  queue-show-icons true
end

# Settings for stacking layout inside a cluster.
stacking:
  max-visible 5
end

# Halley can use gentle physics-style motion instead of purely rigid snapping.
physics:
  enabled true
  damping 0.45
end

# Animation controls for window and layout transitions.
animations:
  enabled true

  smooth-resize:
    enabled true
    duration-ms 90  # lower = tighter, higher = softer
  end

  window-open:
    enabled true
    duration-ms 620
  end

  window-close:
    enabled true
    duration-ms 270
    style "shrink"
  end

  tile:
    enabled true
    duration-ms 240
  end

  stack:
    enabled true
    duration-ms 220
  end
end

# Server-side decoration settings managed by Halley.
decorations:
  border-size 3
  border-radius 0
  border-colour-focused "#d65d26"
  border-colour-unfocused "#333333"
  resize-using-border true

  # Rounded borders usually require server-side decorations so
  # clients do not draw their own square frame inside the compositor shape.
  no-csd false
end

# Styling for compositor-drawn overlays like labels and helper UI.
overlays:
  background-colour "auto"
  text-colour "auto"
  shape "square"
  borders "true"
end

# Main input bindings.
# Some bindings are context-sensitive. The same key may do different things
# in the field versus inside a tile or stacking layout.
keybinds:
  mod "super"

  # Basic compositor controls.
  "$var.mod+shift+r" "reload"
  "$var.mod+n" "toggle-state"
  "$var.mod+q" "close-focused"

  # Zoom controls for the field camera.
  "$var.mod+mousewheelup" "zoom-in"
  "$var.mod+mousewheeldown" "zoom-out"
  "$var.mod+middlemouse" "zoom-reset"

  "$var.mod+shift+e" "quit"

  # Move the selected/latest node in the field.
  "$var.mod+left" "node-move left"
  "$var.mod+right" "node-move right"
  "$var.mod+up" "node-move up"
  "$var.mod+down" "node-move down"

  # Switch active monitor focus.
  "$var.mod+shift+left" "monitor-focus left"
  "$var.mod+shift+right" "monitor-focus right"
  "$var.mod+shift+up" "monitor-focus up"
  "$var.mod+shift+down" "monitor-focus down"

  # Cluster controls.
  "$var.mod+shift+c" "cluster-mode"
  "$var.mod+l" "cluster-layout cycle"

  # Bearings controls.
  "$var.mod+z" "bearings-show"
  "$var.mod+shift+z" "bearings-toggle"

  # Trail navigation.
  "$var.mod+," "trail-prev"
  "$var.mod+." "trail-next"

  # Applications.
  # `open-terminal` picks the first supported Wayland terminal in PATH.
  "$var.mod+return" "open-terminal"
  "$var.mod+d" "fuzzel"

  # Mouse actions.
  "$var.mod+leftmouse" "move-window"
  "$var.mod+rightmouse" "resize-window"
  "$var.mod+shift+leftmouse" "field-jump"

  # Tile layout controls.
  "$var.mod+left" "tile-focus left"
  "$var.mod+right" "tile-focus right"
  "$var.mod+up" "tile-focus up"
  "$var.mod+down" "tile-focus down"

  "$var.mod+ctrl+left" "tile-swap left"
  "$var.mod+ctrl+right" "tile-swap right"
  "$var.mod+ctrl+up" "tile-swap up"
  "$var.mod+ctrl+down" "tile-swap down"

  # Stacking layout controls.
  "$var.mod+left" "stack-cycle forward"
  "$var.mod+right" "stack-cycle backward"

  # Screenshot UI
  "$var.mod+shift+s" "halleyctl capture menu"

  # Media keys.
  "XF86AudioRaiseVolume" "wpctl set-volume -l 1 @default_audio_sink@ 5%+"
  "XF86AudioLowerVolume" "wpctl set-volume @default_audio_sink@ 5%-"
  "XF86AudioMute" "wpctl set-mute @default_audio_sink@ toggle"
end

# Rules let you special-case certain windows/apps.
# This example keeps common Firefox file dialogs centered and floating.
rules:
  rule:
    app-id "firefox"
    title [r"File Upload.*", r"Open File.*", r"Save File.*", r"Choose.*"]
    overlap-policy "all"
    spawn-placement "center"
    cluster-participation "float"
  end
end
"##;

fn render_viewport_section(tty_viewports: &[ViewportOutputConfig]) -> String {
    if tty_viewports.is_empty() {
        return [
            "# A viewport represents one monitor/output.",
            "# On first tty launch Halley writes the detected outputs here for you.",
            "# If you want to manage monitors manually later, edit this section.",
            "viewport:",
            "end",
            "",
        ]
        .join("\n");
    }

    let defaults = RuntimeTuning::builtin_defaults();
    let default_focus_ring = FocusRingConfig {
        rx: defaults.focus_ring_rx,
        ry: defaults.focus_ring_ry,
        offset_x: defaults.focus_ring_offset_x,
        offset_y: defaults.focus_ring_offset_y,
    };

    let mut lines = vec![
        "# A viewport represents one monitor/output.".to_string(),
        "# On first tty launch Halley writes the detected outputs here for you.".to_string(),
        "# If you want to manage monitors manually later, edit this section.".to_string(),
        "viewport:".to_string(),
    ];

    for viewport in tty_viewports {
        let focus_ring = viewport.focus_ring.unwrap_or(default_focus_ring);
        lines.push(format!("  {}:", viewport.connector));
        lines.push(format!("    enabled {}", viewport.enabled));
        lines.push(String::new());
        lines.push(format!("    offset-x {}", viewport.offset_x));
        lines.push(format!("    offset-y {}", viewport.offset_y));
        lines.push(String::new());
        lines.push(format!("    width {}", viewport.width));
        lines.push(format!("    height {}", viewport.height));
        lines.push(String::new());
        lines.push(format!(
            "    rate {:.3}",
            viewport.refresh_rate.unwrap_or(60.0)
        ));
        lines.push(format!("    transform {}", viewport.transform_degrees));
        lines.push(format!("    vrr \"{}\"", viewport.vrr.as_str()));
        lines.push("    # The focus ring is Halley's active zone.".to_string());
        lines.push("    # Windows inside it stay more fully active.".to_string());
        lines.push(
            "    # Windows outside it may decay into nodes depending on config.".to_string(),
        );
        lines.push("    focus-ring:".to_string());
        lines.push(format!("      primary-rx {:.1}", focus_ring.rx));
        lines.push(format!("      primary-ry {:.1}", focus_ring.ry));
        lines.push(format!("      offset-x {:.0}", focus_ring.offset_x));
        lines.push(format!("      offset-y {:.0}", focus_ring.offset_y));
        lines.push("    end".to_string());
        lines.push("  end".to_string());
    }

    lines.extend([
        "  # Example second monitor configuration.".to_string(),
        "  # Uncomment and edit if needed.".to_string(),
        "  #DP-2:".to_string(),
        "  #  enabled true".to_string(),
        "  #".to_string(),
        "  #  offset-x 0".to_string(),
        "  #  offset-y 0".to_string(),
        "  #".to_string(),
        "  #  width 1920".to_string(),
        "  #  height 1200".to_string(),
        "  #".to_string(),
        "  #  rate 75.0".to_string(),
        "  #  transform 0".to_string(),
        "  #  vrr \"off\"".to_string(),
        "  #".to_string(),
        "  #  focus-ring:".to_string(),
        format!("  #    primary-rx {:.1}", default_focus_ring.rx),
        format!("  #    primary-ry {:.1}", default_focus_ring.ry),
        format!("  #    offset-x {:.0}", default_focus_ring.offset_x),
        format!("  #    offset-y {:.0}", default_focus_ring.offset_y),
        "  #  end".to_string(),
        "  #end".to_string(),
    ]);

    if let Some(last) = lines.last()
        && !last.is_empty()
    {
        lines.push(String::new());
    }

    lines.push("end".to_string());
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_borders_force_effective_no_csd() {
        let mut tuning = RuntimeTuning::default();
        tuning.no_csd = false;
        tuning.border_radius_px = 12;
        assert!(tuning.effective_no_csd());

        tuning.border_radius_px = 0;
        assert!(!tuning.effective_no_csd());
    }

    #[test]
    fn builtin_defaults_follow_internal_template() {
        let tuning = RuntimeTuning::builtin_defaults();

        assert_eq!(tuning.node_shape, ShapeStyle::Square);
        assert_eq!(tuning.node_label_shape, ShapeStyle::Square);
        assert_eq!(tuning.cursor.hide_after_ms, 2000);
        assert_eq!(tuning.cluster_dwell_ms, 2000);
    }

    #[test]
    fn render_fresh_config_includes_detected_viewports() {
        let rendered = RuntimeTuning::render_fresh_config(&[ViewportOutputConfig {
                connector: "DP-1".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 2560,
                height: 1440,
                refresh_rate: Some(180.0),
                transform_degrees: 0,
                vrr: crate::ViewportVrrMode::Off,
                focus_ring: None,
            }]);

        assert!(rendered.contains("viewport:\n  DP-1:"));
        assert!(rendered.contains("    rate 180.000"));
        assert!(rendered.contains("# Example second monitor configuration."));
        assert!(rendered.contains("    focus-ring:"));
        assert!(rendered.contains("# Cursor settings apply to the compositor itself"));
    }

    #[test]
    fn render_fresh_config_without_outputs_keeps_documented_viewport_block() {
        let rendered = RuntimeTuning::render_fresh_config(&[]);

        assert!(rendered.contains("# Autostart lets Halley launch bars, notifiers, and background helpers."));
        assert!(rendered.contains("viewport:\nend\n"));
    }
}
