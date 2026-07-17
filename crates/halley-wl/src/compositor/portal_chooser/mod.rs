//! The xdg-desktop-portal source chooser.
//!
//! When an app (Discord, a browser, OBS) asks the portal to share a screen or
//! window, the portal backend asks the compositor to open this overlay. It is a
//! Halley-styled picker: a bottom bar offering "Screen" and/or "Window"
//! (filtered by what the calling app accepts). Picking Window enters a
//! click-to-pick phase; picking Screen selects the focused monitor. The portal
//! backend polls the compositor until the user confirms or cancels.

use std::time::Instant;

use halley_api::{
    PORTAL_SOURCE_TYPE_MONITOR, PORTAL_SOURCE_TYPE_WINDOW, PortalOutput, PortalScreenCastResponse,
    PortalSourceSelection, PortalWindowSource,
};
use halley_core::field::NodeId;

use crate::compositor::root::Halley;
use crate::compositor::surface::current_surface_size_for_node;

#[derive(Clone, Debug)]
pub(crate) struct PortalChooserState {
    pub session_handle: String,
    pub allow_monitor: bool,
    pub allow_window: bool,
    pub phase: PortalChooserPhase,
    pub menu_selected: usize,
    pub menu_hovered: Option<usize>,
    pub monitor: String,
    pub hovered_monitor: Option<String>,
    pub hovered_window: Option<NodeId>,
    pub result: Option<PortalSourceSelection>,
    pub cancelled: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PortalChooserEntry {
    pub is_window: bool,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortalChooserPhase {
    /// Bottom bar with Screen/Window entries.
    Menu,
    /// Click a monitor to pick it.
    ScreenPick,
    /// Click a window to pick it.
    WindowPick,
}

pub(crate) fn portal_chooser_active(st: &Halley) -> bool {
    st.input
        .interaction_state
        .portal_chooser
        .as_ref()
        .is_some_and(|session| session.result.is_none() && !session.cancelled)
}

/// Entries shown in the menu bar. The layout is always Screen + Window so apps
/// that request only one source type still show the unavailable type disabled.
pub(crate) fn portal_chooser_entries(st: &Halley) -> [PortalChooserEntry; 2] {
    let session = match &st.input.interaction_state.portal_chooser {
        Some(s) => s,
        None => {
            return [
                PortalChooserEntry {
                    is_window: false,
                    enabled: false,
                },
                PortalChooserEntry {
                    is_window: true,
                    enabled: false,
                },
            ];
        }
    };
    [
        PortalChooserEntry {
            is_window: false,
            enabled: session.allow_monitor,
        },
        PortalChooserEntry {
            is_window: true,
            enabled: session.allow_window,
        },
    ]
}

pub(crate) fn start_portal_chooser(
    st: &mut Halley,
    session_handle: &str,
    source_types: u32,
    _now: Instant,
) -> bool {
    if st.input.interaction_state.portal_chooser.is_some() {
        return false;
    }
    let allow_monitor = source_types & PORTAL_SOURCE_TYPE_MONITOR != 0;
    let allow_window = source_types & PORTAL_SOURCE_TYPE_WINDOW != 0;
    // If the app refuses both, default to monitor-only so the picker still opens
    // rather than presenting an empty bar.
    if !allow_monitor && !allow_window {
        return false;
    }
    let monitor = chooser_monitor(st);
    st.begin_modal_keyboard_capture();
    // No open-time Enter pre-trap: `begin_modal_keyboard_capture` clears keyboard
    // focus through the `set_keyboard_focus` choke point, whose
    // `flush_stuck_forwarded_keys` already forwards a synthetic release for the
    // held Enter to the app that invoked the portal (e.g. OBS) before focus drops
    // to None. The chooser arms Enter/Escape traps at genuine confirm/cancel
    // presses, and `finish_modal_capture` clears any leftover trap on teardown so
    // none can outlive the chooser and strand a later key in client-side repeat.
    let menu_selected = if allow_monitor { 0 } else { 1 };
    let phase = match (allow_monitor, allow_window) {
        (true, false) => PortalChooserPhase::ScreenPick,
        (false, true) => PortalChooserPhase::WindowPick,
        _ => PortalChooserPhase::Menu,
    };
    let hovered_monitor = matches!(phase, PortalChooserPhase::ScreenPick).then(|| monitor.clone());
    st.input.interaction_state.portal_chooser = Some(PortalChooserState {
        session_handle: session_handle.to_string(),
        allow_monitor,
        allow_window,
        phase,
        menu_selected,
        menu_hovered: matches!(phase, PortalChooserPhase::Menu).then_some(menu_selected),
        monitor,
        hovered_monitor,
        hovered_window: None,
        result: None,
        cancelled: false,
    });
    st.request_maintenance();
    true
}

pub(crate) fn cancel_portal_chooser(st: &mut Halley) -> bool {
    let Some(session) = st.input.interaction_state.portal_chooser.take() else {
        return false;
    };
    finish_modal_capture(st, &session);
    st.runtime.screenshot_full_repaint_until_ms = st.now_ms(Instant::now()).saturating_add(120);
    st.request_maintenance();
    true
}

pub(crate) fn cancel_portal_chooser_for_handle(st: &mut Halley, session_handle: &str) -> bool {
    if st
        .input
        .interaction_state
        .portal_chooser
        .as_ref()
        .is_some_and(|s| s.session_handle == session_handle)
    {
        cancel_portal_chooser(st)
    } else {
        false
    }
}

pub(crate) fn move_portal_chooser_selection(st: &mut Halley, delta: i32) -> bool {
    let entries = portal_chooser_entries(st);
    let Some(session) = st.input.interaction_state.portal_chooser.as_mut() else {
        return false;
    };
    if session.phase != PortalChooserPhase::Menu {
        return false;
    }
    let mut next = session.menu_selected;
    for _ in 0..entries.len() {
        next = (next as i32 + delta).rem_euclid(entries.len() as i32) as usize;
        if entries[next].enabled {
            break;
        }
    }
    if !entries[next].enabled {
        return false;
    }
    session.menu_selected = next;
    session.menu_hovered = Some(next);
    st.request_maintenance();
    true
}

pub(crate) fn hover_portal_chooser_item(st: &mut Halley, index: Option<usize>) {
    let entries = portal_chooser_entries(st);
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut()
        && session.phase == PortalChooserPhase::Menu
    {
        session.menu_hovered = index;
        if let Some(index) = index.filter(|index| entries[*index].enabled) {
            session.menu_selected = index;
        }
        st.request_maintenance();
    }
}

pub(crate) fn return_portal_chooser_to_menu(st: &mut Halley) -> bool {
    let Some(session) = st.input.interaction_state.portal_chooser.as_mut() else {
        return false;
    };
    if session.phase == PortalChooserPhase::Menu {
        return false;
    }
    session.phase = PortalChooserPhase::Menu;
    session.hovered_monitor = None;
    session.hovered_window = None;
    st.request_maintenance();
    true
}

/// Confirm the current menu entry. In WindowPick this is a no-op (a click is
/// required there). Returns true if the chooser resolved (closed).
pub(crate) fn activate_portal_chooser(st: &mut Halley, _now: Instant) -> bool {
    let entry = match &st.input.interaction_state.portal_chooser {
        Some(session) if session.phase == PortalChooserPhase::Menu => {
            let entries = portal_chooser_entries(st);
            match entries.get(session.menu_selected).copied() {
                Some(entry) if entry.enabled => entry,
                None => return false,
                _ => return false,
            }
        }
        _ => return false,
    };
    if entry.is_window {
        if let Some(session) = st.input.interaction_state.portal_chooser.as_mut() {
            session.phase = PortalChooserPhase::WindowPick;
            session.hovered_window = None;
        }
        st.request_maintenance();
        return false;
    }
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut() {
        session.phase = PortalChooserPhase::ScreenPick;
        session.hovered_monitor = Some(session.monitor.clone());
    }
    st.request_maintenance();
    false
}

pub(crate) fn confirm_portal_screen(st: &mut Halley, now: Instant) -> bool {
    let monitor = match &st.input.interaction_state.portal_chooser {
        Some(session) if session.phase == PortalChooserPhase::ScreenPick => session
            .hovered_monitor
            .clone()
            .unwrap_or_else(|| session.monitor.clone()),
        _ => return false,
    };
    let selection = monitor_selection(st, &monitor);
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut() {
        session.result = Some(selection);
    }
    finish_after_selection(st, now)
}

pub(crate) fn pick_portal_screen(st: &mut Halley, monitor: &str, now: Instant) -> bool {
    let Some(session) = st.input.interaction_state.portal_chooser.as_mut() else {
        return false;
    };
    if session.phase != PortalChooserPhase::ScreenPick {
        return false;
    }
    session.hovered_monitor = Some(monitor.to_string());
    confirm_portal_screen(st, now)
}

/// In WindowPick phase, pick the window under the given screen point.
pub(crate) fn pick_portal_window_at(
    st: &mut Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    sx: f32,
    sy: f32,
    now: Instant,
) -> bool {
    let owns = st
        .input
        .interaction_state
        .portal_chooser
        .as_ref()
        .is_some_and(|s| s.phase == PortalChooserPhase::WindowPick);
    if !owns {
        return false;
    }
    let previous_monitor = st.begin_temporary_render_monitor(monitor);
    let hit = crate::spatial::pick_hit_node_at(st, screen_w, screen_h, sx, sy, now, None);
    st.end_temporary_render_monitor(previous_monitor);
    let Some(node_id) = hit.map(|h| h.node_id) else {
        return false;
    };
    let selection = window_selection(st, node_id, monitor);
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut() {
        session.result = Some(selection);
    }
    finish_after_selection(st, now)
}

pub(crate) fn update_portal_chooser_monitor(st: &mut Halley, monitor: String) {
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut() {
        session.monitor = monitor;
    }
}

pub(crate) fn hover_portal_chooser_monitor(st: &mut Halley, monitor: String) {
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut()
        && session.phase == PortalChooserPhase::ScreenPick
    {
        session.hovered_monitor = Some(monitor);
        st.request_maintenance();
    }
}

pub(crate) fn hover_portal_chooser_window(st: &mut Halley, node_id: Option<NodeId>) {
    if let Some(session) = st.input.interaction_state.portal_chooser.as_mut()
        && session.phase == PortalChooserPhase::WindowPick
    {
        session.hovered_window = node_id;
        st.request_maintenance();
    }
}

/// Read the chooser result for a session. Returns a terminal response if done,
/// else None (still pending).
pub(crate) fn poll_portal_chooser(
    st: &mut Halley,
    session_handle: &str,
) -> PortalScreenCastResponse {
    let snapshot = match &st.input.interaction_state.portal_chooser {
        Some(session) if session.session_handle == session_handle => {
            let result = session.result.clone();
            let cancelled = session.cancelled;
            Some((result, cancelled))
        }
        Some(_) => None,
        None => None,
    };
    let Some((result, cancelled)) = snapshot else {
        // No chooser for this handle (or wrong handle): treat as cancelled so
        // the portal backend stops polling.
        return PortalScreenCastResponse::SourceChooserCancelled;
    };
    if let Some(selection) = result {
        let session = st
            .input
            .interaction_state
            .portal_chooser
            .take()
            .expect("chooser present");
        finish_modal_capture(st, &session);
        st.runtime.screenshot_full_repaint_until_ms = st.now_ms(Instant::now()).saturating_add(120);
        st.request_maintenance();
        PortalScreenCastResponse::SourceChooserSelected(selection)
    } else if cancelled {
        let session = st
            .input
            .interaction_state
            .portal_chooser
            .take()
            .expect("chooser present");
        finish_modal_capture(st, &session);
        st.request_maintenance();
        PortalScreenCastResponse::SourceChooserCancelled
    } else {
        PortalScreenCastResponse::SourceChooserPending
    }
}

/// Hide the chooser after confirmation while keeping the result available for
/// the portal backend's next `PollSourceChooser` call.
fn finish_after_selection(st: &mut Halley, now: Instant) -> bool {
    st.runtime.screenshot_full_repaint_until_ms = st.now_ms(now).saturating_add(120);
    st.request_maintenance();
    true
}

fn finish_modal_capture(st: &mut Halley, _session: &PortalChooserState) {
    // Drop any modal release-traps the chooser armed (Enter/Escape on
    // confirm/cancel). They are only meaningful while the chooser owns the
    // keyboard; a leftover trap diverts the next unrelated key release into
    // `flush_trapped_modal_release` and strands the client in client-side key
    // repeat. `forwarded_pressed_keys` is already drained by the focus-clear flush
    // at open, but clear it too as belt-and-suspenders.
    st.input.interaction_state.modal_release_keys.clear();
    st.input.interaction_state.forwarded_pressed_keys.clear();

    let restore_focus = st
        .last_input_surface_node_for_monitor(st.model.monitor_state.current_monitor.as_str())
        .or(st.last_input_surface_node());
    st.schedule_modal_focus_restore_after(restore_focus, Instant::now(), 260);
}

/// Resolve which monitor the chooser opens on, mirroring the screenshot
/// picker: explicit pointer monitor → interaction monitor → current monitor.
fn chooser_monitor(st: &Halley) -> String {
    if let Some((sx, sy)) = st.input.interaction_state.cursor.last_screen_global
        && let Some(monitor) = st.monitor_for_screen(sx, sy)
    {
        return monitor.to_string();
    }
    let interaction = st.interaction_monitor().to_string();
    if st
        .model
        .monitor_state
        .monitors
        .contains_key(interaction.as_str())
    {
        interaction
    } else {
        st.model.monitor_state.current_monitor.clone()
    }
}

fn monitor_selection(st: &Halley, monitor: &str) -> PortalSourceSelection {
    let (width, height, offset_x, offset_y, focused) = st
        .model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| {
            let mode_size = st
                .model
                .monitor_state
                .outputs
                .get(monitor)
                .and_then(|output| output.current_mode())
                .map(|mode| (mode.size.w, mode.size.h));
            let (w, h) = mode_size.unwrap_or((space.width, space.height));
            (
                w,
                h,
                space.offset_x,
                space.offset_y,
                monitor == st.focused_monitor(),
            )
        })
        .unwrap_or((1920, 1080, 0, 0, false));
    PortalSourceSelection::Monitor(PortalOutput {
        name: monitor.to_string(),
        width,
        height,
        offset_x,
        offset_y,
        focused,
    })
}

fn window_selection(st: &Halley, node_id: NodeId, monitor: &str) -> PortalSourceSelection {
    let (title, app_id, width, height) = st
        .model
        .field
        .node(node_id)
        .map(|node| {
            let size = current_surface_size_for_node(st, node_id).unwrap_or(node.intrinsic_size);
            (
                node.label.clone(),
                st.model.node_app_ids.get(&node_id).cloned(),
                size.x as i32,
                size.y as i32,
            )
        })
        .unwrap_or_else(|| (format!("window {}", node_id.as_u64()), None, 1, 1));
    PortalSourceSelection::Window(PortalWindowSource {
        node_id: node_id.as_u64(),
        title,
        app_id,
        output: monitor.to_string(),
        width: width.max(1),
        height: height.max(1),
    })
}
