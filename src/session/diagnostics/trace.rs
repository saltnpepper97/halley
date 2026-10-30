use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{Window, layer_map_for_output};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::seat::WaylandFocus;
use smithay::xwayland::X11Surface;

use super::{Session, SessionDriver, pointer};

const SELECTOR_ENV: &str = "HALLEY_TRACE_WINDOW";
const MAX_TRACKED_WINDOWS: usize = 32;
const MAX_EVENTS: u64 = 4_096;
const MAX_DETAIL_BYTES: usize = 8_192;
const MOTION_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

/// Opt-in, bounded state-change tracing for one window identity.
///
/// This is deliberately session-owned and inert unless `HALLEY_TRACE_WINDOW`
/// is present. Debug-level eventline output goes to Halley's existing debug
/// file and does not alter compositor, XWM, or pointer-constraint policy.
pub(super) struct WindowTrace {
    selector: Option<String>,
    tracked_xids: HashSet<u32>,
    last_by_event: HashMap<(u32, &'static str), String>,
    last_emitted_at: HashMap<(u32, &'static str), Instant>,
    emitted: u64,
    limit_reported: bool,
}

impl WindowTrace {
    pub(super) fn from_env() -> Self {
        let selector = std::env::var_os(SELECTOR_ENV)
            .map(|value| value.to_string_lossy().trim().to_string())
            .filter(|value| !value.is_empty());
        if let Some(selector) = selector.as_deref() {
            eventline::debug!("window-trace armed selector={selector:?} max_events={MAX_EVENTS}");
        }
        Self {
            selector,
            tracked_xids: HashSet::new(),
            last_by_event: HashMap::new(),
            last_emitted_at: HashMap::new(),
            emitted: 0,
            limit_reported: false,
        }
    }

    fn enabled(&self) -> bool {
        self.selector.is_some() && self.emitted < MAX_EVENTS
    }

    fn is_tracked(&self, xid: u32) -> bool {
        self.tracked_xids.contains(&xid)
    }

    fn observe_x11(&mut self, surface: &X11Surface) -> bool {
        let xid = surface.window_id();
        if self.is_tracked(xid) {
            return true;
        }
        let Some(selector) = self.selector.as_deref() else {
            return false;
        };
        if self.tracked_xids.len() >= MAX_TRACKED_WINDOWS {
            return false;
        }

        let process_labels = surface.pid().map(process_labels).unwrap_or_default();
        let metadata = [surface.instance(), surface.class(), surface.title()];
        if !metadata
            .iter()
            .chain(process_labels.iter())
            .any(|candidate| selector_matches(selector, candidate))
        {
            return false;
        }

        self.tracked_xids.insert(xid);
        self.emit(
            xid,
            "registered",
            format!(
                "pid={:?} instance={:?} class={:?} title={:?} process={:?}",
                surface.pid(),
                metadata[0],
                metadata[1],
                metadata[2],
                process_labels,
            ),
        );
        true
    }

    fn emit(&mut self, xid: u32, event: &'static str, details: String) {
        self.emit_with_interval(xid, event, details, None);
    }

    fn emit_sampled(&mut self, xid: u32, event: &'static str, details: String) {
        self.emit_with_interval(xid, event, details, Some(MOTION_SAMPLE_INTERVAL));
    }

    fn emit_with_interval(
        &mut self,
        xid: u32,
        event: &'static str,
        details: String,
        minimum_interval: Option<Duration>,
    ) {
        if !self.enabled() {
            self.report_limit_once();
            return;
        }
        let details = bounded_single_line(details);
        let key = (xid, event);
        if self.last_by_event.get(&key) == Some(&details) {
            return;
        }
        let now = Instant::now();
        if minimum_interval.is_some_and(|minimum| {
            self.last_emitted_at
                .get(&key)
                .is_some_and(|previous| now.duration_since(*previous) < minimum)
        }) {
            return;
        }
        self.last_by_event.insert(key, details.clone());
        self.last_emitted_at.insert(key, now);
        self.emitted += 1;
        eventline::debug!(
            "window-trace seq={} selector={:?} xid={} event={} {}",
            self.emitted,
            self.selector.as_deref().unwrap_or_default(),
            xid,
            event,
            details,
        );
        self.report_limit_once();
    }

    fn report_limit_once(&mut self) {
        if self.selector.is_none() || self.limit_reported || self.emitted < MAX_EVENTS {
            return;
        }
        self.limit_reported = true;
        eventline::debug!(
            "window-trace stopped selector={:?} reason=event-limit max_events={MAX_EVENTS}",
            self.selector.as_deref().unwrap_or_default(),
        );
    }

    fn forget(&mut self, xid: u32) {
        self.tracked_xids.remove(&xid);
        self.last_by_event
            .retain(|(candidate, _), _| *candidate != xid);
        self.last_emitted_at
            .retain(|(candidate, _), _| *candidate != xid);
    }
}

pub(crate) fn x11_event<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    event: &'static str,
    details: fmt::Arguments<'_>,
) {
    if !session.window_trace.enabled() || !session.window_trace.observe_x11(surface) {
        return;
    }
    session
        .window_trace
        .emit(surface.window_id(), event, details.to_string());
}

pub(crate) fn forget_x11<D: SessionDriver>(session: &mut Session<D>, surface: &X11Surface) {
    session.window_trace.forget(surface.window_id());
}

pub(crate) fn x11_sampled_event<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &X11Surface,
    event: &'static str,
    details: fmt::Arguments<'_>,
) {
    if !session.window_trace.enabled() || !session.window_trace.observe_x11(surface) {
        return;
    }
    session
        .window_trace
        .emit_sampled(surface.window_id(), event, details.to_string());
}

pub(crate) fn surface_event<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    event: &'static str,
    details: fmt::Arguments<'_>,
) {
    if !session.window_trace.enabled() {
        return;
    }
    let x11 = x11_for_surface_or_constraint_proxy(session, surface);
    if let Some(x11) = x11 {
        x11_event(session, &x11, event, details);
    }
}

pub(crate) fn surface_sampled_event<D: SessionDriver>(
    session: &mut Session<D>,
    surface: &WlSurface,
    event: &'static str,
    details: fmt::Arguments<'_>,
) {
    if !session.window_trace.enabled() {
        return;
    }
    let x11 = x11_for_surface_or_constraint_proxy(session, surface);
    if let Some(x11) = x11 {
        x11_sampled_event(session, &x11, event, details);
    }
}

fn x11_for_surface_or_constraint_proxy<D: SessionDriver>(
    session: &Session<D>,
    surface: &WlSurface,
) -> Option<X11Surface> {
    let root = crate::wayland::compositor::root_surface(surface);
    let authority =
        crate::xwayland::pointer_constraint_proxy_authority(&session.wayland.space, surface);
    session
        .wayland
        .space
        .elements()
        .find(|window| {
            window.wl_surface().is_some_and(|candidate| {
                let candidate = crate::wayland::compositor::root_surface(candidate.as_ref());
                candidate == root || authority.as_ref() == Some(&candidate)
            })
        })
        .and_then(Window::x11_surface)
        .cloned()
}

pub(crate) fn snapshot<D: SessionDriver>(session: &mut Session<D>) {
    if !session.window_trace.enabled() {
        return;
    }
    let windows = session
        .wayland
        .space
        .elements()
        .filter(|window| {
            window
                .x11_surface()
                .is_some_and(|surface| session.window_trace.is_tracked(surface.window_id()))
        })
        .cloned()
        .collect::<Vec<_>>();
    for window in windows {
        let Some((xid, state)) = runtime_state(session, &window) else {
            continue;
        };
        session.window_trace.emit_sampled(xid, "state", state);
    }
}

fn runtime_state<D: SessionDriver>(session: &Session<D>, window: &Window) -> Option<(u32, String)> {
    let x11 = window.x11_surface()?;
    let root = window.wl_surface()?.into_owned();
    let xid = x11.window_id();
    let now = crate::frame_clock::monotonic_now();
    let node_id = session.nodes.id_for_surface(&root);
    let record = node_id.and_then(|id| session.nodes.record(id));
    let output_name = record
        .map(|record| record.output.clone())
        .or_else(|| crate::wayland::window_output_name(window))
        .unwrap_or_else(|| session.driver.primary_output().name());
    let output = session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == output_name)
        .cloned()
        .unwrap_or_else(|| session.driver.primary_output().clone());
    let output_geometry = session.wayland.space.output_geometry(&output);
    let work_area = layer_map_for_output(&output).non_exclusive_zone();
    let cluster_id = node_id.and_then(|id| session.clusters.cluster_for_member(id));
    let cluster_metadata = cluster_id.and_then(|id| session.clusters.metadata(id));
    let cluster_target = node_id.map(|id| {
        session
            .clusters
            .window_presentation(id, &output_name, work_area, None, now)
    });
    let visual = crate::presentation::window::window_visual_state(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        window,
        &output,
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        now,
    );
    let presentation = crate::presentation::window::WindowPresentation::for_window(
        &session.wayland.space,
        &session.cameras,
        Some(&session.clusters),
        Some(&session.nodes),
        &session.window_animations,
        &session.fullscreen,
        &session.maximize,
        &session.settings.decorations,
        &session.settings.font,
        window,
        &output,
        now,
    );
    let (has_buffer, buffer_size) = with_renderer_surface_state(&root, |state| {
        (state.buffer().is_some(), state.surface_size())
    })
    .unwrap_or((false, None));
    let keyboard_focus = session
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|focus| {
            focus
                .wl_surface()
                .map(|surface| crate::wayland::compositor::root_surface(surface.as_ref()))
        });
    let pointer_focus = session
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.current_focus())
        .map(|surface| crate::wayland::compositor::root_surface(&surface));
    let constraint = pointer::constraint_diagnostic(session, &root);
    let route = pointer_route_state(session);
    let identity = crate::window::rules::identity(window);

    let mut state = String::new();
    let _ = write!(
        state,
        "identity={{pid:{:?},app:{:?},title:{:?}}} surface={{wl:{},mapped:{},buffer:{},buffer_size:{:?},drawable:{:?},window:{:?}}} ",
        x11.pid(),
        identity.app_id,
        identity.title,
        root.id().protocol_id(),
        x11.is_mapped(),
        has_buffer,
        buffer_size,
        x11.geometry(),
        window.geometry(),
    );
    let _ = write!(
        state,
        "space={{location:{:?},geometry:{:?},bbox:{:?},output:{:?},output_geometry:{:?}}} ",
        session.wayland.space.element_location(window),
        session.wayland.space.element_geometry(window),
        session.wayland.space.element_bbox(window),
        output_name,
        output_geometry,
    );
    let _ = write!(
        state,
        "node={{id:{:?},record_geometry:{:?},collapsed:{:?},attached:{:?}}} cluster={{id:{:?},active:{:?},layout:{:?},metadata_layout:{:?},target:{:?}}} ",
        node_id,
        record.map(|record| record.geometry),
        record.map(|record| record.collapsed),
        record.map(|record| record.attached),
        cluster_id,
        session.clusters.active_on(&output_name),
        node_id.and_then(|id| session.clusters.active_layout_for_member(id)),
        cluster_metadata.map(|metadata| metadata.layout),
        cluster_target,
    );
    let _ = write!(
        state,
        "lifecycle={{opening:{},fullscreen_halley:{},fullscreen_x11:{},maximize_halley:{},maximize_x11:{}}} camera={:?} visual={:?} presentation={{source:{:?},visual:{:?},hit:{:?}}} ",
        session.window_animations.is_animating(&root, now),
        session.fullscreen.is_fullscreen_or_pending(&root),
        x11.is_fullscreen(),
        session.maximize.contains(&root),
        x11.is_maximized(),
        session.cameras.view(&output_name),
        visual,
        presentation.as_ref().map(|value| value.source_geometry()),
        presentation.as_ref().map(|value| value.visual_geometry()),
        presentation.as_ref().map(|value| value.hit_geometry()),
    );
    let _ = write!(
        state,
        "focus={{compositor:{:?},keyboard:{:?},pointer:{:?},owns_compositor:{},owns_keyboard:{},owns_pointer:{}}} pointer={{screen:{:?},route:{}}} constraint={{resource:{:?},kind:{:?},protocol_active:{},halley_tracked:{},tracked_kind:{:?},tracked_surface:{:?}}}",
        surface_id(session.wayland.focused_window.as_ref()),
        surface_id(keyboard_focus.as_ref()),
        surface_id(pointer_focus.as_ref()),
        session.wayland.focused_window.as_ref() == Some(&root),
        keyboard_focus.as_ref() == Some(&root),
        pointer_focus.as_ref() == Some(&root),
        session.pointer.position(),
        route,
        constraint.protocol_resource,
        constraint.protocol_kind,
        constraint.protocol_active,
        constraint.halley_tracked,
        constraint.tracked_kind,
        constraint.tracked_surface_id,
    );
    Some((xid, state))
}

fn pointer_route_state<D: SessionDriver>(session: &Session<D>) -> String {
    let Some(route) = pointer::route_client(session) else {
        return "none".to_string();
    };
    let target = match &route.target {
        crate::input::pointer::PointerTarget::Window(window) => window.x11_surface().map_or_else(
            || "wayland-window".to_string(),
            |surface| format!("xid:{}", surface.window_id()),
        ),
        crate::input::pointer::PointerTarget::Layer(_) => "layer".to_string(),
        crate::input::pointer::PointerTarget::Decoration { window, hit } => {
            window.x11_surface().map_or_else(
                || format!("wayland-decoration:{hit:?}"),
                |surface| format!("xid:{}-decoration:{hit:?}", surface.window_id()),
            )
        }
        crate::input::pointer::PointerTarget::Background => "background".to_string(),
    };
    let local = route
        .focus
        .as_ref()
        .map(|(surface, origin)| (surface.id().protocol_id(), route.location - *origin));
    format!(
        "{{output:{:?},target:{target},location:{:?},local:{:?},visual:{:?}}}",
        route.output.name(),
        route.location,
        local,
        route.visual_geometry,
    )
}

fn surface_id(surface: Option<&WlSurface>) -> Option<u32> {
    surface.map(|surface| surface.id().protocol_id())
}

fn process_labels(pid: u32) -> Vec<String> {
    let mut labels = Vec::new();
    if let Ok(executable) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        labels.push(executable.to_string_lossy().into_owned());
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        labels.push(comm.trim().to_string());
    }
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let cmdline = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part))
            .collect::<Vec<_>>()
            .join(" ");
        if !cmdline.is_empty() {
            labels.push(cmdline);
        }
    }
    labels
}

fn selector_matches(selector: &str, candidate: &str) -> bool {
    candidate
        .to_lowercase()
        .contains(selector.to_lowercase().as_str())
}

fn bounded_single_line(mut value: String) -> String {
    value = value.replace(['\n', '\r'], " ");
    if value.len() <= MAX_DETAIL_BYTES {
        return value;
    }
    let mut end = MAX_DETAIL_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push('…');
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_matching_is_case_insensitive_and_supports_process_paths() {
        assert!(selector_matches(
            "tf_linux64",
            "/games/Team Fortress 2/tf_linux64 -steam"
        ));
        assert!(selector_matches("TF_LINUX64", "tf_linux64"));
        assert!(!selector_matches("tf_linux64", "steamwebhelper"));
    }

    #[test]
    fn trace_details_are_single_line_and_bounded() {
        let value = format!("first\n{}", "x".repeat(MAX_DETAIL_BYTES + 10));
        let bounded = bounded_single_line(value);
        assert!(!bounded.contains('\n'));
        assert!(bounded.len() <= MAX_DETAIL_BYTES + '…'.len_utf8());
    }
}
