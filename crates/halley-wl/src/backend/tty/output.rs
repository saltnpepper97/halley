use std::collections::HashMap;

use eventline::warn;
use halley_config::{RuntimeTuning, ViewportOutputConfig, ViewportVrrMode};
use smithay::reexports::drm::control as drm_control;

use super::drm::TtyDrmOutput;

pub(super) fn active_output_names(outputs: &[TtyDrmOutput]) -> Vec<String> {
    outputs
        .iter()
        .map(|output| output.connector_name.clone())
        .collect()
}

pub(super) fn active_mode_map(outputs: &[TtyDrmOutput]) -> HashMap<String, drm_control::Mode> {
    outputs
        .iter()
        .map(|output| (output.connector_name.clone(), output.mode))
        .collect()
}

pub(super) fn outputs_match(a: &[TtyDrmOutput], b: &[TtyDrmOutput]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter().all(|left| {
        b.iter().any(|right| {
            left.connector_name == right.connector_name
                && left.crtc == right.crtc
                && left.mode.size() == right.mode.size()
                && left.mode.vrefresh() == right.mode.vrefresh()
        })
    })
}

pub(super) fn bootstrap_tty_viewports(outputs: &[TtyDrmOutput]) -> Vec<ViewportOutputConfig> {
    let mut ordered: Vec<_> = outputs
        .iter()
        .map(|output| {
            let (width, height) = output.mode.size();
            (
                output.connector_name.clone(),
                width as u32,
                height as u32,
                output.mode.vrefresh() as f64,
            )
        })
        .collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    let mut offset_x = 0;
    ordered
        .into_iter()
        .map(|(connector, width, height, refresh_rate)| {
            let viewport = ViewportOutputConfig {
                connector,
                enabled: true,
                offset_x,
                offset_y: 0,
                width,
                height,
                refresh_rate: Some(refresh_rate),
                transform_degrees: 0,
                vrr: ViewportVrrMode::Off,
                focus_ring: None,
            };
            offset_x += width as i32;
            viewport
        })
        .collect()
}

pub(super) fn effective_tty_viewports_for_outputs(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> Vec<ViewportOutputConfig> {
    let active_names = active_output_names(outputs);
    let configured: Vec<_> = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .filter(|viewport| active_names.iter().any(|name| name == &viewport.connector))
        .cloned()
        .collect();
    if !configured.is_empty() {
        return configured;
    }

    bootstrap_tty_viewports(outputs)
}

fn effective_tty_viewport_fallback_reason(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> Option<&'static str> {
    let active_names = active_output_names(outputs);
    let enabled_configured = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled);
    let matched = enabled_configured
        .clone()
        .any(|viewport| active_names.iter().any(|name| name == &viewport.connector));
    if matched {
        return None;
    }

    if tuning.tty_viewports.is_empty() {
        Some("no viewport outputs configured")
    } else if tuning
        .tty_viewports
        .iter()
        .all(|viewport| !viewport.enabled)
    {
        Some("viewport outputs configured but none are enabled")
    } else {
        Some("no enabled viewport outputs matched detected outputs")
    }
}

pub(super) fn log_effective_tty_viewport_fallback(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
    source: &str,
) {
    let Some(reason) = effective_tty_viewport_fallback_reason(tuning, outputs) else {
        return;
    };
    let layout = effective_tty_viewports_for_outputs(tuning, outputs)
        .into_iter()
        .map(|viewport| {
            let refresh = viewport
                .refresh_rate
                .map(|hz| format!("@{hz:.3}Hz"))
                .unwrap_or_default();
            format!(
                "{}={}x{}{}+{}+{}",
                viewport.connector,
                viewport.width,
                viewport.height,
                refresh,
                viewport.offset_x,
                viewport.offset_y,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    warn!(
        "{}: tty monitor fallback active: {}; derived layout [{}]",
        source, reason, layout
    );
}

fn effective_tty_viewport_for_output(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
    output_name: &str,
) -> Option<ViewportOutputConfig> {
    effective_tty_viewports_for_outputs(tuning, outputs)
        .into_iter()
        .find(|viewport| viewport.connector == output_name)
}

pub(super) fn canonical_tty_main_output_name(
    outputs: &[TtyDrmOutput],
    tuning: &RuntimeTuning,
) -> Option<String> {
    let effective_viewports = effective_tty_viewports_for_outputs(tuning, outputs);
    outputs
        .iter()
        .min_by(|a, b| {
            let a_viewport = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == a.connector_name);
            let b_viewport = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == b.connector_name);

            let a_offset_x = a_viewport.map(|viewport| viewport.offset_x).unwrap_or(0);
            let b_offset_x = b_viewport.map(|viewport| viewport.offset_x).unwrap_or(0);
            let a_offset_y = a_viewport.map(|viewport| viewport.offset_y).unwrap_or(0);
            let b_offset_y = b_viewport.map(|viewport| viewport.offset_y).unwrap_or(0);

            a_offset_x
                .cmp(&b_offset_x)
                .then(a_offset_y.cmp(&b_offset_y))
                .then(a.connector_name.cmp(&b.connector_name))
        })
        .map(|output| output.connector_name.clone())
}

pub(super) fn output_advertise_order(
    outputs: &[TtyDrmOutput],
    tuning: &RuntimeTuning,
) -> Vec<String> {
    let main_output = canonical_tty_main_output_name(outputs, tuning);
    let effective_viewports = effective_tty_viewports_for_outputs(tuning, outputs);
    let mut ordered: Vec<(String, i32, i32, bool)> = outputs
        .iter()
        .map(|output| {
            let (offset_x, offset_y) = effective_viewports
                .iter()
                .find(|viewport| viewport.connector == output.connector_name)
                .map(|viewport| (viewport.offset_x, viewport.offset_y))
                .unwrap_or((0, 0));
            let is_main = main_output
                .as_deref()
                .is_some_and(|name| name == output.connector_name.as_str());
            (output.connector_name.clone(), offset_x, offset_y, is_main)
        })
        .collect();

    // Xwayland/XRandR output listing follows wl_output global creation order.
    // Advertise the compositor's canonical main output first so Xwayland marks
    // it primary, then keep the rest in layout order for stable monitor indices.
    ordered.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });

    ordered.into_iter().map(|(name, _, _, _)| name).collect()
}

pub(super) fn layout_size_for_outputs(
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> (i32, i32) {
    let active_viewports = effective_tty_viewports_for_outputs(tuning, outputs);

    if active_viewports.is_empty() {
        return (
            tuning.viewport_size.x.max(1.0).round() as i32,
            tuning.viewport_size.y.max(1.0).round() as i32,
        );
    }

    let min_x = active_viewports.iter().map(|v| v.offset_x).min().unwrap();
    let max_x = active_viewports
        .iter()
        .map(|v| v.offset_x + v.width as i32)
        .max()
        .unwrap();
    let min_y = active_viewports.iter().map(|v| v.offset_y).min().unwrap();
    let max_y = active_viewports
        .iter()
        .map(|v| v.offset_y + v.height as i32)
        .max()
        .unwrap();

    (max_x - min_x, max_y - min_y)
}

/// Returns `(width, height, offset_x, offset_y)` for the compositor's current
/// tty monitor when available, otherwise for the canonical live main output.
/// We use one real monitor's dimensions, not the full combined-layout size,
/// when calling libinput's `x_transformed` / `y_transformed` so that the
/// normalised [0,1] range maps to one monitor rather than being stretched
/// across all of them.
pub(super) fn primary_tty_monitor_dims(
    current_monitor: &str,
    tuning: &RuntimeTuning,
    outputs: &[TtyDrmOutput],
) -> (i32, i32, i32, i32) {
    let canonical_name = canonical_tty_main_output_name(outputs, tuning);
    let preferred_name = if outputs
        .iter()
        .any(|output| output.connector_name == current_monitor)
    {
        Some(current_monitor)
    } else {
        canonical_name.as_deref()
    };

    preferred_name
        .and_then(|name| effective_tty_viewport_for_output(tuning, outputs, name))
        .map(|viewport| {
            (
                viewport.width as i32,
                viewport.height as i32,
                viewport.offset_x,
                viewport.offset_y,
            )
        })
        .or_else(|| {
            outputs.iter().find_map(|output| {
                (output.connector_name == current_monitor).then(|| {
                    let (w, h) = output.mode.size();
                    (w as i32, h as i32, 0, 0)
                })
            })
        })
        .or_else(|| {
            canonical_tty_main_output_name(outputs, tuning).and_then(|name| {
                effective_tty_viewport_for_output(tuning, outputs, name.as_str()).map(|viewport| {
                    (
                        viewport.width as i32,
                        viewport.height as i32,
                        viewport.offset_x,
                        viewport.offset_y,
                    )
                })
            })
        })
        .unwrap_or((1920, 1080, 0, 0))
}
