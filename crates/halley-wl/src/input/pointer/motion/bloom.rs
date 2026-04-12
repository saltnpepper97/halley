use std::time::{Duration, Instant};

use crate::backend::interface::BackendView;
use crate::compositor::interaction::{BloomDragCtx, HitNode, PointerState};
use crate::compositor::root::Halley;
use super::super::button::ButtonFrame;
use super::drag::begin_drag;

pub(super) fn detach_bloom_drag_into_pointer_drag(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &impl BackendView,
    bloom_drag: BloomDragCtx,
    effective_sx: f32,
    effective_sy: f32,
) {
    let (bloom_w, bloom_h, bloom_local_sx, bloom_local_sy) =
        st.local_screen_in_monitor(bloom_drag.monitor.as_str(), effective_sx, effective_sy);
    let previous_monitor = st.begin_temporary_render_monitor(bloom_drag.monitor.as_str());
    let pointer_world =
        crate::spatial::screen_to_world(st, bloom_w, bloom_h, bloom_local_sx, bloom_local_sy);
    st.end_temporary_render_monitor(previous_monitor);
    let now = Instant::now();
    if st.detach_member_from_cluster(
        bloom_drag.cluster_id,
        bloom_drag.member_id,
        pointer_world,
        now,
    ) {
        st.assign_node_to_monitor(bloom_drag.member_id, bloom_drag.monitor.as_str());
        st.set_interaction_focus(Some(bloom_drag.member_id), 30_000, now);
        begin_drag(
            st,
            ps,
            backend,
            HitNode {
                node_id: bloom_drag.member_id,
                on_titlebar: false,
                is_core: false,
            },
            ButtonFrame {
                ws_w: bloom_w,
                ws_h: bloom_h,
                global_sx: effective_sx,
                global_sy: effective_sy,
                sx: bloom_local_sx,
                sy: bloom_local_sy,
                world_now: pointer_world,
                workspace_active: false,
            },
            pointer_world,
            false,
            false,
        );
    }
}

pub(super) fn handle_bloom_pull_motion(
    st: &mut Halley,
    ps: &mut PointerState,
    backend: &impl BackendView,
    effective_sx: f32,
    effective_sy: f32,
    now: Instant,
) -> bool {
    let Some(bloom_drag) = ps.bloom_drag.clone() else {
        return false;
    };

    let (_, _, bloom_local_sx, bloom_local_sy) =
        st.local_screen_in_monitor(bloom_drag.monitor.as_str(), effective_sx, effective_sy);
    let pointer_screen = halley_core::field::Vec2 {
        x: bloom_local_sx,
        y: bloom_local_sy,
    };
    let slot_screen = halley_core::field::Vec2 {
        x: bloom_drag.slot_screen.0,
        y: bloom_drag.slot_screen.1,
    };
    let raw_offset = halley_core::field::Vec2 {
        x: pointer_screen.x - slot_screen.x,
        y: pointer_screen.y - slot_screen.y,
    };
    let pull_dist = raw_offset.x.hypot(raw_offset.y);
    let slop_px = crate::compositor::interaction::state::bloom_pull_slop_px();
    let display_offset =
        crate::compositor::interaction::state::bloom_pull_constrained_offset(raw_offset);
    let now_ms = st.now_ms(now);
    let mut should_detach = false;
    if let Some(preview) = st.input.interaction_state.bloom_pull_preview.as_mut() {
        let outward_axis = halley_core::field::Vec2 {
            x: preview.slot_screen.x - preview.core_screen.x,
            y: preview.slot_screen.y - preview.core_screen.y,
        };
        let outward_len = outward_axis.x.hypot(outward_axis.y);
        let outward_pull = if outward_len > 0.001 {
            (raw_offset.x * (outward_axis.x / outward_len)
                + raw_offset.y * (outward_axis.y / outward_len))
                .max(0.0)
        } else {
            pull_dist
        };
        preview.pointer_screen = pointer_screen;
        preview.display_offset = display_offset;
        match preview.phase.clone() {
            crate::compositor::interaction::state::BloomPullPhase::Pressed => {
                preview.hold_progress = 0.0;
                if outward_pull >= slop_px {
                    preview.phase =
                        crate::compositor::interaction::state::BloomPullPhase::Tethered {
                            started_at_ms: now_ms,
                        };
                }
            }
            crate::compositor::interaction::state::BloomPullPhase::Tethered {
                started_at_ms,
            } => {
                if outward_pull < slop_px * 0.75 {
                    preview.phase =
                        crate::compositor::interaction::state::BloomPullPhase::Pressed;
                    preview.hold_progress = 0.0;
                } else {
                    preview.hold_progress = (now_ms.saturating_sub(started_at_ms) as f32
                        / crate::compositor::interaction::state::bloom_detach_hold_ms().max(1)
                            as f32)
                        .clamp(0.0, 1.0);
                    should_detach = preview.hold_progress >= 1.0;
                }
            }
            crate::compositor::interaction::state::BloomPullPhase::Snapback { .. } => {
                preview.phase = crate::compositor::interaction::state::BloomPullPhase::Pressed;
                preview.hold_progress = 0.0;
            }
        }
    }
    st.input.interaction_state.overlay_hover_target = None;
    ps.hover_node = None;
    ps.hover_started_at = None;
    if should_detach {
        ps.bloom_drag = None;
        st.input.interaction_state.bloom_pull_preview = None;
        ps.preview_block_until = Some(now + Duration::from_millis(500));
        detach_bloom_drag_into_pointer_drag(
            st,
            ps,
            backend,
            bloom_drag,
            effective_sx,
            effective_sy,
        );
        backend.request_redraw();
    } else {
        st.request_maintenance();
    }
    true
}
