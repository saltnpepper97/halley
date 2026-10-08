mod control;
mod focus;
mod lifecycle;
mod selection;
mod xwm;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::process::Stdio;

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use smithay::desktop::{Space, Window};
use smithay::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::{
    zwp_xwayland_keyboard_grab_manager_v1::ZwpXwaylandKeyboardGrabManagerV1,
    zwp_xwayland_keyboard_grab_v1::ZwpXwaylandKeyboardGrabV1,
};
use smithay::reexports::wayland_protocols::xwayland::shell::v1::server::{
    xwayland_shell_v1::XwaylandShellV1, xwayland_surface_v1::XwaylandSurfaceV1,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Dispatch, DisplayHandle, GlobalDispatch};
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::CompositorClientState;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::xwayland_keyboard_grab::{
    XWaylandKeyboardGrabHandler, XWaylandKeyboardGrabState,
};
use smithay::wayland::xwayland_shell::{
    XWaylandShellHandler, XWaylandShellState, XWaylandSurfaceUserData,
};
use smithay::xwayland::{X11Wm, XWayland, XWaylandClientData, XWaylandEvent};

use crate::session::{Session, SessionDriver};

pub use focus::KeyboardFocusTarget;

struct PendingWindow {
    surface: smithay::xwayland::X11Surface,
    window: Window,
    initial_size: smithay::utils::Size<i32, smithay::utils::Logical>,
}

#[derive(Clone, Copy)]
struct OverrideRedirectPlacement {
    geometry: Rectangle<i32, Logical>,
}

struct PendingOverrideRedirect {
    surface: smithay::xwayland::X11Surface,
    geometry: Rectangle<i32, Logical>,
    timer: RegistrationToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WindowIdentity {
    instance: String,
    class: String,
    title: String,
}

#[derive(Clone, Copy, Debug)]
struct SavedNormalSize(Size<i32, Logical>);

pub struct State<D: SessionDriver> {
    shell_state: XWaylandShellState,
    _keyboard_grab_state: XWaylandKeyboardGrabState,
    xwm: Option<X11Wm>,
    control: Option<control::X11Control>,
    display: Option<u32>,
    pending_windows: HashMap<u32, PendingWindow>,
    opening_placements: HashMap<u32, lifecycle::OpeningPlacement>,
    client_geometry_guards: HashMap<u32, lifecycle::ClientGeometryGuard>,
    loop_handle: LoopHandle<'static, Session<D>>,
    known_override_redirects: HashSet<u32>,
    pending_override_redirects: HashMap<u32, PendingOverrideRedirect>,
    override_redirect_placements: HashMap<u32, OverrideRedirectPlacement>,
    normal_sizes: HashMap<WindowIdentity, SavedNormalSize>,
    managed_states: xwm::ManagedStateRegistry,
    published_stacking: Vec<u32>,
}

impl<D: SessionDriver> State<D> {
    pub fn new(display: &DisplayHandle, loop_handle: LoopHandle<'static, Session<D>>) -> Self
    where
        Session<D>: XWaylandShellHandler + XWaylandKeyboardGrabHandler + 'static,
        Session<D>: GlobalDispatch<XwaylandShellV1, smithay::wayland::GlobalData>,
        Session<D>: Dispatch<XwaylandShellV1, smithay::wayland::GlobalData>,
        Session<D>: Dispatch<XwaylandSurfaceV1, XWaylandSurfaceUserData>,
        Session<D>: GlobalDispatch<ZwpXwaylandKeyboardGrabManagerV1, smithay::wayland::GlobalData>,
        Session<D>: Dispatch<ZwpXwaylandKeyboardGrabManagerV1, smithay::wayland::GlobalData>,
        Session<D>: Dispatch<ZwpXwaylandKeyboardGrabV1, smithay::wayland::GlobalData>,
    {
        Self {
            shell_state: XWaylandShellState::new::<Session<D>>(display),
            _keyboard_grab_state: XWaylandKeyboardGrabState::new::<Session<D>>(display),
            xwm: None,
            control: None,
            display: None,
            pending_windows: HashMap::new(),
            opening_placements: HashMap::new(),
            client_geometry_guards: HashMap::new(),
            loop_handle,
            known_override_redirects: HashSet::new(),
            pending_override_redirects: HashMap::new(),
            override_redirect_placements: HashMap::new(),
            normal_sizes: HashMap::new(),
            managed_states: xwm::ManagedStateRegistry::default(),
            published_stacking: Vec::new(),
        }
    }

    pub(crate) fn arm_speculative_close_timeout(
        &self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        const SPECULATIVE_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

        let loop_handle = self.loop_handle.clone();
        if let Err(err) = loop_handle.insert_source(
            Timer::from_duration(SPECULATIVE_CLOSE_TIMEOUT),
            move |_, _, session| {
                if session
                    .render
                    .window_close_animations
                    .cancel_speculative(&surface)
                {
                    session.request_redraw();
                }
                TimeoutAction::Drop
            },
        ) {
            eventline::warn!("xwayland: failed to schedule speculative close timeout: {err}");
        }
    }

    pub fn display_name(&self) -> Option<OsString> {
        self.display
            .map(|display| OsString::from(format!(":{display}")))
    }

    pub fn move_override_redirect(
        &self,
        surface: &smithay::xwayland::X11Surface,
        location: Point<i32, Logical>,
    ) {
        let Some(control) = self.control.as_ref() else {
            return;
        };
        if let Err(err) =
            control.move_override_redirect(surface.window_id(), location.x, location.y)
        {
            eventline::warn!("xwayland: pop-out move failed: {err}");
        }
    }

    pub fn raise_window(&mut self, window: &smithay::desktop::Window) {
        let Some(surface) = window.x11_surface() else {
            return;
        };
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        // This changes the real X stack even when parent constraints leave
        // the final Space order unchanged. Republish that order afterwards.
        self.published_stacking.clear();
        if let Err(err) = xwm.raise_window(surface) {
            eventline::warn!("xwayland: failed to raise window: {err}");
        }
    }

    /// Publishes the current output layout as EWMH root desktop geometry.
    ///
    /// Cheap to call on every output change: the control connection dedups on
    /// the published values.
    pub fn sync_desktop_geometry(&self, space: &Space<Window>) {
        let Some(control) = self.control.as_ref() else {
            return;
        };
        let Some(geometry) = desktop_geometry(space) else {
            return;
        };
        if let Err(err) = control.publish_desktop_geometry(geometry.desktop, geometry.work_area) {
            eventline::warn!("xwayland: failed to publish desktop geometry: {err}");
        }
    }

    /// Pushes the compositor's stacking order down to the X server.
    ///
    /// Smithay's XWM owns `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING`
    /// and keeps them in step with the real X stack, so the compositor's job
    /// is to make that stack agree with what is on screen rather than to
    /// rewrite the properties underneath it. `X11Wm::raise_window` alone only
    /// covers raise-to-top; lowering, cluster relayout, and popup restacking
    /// all reorder `Space` without it.
    ///
    /// Override-redirect windows are excluded: they are client-stacked, and
    /// their order is mirrored the other way, from `configure_notify`.
    pub fn sync_stacking_order(&mut self, space: &Space<Window>) {
        let order = space
            .elements()
            .filter_map(|window| window.x11_surface())
            .filter(|surface| !surface.is_override_redirect())
            .cloned()
            .collect::<Vec<_>>();
        // `update_stacking_order_upwards` grabs the X server for the whole
        // walk, so a settled desktop must not reach it. This mirrors the
        // compare-before-configure guard in `sync_position`.
        let published = order
            .iter()
            .map(smithay::xwayland::X11Surface::window_id)
            .collect::<Vec<_>>();
        if self.published_stacking == published {
            return;
        }
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        // Space and `_NET_CLIENT_LIST_STACKING` are both bottom-to-top. Walk
        // upwards from the bottom anchor so each later element is placed above
        // its predecessor. The downward walk expects a top-to-bottom input;
        // feeding it Space order reverses overlapping X11 windows, leaving a
        // newly focused Steam dialog visually above the main window but below
        // it in XWayland's input stack until the first click raises it.
        if let Err(err) = xwm.update_stacking_order_upwards(order.iter()) {
            eventline::warn!("xwayland: failed to publish stacking order: {err}");
            return;
        }
        self.published_stacking = published;
    }

    /// Publishes `_NET_FRAME_EXTENTS` for one managed X11 window.
    ///
    /// Cheap to call on every decoration-relevant change: the control
    /// connection dedups per window on the published values.
    pub fn sync_frame_extents(
        &self,
        window: &Window,
        decorations: &halley_config::Decorations,
        font: &halley_config::Font,
    ) {
        let Some(control) = self.control.as_ref() else {
            return;
        };
        let Some(surface) = window.x11_surface() else {
            return;
        };
        if surface.is_override_redirect() {
            return;
        }
        let extents = crate::titlebar::frame_extents(window, decorations, font);
        if let Err(err) = control.set_frame_extents(surface.window_id(), extents) {
            eventline::warn!(
                "xwayland: failed to publish frame extents xid={}: {err}",
                surface.window_id()
            );
        }
    }

    /// Republishes `_NET_FRAME_EXTENTS` for every managed X11 window, for
    /// when the decoration configuration itself changed rather than a window.
    pub fn sync_all_frame_extents(
        &self,
        space: &Space<Window>,
        decorations: &halley_config::Decorations,
        font: &halley_config::Font,
    ) {
        if self.control.is_none() {
            return;
        }
        for window in space.elements() {
            self.sync_frame_extents(window, decorations, font);
        }
    }

    fn forget_frame_extents(&self, xid: u32) {
        if let Some(control) = self.control.as_ref() {
            control.forget_window(xid);
        }
    }

    pub fn sync_active_window(&self, window: Option<u32>, has_keyboard_focus: bool) {
        let Some(control) = self.control.as_ref() else {
            return;
        };
        if let Err(err) = control.set_active_window(window, has_keyboard_focus) {
            eventline::warn!("xwayland: failed to publish active window {window:?}: {err}");
        }
    }

    pub fn focus_globally_active_window(&self, window: Option<u32>) {
        let (Some(control), Some(window)) = (self.control.as_ref(), window) else {
            return;
        };
        if let Err(err) = control.focus_window(window) {
            eventline::warn!("xwayland: failed to focus globally-active xid={window}: {err}");
        }
    }

    pub fn configure_key_repeat(&self, delay: i32, rate: i32) {
        if let Some(control) = self.control.as_ref()
            && let Err(err) = control.configure_key_repeat(delay, rate)
        {
            eventline::warn!("xwayland: failed to configure key repeat: {err}");
        }
    }

    fn transition_surface(
        &mut self,
        surface: &smithay::xwayland::X11Surface,
        state: xwm::WindowState,
    ) {
        let xid = surface.window_id();
        self.managed_states.transition(xid, state);
        let Some(control) = self.control.as_ref() else {
            return;
        };
        let result = match state {
            xwm::WindowState::Withdrawn => control.withdraw(xid),
            xwm::WindowState::Normal => control.set_wm_state(xid, control::IcccmState::Normal),
            xwm::WindowState::Iconic => control.set_wm_state(xid, control::IcccmState::Iconic),
        };
        if let Err(err) = result {
            eventline::warn!("xwayland: failed ICCCM transition xid={xid} state={state:?}: {err}");
        }
    }

    pub fn set_window_iconic(&mut self, window: &Window) {
        if let Some(surface) = window.x11_surface() {
            self.transition_surface(surface, xwm::WindowState::Iconic);
        }
    }

    pub fn set_window_normal(&mut self, window: &Window) {
        if let Some(surface) = window.x11_surface() {
            self.transition_surface(surface, xwm::WindowState::Normal);
        }
    }

    /// Temporarily keeps cluster layout from configuring an X11 client while
    /// its own fullscreen/mode negotiation is still changing. A one-shot
    /// redraw at the end ensures a stable windowed client is subsequently
    /// reconciled into its tile even when no animation is running.
    pub(crate) fn arm_client_geometry_guard(
        &mut self,
        xid: u32,
        now: std::time::Duration,
    ) -> std::time::Duration {
        let guard = lifecycle::ClientGeometryGuard::arm(now);
        let quiet_until = guard.quiet_until();
        self.client_geometry_guards.insert(xid, guard);
        let loop_handle = self.loop_handle.clone();
        if let Err(err) = loop_handle.insert_source(
            Timer::from_duration(lifecycle::CLIENT_GEOMETRY_QUIET_PERIOD),
            move |_, _, session| {
                if !session
                    .xwayland
                    .client_geometry_guarded(xid, crate::frame_clock::monotonic_now())
                {
                    session.request_redraw();
                }
                TimeoutAction::Drop
            },
        ) {
            eventline::warn!("xwayland: failed to schedule client geometry reconciliation: {err}");
        }
        quiet_until
    }

    pub(crate) fn client_geometry_guarded(&self, xid: u32, now: std::time::Duration) -> bool {
        self.client_geometry_guards
            .get(&xid)
            .is_some_and(|guard| guard.is_active(now))
    }

    pub(crate) fn client_geometry_guarded_for_window(
        &self,
        window: &Window,
        now: std::time::Duration,
    ) -> bool {
        window
            .x11_surface()
            .is_some_and(|surface| self.client_geometry_guarded(surface.window_id(), now))
    }

    pub(crate) fn forget_client_geometry_guard(&mut self, xid: u32) {
        self.client_geometry_guards.remove(&xid);
    }

    fn clear(&mut self) {
        self.control = None;
        for (_, pending) in self.pending_override_redirects.drain() {
            self.loop_handle.remove(pending.timer);
        }
        self.xwm = None;
        self.display = None;
        self.pending_windows.clear();
        self.opening_placements.clear();
        self.client_geometry_guards.clear();
        self.known_override_redirects.clear();
        self.override_redirect_placements.clear();
        self.normal_sizes.clear();
        self.managed_states.clear();
        self.published_stacking.clear();
    }
}

pub fn start<D>(
    loop_handle: &LoopHandle<'static, Session<D>>,
    session: &mut Session<D>,
    publish_environment: bool,
) -> Result<(), Box<dyn std::error::Error>>
where
    D: SessionDriver,
{
    let display_handle = session.wayland.display_handle.clone();
    let (source, client) = XWayland::spawn(
        &display_handle,
        None,
        session.launch_environment(),
        std::iter::empty::<&str>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| {},
    )?;
    let xwm_loop_handle = loop_handle.clone();
    loop_handle.insert_source(source, move |event, _, session| match event {
        XWaylandEvent::Ready {
            x11_socket,
            display_number,
        } => {
            if let Some(data) = client.get_data::<XWaylandClientData>() {
                data.compositor_state.set_client_scale(1.0);
            }
            match X11Wm::start_wm(
                xwm_loop_handle.clone(),
                &display_handle,
                x11_socket,
                client.clone(),
            ) {
                Ok(xwm) => {
                    session.xwayland.xwm = Some(xwm);
                    session.xwayland.display = Some(display_number);
                    match control::X11Control::connect(display_number) {
                        Ok(control) => {
                            if let Err(err) = control.configure_key_repeat(
                                session.settings.input.repeat_delay,
                                session.settings.input.repeat_rate,
                            ) {
                                eventline::warn!("xwayland: failed to configure key repeat: {err}");
                            }
                            session.xwayland.control = Some(control);
                            // `publish_ewmh` seeded the root geometry from
                            // whatever size XWayland's root happened to have at
                            // connect, which need not be the settled layout.
                            session
                                .xwayland
                                .sync_desktop_geometry(&session.wayland.space);
                            eventline::info!("xwayland: ICCCM/EWMH control connection ready");
                        }
                        Err(err) => {
                            eventline::error!(
                                "xwayland: failed to initialize ICCCM/EWMH control: {err}"
                            );
                        }
                    }
                    let display = format!(":{display_number}");
                    if publish_environment {
                        crate::session::environment::activate_xwayland(OsStr::new(&display));
                    }
                    eventline::info!("xwayland: ready, DISPLAY={display}");
                    session.run_autostart_once();
                    session.request_redraw();
                }
                Err(err) => {
                    eventline::error!("xwayland: failed to attach window manager: {err}");
                    session.xwayland.clear();
                    session.run_autostart_once();
                }
            }
        }
        XWaylandEvent::Error => {
            eventline::error!("xwayland: server exited during startup");
            session.xwayland.clear();
            session.run_autostart_once();
        }
    })?;
    Ok(())
}

pub fn reconfigure_fullscreen(
    windows: Vec<(
        smithay::desktop::Window,
        smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    )>,
) {
    xwm::reconfigure_fullscreen(windows);
}

pub fn close_window(window: &smithay::desktop::Window) {
    if let Some(surface) = window.x11_surface()
        && let Err(err) = surface.close()
    {
        eventline::warn!("xwayland: failed to close window: {err}");
    }
}

pub fn configure_window<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
) {
    xwm::configure_window(session, window, geometry);
}

pub fn sync_position<D: SessionDriver>(
    session: &Session<D>,
    window: &smithay::desktop::Window,
) -> bool {
    xwm::sync_position(session, window)
}

pub fn sync_positions<D: SessionDriver>(session: &Session<D>) -> bool {
    xwm::sync_positions(session)
}

/// Sweeps the compositor/X stacking split once per dispatch, next to
/// [`sync_positions`].
///
/// `session::sync_keyboard_focus` already covers map, unmap, destroy and
/// raise. This is the backstop for the rest: a fullscreen transition raises
/// its window in `Space` with no accompanying focus change. The memo in
/// [`State::sync_stacking_order`] is what makes a settled desktop free, the
/// same way the compare-before-configure check does for [`sync_positions`].
pub fn sync_stacking_order<D: SessionDriver>(session: &mut Session<D>) {
    session.xwayland.sync_stacking_order(&session.wayland.space);
}

pub fn constrain_window_size(
    window: &smithay::desktop::Window,
    requested: Size<i32, Logical>,
) -> Size<i32, Logical> {
    xwm::constrain_window_size(window, requested)
}

pub fn set_window_fullscreen<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
    fullscreen: bool,
) {
    xwm::set_window_fullscreen(session, window, fullscreen);
}

pub fn restore_maximized_window<D: SessionDriver>(
    session: &mut Session<D>,
    window: &smithay::desktop::Window,
) {
    xwm::restore_maximized_window(session, window);
}

pub fn handle_commit<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) {
    xwm::handle_commit(session, surface);
}

struct DesktopGeometry {
    desktop: (u32, u32),
    work_area: Option<(i32, i32, u32, u32)>,
}

/// Derives the EWMH root geometry from Halley's own output layout.
///
/// Computed here rather than read back from the X root window because XWayland
/// resizes the root asynchronously from the `wl_output`/`xdg_output` updates,
/// so an immediate read after an output change still returns the old size.
///
/// `_NET_DESKTOP_GEOMETRY` is the union bounding box. The work area is the
/// layer-shell non-exclusive zone, and only exists for a single-output layout —
/// see `X11Control::publish_desktop_geometry` for why multi-output is `None`.
fn desktop_geometry(space: &Space<Window>) -> Option<DesktopGeometry> {
    let mut outputs = space.outputs();
    let first = outputs.next()?;
    let first_geometry = space.output_geometry(first)?;
    let single = outputs.next().is_none();
    let bounds = space
        .outputs()
        .filter_map(|output| space.output_geometry(output))
        .fold(first_geometry, |bounds, geometry| bounds.merge(geometry));
    let work_area = single.then(|| {
        work_area_extent(
            first_geometry,
            smithay::desktop::layer_map_for_output(first).non_exclusive_zone(),
        )
    });
    Some(DesktopGeometry {
        desktop: desktop_extent(bounds),
        work_area,
    })
}

/// EWMH describes the desktop as a size anchored at the origin, so an output
/// laid out at a positive offset extends the reported extent rather than
/// shifting it.
fn desktop_extent(bounds: Rectangle<i32, Logical>) -> (u32, u32) {
    (
        bounds.loc.x.saturating_add(bounds.size.w).max(0) as u32,
        bounds.loc.y.saturating_add(bounds.size.h).max(0) as u32,
    )
}

/// The non-exclusive zone is output-local; `_NET_WORKAREA` is in root
/// coordinates, so it has to be rebased onto the output's position.
fn work_area_extent(
    output: Rectangle<i32, Logical>,
    zone: Rectangle<i32, Logical>,
) -> (i32, i32, u32, u32) {
    (
        output.loc.x.saturating_add(zone.loc.x),
        output.loc.y.saturating_add(zone.loc.y),
        zone.size.w.max(0) as u32,
        zone.size.h.max(0) as u32,
    )
}

pub fn is_override_redirect(window: &smithay::desktop::Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| surface.is_override_redirect())
}

pub fn is_x11(window: &smithay::desktop::Window) -> bool {
    window.x11_surface().is_some()
}

const STEAM_CLOSE_HIT_WIDTH: f64 = 40.0;
const STEAM_CLOSE_HIT_HEIGHT: f64 = 40.0;

/// Steam's X11/CEF windows draw their own titlebar, so clicking their close
/// glyph has no window-management protocol meaning until CEF begins its
/// delayed teardown. Recognize only Steam's conventional top-right control
/// so Halley can own the visual handoff before the black buffer is submitted.
pub(crate) fn is_steam_client_close_hit(
    window: &smithay::desktop::Window,
    local: Point<f64, Logical>,
) -> bool {
    let Some(surface) = window.x11_surface() else {
        return false;
    };
    if !surface.is_decorated() || !surface.class().eq_ignore_ascii_case("steam") {
        return false;
    }
    steam_close_hit(local, window.geometry().size)
}

fn steam_close_hit(local: Point<f64, Logical>, size: Size<i32, Logical>) -> bool {
    let right = f64::from(size.w.max(0));
    local.x >= (right - STEAM_CLOSE_HIT_WIDTH).max(0.0)
        && local.x < right
        && local.y >= 0.0
        && local.y < STEAM_CLOSE_HIT_HEIGHT.min(f64::from(size.h.max(0)))
}

pub fn is_fullscreen(window: &smithay::desktop::Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| surface.is_fullscreen())
}

pub fn uses_server_decorations(window: &smithay::desktop::Window) -> bool {
    window
        .x11_surface()
        .is_some_and(|surface| !surface.is_decorated())
}

pub fn can_maximize(window: &smithay::desktop::Window) -> bool {
    window
        .x11_surface()
        .map(|surface| {
            !surface
                .min_size()
                .zip(surface.max_size())
                .is_some_and(|(minimum, maximum)| minimum == maximum)
        })
        .unwrap_or(true)
}

pub fn window_for_xid(space: &Space<Window>, xid: u32) -> Option<Window> {
    space
        .elements()
        .find(|window| {
            window
                .x11_surface()
                .is_some_and(|surface| surface.window_id() == xid)
        })
        .cloned()
}

/// Resolves an XWayland grab-proxy surface to the managed window which retains
/// compositor and keyboard focus while the proxy owns pointer focus.
pub fn pointer_constraint_proxy_authority(
    space: &Space<Window>,
    surface: &WlSurface,
) -> Option<WlSurface> {
    let root = crate::wayland::compositor::root_surface(surface);
    let proxy = space.elements().find(|window| {
        window
            .x11_surface()
            .is_some_and(|surface| surface.is_override_redirect())
            && window.wl_surface().as_deref() == Some(&root)
    })?;
    let owner = crate::wayland::window_presentation_owner(proxy)?;
    let owner = window_for_xid(space, owner)?;
    owner
        .wl_surface()
        .map(|surface| crate::wayland::compositor::root_surface(surface.as_ref()))
}

pub fn parent_window(space: &Space<Window>, window: &Window) -> Option<Window> {
    let parent = window.x11_surface()?.is_transient_for()?;
    window_for_xid(space, parent)
}

pub fn metadata(window: &Window) -> Option<(String, String)> {
    let surface = window.x11_surface()?;
    Some((surface.title(), surface.class()))
}

pub fn set_hidden(window: &Window, hidden: bool) {
    if let Some(surface) = window.x11_surface()
        && let Err(err) = surface.set_hidden(hidden)
    {
        eventline::warn!("xwayland: failed to set hidden={hidden}: {err}");
    }
}

pub fn set_maximized(window: &Window, maximized: bool) {
    if let Some(surface) = window.x11_surface()
        && surface.is_maximized() != maximized
        && let Err(err) = surface.set_maximized(maximized)
    {
        eventline::warn!("xwayland: failed to set maximized={maximized}: {err}");
    }
}

pub fn compositor_client_state(
    client: &smithay::reexports::wayland_server::Client,
) -> Option<&CompositorClientState> {
    client
        .get_data::<XWaylandClientData>()
        .map(|data| &data.compositor_state)
}

impl<D: SessionDriver> XWaylandShellHandler for Session<D> {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland.shell_state
    }

    fn surface_associated(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        wl_surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        surface: smithay::xwayland::X11Surface,
    ) {
        xwm::surface_associated(self, wl_surface, surface);
    }
}

impl<D: SessionDriver> XWaylandKeyboardGrabHandler for Session<D> {
    fn keyboard_focus_for_xsurface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> Option<Self::KeyboardFocus> {
        if self.session_lock.active() {
            return None;
        }
        self.wayland
            .space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .is_some_and(|candidate| candidate.as_ref() == surface)
            })
            .and_then(KeyboardFocusTarget::for_window)
    }
}

#[cfg(test)]
mod tests {
    use super::{desktop_extent, steam_close_hit, work_area_extent};
    use smithay::utils::{Point, Rectangle, Size};

    #[test]
    fn desktop_extent_spans_to_the_far_edge_of_the_layout() {
        // A side-by-side pair: the reported desktop must cover both, not just
        // the bounding box's own size.
        let bounds = Rectangle::<i32, smithay::utils::Logical>::from_size((2560, 1440).into())
            .merge(Rectangle::new((2560, 0).into(), (1920, 1200).into()));

        assert_eq!(desktop_extent(bounds), (4480, 1440));
    }

    #[test]
    fn work_area_is_rebased_onto_its_output() {
        let output =
            Rectangle::<i32, smithay::utils::Logical>::new((2560, 0).into(), (1920, 1200).into());
        // A 40px top bar reserved by layer shell.
        let zone =
            Rectangle::<i32, smithay::utils::Logical>::new((0, 40).into(), (1920, 1160).into());

        assert_eq!(work_area_extent(output, zone), (2560, 40, 1920, 1160));
    }

    #[test]
    fn steam_close_probe_is_limited_to_the_top_right_control() {
        let size = Size::<i32, smithay::utils::Logical>::from((850, 722));

        assert!(steam_close_hit(Point::from((836.0, 17.0)), size));
        assert!(!steam_close_hit(Point::from((800.0, 17.0)), size));
        assert!(!steam_close_hit(Point::from((836.0, 45.0)), size));
        assert!(!steam_close_hit(Point::from((850.0, 17.0)), size));
    }
}
