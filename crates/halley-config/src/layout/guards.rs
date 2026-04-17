use super::{CursorConfig, FontConfig, RuntimeTuning};

impl RuntimeTuning {
    pub fn enforce_guards(&mut self) {
        self.clamp_values();
    }

    pub(crate) fn clamp_values(&mut self) {
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
        self.node_icon_size = self.node_icon_size.clamp(0.35, 0.95);
        self.decorations.border.size_px = self.decorations.border.size_px.clamp(0, 64);
        self.decorations.border.radius_px = self.decorations.border.radius_px.clamp(0, 256);
        self.decorations.secondary_border.size_px =
            self.decorations.secondary_border.size_px.clamp(0, 64);
        self.decorations.secondary_border.gap_px =
            self.decorations.secondary_border.gap_px.clamp(0, 8);
        self.bearings.fade_distance = self.bearings.fade_distance.clamp(120.0, 100_000.0);

        self.cluster_distance_px = self.cluster_distance_px.clamp(24.0, 4_000.0);
        self.cluster_dwell_ms = self.cluster_dwell_ms.clamp(0, 30_000);
        self.tile_gaps_inner_px = self.tile_gaps_inner_px.clamp(0.0, 256.0);
        self.tile_gaps_outer_px = self.tile_gaps_outer_px.clamp(0.0, 512.0);
        self.tile_max_stack = self.tile_max_stack.clamp(0, 64);
        self.stacking_max_visible = self.stacking_max_visible.clamp(0, 64);
        self.trail_history_length = self.trail_history_length.clamp(1, 512);
        self.input.repeat_rate = self.input.repeat_rate.clamp(0, 1000);
        self.input.repeat_delay = self.input.repeat_delay.clamp(0, 10_000);
        self.cursor.size = self.cursor.size.clamp(8, 128);
        if self.cursor.theme.trim().is_empty() {
            self.cursor.theme = CursorConfig::default().theme;
        }
        self.font.size = self.font.size.clamp(8, 32);
        if self.font.family.trim().is_empty() {
            self.font.family = FontConfig::default().family;
        }

        self.active_outside_ring_delay_ms = self.active_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.inactive_outside_ring_delay_ms =
            self.inactive_outside_ring_delay_ms.clamp(0, 7_200_000);
        self.docked_offscreen_delay_ms = self.docked_offscreen_delay_ms.clamp(0, 7_200_000);

        self.non_overlap_gap_px = self.non_overlap_gap_px.clamp(0.0, 256.0);
        self.field_active_windows_allowed = self.field_active_windows_allowed.clamp(0, 64);
        self.zoom_step = self.zoom_step.clamp(1.001, 4.0);
        self.zoom_min = self.zoom_min.clamp(0.05, 1.0);
        self.zoom_max = self.zoom_max.clamp(1.0, 16.0);
        if self.zoom_max < self.zoom_min {
            self.zoom_max = self.zoom_min;
        }
        self.zoom_smooth_rate = self.zoom_smooth_rate.clamp(0.1, 120.0);
        self.non_overlap_active_gap_scale = self.non_overlap_active_gap_scale.clamp(0.0, 1.2);
        self.non_overlap_bump_damping = self.non_overlap_bump_damping.clamp(0.05, 1.0);
        self.drag_smoothing_boost = self.drag_smoothing_boost.clamp(0.1, 20.0);
        self.animations.smooth_resize.duration_ms =
            self.animations.smooth_resize.duration_ms.clamp(1, 10_000);
        self.animations.window_close.duration_ms =
            self.animations.window_close.duration_ms.clamp(1, 10_000);
        self.animations.window_open.duration_ms =
            self.animations.window_open.duration_ms.clamp(1, 10_000);
        self.animations.tile.duration_ms = self.animations.tile.duration_ms.clamp(1, 10_000);
        self.animations.stack.duration_ms = self.animations.stack.duration_ms.clamp(1, 10_000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_border_gap_is_clamped_to_small_range() {
        let mut tuning = RuntimeTuning::default();
        tuning.decorations.secondary_border.gap_px = 99;

        tuning.enforce_guards();

        assert_eq!(tuning.decorations.secondary_border.gap_px, 8);
    }
}
