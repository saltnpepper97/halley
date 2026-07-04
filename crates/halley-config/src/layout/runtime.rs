use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;

use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::decay::FocusRingDecayPolicy;
use halley_core::field::Vec2;
use halley_core::viewport::{FocusRing, Viewport};

use crate::keybinds::{CompositorBinding, Keybinds, LaunchBinding, PointerBinding};

use super::paths::{absolutize_path, default_config_path, global_config_path};
use super::{
    AnimationsConfig, ApogeeConfig, BackgroundConfig, BearingsConfig,
    ClickCollapsedOutsideFocusMode, ClickCollapsedPanMode, CloseRestorePanMode,
    ClusterBloomDirection, ClusterDefaultLayout, CursorConfig, DebugConfig, DecorationsConfig,
    EffectsConfig, FocusRingConfig, FontConfig, GamescopeConfig, InputConfig,
    NodeBackgroundColorMode, NodeBorderColorMode, NodeDisplayPolicy, OverlayStyleConfig,
    PanToNewMode, PinsConfig, PlacementConfig, RaiseAnimationTrigger, ScreenshotConfig, ShapeStyle,
    ViewportOutputConfig, WindowCloseAnimationStyle, WindowRule,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigPathSource {
    Explicit,
    User,
    System,
    GeneratedUser,
}

impl ConfigPathSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigPathSource::Explicit => "explicit",
            ConfigPathSource::User => "user",
            ConfigPathSource::System => "system",
            ConfigPathSource::GeneratedUser => "generated user",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedConfigPath {
    pub path: PathBuf,
    pub source: ConfigPathSource,
}

#[derive(Clone, Debug)]
pub struct RuntimeTuning {
    pub viewport_center: Vec2,
    pub viewport_size: Vec2,
    pub background: BackgroundConfig,

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
    pub node_opacity: f32,
    pub node_background_color: NodeBackgroundColorMode,
    pub node_border_color_hover: NodeBorderColorMode,
    pub node_border_color_inactive: NodeBorderColorMode,
    pub decorations: DecorationsConfig,
    pub effects: EffectsConfig,
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
    pub field_active_windows_allowed: usize,
    pub pan_to_new: PanToNewMode,
    pub placement: PlacementConfig,
    pub pins: PinsConfig,
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
    pub window_rules: Vec<WindowRule>,

    pub keybinds: Keybinds,
    pub compositor_bindings: Vec<CompositorBinding>,
    pub launch_bindings: Vec<LaunchBinding>,
    pub pointer_bindings: Vec<PointerBinding>,

    pub tty_viewports: Vec<ViewportOutputConfig>,
    pub autostart_once: Vec<String>,
    pub autostart_on_reload: Vec<String>,
    pub input: InputConfig,
    pub cursor: CursorConfig,
    pub font: FontConfig,
    pub debug: DebugConfig,
    pub apogee: ApogeeConfig,
    pub animations: AnimationsConfig,
    pub overlay_style: OverlayStyleConfig,
    pub screenshot: ScreenshotConfig,
    pub gamescope: GamescopeConfig,
    pub env: HashMap<String, String>,
}
impl RuntimeTuning {
    pub fn default_home_config_path() -> String {
        default_config_path().to_string_lossy().to_string()
    }

    pub fn global_config_path() -> String {
        global_config_path().to_string_lossy().to_string()
    }

    pub fn explicit_config_path_from_env() -> Option<PathBuf> {
        env::var("HALLEY_WL_CONFIG")
            .ok()
            .and_then(|path| explicit_config_path_from_value(path.as_str()))
    }

    pub fn internal_config_template() -> String {
        Self::render_fresh_config(&[])
    }

    pub fn builtin_defaults() -> Self {
        static BUILTIN_DEFAULTS: OnceLock<RuntimeTuning> = OnceLock::new();

        BUILTIN_DEFAULTS
            .get_or_init(|| {
                let template = RuntimeTuning::internal_config_template();
                RuntimeTuning::from_rune_str_with_seed(&template, RuntimeTuning::default())
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

    pub fn window_primary_border_size_px(&self) -> i32 {
        self.decorations.border.size_px.max(0)
    }

    pub fn window_border_radius_px(&self) -> i32 {
        self.decorations.border.radius_px.max(0)
    }

    pub fn window_secondary_border_enabled(&self) -> bool {
        self.decorations.secondary_border.enabled && self.decorations.secondary_border.size_px > 0
    }

    pub fn window_secondary_border_size_px(&self) -> i32 {
        if self.window_secondary_border_enabled() {
            self.decorations.secondary_border.size_px.max(0)
        } else {
            0
        }
    }

    pub fn window_secondary_border_gap_px(&self) -> i32 {
        if self.window_secondary_border_enabled() {
            self.decorations.secondary_border.gap_px.max(0)
        } else {
            0
        }
    }

    pub fn total_window_border_footprint_px(&self) -> i32 {
        self.window_primary_border_size_px()
            + self.window_secondary_border_gap_px()
            + self.window_secondary_border_size_px()
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

    pub fn maximize_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.maximize.enabled
    }

    pub fn maximize_animation_duration_ms(&self) -> u64 {
        self.animations.maximize.duration_ms.max(1)
    }

    pub fn fullscreen_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.fullscreen.enabled
    }

    pub fn fullscreen_animation_duration_ms(&self) -> u64 {
        self.animations.fullscreen.duration_ms.max(1)
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

    pub fn cluster_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.cluster.enabled
    }

    pub fn cluster_tiling_open_duration_ms(&self) -> u64 {
        self.animations.cluster.tiling.open_duration_ms.max(1)
    }

    pub fn cluster_tiling_stagger_ms(&self) -> u64 {
        self.animations.cluster.tiling.stagger_ms
    }

    pub fn cluster_tiling_reflow_duration_ms(&self) -> u64 {
        self.animations.cluster.tiling.reflow_duration_ms.max(1)
    }

    pub fn cluster_tiling_close_duration_ms(&self) -> u64 {
        self.animations.cluster.tiling.close_duration_ms.max(1)
    }

    pub fn cluster_stacking_open_duration_ms(&self) -> u64 {
        self.animations.cluster.stacking.open_duration_ms.max(1)
    }

    pub fn cluster_stacking_close_duration_ms(&self) -> u64 {
        self.animations.cluster.stacking.close_duration_ms.max(1)
    }

    pub fn raise_animation_enabled(&self) -> bool {
        self.animations_enabled() && self.animations.raise.enabled
    }

    pub fn raise_animation_duration_ms(&self) -> u64 {
        self.animations.raise.duration_ms.max(1)
    }

    pub fn raise_animation_scale(&self) -> f32 {
        self.animations.raise.scale.max(1.0)
    }

    pub fn raise_animation_shadow_boost(&self) -> f32 {
        self.animations.raise.shadow_boost.clamp(0.0, 1.0)
    }

    pub fn raise_animation_trigger(&self) -> RaiseAnimationTrigger {
        self.animations.raise.trigger
    }

    pub fn config_path() -> String {
        Self::resolved_config_path()
            .path
            .to_string_lossy()
            .to_string()
    }

    pub fn resolved_config_path() -> ResolvedConfigPath {
        let user_path = default_config_path();
        let system_path = global_config_path();
        resolve_config_path_from_inputs(
            env::var("HALLEY_WL_CONFIG").ok().as_deref(),
            user_path.exists(),
            system_path.exists(),
            user_path,
            system_path,
        )
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
        Self::try_load_from_path_diagnostic(path).ok()
    }

    pub fn try_load_from_path_diagnostic(
        path: &str,
    ) -> Result<Self, crate::parse::ConfigLoadDiagnostic> {
        let mut out = Self::from_rune_file_diagnostic(path)?;
        out.clamp_values();
        Ok(out)
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

fn explicit_config_path_from_value(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| absolutize_path(trimmed))
}

pub(crate) fn resolve_config_path_from_inputs(
    explicit: Option<&str>,
    user_exists: bool,
    system_exists: bool,
    user_path: PathBuf,
    system_path: PathBuf,
) -> ResolvedConfigPath {
    if let Some(path) = explicit.and_then(explicit_config_path_from_value) {
        return ResolvedConfigPath {
            path,
            source: ConfigPathSource::Explicit,
        };
    }

    if user_exists {
        return ResolvedConfigPath {
            path: user_path,
            source: ConfigPathSource::User,
        };
    }

    if system_exists {
        return ResolvedConfigPath {
            path: system_path,
            source: ConfigPathSource::System,
        };
    }

    ResolvedConfigPath {
        path: user_path,
        source: ConfigPathSource::GeneratedUser,
    }
}

const INTERNAL_CONFIG_PREFIX: &str = r##"@author "Dustin Pilgrim"
@description "Spatial Wayland compositor built around infinite workspace navigation"

# Halley is a spatial compositor.
# Instead of fixed workspaces, each monitor has a navigable field where
# windows live in space. You move through that space with panning, zooming,
# clusters, and focus-aware behavior.

# Split configs can be included with `gather`. A gathered file without `as`
# is merged into this config; explicit values here override gathered defaults.
#gather "colors.rune"

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

# Keyboard repeat and pointer-driven focus behavior.
# `focus-mode "click"` preserves the existing click-to-focus behavior.
input:
  repeat-rate 30
  repeat-delay 500
  focus-mode "click"
  # Raise clicked windows independently from focus mode. Hover focus does not imply raise.
  raise-on-click true
  keyboard:
    layout "us"
    variant ""
    options ""
    model ""
  end
  # Pointer gesture and touchscreen protocol passthrough.
  # Touchpad gestures are forwarded through wp_pointer_gestures_v1;
  # touchscreen contacts are forwarded through wl_touch.
  gestures:
    enabled true
    client-passthrough true
    touch-passthrough true
    pinch-to-zoom true
    pinch-scope "empty-field"
    compositor-scope "global"
    modifier "$mod"
    scroll-pan "empty-field"
    swipe-threshold-px 120
    # 3-finger swipe pans the canvas (continuous, with flick momentum).
    pan-fingers 3
    pan-momentum true
    pan-decay-rate 6
    flick-min-px-per-s 200
    # Apogee lives on 4-finger: up opens, down closes.
    swipe-up-4 "apogee-open"
    apogee-swipe-down-4 "apogee-close"
    # Multi-finger hold bindings (released via libinput hold gesture).
    # hold-3 "toggle-state"
    # hold-4 "apogee"
  end
  # Touchpad libinput settings. Unset keys keep libinput's own defaults.
  touchpad:
    tap true
    natural-scroll true
    dwt true
    accel-profile "adaptive"
    scroll-method "two-finger"
    click-method "clickfinger"
  end
  # Mouse / generic-pointer libinput settings.
  mouse:
    natural-scroll false
    accel-profile "flat"
  end
  # Per-device overrides layer on top of the type sections above. Match the name from
  # `libinput list-devices` (exact or substring). Example:
  # devices:
  #   "Logitech MX Master 3":
  #     accel-speed 0.6
  #     natural-scroll true
  #   end
  # end
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

# Debug-only compositor diagnostics.
debug:
  overlay-fps false
  show-ring-when-resizing true
end

# Apogee is the flat overview mosaic. It uses cached window snapshots by default
# so opening it has alt-tab-like performance instead of continuously rendering all windows.
apogee:
  enabled true
  live-previews false
  transition-ms 320
  gap 24
  max-rows 3
  background-dim 0.85
end

"##;

const INTERNAL_CONFIG_SUFFIX: &str = r##"
# Background/gesso draws below the field, windows, and layer-shell surfaces.
# `field-shader` is spatial: it pans and zooms with the Field camera.
background:
  # `field-shader`, `classic`, or `none`.
  mode "field-shader"
  # Built-in field shader.
  shader "space"
  # For mode "classic", set a PNG/JPG path and choose cover/contain/stretch.
  path ""
  fit "cover"
  # Base and accent colours for built-in shaders.
  colour "#181a26"
  accent-colour "#8fa8d8"
  # Overall brightness for shader/image backgrounds.
  intensity 1.0
  # Animated shaders keep repainting for subtle twinkle/pulse.
  animated true
end

# The field is Halley's spatial world for a monitor.
# Windows live on this field instead of being arranged into fixed desktops.
field:
  # Gap in pixels between windows and layout elements.
  gap 20.0
  # Maximum number of non-node windows allowed on the Field before decay takes over.
  # Set to 0 to disable decay entirely.
  active-windows-allowed 5
  # Pinned windows/nodes stay locked in place and remain visible in Bearings.
  pins:
    corner "top-right"
    colour "auto"
    background-colour "auto"
    # Scale for the circular pin badge and glyph.
    size 1.0
  end
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

# Placement controls where new expanded windows initially appear and how the
# readable landmark layer behaves. Expanded windows always allow overlap with
# other expanded windows; this block does not configure overlap permission.
placement:
  expanded:
    # Initial spawn strategy for expanded windows.
    # `center` opens at the target view center. `find-empty` best-effort searches
    # around that center while ignoring expanded windows as blockers.
    strategy "center"
    fallback "center"
    find-empty-mode "best-effort"
  end

  landmarks:
    # Nodes, core nodes, and collapsed clusters remain non-overlapping map objects.
    strategy "nearest-free"
    normal-blocker "relocate"
    pinned-blocker "preserve"
  end

  reveal:
    enabled true
    max-pan-px 360
    animation-ms 180
    # After placement, reveal the new active window if it would otherwise be awkward/offscreen.
    pan-to-new "if-needed"
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

  # Node body (fill) opacity (0.0–1.0). Lower values let the marker fill
  # recede into the field; the border ring and app icon stay opaque.
  opacity 1.0

  # Auto tints the node fill from its border colour.
  background-colour "auto"

  # Border colour source for hovered/inactive nodes.
  # Allowed values: "use-window-active", "use-window-inactive",
  # "use-window-secondary-active", "use-window-secondary-inactive".
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
  show-pinned true
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

  maximize:
    enabled true
    # Visual-only maximize/unmaximize tween; field geometry stays unchanged.
    duration-ms 240
  end

  fullscreen:
    enabled true
    # Visual-only window-to-fullscreen tween for browser videos and apps.
    duration-ms 240
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

  cluster:
    # Opening/closing a cluster *workspace* (distinct from `tile`/`stack` above,
    # which animate reflow/cycling *within* an already-open cluster). open slides
    # members in; close sucks them into the core node. Split per layout.
    enabled true
    tiling:
      open-duration-ms 300   # slide-in cascade
      stagger-ms 55          # per-member delay (slaves first, master last); 0 = together
      close-duration-ms 420  # suck-into-core
      reflow-duration-ms 400 # visible tile glide+grow as a sibling is added/removed
    end
    stacking:
      open-duration-ms 240   # card grow-in
      close-duration-ms 360  # suck-into-core
    end
  end

  raise:
    enabled true
    duration-ms 140
    scale 1.025
    shadow-boost 0.18
    trigger "always"
  end
end

# Renderer-level visual effects: backdrop blur and shadows.
# These sample/composite the world behind a surface, so they live here rather
# than under `decorations:` (which is compositor chrome attached to surfaces).
effects:
  blur:
    # Master switch. When false, no blur is computed anywhere.
    enabled false

    # Allow compositor-owned overlays (Lift/Monocle, Aperture, Alt-Tab,
    # Overview backdrop, popups, labels) to use blur. Each overlay can still
    # opt out via `overlays.blur`.
    overlays true

    # Client window blur behaviour:
    #   "off"    = no global client-window blur (rules may still opt windows in)
    #   "auto"   = blur only when useful (e.g. a translucent/opacity window)
    #   "always" = blur eligible windows unless a rule opts them out
    windows "auto"

    # Layer-shell blur behaviour for bars, launchers, notifications, etc.:
    #   "off"    = never blur layer-shell clients
    #   "auto"   = blur top/overlay layer-shell clients
    #   "always" = blur bottom/top/overlay layer-shell clients
    layer-shell "off"

    # Quality / performance. Dual Kawase is a downsampled multi-pass blur tuned
    # to look Gaussian-ish without the cost of a true large-kernel Gaussian.
    method "dual-kawase"
    radius 24
    passes 3

    # Frosted-glass polish.
    saturation 1.10
    noise 0.012
  end

  shadows:
    window:
      enabled true
      blur-radius 8
      spread 0
      offset-x 0
      offset-y 5
      colour "#05030530"
    end

    node:
      enabled true
      blur-radius 14
      spread 0
      offset-x 0
      offset-y 3
      colour "#05030524"
    end

    overlay:
      enabled true
      blur-radius 24
      spread 1
      offset-x 0
      offset-y 7
      colour "#05030538"
    end
  end
end

# Compositor-owned window borders managed by Halley.
decorations:
  border:
    size 3
    radius 0
    colour-focused "#d65d26"
    colour-unfocused "#333333"
  end

  secondary-border:
    enabled false
    size 1
    gap 2
    colour-focused "#fabd2f"
    colour-unfocused "#1f1f1f"
  end

  resize-using-border true
end

# Styling for compositor-drawn overlays like labels and helper UI.
overlays:
  background-colour "auto"
  text-colour "auto"
  error-colour "#fb4934"
  shape "square"
  borders "true"
  border-source "primary"

  # Opt overlays into the global blur system (requires effects.blur.overlays).
  blur true
end

# Main input bindings.
# Some bindings are context-sensitive. The same key may do different things
# in the field versus inside a tile or stacking layout.
keybinds:
  mod "super"

  # Basic compositor controls.
  "$var.mod+shift+r" "reload"
  "$var.mod+n" "toggle-state"
  "$var.mod+m" "maximize-focused"
  "$var.mod+f" "toggle-fullscreen"
  "$var.mod+p" "toggle-focused-pin"
  "$var.mod+q" "close-focused"
  "$var.mod+o" "apogee"          # the Observatory overview

  # Screenshot capture menu on the bare PrintScreen key.
  "print" "screenshot"

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

  # Vim-style directional focus: walk focus to the nearest window in the field.
  "$var.mod+h" "focus-left"
  "$var.mod+j" "focus-down"
  "$var.mod+k" "focus-up"
  "$var.mod+l" "focus-right"

  # Pan the camera back to centre on the last focused node (quick "go back").
  "$var.mod+space" "center-last-focused"

  # Switch active monitor focus.
  "$var.mod+shift+left" "monitor-focus left"
  "$var.mod+shift+right" "monitor-focus right"
  "$var.mod+shift+up" "monitor-focus up"
  "$var.mod+shift+down" "monitor-focus down"

  # Cluster controls.
  "$var.mod+shift+c" "cluster-mode"
  "$var.mod+l" "cluster-layout cycle"
  "$var.mod+1" "cluster slot 1"
  "$var.mod+2" "cluster slot 2"
  "$var.mod+3" "cluster slot 3"
  "$var.mod+4" "cluster slot 4"
  "$var.mod+5" "cluster slot 5"
  "$var.mod+6" "cluster slot 6"
  "$var.mod+7" "cluster slot 7"
  "$var.mod+8" "cluster slot 8"
  "$var.mod+9" "cluster slot 9"
  "$var.mod+0" "cluster slot 10"

  # Bearings controls.
  "$var.mod+z" "bearings-show"
  "$var.mod+shift+z" "bearings-toggle"

  # Trail navigation.
  "$var.mod+," "trail-prev"
  "$var.mod+." "trail-next"

  # Focus cycling.
  "alt+tab" "cycle-focus"
  "alt+shift+tab" "cycle-focus-backward"

  # Applications.
  # `open-terminal` picks the first supported Wayland terminal in PATH.
  "$var.mod+return" "open-terminal"
  "$var.mod+d" "fuzzel"

  # Mouse actions.
  "$var.mod+leftmouse" "move-window"
  "$var.mod+rightmouse" "resize-window"
  "$var.mod+shift+leftmouse" "pan-field"

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
    # Optional fixed initial size for matching windows.
    #width 720
    #height 520
    # Optional window opacity from 0.0 through 1.0.
    #opacity 0.85
    # Optional per-window backdrop blur override (true/false).
    # `blur false` always wins; `blur true` opts a window in even when
    # effects.blur.windows is "off" or "auto".
    #blur true
    spawn-placement "center"
    cluster-participation "float"
  end
end

# Gamescope integration. When enabled, `halleyctl gamescope run -- %command%`
# (used in a game's Steam launch options) wraps the game in a nested gamescope
# session sized to the selected monitor. The `gamescope` binary is an optional
# runtime dependency; if it is missing the game still launches unwrapped.
gamescope:
  enabled true
  # Monitor to size the session to: "focused", "cursor", "primary", or a connector name.
  monitor "focused"
  # "auto" resolves from the selected monitor; or set explicit pixel values.
  output-width "auto"
  output-height "auto"
  game-width "auto"
  game-height "auto"
  refresh "auto"
  # Fullscreen wins if both fullscreen and borderless are true.
  fullscreen true
  borderless false
  # While a gamescope game holds the pointer, keep Halley's UI out of its way.
  suppress-overlays true
  passthrough-pointer-lock true
  bypass-spatial-camera true

  # Per-game profiles match by `app-id` and inherit the globals above.
  # Set `enabled false` to opt a game out of wrapping.
  #game:
  #  name "Deep Rock Galactic"
  #  app-id "steam_app_548430"
  #  enabled true
  #end
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
        lines
            .push("    # Windows outside it may decay into nodes depending on config.".to_string());
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
    use std::path::PathBuf;

    fn resolved(
        explicit: Option<&str>,
        user_exists: bool,
        system_exists: bool,
    ) -> ResolvedConfigPath {
        resolve_config_path_from_inputs(
            explicit,
            user_exists,
            system_exists,
            PathBuf::from("/home/test/.config/halley/halley.rune"),
            PathBuf::from("/etc/halley/halley.rune"),
        )
    }

    #[test]
    fn explicit_config_wins_over_env_home_and_system() {
        let out = resolved(Some("/tmp/test-halley.rune"), true, true);

        assert_eq!(out.source, ConfigPathSource::Explicit);
        assert_eq!(out.path, PathBuf::from("/tmp/test-halley.rune"));
    }

    #[test]
    fn non_empty_env_config_wins_over_home_and_system() {
        let out = resolved(Some("/tmp/env-halley.rune"), true, true);

        assert_eq!(out.source, ConfigPathSource::Explicit);
        assert_eq!(out.path, PathBuf::from("/tmp/env-halley.rune"));
    }

    #[test]
    fn empty_env_config_is_ignored() {
        let out = resolved(Some("   "), true, true);

        assert_eq!(out.source, ConfigPathSource::User);
        assert_eq!(
            out.path,
            PathBuf::from("/home/test/.config/halley/halley.rune")
        );
    }

    #[test]
    fn home_config_wins_over_system_config() {
        let out = resolved(None, true, true);

        assert_eq!(out.source, ConfigPathSource::User);
        assert_eq!(
            out.path,
            PathBuf::from("/home/test/.config/halley/halley.rune")
        );
    }

    #[test]
    fn system_config_is_used_when_home_config_is_missing() {
        let out = resolved(None, false, true);

        assert_eq!(out.source, ConfigPathSource::System);
        assert_eq!(out.path, PathBuf::from("/etc/halley/halley.rune"));
    }

    #[test]
    fn user_config_is_generation_target_when_no_config_exists() {
        let out = resolved(None, false, false);

        assert_eq!(out.source, ConfigPathSource::GeneratedUser);
        assert_eq!(
            out.path,
            PathBuf::from("/home/test/.config/halley/halley.rune")
        );
    }

    #[test]
    fn total_window_border_footprint_includes_secondary_border_when_enabled() {
        let mut tuning = RuntimeTuning::default();
        assert_eq!(tuning.total_window_border_footprint_px(), 3);

        tuning.decorations.secondary_border.enabled = true;
        tuning.decorations.secondary_border.size_px = 2;
        tuning.decorations.secondary_border.gap_px = 4;
        assert_eq!(tuning.total_window_border_footprint_px(), 9);
    }

    #[test]
    fn builtin_defaults_follow_internal_template() {
        let tuning = RuntimeTuning::builtin_defaults();

        assert_eq!(tuning.node_shape, ShapeStyle::Square);
        assert_eq!(tuning.node_label_shape, ShapeStyle::Square);
        assert_eq!(tuning.cursor.hide_after_ms, 2000);
        assert_eq!(tuning.cluster_dwell_ms, 2000);
        assert_eq!(tuning.field_active_windows_allowed, 5);
        assert_eq!(tuning.input.repeat_rate, 30);
        assert_eq!(tuning.input.repeat_delay, 500);
        assert!(!tuning.debug.overlay_fps);
        assert!(tuning.debug.show_ring_when_resizing);
        assert_eq!(
            tuning.input.keyboard,
            crate::layout::KeyboardConfig::default()
        );
        assert_eq!(tuning.animations.maximize.duration_ms, 240);
        assert_eq!(tuning.animations.fullscreen.duration_ms, 240);
        assert_eq!(tuning.animations.raise.duration_ms, 140);
        assert_eq!(tuning.animations.raise.scale, 1.025);
        assert_eq!(
            tuning.animations.raise.trigger,
            RaiseAnimationTrigger::Always
        );
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
        assert!(rendered.contains("#gather \"colors.rune\""));
        assert!(rendered.contains(
            "  pins:\n    corner \"top-right\"\n    colour \"auto\"\n    background-colour \"auto\""
        ));
        assert!(rendered.contains("    size 1.0"));
        assert!(rendered.contains("  maximize:\n    enabled true"));
        assert!(rendered.contains("  fullscreen:\n    enabled true"));
        assert!(rendered.contains("    duration-ms 240"));
        assert!(rendered.contains("  raise:\n    enabled true\n    duration-ms 140"));
        assert!(rendered.contains("    trigger \"always\""));
        assert!(rendered.contains("  shadows:\n    window:"));
        assert!(rendered.contains("      colour \"#05030530\""));
        assert!(rendered.contains("\"$var.mod+1\" \"cluster slot 1\""));
        assert!(rendered.contains("\"alt+tab\" \"cycle-focus\""));
        assert!(
            rendered
                .contains("input:\n  repeat-rate 30\n  repeat-delay 500\n  focus-mode \"click\"")
        );
        assert!(rendered.contains("  raise-on-click true"));
        assert!(
            rendered.contains("debug:\n  overlay-fps false\n  show-ring-when-resizing true\nend")
        );
        assert!(rendered.contains(
            "  keyboard:\n    layout \"us\"\n    variant \"\"\n    options \"\"\n    model \"\"\n  end"
        ));
        assert!(rendered.contains(
            "  gestures:\n    enabled true\n    client-passthrough true\n    touch-passthrough true\n    pinch-to-zoom true\n    pinch-scope \"empty-field\"\n    compositor-scope \"global\"\n    modifier \"$mod\"\n    scroll-pan \"empty-field\"\n    swipe-threshold-px 120\n"
        ));
        assert!(rendered.contains("    pan-fingers 3\n    pan-momentum true"));
        assert!(rendered.contains("    swipe-up-4 \"apogee-open\""));
        assert!(rendered.contains("    apogee-swipe-down-4 \"apogee-close\""));
        assert!(rendered.contains("  touchpad:\n    tap true\n    natural-scroll true"));
        assert!(rendered.contains("  mouse:\n    natural-scroll false"));
        // The compositor template must never absorb standalone companion-app configs.
        assert!(!rendered.contains("\naperture:"));
        assert!(!rendered.contains("\nlift:"));
    }

    #[test]
    fn render_fresh_config_without_outputs_keeps_documented_viewport_block() {
        let rendered = RuntimeTuning::render_fresh_config(&[]);

        assert!(
            rendered.contains(
                "# Autostart lets Halley launch bars, notifiers, and background helpers."
            )
        );
        assert!(rendered.contains("viewport:\nend\n"));
    }
}
