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

    pub fn internal_config_template() -> &'static str {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/halley.rune"
        ))
    }

    pub fn builtin_defaults() -> Self {
        static BUILTIN_DEFAULTS: OnceLock<RuntimeTuning> = OnceLock::new();

        BUILTIN_DEFAULTS
            .get_or_init(|| {
                RuntimeTuning::from_rune_str_with_seed(
                    RuntimeTuning::internal_config_template(),
                    RuntimeTuning::default(),
                )
                .unwrap_or_default()
            })
            .clone()
    }

    pub fn render_bootstrap_config(
        base_template: &str,
        tty_viewports: &[ViewportOutputConfig],
    ) -> String {
        let viewport_block = render_viewport_section(tty_viewports);
        replace_or_insert_top_level_section(base_template, "viewport", viewport_block.as_str())
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

fn render_viewport_section(tty_viewports: &[ViewportOutputConfig]) -> String {
    if tty_viewports.is_empty() {
        return [
            "# A viewport represents one monitor/output.",
            "# Halley bootstraps detected outputs here on first tty launch.",
            "viewport:",
            "end",
            "",
        ]
        .join("\n");
    }

    let mut lines = vec![
        "# A viewport represents one monitor/output.".to_string(),
        "# Bootstrapped from detected outputs on first launch.".to_string(),
        "viewport:".to_string(),
    ];

    for viewport in tty_viewports {
        lines.push(format!("  {}:", viewport.connector));
        lines.push(format!("    enabled {}", viewport.enabled));
        lines.push(format!("    offset-x {}", viewport.offset_x));
        lines.push(format!("    offset-y {}", viewport.offset_y));
        lines.push(format!("    width {}", viewport.width));
        lines.push(format!("    height {}", viewport.height));
        if let Some(refresh_rate) = viewport.refresh_rate {
            lines.push(format!("    rate {:.3}", refresh_rate));
        }
        lines.push(format!("    transform {}", viewport.transform_degrees));
        lines.push(format!("    vrr \"{}\"", viewport.vrr.as_str()));
        lines.push("  end".to_string());
        lines.push(String::new());
    }

    lines.push("end".to_string());
    lines.push(String::new());
    lines.join("\n")
}

fn replace_or_insert_top_level_section(
    template: &str,
    section_name: &str,
    replacement: &str,
) -> String {
    let sections = top_level_sections(template);
    if let Some((start, end)) = sections
        .iter()
        .find(|(name, _, _)| name == section_name)
        .map(|(_, start, end)| (*start, *end))
    {
        let mut rendered = String::with_capacity(template.len() + replacement.len());
        rendered.push_str(&template[..start]);
        rendered.push_str(replacement);
        rendered.push_str(&template[end..]);
        return rendered;
    }

    let insert_at = sections
        .iter()
        .find(|(name, _, _)| name == "field")
        .map(|(_, start, _)| *start)
        .unwrap_or(template.len());
    let mut rendered = String::with_capacity(template.len() + replacement.len() + 1);
    rendered.push_str(&template[..insert_at]);
    if insert_at > 0 && !template[..insert_at].ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(replacement);
    rendered.push_str(&template[insert_at..]);
    rendered
}

fn top_level_sections(template: &str) -> Vec<(String, usize, usize)> {
    let mut headers = Vec::new();
    let mut offset = 0usize;
    for line in template.split_inclusive('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !line.starts_with([' ', '\t'])
            && !trimmed.starts_with('#')
            && trimmed.ends_with(':')
        {
            headers.push((trimmed.trim_end_matches(':').to_string(), offset));
        }
        offset += line.len();
    }
    if !template.ends_with('\n') {
        let trailing = template[offset..].trim();
        if !trailing.is_empty()
            && !template[offset..].starts_with([' ', '\t'])
            && !trailing.starts_with('#')
            && trailing.ends_with(':')
        {
            headers.push((trailing.trim_end_matches(':').to_string(), offset));
        }
    }

    headers
        .iter()
        .enumerate()
        .map(|(index, (name, start))| {
            let end = headers
                .get(index + 1)
                .map(|(_, next_start)| *next_start)
                .unwrap_or(template.len());
            (name.clone(), *start, end)
        })
        .collect()
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
    fn builtin_defaults_follow_example_config() {
        let tuning = RuntimeTuning::builtin_defaults();

        assert_eq!(tuning.node_shape, ShapeStyle::Square);
        assert_eq!(tuning.node_label_shape, ShapeStyle::Square);
        assert_eq!(tuning.cursor.hide_after_ms, 2000);
        assert_eq!(tuning.cluster_dwell_ms, 2000);
    }

    #[test]
    fn render_bootstrap_config_replaces_viewport_section() {
        let rendered = RuntimeTuning::render_bootstrap_config(
            RuntimeTuning::internal_config_template(),
            &[ViewportOutputConfig {
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
            }],
        );

        assert!(rendered.contains("viewport:\n  DP-1:"));
        assert!(rendered.contains("    rate 180.000"));
        assert!(!rendered.contains("# Example second monitor configuration."));
    }
}
