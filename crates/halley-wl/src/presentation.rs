use halley_config::{NodeBackgroundColorMode, NodeBorderColorMode, RuntimeTuning};
use halley_core::field::{NodeId, Vec2};
use smithay::backend::renderer::Color32F;

use crate::animation::{ease_in_out_cubic, proxy_anim_scale};
use crate::compositor::interaction::state::CursorParallaxState;
use crate::compositor::monitor::camera::camera_controller;
use crate::compositor::root::Halley;

const CURSOR_PARALLAX_DEAD_ZONE: f32 = 0.01;
const CURSOR_PARALLAX_SETTLE_EPSILON: f32 = 0.01;

pub(crate) fn world_to_screen(st: &Halley, w: i32, h: i32, x: f32, y: f32) -> (i32, i32) {
    let view = camera_controller(st).view_size();
    let vw = view.x.max(1.0);
    let vh = view.y.max(1.0);

    let nx = ((x - st.model.viewport.center.x) / vw) + 0.5;
    let ny = ((y - st.model.viewport.center.y) / vh) + 0.5;

    let sx = (nx * w as f32).round() as i32;
    let sy = (ny * h as f32).round() as i32;
    (sx, sy)
}

pub(crate) fn cursor_parallax_position(st: &Halley, node_id: NodeId, pos: Vec2) -> Vec2 {
    if st
        .input
        .interaction_state
        .active_drag
        .as_ref()
        .is_some_and(|drag| drag.node_id == node_id)
    {
        return pos;
    }
    if st.model.field.node(node_id).is_some_and(|node| node.pinned) {
        return pos;
    }
    if !st.runtime.tuning.parallax.enabled || st.input.interaction_state.apogee_session.is_some() {
        return pos;
    }

    let offset =
        cursor_parallax_offset_for_monitor(st, st.model.monitor_state.current_monitor.as_str());
    if vec2_len_sq(offset) <= CURSOR_PARALLAX_SETTLE_EPSILON * CURSOR_PARALLAX_SETTLE_EPSILON {
        return pos;
    }
    Vec2 {
        x: pos.x - offset.x,
        y: pos.y - offset.y,
    }
}

pub(crate) fn cursor_parallax_offset_for_monitor(st: &Halley, monitor: &str) -> Vec2 {
    st.input
        .interaction_state
        .cursor_parallax
        .get(monitor)
        .map(|state| state.current)
        .unwrap_or(Vec2 { x: 0.0, y: 0.0 })
}

pub(crate) fn cursor_parallax_active_for_monitor(st: &Halley, monitor: &str) -> bool {
    st.input
        .interaction_state
        .cursor_parallax
        .get(monitor)
        .is_some_and(|state| {
            vec2_len_sq(state.current)
                > CURSOR_PARALLAX_SETTLE_EPSILON * CURSOR_PARALLAX_SETTLE_EPSILON
                || vec2_len_sq(state.target)
                    > CURSOR_PARALLAX_SETTLE_EPSILON * CURSOR_PARALLAX_SETTLE_EPSILON
        })
}

pub(crate) fn tick_cursor_parallax(st: &mut Halley, now: std::time::Instant) -> bool {
    let elapsed = now
        .saturating_duration_since(st.input.interaction_state.cursor_parallax_last_tick)
        .as_secs_f32()
        .clamp(0.0, 0.032);
    st.input.interaction_state.cursor_parallax_last_tick = now;
    let tau_secs = (st.runtime.tuning.parallax.tau_ms as f32 / 1000.0).max(0.001);
    let alpha = 1.0 - (-elapsed / tau_secs).exp();
    let pointer_monitor = st
        .input
        .interaction_state
        .last_pointer_screen_global
        .and_then(|(sx, sy)| st.monitor_for_screen(sx, sy));
    let monitors = st
        .model
        .monitor_state
        .monitors
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut changed = false;
    let mut remove = Vec::new();

    for monitor in monitors {
        let target = if st.runtime.tuning.parallax.enabled
            && st.input.interaction_state.apogee_session.is_none()
            && pointer_monitor.as_deref() == Some(monitor.as_str())
        {
            cursor_parallax_target_for_monitor(st, monitor.as_str())
        } else {
            Vec2 { x: 0.0, y: 0.0 }
        };
        let entry = st
            .input
            .interaction_state
            .cursor_parallax
            .entry(monitor.clone())
            .or_insert(CursorParallaxState {
                current: Vec2 { x: 0.0, y: 0.0 },
                target,
            });
        let before = entry.current;
        entry.target = target;
        entry.current = Vec2 {
            x: entry.current.x + (target.x - entry.current.x) * alpha,
            y: entry.current.y + (target.y - entry.current.y) * alpha,
        };
        if vec2_len_sq(entry.current)
            <= CURSOR_PARALLAX_SETTLE_EPSILON * CURSOR_PARALLAX_SETTLE_EPSILON
            && vec2_len_sq(entry.target)
                <= CURSOR_PARALLAX_SETTLE_EPSILON * CURSOR_PARALLAX_SETTLE_EPSILON
        {
            entry.current = Vec2 { x: 0.0, y: 0.0 };
            remove.push(monitor.clone());
        }
        if (entry.current.x - before.x).abs() > 0.001
            || (entry.current.y - before.y).abs() > 0.001
            || (entry.current.x - entry.target.x).abs() > CURSOR_PARALLAX_SETTLE_EPSILON
            || (entry.current.y - entry.target.y).abs() > CURSOR_PARALLAX_SETTLE_EPSILON
        {
            st.request_tty_redraw_for_monitor(monitor.as_str());
            changed = true;
        }
    }

    for monitor in remove {
        st.input.interaction_state.cursor_parallax.remove(&monitor);
    }
    if changed {
        st.request_maintenance();
    }
    changed
}

fn cursor_parallax_target_for_monitor(st: &Halley, monitor: &str) -> Vec2 {
    let Some(space) = st.model.monitor_state.monitors.get(monitor) else {
        return Vec2 { x: 0.0, y: 0.0 };
    };
    let (viewport_size, view_size) = if monitor == st.model.monitor_state.current_monitor {
        (st.model.viewport.size, st.model.zoom_ref_size)
    } else {
        (space.viewport.size, space.zoom_ref_size)
    };
    let scale = (viewport_size.x.max(1.0) / view_size.x.max(1.0)).max(0.01);
    let zoom_out = 1.0 - scale;
    if zoom_out <= CURSOR_PARALLAX_DEAD_ZONE {
        return Vec2 { x: 0.0, y: 0.0 };
    }
    let Some((sx, sy)) = st.input.interaction_state.last_pointer_screen_global else {
        return Vec2 { x: 0.0, y: 0.0 };
    };
    let (screen_w, screen_h, local_sx, local_sy) = st.local_screen_in_monitor(monitor, sx, sy);
    let nx = (local_sx / screen_w.max(1) as f32 - 0.5).clamp(-0.5, 0.5) * 2.0;
    let ny = (local_sy / screen_h.max(1) as f32 - 0.5).clamp(-0.5, 0.5) * 2.0;
    if nx.abs() <= 0.01 && ny.abs() <= 0.01 {
        return Vec2 { x: 0.0, y: 0.0 };
    }
    let strength = st.runtime.tuning.parallax.strength * (zoom_out / 0.5).clamp(0.0, 1.0);
    Vec2 {
        x: nx * view_size.x * strength,
        y: ny * view_size.y * strength,
    }
}

#[inline]
fn vec2_len_sq(v: Vec2) -> f32 {
    v.x * v.x + v.y * v.y
}

pub(crate) fn preview_proxy_size(_real_w: f32, _real_h: f32) -> (f32, f32) {
    (220.0, 220.0)
}

pub(crate) fn node_render_diameter_px(
    st: &Halley,
    intrinsic_size: Vec2,
    label_len: usize,
    anim_scale: f32,
) -> f32 {
    const PROXY_TO_MARKER_START: f32 = 0.50;
    const PROXY_TO_MARKER_END: f32 = 0.20;

    let marker_mix_lin = ((PROXY_TO_MARKER_START - anim_scale)
        / (PROXY_TO_MARKER_START - PROXY_TO_MARKER_END))
        .clamp(0.0, 1.0);
    let marker_mix = ease_in_out_cubic(marker_mix_lin);

    let (dot_half, _, _, _) = node_marker_metrics(st, label_len, anim_scale);
    let marker_diameter = ((dot_half as f32 * 1.5).round().max(1.0)) * 2.0;

    let (pw, ph) = preview_proxy_size(intrinsic_size.x, intrinsic_size.y);
    let proxy_diameter = pw.min(ph) * proxy_anim_scale(anim_scale);

    (proxy_diameter + (marker_diameter - proxy_diameter) * marker_mix).max(marker_diameter)
}

pub(crate) fn node_marker_metrics(
    _st: &Halley,
    label_len: usize,
    _anim_scale: f32,
) -> (i32, i32, i32, i32) {
    let dot_half = 17i32;
    let label_h = 26i32;
    let label_gap = 14i32;
    let label_w = ((label_len as f32) * 9.5).round().clamp(72.0, 420.0) as i32;
    (dot_half, label_gap, label_w, label_h)
}

pub(crate) fn node_marker_bounds(
    cx: i32,
    cy: i32,
    dot_half: i32,
    label_gap: i32,
    label_w: i32,
    label_h: i32,
    pad: i32,
) -> (i32, i32, i32, i32) {
    let pad = pad.max(0);
    let dot_d = (dot_half * 2).max(1);

    let content_w = (dot_d + label_gap.max(0) + label_w.max(0)).max(dot_d);
    let content_h = dot_d.max(label_h).max(1);

    let x0 = cx - dot_half - pad;
    let y0 = cy - (content_h / 2) - pad;
    let w = (content_w + pad * 2).max(8);
    let h = (content_h + pad * 2).max(8);

    (x0, y0, w, h)
}

fn window_active_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = tuning.decorations.border.color_focused;
    Color32F::new(color.r, color.g, color.b, 1.0)
}

fn window_inactive_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = tuning.decorations.border.color_unfocused;
    Color32F::new(color.r, color.g, color.b, 1.0)
}

fn window_secondary_active_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = if tuning.window_secondary_border_enabled() {
        tuning.decorations.secondary_border.color_focused
    } else {
        tuning.decorations.border.color_focused
    };
    Color32F::new(color.r, color.g, color.b, 1.0)
}

fn window_secondary_inactive_border_color_for_tuning(tuning: &RuntimeTuning) -> Color32F {
    let color = if tuning.window_secondary_border_enabled() {
        tuning.decorations.secondary_border.color_unfocused
    } else {
        tuning.decorations.border.color_unfocused
    };
    Color32F::new(color.r, color.g, color.b, 1.0)
}

pub(crate) fn themed_node_ring_color(
    tuning: &RuntimeTuning,
    hovered: bool,
    alpha: f32,
) -> Color32F {
    let mode = if hovered {
        tuning.node_border_color_hover
    } else {
        tuning.node_border_color_inactive
    };
    let base = match mode {
        NodeBorderColorMode::UseWindowActive => window_active_border_color_for_tuning(tuning),
        NodeBorderColorMode::UseWindowInactive => window_inactive_border_color_for_tuning(tuning),
        NodeBorderColorMode::UseWindowSecondaryActive => {
            window_secondary_active_border_color_for_tuning(tuning)
        }
        NodeBorderColorMode::UseWindowSecondaryInactive => {
            window_secondary_inactive_border_color_for_tuning(tuning)
        }
    };
    Color32F::new(base.r(), base.g(), base.b(), alpha)
}

pub(crate) fn themed_node_fill_color(tuning: &RuntimeTuning, hovered: bool) -> Color32F {
    match tuning.node_background_color {
        NodeBackgroundColorMode::Auto | NodeBackgroundColorMode::Theme => {
            let ring = themed_node_ring_color(tuning, hovered, 1.0);
            let base = (0.94, 0.96, 0.985);
            Color32F::new(
                base.0 * 0.86 + ring.r() * 0.14,
                base.1 * 0.86 + ring.g() * 0.14,
                base.2 * 0.86 + ring.b() * 0.14,
                1.0,
            )
        }
        NodeBackgroundColorMode::Light => Color32F::new(0.92, 0.95, 0.98, 1.0),
        NodeBackgroundColorMode::Dark => Color32F::new(0.15, 0.18, 0.22, 1.0),
        NodeBackgroundColorMode::Fixed { r, g, b } => Color32F::new(r, g, b, 1.0),
    }
}

pub(crate) fn themed_node_label_text_color(fill_color: Color32F, alpha: f32) -> Color32F {
    let luminance = fill_color.r() * 0.2126 + fill_color.g() * 0.7152 + fill_color.b() * 0.0722;
    if luminance < 0.45 {
        Color32F::new(0.96, 0.98, 1.0, alpha)
    } else {
        Color32F::new(0.08, 0.10, 0.12, alpha)
    }
}

pub(crate) fn themed_node_label_fill_color(
    tuning: &RuntimeTuning,
    hovered: bool,
    alpha: f32,
) -> Color32F {
    let fill = themed_node_fill_color(tuning, hovered);
    Color32F::new(fill.r(), fill.g(), fill.b(), alpha)
}

pub(crate) fn themed_node_label_colors(
    tuning: &RuntimeTuning,
    hovered: bool,
    fill_alpha: f32,
    text_alpha: f32,
) -> (Color32F, Color32F) {
    let fill = themed_node_label_fill_color(tuning, hovered, fill_alpha);
    let text = themed_node_label_text_color(fill, text_alpha);
    (fill, text)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use halley_config::{DecorationBorderColor, NodeBorderColorMode, RuntimeTuning};
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;

    use super::{cursor_parallax_position, themed_node_ring_color, tick_cursor_parallax};
    use crate::compositor::interaction::DragAxisMode;
    use crate::compositor::interaction::state::ActiveDragState;
    use crate::compositor::overview::{ApogeePhase, ApogeeSession};
    use crate::compositor::root::Halley;

    #[test]
    fn cursor_parallax_is_zoom_out_gated_and_ignores_drag_delta() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, RuntimeTuning::default());
        let dragged = st.model.field.spawn_surface(
            "dragged",
            Vec2 { x: 100.0, y: 0.0 },
            Vec2 { x: 400.0, y: 260.0 },
        );
        let background = st.model.field.spawn_surface(
            "background",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 400.0, y: 260.0 },
        );
        st.input.interaction_state.active_drag = Some(ActiveDragState {
            node_id: dragged,
            parallax_origin: Vec2 { x: 0.0, y: 0.0 },
            allow_monitor_transfer: true,
            edge_pan_eligible: false,
            current_offset: Vec2 { x: 0.0, y: 0.0 },
            pointer_monitor: st.model.monitor_state.current_monitor.clone(),
            pointer_workspace_size: (1600, 1200),
            pointer_screen_local: (200.0, 120.0),
            edge_pan_x: DragAxisMode::Free,
            edge_pan_y: DragAxisMode::Free,
            last_edge_pan_at: Instant::now(),
        });
        let monitor = st
            .model
            .monitor_state
            .monitors
            .get(st.model.monitor_state.current_monitor.as_str())
            .expect("test monitor");
        st.input.interaction_state.last_pointer_screen_global = Some((
            monitor.offset_x as f32 + monitor.width as f32 * 0.75,
            monitor.offset_y as f32 + monitor.height as f32 * 0.5,
        ));

        let pos = Vec2 { x: 20.0, y: 10.0 };
        assert_eq!(cursor_parallax_position(&st, background, pos), pos);

        st.model.zoom_ref_size.x = st.model.viewport.size.x * 2.0;
        st.model.zoom_ref_size.y = st.model.viewport.size.y * 2.0;
        assert_eq!(cursor_parallax_position(&st, background, pos), pos);

        let tick_at = st
            .input
            .interaction_state
            .cursor_parallax_last_tick
            .checked_add(Duration::from_millis(16))
            .unwrap();
        assert!(tick_cursor_parallax(&mut st, tick_at));
        let shifted = cursor_parallax_position(&st, background, pos);

        assert!(shifted.x < pos.x);
        assert_eq!(shifted.y, pos.y);
        assert_eq!(cursor_parallax_position(&st, dragged, pos), pos);

        st.model.field.node_mut(dragged).unwrap().pos.x = 900.0;
        assert_eq!(cursor_parallax_position(&st, background, pos), shifted);

        st.input.interaction_state.apogee_session = Some(ApogeeSession {
            monitor: st.model.monitor_state.current_monitor.clone(),
            phase: ApogeePhase::Open,
            started_at: Instant::now(),
            duration: Duration::from_millis(320),
            core_scroll_offset: 0.0,
            core_atlas_width: 0.0,
            tiles: Vec::new(),
            core_tiles: Vec::new(),
        });
        assert_eq!(cursor_parallax_position(&st, background, pos), pos);
    }

    #[test]
    fn themed_node_ring_color_uses_secondary_border_when_enabled() {
        let mut tuning = RuntimeTuning::default();
        tuning.decorations.secondary_border.enabled = true;
        tuning.decorations.secondary_border.color_focused = DecorationBorderColor {
            r: 0.9,
            g: 0.8,
            b: 0.1,
        };
        tuning.node_border_color_hover = NodeBorderColorMode::UseWindowSecondaryActive;

        let color = themed_node_ring_color(&tuning, true, 0.75);

        assert_eq!(
            (color.r(), color.g(), color.b(), color.a()),
            (0.9, 0.8, 0.1, 0.75)
        );
    }

    #[test]
    fn themed_node_ring_color_falls_back_to_primary_when_secondary_disabled() {
        let mut tuning = RuntimeTuning::default();
        tuning.decorations.border.color_unfocused = DecorationBorderColor {
            r: 0.2,
            g: 0.3,
            b: 0.4,
        };
        tuning.node_border_color_inactive = NodeBorderColorMode::UseWindowSecondaryInactive;

        let color = themed_node_ring_color(&tuning, false, 0.6);

        assert_eq!(
            (color.r(), color.g(), color.b(), color.a()),
            (0.2, 0.3, 0.4, 0.6)
        );
    }
}
