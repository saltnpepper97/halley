use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, SERIAL_COUNTER};
use smithay::wayland::seat::WaylandFocus;

use super::{Session, SessionDriver};
use crate::wayland::WaylandState;

#[derive(Default)]
struct UnmapParents(std::sync::Mutex<Option<Vec<WlSurface>>>);

struct FocusSuccession {
    output: Option<String>,
    cluster: Option<halley_core::cluster::ClusterId>,
    preferred: Option<WlSurface>,
    parents: Vec<WlSurface>,
    pan: halley_config::CloseRestorePan,
    clipboard_return: Option<crate::wayland::clipboard_helper::SavedFocus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseSuccessorAction {
    FocusWindow,
    FocusNode,
    RestoreNode,
}

fn close_successor_action(collapsed: bool, restore_nodes: bool) -> CloseSuccessorAction {
    match (collapsed, restore_nodes) {
        (false, _) => CloseSuccessorAction::FocusWindow,
        (true, false) => CloseSuccessorAction::FocusNode,
        (true, true) => CloseSuccessorAction::RestoreNode,
    }
}

fn close_handoff_needs_fallback(active_before_cleanup: bool) -> bool {
    !active_before_cleanup
}

pub(crate) struct WindowUnmapPreparation {
    surface: WlSurface,
    focus: Option<FocusSuccession>,
}

impl WindowUnmapPreparation {
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }
}

fn mapped_managed_window(wayland: &WaylandState, surface: &WlSurface) -> Option<Window> {
    if !wayland.managed_windows.contains(surface) {
        return None;
    }
    wayland
        .space
        .elements()
        .find(|window| {
            !crate::xwayland::is_override_redirect(window)
                && window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == surface)
        })
        .cloned()
}

fn remember_presentation_close_size<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
) {
    let Some(window) = mapped_managed_window(&session.wayland, surface) else {
        return;
    };
    let Some(app_id) = crate::window::recovery::independent_toplevel_app_id(&window) else {
        return;
    };

    let restore = session
        .fullscreen
        .restore_placement(surface)
        .map(|(geometry, output)| (geometry.size, output))
        .or_else(|| {
            session
                .maximize
                .restore(surface)
                .map(|restore| (restore.geometry.size, Some(restore.output)))
        });

    if let Some((restore, restore_output)) = restore {
        let output_name = restore_output.or_else(|| crate::wayland::window_output_name(&window));
        let output_size = output_name
            .as_deref()
            .and_then(|name| {
                session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == name)
            })
            .and_then(|output| session.wayland.space.output_geometry(output))
            .map(|geometry| geometry.size);
        let size = crate::window::recovery::presentation_close_recovery_size(restore, output_size);
        eventline::debug!(
            "window-size recovery: remembered app_id={app_id:?} restore={}x{} next={}x{}",
            restore.w,
            restore.h,
            size.w,
            size.h
        );
        session
            .presentation_close_size_recovery
            .remember(app_id, size);
    } else {
        // A normal close lets a client update its own remembered size, so a
        // previous presentation recovery is no longer needed.
        session.presentation_close_size_recovery.clear(&app_id);
    }
}

fn select_focus_successor(
    wayland: &WaylandState,
    nodes: &crate::nodes::NodesState,
    closing: &WlSurface,
    closing_output: Option<&str>,
) -> Option<WlSurface> {
    select_ordered_successor(
        wayland.managed_windows.top_to_bottom().cloned(),
        closing,
        closing_output,
        |surface| {
            nodes
                .id_for_surface(surface)
                .and_then(|id| nodes.record(id))
                .filter(|record| record.attached)
                .map(|record| Some(record.output.clone()))
        },
        |surface| {
            nodes
                .id_for_surface(surface)
                .and_then(|id| nodes.last_focus_ms().get(&id))
                .copied()
                .unwrap_or(0)
        },
    )
}

/// Capture the declared family before teardown removes the child or clears
/// its protocol parent. Bound traversal because X11 clients can form cycles.
fn closing_parent_chain<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Vec<WlSurface> {
    let Some(mut window) = mapped_managed_window(&session.wayland, surface) else {
        return Vec::new();
    };
    let mut parents = Vec::new();
    // Collapsed parents leave Space but retain their declared relationships.
    // Follow them through the registry without restoring or focusing them.
    let windows = || {
        session
            .wayland
            .space
            .elements()
            .chain(session.nodes.records().map(|record| &record.window))
    };
    for _ in 0..windows().count() {
        let Some(parent) = crate::window::stacking::parent_window_from(windows(), &window) else {
            break;
        };
        let Some(parent_surface) = parent.wl_surface().map(std::borrow::Cow::into_owned) else {
            break;
        };
        if &parent_surface == surface || parents.contains(&parent_surface) {
            break;
        }
        parents.push(parent_surface);
        window = parent;
    }
    parents
}

/// The wl_surface hook installed before the XDG role runs before Smithay
/// resets that role on a null-buffer commit. Keep this one-commit snapshot
/// separate from the protocol parent, which must still clear on unmap.
pub(crate) fn capture_unmap_parents<D: SessionDriver>(session: &Session<D>, surface: &WlSurface) {
    if !mapped_managed_window(&session.wayland, surface)
        .is_some_and(|window| window.toplevel().is_some())
    {
        return;
    }
    let parents = closing_parent_chain(session, surface);
    smithay::wayland::compositor::with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing_threadsafe(UnmapParents::default);
        *states
            .data_map
            .get::<UnmapParents>()
            .unwrap()
            .0
            .lock()
            .unwrap() = Some(parents);
    });
}

/// Dialog dismissal returns to its nearest surviving, focusable parent rather
/// than an unrelated MRU window. Revalidate after unmap; a parent may close,
/// collapse, or leave the active workspace in the same dispatch batch.
fn parent_focus_successor<D: SessionDriver>(
    session: &Session<D>,
    parents: &[WlSurface],
) -> Option<WlSurface> {
    parents.iter().find_map(|surface| {
        let record = session
            .nodes
            .id_for_surface(surface)
            .and_then(|id| session.nodes.record(id))?;
        (surface.alive()
            && record.attached
            && !record.collapsed
            && session.wayland.managed_windows.contains(surface)
            && session
                .wayland
                .space
                .elements()
                .any(|window| window == &record.window)
            && crate::window::accepts_wm_focus(&record.window)
            && crate::presentation::surface_workspace_is_active(
                &session.clusters,
                &session.nodes,
                surface,
                &record.output,
                crate::frame_clock::monotonic_now(),
            ))
        .then(|| surface.clone())
    })
}

fn active_cluster_for_closing<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<halley_core::cluster::ClusterId> {
    let member = session.nodes.id_for_surface(surface)?;
    let cluster = session.clusters.cluster_for_member(member)?;
    let output = &session.clusters.metadata(cluster)?.output;
    (session.clusters.active_on(output) == Some(cluster)).then_some(cluster)
}

fn cluster_focus_successor<D: SessionDriver>(
    session: &Session<D>,
    closing: &WlSurface,
) -> Option<WlSurface> {
    let closing = session.nodes.id_for_surface(closing)?;
    let cluster = session.clusters.cluster_for_member(closing)?;
    let metadata = session.clusters.metadata(cluster)?;
    if session.clusters.active_on(&metadata.output) != Some(cluster) {
        return None;
    }

    let members = session.clusters.member_ids(cluster);
    let available = |member| {
        let Some(record) = session.nodes.record(member) else {
            return false;
        };
        record.attached
            && !record.collapsed
            && session
                .wayland
                .space
                .elements()
                .any(|window| window == &record.window)
    };
    let successor = match metadata.layout {
        halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking => {
            select_stacking_successor(members, closing, available)
        }
        halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling => {
            select_tiling_successor(&members, closing, available)
        }
    }?;
    session
        .nodes
        .record(successor)
        .map(|record| record.surface.clone())
}

fn select_stacking_successor<T: Copy + Eq>(
    members: impl IntoIterator<Item = T>,
    closing: T,
    mut available: impl FnMut(T) -> bool,
) -> Option<T> {
    members
        .into_iter()
        .find(|member| *member != closing && available(*member))
}

fn select_tiling_successor<T: Copy + Eq>(
    members: &[T],
    closing: T,
    mut available: impl FnMut(T) -> bool,
) -> Option<T> {
    let closing_index = members.iter().position(|member| *member == closing)?;
    members[closing_index + 1..]
        .iter()
        .copied()
        .chain(members[..closing_index].iter().rev().copied())
        .find(|member| available(*member))
}

fn select_ordered_successor<T>(
    candidates: impl IntoIterator<Item = T>,
    closing: &T,
    closing_output: Option<&str>,
    mut mapped_output: impl FnMut(&T) -> Option<Option<String>>,
    mut last_focus_ms: impl FnMut(&T) -> u64,
) -> Option<T>
where
    T: Clone + Eq,
{
    let mut global: Option<(T, u64)> = None;
    let mut local: Option<(T, u64)> = None;
    for candidate in candidates {
        if &candidate == closing {
            continue;
        }
        let Some(output) = mapped_output(&candidate) else {
            continue;
        };
        let recency = last_focus_ms(&candidate);
        if global
            .as_ref()
            .is_none_or(|(_, current)| recency > *current)
        {
            global = Some((candidate.clone(), recency));
        }
        if closing_output.is_some_and(|closing_output| output.as_deref() == Some(closing_output))
            && local.as_ref().is_none_or(|(_, current)| recency > *current)
        {
            local = Some((candidate, recency));
        }
    }
    local.or(global).map(|(candidate, _)| candidate)
}

pub(crate) fn prepare_window_unmap<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
) -> WindowUnmapPreparation {
    let captured_parents = smithay::wayland::compositor::with_states(surface, |states| {
        states
            .data_map
            .get::<UnmapParents>()
            .and_then(|parents| parents.0.lock().unwrap().take())
    });
    if let Some(id) = session.nodes.id_for_surface(surface) {
        session.wayland.foreign_toplevel_state.unmap(id.as_u64());
    }
    super::touch::cancel_surface(session, surface);
    super::gesture::cancel_surface(session, surface);
    super::pointer::prepare_unmap(session, surface);
    remember_presentation_close_size(session, surface);
    let focus = (session.wayland.focused_window.as_ref() == Some(surface)).then(|| {
        let output = mapped_managed_window(&session.wayland, surface)
            .and_then(|window| crate::wayland::window_output_name(&window));
        let cluster = active_cluster_for_closing(session, surface);
        let parents = captured_parents.unwrap_or_else(|| closing_parent_chain(session, surface));
        let preferred = session
            .settings
            .field
            .close_restore_focus
            .then(|| {
                if cluster.is_some() {
                    cluster_focus_successor(session, surface)
                } else {
                    parent_focus_successor(session, &parents).or_else(|| {
                        select_focus_successor(
                            &session.wayland,
                            &session.nodes,
                            surface,
                            output.as_deref(),
                        )
                    })
                }
            })
            .flatten();
        FocusSuccession {
            output,
            cluster,
            preferred,
            parents,
            pan: session.settings.field.close_restore_pan,
            clipboard_return: crate::wayland::clipboard_helper::saved_focus(surface),
        }
    });
    WindowUnmapPreparation {
        surface: surface.clone(),
        focus,
    }
}

pub(crate) fn finish_window_unmap<D: SessionDriver>(
    session: &mut Session<D>,
    preparation: WindowUnmapPreparation,
) {
    let WindowUnmapPreparation { surface, focus } = preparation;
    session
        .interactions
        .field_arrange
        .invalidate_surface(&surface);
    session.wayland.managed_windows.remove(&surface);
    session
        .presentation_close_size_recovery
        .forget_surface(&surface);
    session.opening_origins.forget(&surface);
    session.window_animations.remove(&surface);
    session.fullscreen.remove(&surface);
    if session.maximize.remove(&surface)
        && let Some(output) = focus.as_ref().and_then(|focus| focus.output.as_deref())
    {
        let _ = session.cameras.apply_field_maximize(output, None);
    }
    session.render.fullscreen_textures.remove(&surface);
    session.render.arrange_textures.remove(&surface);
    super::cancel_grab_for_surface(session, &surface);
    crate::input::grab::forget_resize_anchor(&mut session.interactions.resize_anchor, &surface);
    if close_handoff_needs_fallback(session.render.window_close_animations.is_active(&surface)) {
        super::closing::start(session, &surface);
    }

    let Some(focus) = focus else {
        return;
    };
    if session
        .wayland
        .focused_window
        .as_ref()
        .is_some_and(|focused| focused != &surface)
    {
        return;
    }

    if let Some(saved) = focus.clipboard_return {
        // This is the end of a borrowed clipboard serial, not an application
        // close. Restore its exact caller without cluster succession or camera
        // motion, independently of the user's ordinary close-focus policy.
        let window = saved.window.as_ref().and_then(|surface| {
            session
                .wayland
                .space
                .elements()
                .find(|window| {
                    window
                        .wl_surface()
                        .is_some_and(|candidate| candidate.as_ref() == surface)
                        && crate::wayland::window_output_name(window).is_some_and(|output| {
                            crate::presentation::surface_workspace_is_active(
                                &session.clusters,
                                &session.nodes,
                                surface,
                                &output,
                                crate::frame_clock::monotonic_now(),
                            )
                        })
                })
                .cloned()
        });
        let serial = SERIAL_COUNTER.next_serial();
        if let Some(window) = window {
            super::focus_window_after_close(session, &window, serial);
        } else {
            crate::window::clear_focus(&mut session.wayland);
        }
        if saved.layer.is_some() {
            super::focus::focus_layer(session, saved.layer, serial);
        } else {
            super::sync_keyboard_focus(session, serial);
        }
        return;
    }

    // A workspace keeps focus ownership even after its final client closes.
    // Never let an empty cluster fall through to Field close succession: those
    // windows are hidden and must not receive the next close shortcut.
    let active_cluster = focus.cluster.filter(|id| {
        session
            .clusters
            .metadata(*id)
            .is_some_and(|metadata| session.clusters.active_on(&metadata.output) == Some(*id))
    });
    if let Some(cluster) = active_cluster
        && cluster_focus_successor(session, &surface).is_none()
    {
        crate::window::clear_focus(&mut session.wayland);
        session.nodes.focus(
            session.clusters.core_node(cluster),
            session.start_time.elapsed().as_millis() as u64,
        );
        super::sync_keyboard_focus(session, SERIAL_COUNTER.next_serial());
        session.request_redraw();
        return;
    }

    if !session.settings.field.close_restore_focus {
        crate::window::clear_focus(&mut session.wayland);
        session
            .nodes
            .focus(None, session.start_time.elapsed().as_millis() as u64);
        return;
    }

    let revalidated = if active_cluster.is_some() {
        cluster_focus_successor(session, &surface)
    } else {
        parent_focus_successor(session, &focus.parents).or_else(|| {
            select_focus_successor(
                &session.wayland,
                &session.nodes,
                &surface,
                focus.output.as_deref(),
            )
        })
    };
    if revalidated != focus.preferred {
        eventline::debug!("focus: successor changed while window teardown completed");
    }
    let successor = revalidated
        .as_ref()
        .and_then(|surface| session.nodes.id_for_surface(surface));
    if let Some(id) = successor {
        let collapsed = session
            .nodes
            .record(id)
            .is_some_and(|record| record.collapsed);
        let serial = SERIAL_COUNTER.next_serial();
        match close_successor_action(collapsed, session.settings.field.close_restore_nodes) {
            CloseSuccessorAction::FocusWindow => {
                if let Some(window) = session.nodes.record(id).map(|record| record.window.clone()) {
                    super::focus_window_after_close(session, &window, serial);
                }
                crate::nodes::pan_after_close_restore(session, id, focus.pan);
            }
            CloseSuccessorAction::FocusNode => {
                crate::window::clear_focus(&mut session.wayland);
                session
                    .nodes
                    .focus(Some(id), session.start_time.elapsed().as_millis() as u64);
                super::sync_keyboard_focus(session, serial);
                session.request_redraw();
            }
            CloseSuccessorAction::RestoreNode => {
                let _ = crate::nodes::restore_for_close(session, id, serial);
                crate::nodes::pan_after_close_restore(session, id, focus.pan);
            }
        }
    } else {
        crate::window::clear_focus(&mut session.wayland);
        session
            .nodes
            .focus(None, session.start_time.elapsed().as_millis() as u64);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        CloseSuccessorAction, close_handoff_needs_fallback, close_successor_action,
        select_ordered_successor, select_stacking_successor, select_tiling_successor,
    };

    #[test]
    fn active_close_handoff_is_not_restarted_after_unmap() {
        assert!(!close_handoff_needs_fallback(true));
    }

    #[test]
    fn missing_close_handoff_keeps_the_existing_unmap_fallback() {
        assert!(close_handoff_needs_fallback(false));
    }

    fn output_lookup(
        outputs: &HashMap<&'static str, Option<&'static str>>,
        candidate: &&'static str,
    ) -> Option<Option<String>> {
        outputs
            .get(candidate)
            .map(|output| output.map(str::to_owned))
    }

    #[test]
    fn focus_successor_prefers_managed_stack_entry_on_closing_output() {
        let outputs = HashMap::from([
            ("closing", Some("DP-1")),
            ("global-top", Some("DP-2")),
            ("same-output", Some("DP-1")),
        ]);

        assert_eq!(
            select_ordered_successor(
                ["closing", "global-top", "same-output"],
                &"closing",
                Some("DP-1"),
                |candidate| output_lookup(&outputs, candidate),
                |candidate| match *candidate {
                    "global-top" => 200,
                    "same-output" => 100,
                    _ => 0,
                },
            ),
            Some("same-output")
        );
    }

    #[test]
    fn focus_successor_falls_back_to_most_recent_managed_window() {
        let outputs = HashMap::from([
            ("closing", Some("DP-1")),
            ("global-top", Some("DP-2")),
            ("global-bottom", Some("DP-3")),
        ]);

        assert_eq!(
            select_ordered_successor(
                ["closing", "global-top", "global-bottom"],
                &"closing",
                Some("DP-1"),
                |candidate| output_lookup(&outputs, candidate),
                |candidate| match *candidate {
                    "global-bottom" => 200,
                    "global-top" => 100,
                    _ => 0,
                },
            ),
            Some("global-bottom")
        );
    }

    #[test]
    fn focus_successor_skips_entries_that_are_no_longer_mapped() {
        let outputs = HashMap::from([
            ("closing", Some("DP-1")),
            ("stale", None),
            ("remaining", Some("DP-1")),
        ]);

        assert_eq!(
            select_ordered_successor(
                ["closing", "stale", "remaining"],
                &"closing",
                Some("DP-1"),
                |candidate| {
                    outputs
                        .get(candidate)
                        .and_then(|output| output.map(|output| Some(output.to_owned())))
                },
                |_| 0,
            ),
            Some("remaining")
        );
    }

    #[test]
    fn focus_successor_is_none_after_the_last_managed_window_closes() {
        assert_eq!(
            select_ordered_successor(
                ["closing"],
                &"closing",
                Some("DP-1"),
                |_| Some(Some("DP-1".to_owned())),
                |_| 0
            ),
            None
        );
    }

    #[test]
    fn stacking_successor_is_the_first_remaining_available_card() {
        assert_eq!(
            select_stacking_successor([10, 20, 30, 40], 10, |member| member != 20),
            Some(30)
        );
        assert_eq!(
            select_stacking_successor([10, 20, 30, 40], 30, |_| true),
            Some(10)
        );
    }

    #[test]
    fn tiling_successor_promotes_the_next_member_when_master_closes() {
        assert_eq!(
            select_tiling_successor(&[10, 20, 30, 40], 10, |_| true),
            Some(20)
        );
    }

    #[test]
    fn tiling_successor_fills_the_closed_stack_slot() {
        assert_eq!(
            select_tiling_successor(&[10, 20, 30, 40], 20, |_| true),
            Some(30)
        );
    }

    #[test]
    fn tiling_successor_uses_the_preceding_member_at_the_end() {
        assert_eq!(
            select_tiling_successor(&[10, 20, 30, 40], 40, |_| true),
            Some(30)
        );
    }

    #[test]
    fn tiling_successor_skips_unavailable_members_in_layout_order() {
        assert_eq!(
            select_tiling_successor(&[10, 20, 30, 40], 20, |member| member != 30),
            Some(40)
        );
        assert_eq!(
            select_tiling_successor(&[10, 20, 30, 40], 40, |member| member != 30),
            Some(20)
        );
    }

    #[test]
    fn collapsed_successor_stays_a_node_by_default() {
        assert_eq!(
            close_successor_action(true, false),
            CloseSuccessorAction::FocusNode
        );
    }

    #[test]
    fn collapsed_successor_restores_when_enabled() {
        assert_eq!(
            close_successor_action(true, true),
            CloseSuccessorAction::RestoreNode
        );
    }

    #[test]
    fn active_successor_focus_is_independent_of_node_restore_policy() {
        assert_eq!(
            close_successor_action(false, false),
            CloseSuccessorAction::FocusWindow
        );
        assert_eq!(
            close_successor_action(false, true),
            CloseSuccessorAction::FocusWindow
        );
    }
}
