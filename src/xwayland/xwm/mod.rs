use std::cmp::Reverse;
use std::os::fd::OwnedFd;
use std::sync::Mutex;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::SelectionTarget;
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XwmHandler};

use crate::session::{Session, SessionDriver};
use crate::wayland::fullscreen::{ExternalConfigureResult, ExternalTransactionRequest};

use super::lifecycle::{
    MapAdmission, OpeningPlacement, map_admission, opening_placement_is_authoritative,
};
use super::{
    OverrideRedirectPlacement, PendingOverrideRedirect, PendingWindow, SavedNormalSize,
    WindowIdentity,
};
mod configure;
mod lifecycle;
mod override_redirect;
mod policy;
mod presentation;
mod state;

use override_redirect::*;
use policy::*;
pub(super) use presentation::restore_maximized_window;
use presentation::*;
pub(super) use presentation::{
    configure_window, reconfigure_fullscreen, set_window_fullscreen, sync_position, sync_positions,
};
pub(super) use state::{ManagedStateRegistry, WindowState};

#[derive(Clone, Debug)]
struct MaximizeRestore {
    geometry: Rectangle<i32, Logical>,
    output: Option<String>,
}

#[derive(Default, Debug)]
struct MaximizeFullscreenState {
    active: bool,
    restore: Option<MaximizeRestore>,
}

#[derive(Default)]
struct MaximizeFullscreen(Mutex<MaximizeFullscreenState>);

fn window_for_surface(
    wayland: &crate::wayland::WaylandState,
    nodes: &crate::nodes::NodesState,
    surface: &X11Surface,
) -> Option<Window> {
    wayland
        .space
        .elements()
        .find(|window| {
            window
                .x11_surface()
                .is_some_and(|candidate| same_surface(candidate, surface))
        })
        .cloned()
        .or_else(|| {
            nodes
                .records()
                .find(|record| {
                    record
                        .window
                        .x11_surface()
                        .is_some_and(|candidate| same_surface(candidate, surface))
                })
                .map(|record| record.window.clone())
        })
}

fn same_surface(left: &X11Surface, right: &X11Surface) -> bool {
    // Smithay's PartialEq intentionally stops matching after destruction.
    // Clones still share this allocation, including during teardown and XID reuse.
    std::ptr::eq(left.user_data(), right.user_data())
}

fn window_identity(surface: &X11Surface) -> Option<WindowIdentity> {
    if surface.is_override_redirect()
        || surface.is_transient_for().is_some()
        || !matches!(surface.window_type(), None | Some(WmWindowType::Normal))
    {
        return None;
    }
    let instance = surface.instance();
    let class = surface.class();
    if instance.is_empty() || class.is_empty() {
        return None;
    }
    Some(WindowIdentity {
        instance,
        class,
        title: surface.title(),
    })
}

fn is_steam_main_window(surface: &X11Surface) -> bool {
    window_identity(surface).is_some_and(|identity| {
        identity.instance == "steamwebhelper"
            && identity.class.eq_ignore_ascii_case("steam")
            && identity.title == "Steam"
    })
}

fn maximize_restore(surface: &X11Surface) -> Option<MaximizeRestore> {
    surface
        .user_data()
        .get::<MaximizeFullscreen>()
        .and_then(|state| {
            let state = state.0.lock().expect("X11 maximize state lock poisoned");
            state.active.then(|| state.restore.clone()).flatten()
        })
}

fn maximize_origin_active(surface: &X11Surface) -> bool {
    surface
        .user_data()
        .get::<MaximizeFullscreen>()
        .is_some_and(|state| {
            state
                .0
                .lock()
                .expect("X11 maximize state lock poisoned")
                .active
        })
}

fn remember_normal_size<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    size: Size<i32, Logical>,
) {
    if size.w <= 0
        || size.h <= 0
        || surface.is_fullscreen()
        || surface.is_maximized()
        || maximize_origin_active(surface)
    {
        return;
    }
    if is_steam_main_window(surface)
        && window_for_surface(&session.wayland, &session.nodes, surface)
            .and_then(|window| crate::wayland::window_output_name(&window))
            .and_then(|name| output_named(&session.wayland, &name))
            .and_then(|output| session.wayland.space.output_geometry(&output))
            .is_some_and(|geometry| size_fills_output(size, geometry.size))
    {
        return;
    }
    let Some(identity) = window_identity(surface) else {
        return;
    };
    session
        .xwayland
        .normal_sizes
        .insert(identity, SavedNormalSize(size));
}

fn cached_normal_size<D: SessionDriver>(
    session: &Session<D>,
    surface: &X11Surface,
) -> Option<Size<i32, Logical>> {
    let identity = window_identity(surface)?;
    session
        .xwayland
        .normal_sizes
        .get(&identity)
        .map(|saved| saved.0)
}

fn size_fills_output(size: Size<i32, Logical>, output_size: Size<i32, Logical>) -> bool {
    size.w >= output_size.w && size.h >= output_size.h
}

fn presentation_configures_initial_geometry(saved_maximize: bool, fullscreen: bool) -> bool {
    saved_maximize || fullscreen
}

fn steam_recovery_size(
    surface: &X11Surface,
    output_size: Size<i32, Logical>,
) -> Size<i32, Logical> {
    recovery_window_size(output_size, surface.min_size(), surface.max_size())
}

fn recovery_window_size(
    output_size: Size<i32, Logical>,
    minimum: Option<Size<i32, Logical>>,
    maximum: Option<Size<i32, Logical>>,
) -> Size<i32, Logical> {
    let mut size = Size::from((
        output_size.w.saturating_mul(3) / 4,
        output_size.h.saturating_mul(3) / 4,
    ));
    if let Some(minimum) = minimum {
        size.w = size.w.max(minimum.w);
        size.h = size.h.max(minimum.h);
    }
    if let Some(maximum) = maximum {
        size.w = size.w.min(maximum.w);
        size.h = size.h.min(maximum.h);
    }
    size.w = size.w.clamp(1, output_size.w.max(1));
    size.h = size.h.clamp(1, output_size.h.max(1));
    size
}

pub(super) fn surface_associated<D: SessionDriver>(
    session: &mut Session<D>,
    wl_surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    surface: X11Surface,
) {
    consider_map(session, surface.window_id(), Some(&wl_surface));
}

pub(super) fn handle_commit<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    let xid = session
        .xwayland
        .pending_windows
        .iter()
        .find_map(|(xid, pending)| {
            pending
                .surface
                .wl_surface()
                .is_some_and(|candidate| candidate == *surface)
                .then_some(*xid)
        });
    if let Some(xid) = xid {
        if let Some(x11) = session
            .xwayland
            .pending_windows
            .get(&xid)
            .map(|pending| pending.surface.clone())
        {
            let buffer = with_renderer_surface_state(surface, |state| {
                (state.buffer().is_some(), state.surface_size())
            })
            .unwrap_or((false, None));
            crate::session::trace::x11_event(
                session,
                &x11,
                "surface-commit",
                format_args!(
                    "wl_surface={:?} has_buffer={} buffer_size={:?} drawable={:?}",
                    surface,
                    buffer.0,
                    buffer.1,
                    x11.geometry(),
                ),
            );
        }
        consider_map(session, xid, Some(surface));
    }
}

fn consider_map<D: SessionDriver>(
    session: &mut Session<D>,
    xid: u32,
    associated: Option<&smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>,
) {
    let pending = session.xwayland.pending_windows.contains_key(&xid);
    let surface = session
        .xwayland
        .pending_windows
        .get(&xid)
        .and_then(|pending| pending.surface.wl_surface());
    let associated = associated.or(surface.as_ref());
    let has_buffer = associated.is_some_and(|surface| {
        with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
    });
    match map_admission(pending, associated.is_some(), has_buffer) {
        MapAdmission::Wait => {
            eventline::debug!(
                "xwayland: mapping pending xid={xid} associated={} buffer={has_buffer}",
                associated.is_some()
            );
        }
        MapAdmission::Ignore => {}
        MapAdmission::Admit => admit_window(session, xid),
    }
}

fn admit_window<D: SessionDriver>(session: &mut Session<D>, xid: u32) {
    let Some(PendingWindow {
        surface,
        window,
        initial_size,
    }) = session.xwayland.pending_windows.remove(&xid)
    else {
        return;
    };
    let startup_cluster = surface
        .pid()
        .and_then(|pid| session.startup_cluster_for_x11_pid(pid));
    let saved_maximize = maximize_restore(&surface);
    let initially_iconic = session.xwayland.managed_states.state(xid) == WindowState::Iconic;
    let mut opening_size = OpeningPlacement::preferred_size(initial_size, window.geometry().size);
    let rule = session.window_rules.track_window(&window);
    if saved_maximize.is_none()
        && let Some((width, height)) = rule.initial_size
    {
        opening_size = Size::from((
            i32::try_from(width).unwrap_or(i32::MAX).max(96),
            i32::try_from(height).unwrap_or(i32::MAX).max(72),
        ));
    }
    let initial_outer_size = crate::titlebar::outer_size_for_client(
        &window,
        opening_size,
        &session.settings.decorations,
        &session.settings.font,
    );
    let initial_placement = crate::window::routing::initial_window_placement(
        crate::window::routing::InitialWindowPlacementRequest {
            wayland: &session.wayland,
            cameras: &session.cameras,
            primary_output: session.driver.primary_output(),
            window: Some(&window),
            window_size: initial_outer_size,
            placement: rule.spawn_placement,
            cursor_position: smithay::utils::Point::from(session.pointer.position()),
            gap: session.settings.field.gap,
        },
    );
    let mut output = initial_placement.output;
    let mut location = crate::titlebar::client_location_for_outer(
        &window,
        initial_placement.location,
        &session.settings.decorations,
        &session.settings.font,
    );

    if let Some(restore) = saved_maximize.as_ref() {
        opening_size = restore.geometry.size;
        if let Some(saved_output) = restore
            .output
            .as_deref()
            .and_then(|name| output_named(&session.wayland, name))
        {
            output = saved_output;
            location = restore.geometry.loc;
        } else {
            let outer_size = crate::titlebar::outer_size_for_client(
                &window,
                opening_size,
                &session.settings.decorations,
                &session.settings.font,
            );
            let placement = crate::window::routing::initial_window_placement(
                crate::window::routing::InitialWindowPlacementRequest {
                    wayland: &session.wayland,
                    cameras: &session.cameras,
                    primary_output: session.driver.primary_output(),
                    window: Some(&window),
                    window_size: outer_size,
                    placement: halley_config::WindowSpawnPlacement::Default,
                    cursor_position: smithay::utils::Point::from(session.pointer.position()),
                    gap: session.settings.field.gap,
                },
            );
            output = placement.output;
            location = crate::titlebar::client_location_for_outer(
                &window,
                placement.location,
                &session.settings.decorations,
                &session.settings.font,
            );
        }
    } else if is_steam_main_window(&surface) {
        let output_size = session
            .wayland
            .space
            .output_geometry(&output)
            .map(|geometry| geometry.size)
            .unwrap_or(opening_size);
        let suspicious = surface.is_fullscreen()
            || surface.is_maximized()
            || size_fills_output(opening_size, output_size);
        let recovered = cached_normal_size(session, &surface)
            .filter(|size| !size_fills_output(*size, output_size))
            .or_else(|| suspicious.then(|| steam_recovery_size(&surface, output_size)));
        if let Some(recovered) = recovered {
            opening_size = Size::from((
                recovered.w.clamp(1, output_size.w.max(1)),
                recovered.h.clamp(1, output_size.h.max(1)),
            ));
            let outer_size = crate::titlebar::outer_size_for_client(
                &window,
                opening_size,
                &session.settings.decorations,
                &session.settings.font,
            );
            let placement = crate::window::routing::initial_window_placement(
                crate::window::routing::InitialWindowPlacementRequest {
                    wayland: &session.wayland,
                    cameras: &session.cameras,
                    primary_output: session.driver.primary_output(),
                    window: Some(&window),
                    window_size: outer_size,
                    placement: rule.spawn_placement,
                    cursor_position: smithay::utils::Point::from(session.pointer.position()),
                    gap: session.settings.field.gap,
                },
            );
            output = placement.output;
            location = crate::titlebar::client_location_for_outer(
                &window,
                placement.location,
                &session.settings.decorations,
                &session.settings.font,
            );
            if let Err(err) = surface.set_fullscreen(false) {
                eventline::warn!("xwayland: failed to clear stale Steam fullscreen state: {err}");
            }
            if let Err(err) = surface.set_maximized(false) {
                eventline::warn!("xwayland: failed to clear stale Steam maximized state: {err}");
            }
            eventline::debug!(
                "xwayland: recovered Steam main window xid={xid} to {}x{}",
                opening_size.w,
                opening_size.h
            );
        }
    }

    if let Some(cluster) = startup_cluster
        && let Some(output_name) = session
            .clusters
            .metadata(cluster)
            .map(|metadata| metadata.output.clone())
        && let Some(target_output) = output_named(&session.wayland, &output_name)
    {
        output = target_output;
        if let Some(geometry) = session.wayland.space.output_geometry(&output) {
            location = geometry.loc;
        }
    }

    let constrained_size = configure::constrain_surface_size(&surface, opening_size);
    if constrained_size != opening_size && saved_maximize.is_none() {
        location.x = location
            .x
            .saturating_add(opening_size.w.saturating_sub(constrained_size.w) / 2);
        location.y = location
            .y
            .saturating_add(opening_size.h.saturating_sub(constrained_size.h) / 2);
    }
    opening_size = constrained_size;
    let opening_geometry = Rectangle::new(location, opening_size);
    let opening_x_geometry = session
        .wayland
        .space
        .output_geometry(&output)
        .and_then(|output_geometry| {
            presentation::opening_target_bounds(session, &output, opening_geometry, false).map(
                |target| {
                    Rectangle::new(
                        output_geometry.loc + target.loc.to_logical(1),
                        opening_geometry.size,
                    )
                },
            )
        })
        .unwrap_or(opening_geometry);
    let presentation_configures =
        presentation_configures_initial_geometry(saved_maximize.is_some(), surface.is_fullscreen());
    crate::session::trace::x11_event(
        session,
        &surface,
        "admission-configure",
        format_args!(
            "initial_size={initial_size:?} source={opening_geometry:?} x_root={opening_x_geometry:?} output={:?} saved_maximize={} presentation_configures={presentation_configures} rule_cluster={:?}",
            output.name(),
            saved_maximize.is_some(),
            rule.cluster_participation,
        ),
    );
    // Fullscreen/maximize owns its initial configure. Sending the centered
    // windowed configure first lets games draw one intermediate centered frame
    // before the fullscreen configure is processed.
    if !presentation_configures && let Err(err) = surface.configure(opening_x_geometry) {
        eventline::warn!("xwayland: failed to prepare opening geometry: {err}");
    }
    crate::wayland::set_window_output(&window, &output);
    session.xwayland.sync_frame_extents(
        &window,
        &session.settings.decorations,
        &session.settings.font,
    );
    if let Some(wl_surface) = window.wl_surface() {
        crate::session::opening::prepare(session, wl_surface.into_owned(), &output);
    }
    session
        .wayland
        .space
        .map_element(window.clone(), location, true);
    if let Some(wl_surface) = window.wl_surface() {
        session.nodes.register_mapped(
            &session.wayland.space,
            wl_surface.as_ref(),
            session.start_time.elapsed().as_millis() as u64,
        );
        if let Some(id) = session.nodes.id_for_surface(wl_surface.as_ref()) {
            let admitted_to_draft = crate::session::admit_cluster_draft_window(session, id);
            let work_area = smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
            let now = crate::frame_clock::monotonic_now();
            let admitted_to_startup = !admitted_to_draft
                && startup_cluster.is_some_and(|cluster| {
                    session.clusters.admit_attributed_window(
                        &mut session.nodes.field,
                        cluster,
                        id,
                        work_area,
                        now,
                    )
                });
            let admitted_to_active = !admitted_to_draft
                && !admitted_to_startup
                && session.clusters.admit_mapped_window(
                    &mut session.nodes.field,
                    &output.name(),
                    id,
                    rule.cluster_participation,
                    work_area,
                    now,
                );
            if admitted_to_startup || admitted_to_active {
                session.request_redraw();
            }
            if !admitted_to_draft && !admitted_to_startup {
                crate::nodes::displace_landmarks_for_new_window(session, id);
            }
            crate::session::trace::x11_event(
                session,
                &surface,
                "admitted",
                format_args!(
                    "node={id:?} cluster={:?} layout={:?} space_geometry={:?}",
                    session.clusters.cluster_for_member(id),
                    session.clusters.active_layout_for_member(id),
                    session.wayland.space.element_geometry(&window),
                ),
            );
        }
        crate::session::closing::mapped(session, wl_surface.as_ref());
    }
    session.xwayland.opening_placements.insert(
        xid,
        OpeningPlacement::new(
            opening_geometry,
            saved_maximize
                .as_ref()
                .map_or(initial_size, |restore| restore.geometry.size),
        ),
    );
    let cluster_managed = window
        .wl_surface()
        .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
        .is_some_and(|id| session.clusters.cluster_for_member(id).is_some());
    if cluster_managed {
        let quiet_until = session
            .xwayland
            .arm_client_geometry_guard(xid, crate::frame_clock::monotonic_now());
        if let Some(id) = window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
        {
            session.clusters.defer_surface_layout_until(id, quiet_until);
        }
    }
    if saved_maximize.is_some() {
        if let Err(err) = surface.set_maximized(true) {
            eventline::warn!("xwayland: failed to retain remapped maximized state: {err}");
        }
        enter_fullscreen(session, &surface, FullscreenRequestOrigin::Maximize);
    } else if surface.is_fullscreen() {
        enter_fullscreen(session, &surface, FullscreenRequestOrigin::Initial);
    } else if surface.is_maximized() {
        // Startup maximize hints are not user decoration clicks. Keeping the
        // centered opening prevents monitor-sized remaps from becoming the
        // client's next remembered "normal" size (notably Steam).
        if let Err(err) = surface.set_maximized(false) {
            eventline::warn!("xwayland: failed to suppress initial maximized state: {err}");
        }
    }
    // Establish the final presentation target before starting the independent
    // window-open timeline. This mirrors Hyprland's target-then-animate model:
    // fullscreen owns X11 geometry, while opening owns only visual motion.
    let started = !initially_iconic
        && window.wl_surface().is_some_and(|wl_surface| {
            crate::session::opening::start(
                session,
                wl_surface.into_owned(),
                &output,
                crate::frame_clock::monotonic_now(),
            )
        });
    if initially_iconic {
        let collapsed = window
            .wl_surface()
            .and_then(|surface| session.nodes.id_for_surface(surface.as_ref()))
            .is_some_and(|id| crate::nodes::collapse(session, id, SERIAL_COUNTER.next_serial()));
        if !collapsed {
            session
                .xwayland
                .transition_surface(&surface, WindowState::Normal);
            crate::session::focus_window(session, &window, SERIAL_COUNTER.next_serial());
        }
    } else if !surface.is_override_redirect() {
        crate::session::focus_window(session, &window, SERIAL_COUNTER.next_serial());
    }
    refresh_override_redirect_owners(session);
    eventline::debug!(
        "xwayland: mapping admitted xid={xid} fullscreen={} maximized={} animated={started}",
        surface.is_fullscreen(),
        surface.is_maximized()
    );
    session.request_redraw();
}

fn output_for_geometry<D: SessionDriver>(
    session: &Session<D>,
    geometry: Rectangle<i32, Logical>,
) -> Option<Output> {
    let center = Point::<f64, Logical>::from((
        f64::from(geometry.loc.x) + f64::from(geometry.size.w) / 2.0,
        f64::from(geometry.loc.y) + f64::from(geometry.size.h) / 2.0,
    ));
    session
        .wayland
        .space
        .output_under(center)
        .next()
        .cloned()
        .or_else(|| crate::wayland::focus::selected_output(&session.wayland).cloned())
}

pub(super) fn constrain_window_size(
    window: &Window,
    requested: Size<i32, Logical>,
) -> Size<i32, Logical> {
    window
        .x11_surface()
        .map(|surface| configure::constrain_surface_size(surface, requested))
        .unwrap_or(requested)
}

#[cfg(test)]
mod tests {
    use super::presentation::{
        PositionResync, owner_relative_source_point, position_resync_needed,
    };
    use super::{
        FullscreenRequestOrigin, OverrideRedirectIdentity, OverrideRedirectMapAdmission,
        OverrideRedirectStackAction, compositor_fullscreen_should_raise,
        fullscreen_grab_proxy_geometry_matches, override_redirect_map_admission,
        override_redirect_owner_rank, override_redirect_stack_action,
        presentation_configures_initial_geometry, recovery_window_size, size_fills_output,
    };
    use smithay::utils::{Logical, Point, Rectangle, Size};

    fn resync(x: (i32, i32), root_screen: (i32, i32)) -> PositionResync {
        PositionResync {
            override_redirect: false,
            owns_own_geometry: false,
            x_location: Point::from(x),
            root_screen_location: Point::from(root_screen),
        }
    }

    #[test]
    fn a_settled_position_sends_no_configure() {
        assert!(!position_resync_needed(resync((100, 200), (100, 200))));
        assert!(position_resync_needed(resync((1546, -214), (2655, 181))));
    }

    #[test]
    fn geometry_owners_are_never_resynced_underneath() {
        for resync in [
            PositionResync {
                override_redirect: true,
                ..resync((0, 0), (2660, 200))
            },
            PositionResync {
                owns_own_geometry: true,
                ..resync((0, 0), (2660, 200))
            },
        ] {
            assert!(!position_resync_needed(resync));
        }
    }

    #[test]
    fn only_compositor_fullscreen_entry_promotes_the_x11_window() {
        assert!(compositor_fullscreen_should_raise(
            true,
            FullscreenRequestOrigin::Compositor
        ));
        assert!(!compositor_fullscreen_should_raise(
            false,
            FullscreenRequestOrigin::Compositor
        ));
        for origin in [
            FullscreenRequestOrigin::Initial,
            FullscreenRequestOrigin::Client,
            FullscreenRequestOrigin::Maximize,
        ] {
            assert!(!compositor_fullscreen_should_raise(true, origin));
        }
    }

    #[test]
    fn fullscreen_presentation_owns_the_only_initial_x11_configure() {
        assert!(presentation_configures_initial_geometry(false, true));
        assert!(presentation_configures_initial_geometry(true, false));
        assert!(!presentation_configures_initial_geometry(false, false));
    }

    #[test]
    fn first_override_redirect_map_is_never_delayed() {
        assert_eq!(
            override_redirect_map_admission(true, false),
            OverrideRedirectMapAdmission::ImmediateFresh
        );
    }

    #[test]
    fn reused_override_redirect_waits_for_new_placement() {
        assert_eq!(
            override_redirect_map_admission(false, false),
            OverrideRedirectMapAdmission::AwaitConfigure
        );
    }

    #[test]
    fn preconfigured_override_redirect_remap_is_immediate() {
        assert_eq!(
            override_redirect_map_admission(false, true),
            OverrideRedirectMapAdmission::ImmediateConfigured
        );
    }

    #[test]
    fn override_redirect_stacks_immediately_above_a_mapped_sibling() {
        assert_eq!(
            override_redirect_stack_action(Some(41), true),
            OverrideRedirectStackAction::Above(41)
        );
    }

    #[test]
    fn override_redirect_without_an_above_sibling_is_at_the_bottom() {
        assert_eq!(
            override_redirect_stack_action(None, false),
            OverrideRedirectStackAction::Bottom
        );
    }

    #[test]
    fn override_redirect_preserves_order_when_the_x_sibling_is_unmapped() {
        assert_eq!(
            override_redirect_stack_action(Some(41), false),
            OverrideRedirectStackAction::PreserveMissing(41)
        );
    }

    #[test]
    fn steam_group_owner_prefers_matching_window_containing_popup() {
        let center = Point::<i32, Logical>::from((1230, 540));
        let owner = override_redirect_owner_rank(
            OverrideRedirectIdentity {
                pid: Some(77),
                class: "steam",
                instance: "steamwebhelper",
            },
            OverrideRedirectIdentity {
                pid: Some(77),
                class: "steam",
                instance: "steamwebhelper",
            },
            center,
            Rectangle::new((1090, 425).into(), (1920, 1200).into()),
            2,
        );
        let other_group_window = override_redirect_owner_rank(
            OverrideRedirectIdentity {
                pid: Some(77),
                class: "steam",
                instance: "steamwebhelper",
            },
            OverrideRedirectIdentity {
                pid: Some(77),
                class: "steam",
                instance: "steamwebhelper",
            },
            center,
            Rectangle::new((3300, 200).into(), (800, 600).into()),
            3,
        );
        let unrelated_overlap = override_redirect_owner_rank(
            OverrideRedirectIdentity {
                pid: Some(77),
                class: "steam",
                instance: "steamwebhelper",
            },
            OverrideRedirectIdentity {
                pid: Some(88),
                class: "other",
                instance: "other",
            },
            center,
            Rectangle::new((1100, 450).into(), (500, 500).into()),
            4,
        );

        assert!(owner > other_group_window);
        assert!(owner > unrelated_overlap);
    }

    #[test]
    fn fullscreen_grab_proxy_accepts_stalker_insets() {
        assert!(fullscreen_grab_proxy_geometry_matches(
            Rectangle::new((1, 1).into(), (2558, 1438).into()),
            Rectangle::new((0, 0).into(), (2566, 1472).into()),
        ));
    }

    #[test]
    fn fullscreen_grab_proxy_rejects_regular_override_redirects() {
        let fullscreen = Rectangle::new((0, 0).into(), (2560, 1440).into());
        for popup in [
            Rectangle::new((0, 0).into(), (1, 1).into()),
            Rectangle::new((1200, 600).into(), (320, 180).into()),
            Rectangle::new((0, 0).into(), (2560, 1300).into()),
        ] {
            assert!(!fullscreen_grab_proxy_geometry_matches(popup, fullscreen));
        }
    }

    #[test]
    fn zoomed_x11_popup_keeps_its_native_offset_from_the_owner() {
        let owner_source = Point::<i32, Logical>::from((995, 2_158));
        let owner_x_root = Point::<i32, Logical>::from((2_647, -4));
        let popup_x_root = Point::<i32, Logical>::from((2_886, 62));

        let popup_source = owner_relative_source_point(owner_source, owner_x_root, popup_x_root);

        assert_eq!(popup_source, Point::from((1_234, 2_224)));
        let native_offset = popup_source - owner_source;
        assert_eq!(native_offset, popup_x_root - owner_x_root);

        let zoom = 0.5;
        let presented_offset = Point::<f64, Logical>::from((
            f64::from(native_offset.x) * zoom,
            f64::from(native_offset.y) * zoom,
        ));
        assert_eq!(presented_offset, Point::from((119.5, 33.0)));
    }

    #[test]
    fn steam_cache_miss_uses_bounded_three_quarter_recovery() {
        assert_eq!(
            recovery_window_size(
                Size::from((1920, 1200)),
                Some(Size::from((1010, 600))),
                None,
            ),
            Size::from((1440, 900))
        );
        assert_eq!(
            recovery_window_size(
                Size::from((1920, 1200)),
                Some(Size::from((1800, 1000))),
                Some(Size::from((1850, 1100))),
            ),
            Size::from((1800, 1000))
        );
        assert_eq!(
            recovery_window_size(
                Size::from((1920, 1200)),
                None,
                Some(Size::from((1200, 700))),
            ),
            Size::from((1200, 700))
        );
    }

    #[test]
    fn output_sized_steam_geometry_is_not_a_normal_restore_size() {
        let output = Size::from((1920, 1200));
        assert!(size_fills_output(Size::from((1920, 1200)), output));
        assert!(size_fills_output(Size::from((2560, 1440)), output));
        assert!(!size_fills_output(Size::from((1440, 900)), output));
    }
}
