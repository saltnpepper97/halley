use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{
    pick_bool, pick_close_restore_pan_mode, pick_f32, pick_overlay_color_mode,
    pick_pan_to_new_mode, pick_pin_badge_corner, pick_u64,
};

pub(crate) fn load_field_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.non_overlap_gap_px = pick_f32(cfg, &["field.gap", "field.gap-px"], out.non_overlap_gap_px);
    out.field_active_windows_allowed = pick_u64(
        cfg,
        &[
            "field.active-windows-allowed",
            "field.active_windows_allowed",
        ],
        out.field_active_windows_allowed as u64,
    ) as usize;
    out.pan_to_new = pick_pan_to_new_mode(
        cfg,
        &["field.pan-to-new", "field.pan_to_new"],
        out.pan_to_new,
    );
    out.pins.corner = pick_pin_badge_corner(
        cfg,
        &[
            "field.pins.corner",
            "field.pins.badge-corner",
            "field.pins.badge_corner",
        ],
        out.pins.corner,
    );
    out.pins.color = pick_overlay_color_mode(
        cfg,
        &[
            "field.pins.colour",
            "field.pins.color",
            "field.pins.pin-colour",
            "field.pins.pin_color",
            "field.pins.pin-color",
        ],
        out.pins.color,
    );
    out.close_restore_focus = pick_bool(
        cfg,
        &["field.close-restore-focus", "field.close_restore_focus"],
        out.close_restore_focus,
    );
    out.close_restore_pan = pick_close_restore_pan_mode(
        cfg,
        &["field.close-restore-pan", "field.close_restore_pan"],
        out.close_restore_pan,
    );
    out.zoom_enabled = pick_bool(
        cfg,
        &["field.zoom.enabled", "field.zoom_enabled"],
        out.zoom_enabled,
    );
    out.zoom_step = pick_f32(cfg, &["field.zoom.step", "field.zoom_step"], out.zoom_step);
    out.zoom_min = pick_f32(cfg, &["field.zoom.min", "field.zoom_min"], out.zoom_min);
    out.zoom_max = pick_f32(cfg, &["field.zoom.max", "field.zoom_max"], out.zoom_max);
    out.zoom_smooth = pick_bool(
        cfg,
        &["field.zoom.smooth", "field.zoom_smooth"],
        out.zoom_smooth,
    );
    out.zoom_smooth_rate = pick_f32(
        cfg,
        &[
            "field.zoom.smooth-rate",
            "field.zoom.smooth_rate",
            "field.zoom_smooth_rate",
        ],
        out.zoom_smooth_rate,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{OverlayColorMode, PinBadgeCorner, RuntimeTuning};

    use super::load_field_section;

    #[test]
    fn field_section_parses_active_window_limit_without_touching_tile_stack_limit() {
        let cfg = RuneConfig::from_str(
            r##"
field:
  active-windows-allowed 7
end
"##,
        )
        .expect("field config should parse");

        let mut out = RuntimeTuning::default();
        out.tile_max_stack = 11;

        load_field_section(&cfg, &mut out);

        assert_eq!(out.field_active_windows_allowed, 7);
        assert_eq!(out.tile_max_stack, 11);
    }

    #[test]
    fn field_section_parses_nested_pins_config() {
        let cfg = RuneConfig::from_str(
            r##"
field:
  pins:
    corner "top-left"
    colour "#d65d26"
  end
end
"##,
        )
        .expect("field pins config should parse");

        let mut out = RuntimeTuning::default();
        load_field_section(&cfg, &mut out);

        assert_eq!(out.pins.corner, PinBadgeCorner::TopLeft);
        assert_eq!(
            out.pins.color,
            OverlayColorMode::Fixed {
                r: 0xd6 as f32 / 255.0,
                g: 0x5d as f32 / 255.0,
                b: 0x26 as f32 / 255.0,
            }
        );
    }
}
