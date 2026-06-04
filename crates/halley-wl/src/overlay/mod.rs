mod action_row;
mod banner;
mod chip;
mod cluster_bloom;
mod cluster_naming;
mod cluster_overflow;
mod exit_confirm;
mod focus_cycle;
mod fps;
mod hover_label;
mod screenshot;
mod selection_marker;
mod state;
mod style;
mod text;
mod toast;
mod view;

use std::error::Error;

use smithay::{
    backend::renderer::gles::GlesFrame,
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;

pub(crate) use cluster_bloom::{
    bloom_token_hit_test, draw_cluster_bloom, ensure_cluster_bloom_icon_resources,
};
pub(crate) use cluster_naming::{
    ClusterNamingDialogHit, cluster_naming_dialog_hit_test, draw_cluster_naming_dialog,
};
pub(crate) use cluster_overflow::{
    cluster_overflow_icon_hit_test, cluster_overflow_strip_slot_at,
    draw_cluster_overflow_promotion, draw_cluster_overflow_strip,
};
pub(crate) use hover_label::draw_overlay_hover_label;
pub(crate) use screenshot::{ScreenshotMenuHit, draw_screenshot_overlay, screenshot_menu_hit_test};
pub(crate) use selection_marker::draw_cluster_selection_markers;
pub(crate) use state::{
    ClusterBloomAnimSnapshot, ClusterBloomAnimState, ExitConfirmOverlaySnapshot,
    ExitConfirmOverlayState, OverlayActionHint, OverlayBannerSnapshot, OverlayBannerState,
    OverlayToastKind, OverlayToastSnapshot, OverlayToastState,
};
#[cfg(test)]
use style::color_luminance;
use style::{OverlayVisuals, overlay_accent_fill, overlay_text_mix, resolve_overlay_visuals};
pub(crate) use style::{overlay_fill_and_text_colors, overlay_text_color_for_fill};
pub(crate) use toast::{error_toast_hit_test, scroll_error_toast};
pub(crate) use view::OverlayView;

use action_row::{draw_overlay_action_row, overlay_action_row_size};
use banner::draw_persistent_banner;
use chip::{
    draw_overlay_chip, draw_overlay_chip_with_border_color, draw_overlay_chip_without_shadow,
};
use cluster_overflow::draw_overflow_member_chip;
use exit_confirm::draw_exit_confirmation;
use focus_cycle::draw_focus_cycle_switcher;
use fps::draw_debug_fps_overlay;
use text::{truncate_overlay_text, truncate_overlay_text_to_width, visible_overlay_text_window};
use toast::draw_toast;

const BANNER_PAD_X: i32 = 14;
const BANNER_PAD_Y: i32 = 10;
const BANNER_GAP: i32 = 6;
const BANNER_EDGE_PAD: i32 = 18;
const BANNER_TITLE_SCALE: i32 = 2;
const BANNER_META_SCALE: i32 = 2;
const ACTION_ROW_GAP_Y: i32 = 10;
const ACTION_ITEM_GAP: i32 = 18;
const ACTION_LABEL_GAP: i32 = 8;
const ACTION_KEY_PAD_X: i32 = 8;
const ACTION_KEY_PAD_Y: i32 = 6;
const ACTION_KEY_MIN_W: i32 = 48;
const ACTION_KEY_SCALE: i32 = BANNER_META_SCALE;
const ACTION_LABEL_SCALE: i32 = BANNER_META_SCALE;
const SELECT_MARKER_SCALE: i32 = 2;
const TOAST_PAD_X: i32 = 14;
const TOAST_PAD_Y: i32 = 10;
const TOAST_SCALE: i32 = 2;
const TOAST_META_SCALE: i32 = 2;
const ERROR_TOAST_BODY_PAD_X: i32 = 8;
const ERROR_TOAST_BODY_PAD_Y: i32 = 6;
const ERROR_TOAST_BODY_MAX_H: i32 = 120;
const ERROR_TOAST_LINE_GAP: i32 = 5;
const ERROR_TOAST_SCROLLBAR_W: i32 = 4;
const FOCUS_CYCLE_BACKDROP_ALPHA: f32 = 0.20;
const FOCUS_CYCLE_GAP: i32 = 18;
const FOCUS_CYCLE_ICON_PAD: i32 = 10;
const FOCUS_CYCLE_CARD_PAD_X: i32 = 14;
const FOCUS_CYCLE_LABEL_SCALE: i32 = 2;
const FOCUS_CYCLE_META_SCALE: i32 = 1;
const FOCUS_CYCLE_MONITOR_SCALE: i32 = 1;
const FOCUS_CYCLE_VISIBLE_RADIUS: i32 = 3;
const EXIT_CONFIRM_PAD_X: i32 = 18;
const EXIT_CONFIRM_PAD_Y: i32 = 16;
const EXIT_CONFIRM_TITLE_SCALE: i32 = 2;
const EXIT_CONFIRM_MIN_WIDTH: i32 = 280;
const EXIT_CONFIRM_MAX_WIDTH_PAD: i32 = 36;
const SELECT_MARKER_PAD_X: i32 = 8;
const SELECT_MARKER_PAD_Y: i32 = 4;
const OVERFLOW_ICON_PAD: i32 = 8;
const OVERFLOW_ICON_SIZE: i32 = 40;
const OVERFLOW_ICON_GAP: i32 = 8;
const OVERFLOW_VISIBLE_SLOTS: usize = 15;
const OVERFLOW_SCROLLBAR_W: i32 = 4;
const OVERFLOW_SCROLLBAR_PAD: i32 = 6;
const OVERFLOW_REVEAL_ANIM_MS: u64 = 220;
const OVERFLOW_REVEAL_SLIDE_PX: i32 = 28;
const EXIT_CONFIRM_TITLE: &str = "Are you sure you want to leave?";

pub(crate) fn draw_monitor_hud(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    damage: Rectangle<i32, Physical>,
    now: std::time::Instant,
) -> Result<(), Box<dyn Error>> {
    let overlay_monitor = st.model.monitor_state.current_monitor.clone();
    let visuals = resolve_overlay_visuals(&st.runtime.tuning);
    if let Some(exit_confirm) = st
        .ui
        .render_state
        .exit_confirm_snapshot(overlay_monitor.as_str())
    {
        draw_exit_confirmation(
            frame,
            &st.ui.render_state,
            &visuals,
            &st.runtime.tuning.font,
            screen_w,
            screen_h,
            damage,
            &exit_confirm,
        )?;
        return Ok(());
    }
    if draw_focus_cycle_switcher(frame, st, screen_w, screen_h, damage)? {
        return Ok(());
    }
    if let Some(banner) = st
        .ui
        .render_state
        .persistent_mode_banner_snapshot(overlay_monitor.as_str())
    {
        draw_persistent_banner(
            frame,
            &st.ui.render_state,
            &visuals,
            &st.runtime.tuning.font,
            damage,
            &banner,
        )?;
    }
    if let Some(toast) = st
        .ui
        .render_state
        .overlay_toast_snapshot(overlay_monitor.as_str(), st.now_ms(now))
    {
        draw_toast(
            frame,
            &st.ui.render_state,
            &visuals,
            &st.runtime.tuning.font,
            screen_w,
            screen_h,
            damage,
            &toast,
        )?;
    }
    draw_cluster_naming_dialog(frame, st, screen_w, screen_h, damage)?;
    draw_screenshot_overlay(frame, st, screen_w, screen_h, damage)?;
    draw_debug_fps_overlay(frame, st, damage, now)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use halley_config::{
        DecorationBorderColor, OverlayBorderSource, OverlayColorMode, OverlayShape,
    };

    use super::{overlay_accent_fill, overlay_text_color_for_fill, resolve_overlay_visuals};

    #[test]
    fn overlay_auto_text_tracks_background_contrast() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.overlay_style.background_color = OverlayColorMode::Dark;

        let visuals = resolve_overlay_visuals(&tuning);

        assert!(visuals.palette.text.luminance() > visuals.palette.fill.luminance());
    }

    #[test]
    fn overlay_shape_and_border_width_follow_overlay_config() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.decorations.border.size_px = 5;
        tuning.overlay_style.shape = OverlayShape::Rounded;
        tuning.overlay_style.borders = true;

        let visuals = resolve_overlay_visuals(&tuning);

        assert!(visuals.rounded);
        assert_eq!(visuals.border_px, 5.0);

        tuning.overlay_style.borders = false;
        let visuals = resolve_overlay_visuals(&tuning);
        assert_eq!(visuals.border_px, 0.0);
    }

    #[test]
    fn overlay_secondary_border_source_uses_secondary_style_when_enabled() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.decorations.secondary_border.enabled = true;
        tuning.decorations.secondary_border.size_px = 2;
        tuning.decorations.secondary_border.color_focused = DecorationBorderColor {
            r: 0.9,
            g: 0.8,
            b: 0.1,
        };
        tuning.overlay_style.border_source = OverlayBorderSource::Secondary;

        let visuals = resolve_overlay_visuals(&tuning);

        assert_eq!(visuals.border_px, 2.0);
        assert_eq!(
            (
                visuals.palette.border.r,
                visuals.palette.border.g,
                visuals.palette.border.b
            ),
            (0.9, 0.8, 0.1)
        );
    }

    #[test]
    fn overlay_secondary_border_source_falls_back_to_primary_when_disabled() {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.decorations.border.size_px = 4;
        tuning.decorations.border.color_focused = DecorationBorderColor {
            r: 0.1,
            g: 0.2,
            b: 0.3,
        };
        tuning.overlay_style.border_source = OverlayBorderSource::Secondary;

        let visuals = resolve_overlay_visuals(&tuning);

        assert_eq!(visuals.border_px, 4.0);
        assert_eq!(
            (
                visuals.palette.border.r,
                visuals.palette.border.g,
                visuals.palette.border.b
            ),
            (0.1, 0.2, 0.3)
        );
    }

    #[test]
    fn overlay_accent_fill_pulls_toward_border_color() {
        let tuning = halley_config::RuntimeTuning::default();
        let visuals = resolve_overlay_visuals(&tuning);

        let accent = overlay_accent_fill(&visuals, 0.5, 1.0);

        assert_ne!(accent.r(), visuals.palette.fill.r);
        assert_ne!(accent.g(), visuals.palette.fill.g);
        assert_ne!(accent.b(), visuals.palette.fill.b);
    }

    #[test]
    fn overlay_text_for_fill_tracks_fill_contrast() {
        let dark_text = overlay_text_color_for_fill(
            smithay::backend::renderer::Color32F::new(0.10, 0.12, 0.14, 1.0),
            1.0,
        );
        let light_text = overlay_text_color_for_fill(
            smithay::backend::renderer::Color32F::new(0.92, 0.95, 0.98, 1.0),
            1.0,
        );

        assert!(super::color_luminance(dark_text) > super::color_luminance(light_text));
    }
}
