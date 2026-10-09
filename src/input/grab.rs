use halley_core::camera::Camera;
use halley_core::field::Vec2;
use smithay::desktop::{Space, Window};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
#[cfg(test)]
use smithay::utils::Physical;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size};
use smithay::wayland::seat::WaylandFocus;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum ClusterWindowDragKind {
    Layout(halley_core::cluster::layout::ClusterWorkspaceLayoutKind),
    Floating,
}

#[derive(Clone, Debug)]
pub struct ClusterWindowDrag {
    pub cluster_id: halley_core::cluster::ClusterId,
    pub output: String,
    pub kind: ClusterWindowDragKind,
    /// Membership remains provisional for the whole grab. This tracks whether
    /// the pointer is currently over the source output: returning there keeps
    /// the cluster intact, while release elsewhere commits the detach.
    pub on_origin_output: bool,
}

pub const WINDOW_EDGE_PAN_DWELL: Duration = Duration::from_millis(450);
pub const WINDOW_EDGE_PAN_SPEED_PXPS: f32 = 240.0;

#[derive(Clone, Debug, PartialEq)]
pub struct WindowEdgePan {
    pub output: String,
    pub contact: Vec2,
    pub contact_since: Option<Duration>,
    pub last_tick: Duration,
}

impl WindowEdgePan {
    pub fn new(output: String, now: Duration) -> Self {
        Self {
            output,
            contact: Vec2 { x: 0.0, y: 0.0 },
            contact_since: None,
            last_tick: now,
        }
    }

    pub fn update_contact(&mut self, contact: Vec2, now: Duration) {
        if self.contact != contact {
            self.contact = contact;
            self.contact_since = (contact.x != 0.0 || contact.y != 0.0).then_some(now);
        } else if contact.x == 0.0 && contact.y == 0.0 {
            self.contact_since = None;
        }
    }

    pub fn ready(&self, now: Duration) -> bool {
        self.contact_since
            .is_some_and(|started| now.saturating_sub(started) >= WINDOW_EDGE_PAN_DWELL)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowEdgePanPlacement {
    pub location: Point<i32, Logical>,
    pub center: Vec2,
    pub contact: Vec2,
}

pub fn window_edge_pan_placement(
    pointer: (f64, f64),
    anchor: WindowGrabAnchor,
    size: Size<i32, Logical>,
    frame_extents: (i32, i32, i32, i32),
    camera: &Camera,
    output_geometry: Rectangle<i32, Logical>,
) -> Option<WindowEdgePanPlacement> {
    let desired = anchor.world_location(pointer, camera, output_geometry);
    let desired_center = Vec2 {
        x: desired.x as f32 + size.w as f32 * 0.5,
        y: desired.y as f32 + size.h as f32 * 0.5,
    };
    let (left, right, top, bottom) = frame_extents;
    let left = left.max(0) as f32;
    let right = right.max(0) as f32;
    let top = top.max(0) as f32;
    let bottom = bottom.max(0) as f32;
    let outer_width = size.w as f32 + left + right;
    let outer_height = size.h as f32 + top + bottom;
    if outer_width > camera.view_size.x || outer_height > camera.view_size.y {
        return None;
    }

    let screen_center = (
        f64::from(output_geometry.loc.x) + f64::from(output_geometry.size.w) * 0.5,
        f64::from(output_geometry.loc.y) + f64::from(output_geometry.size.h) * 0.5,
    );
    let view_center = screen_to_world_on_output(screen_center, camera, output_geometry);
    let half_w = size.w as f32 * 0.5;
    let half_h = size.h as f32 * 0.5;
    let min_x = view_center.x - camera.view_size.x * 0.5 + half_w + left;
    let max_x = view_center.x + camera.view_size.x * 0.5 - half_w - right;
    let min_y = view_center.y - camera.view_size.y * 0.5 + half_h + top;
    let max_y = view_center.y + camera.view_size.y * 0.5 - half_h - bottom;
    let center = Vec2 {
        x: desired_center.x.clamp(min_x, max_x),
        y: desired_center.y.clamp(min_y, max_y),
    };
    let contact = Vec2 {
        x: if desired_center.x <= min_x + 0.01 {
            -1.0
        } else if desired_center.x >= max_x - 0.01 {
            1.0
        } else {
            0.0
        },
        y: if desired_center.y <= min_y + 0.01 {
            -1.0
        } else if desired_center.y >= max_y - 0.01 {
            1.0
        } else {
            0.0
        },
    };
    Some(WindowEdgePanPlacement {
        location: Point::from((
            (center.x - half_w).round() as i32,
            (center.y - half_h).round() as i32,
        )),
        center,
        contact,
    })
}

#[derive(Clone, Debug)]
pub struct PendingWindowMove {
    pub window: Window,
    pub serial: Serial,
    pub button: u32,
    pub press_screen: Point<f64, Logical>,
    pub output: String,
    pub visual_geometry: Rectangle<i32, Logical>,
    pub maximized: bool,
    pub client_owned: bool,
}

/// Coordinate space used to keep a dragged window attached to the pointer.
///
/// Field windows scale with the destination camera, so their grip must remain
/// in source/`Space` coordinates. Cluster workspace cards are deliberately
/// screen-sized while held, so they retain a screen-pixel offset instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowGrabAnchor {
    Source(Vec2),
    Screen(Vec2),
}

impl WindowGrabAnchor {
    pub fn world_location(
        self,
        pointer: (f64, f64),
        camera: &Camera,
        output_geometry: Rectangle<i32, Logical>,
    ) -> Point<i32, Logical> {
        let world = screen_to_world_on_output(pointer, camera, output_geometry);
        let offset = match self {
            Self::Source(offset) => offset,
            Self::Screen(offset) => screen_offset_to_world(offset, camera),
        };
        Point::from((
            (world.x + offset.x).round() as i32,
            (world.y + offset.y).round() as i32,
        ))
    }

    pub fn screen_offset(self, camera: &Camera) -> Vec2 {
        match self {
            Self::Source(offset) => {
                let scale = crate::input::zoom::scale(camera);
                Vec2 {
                    x: offset.x * scale,
                    y: offset.y * scale,
                }
            }
            Self::Screen(offset) => offset,
        }
    }
}

/// What's currently being dragged with the left mouse button held, if
/// anything - `None` the rest of the time. Lives on `App`/`TtyApp` next to
/// `pointer`/`camera`, mirroring how each of those was added for one
/// concrete reason.
pub enum Grab {
    None,
    /// A client requested an interactive move on button press. Toolkits may
    /// do this before they know whether the gesture is a click, double-click,
    /// or drag, so no compositor move side effects happen until motion crosses
    /// the shared drag threshold.
    PendingWindowMove(PendingWindowMove),
    /// Explicit modifier-drag of a standalone, screen-positioned X11 pop-out.
    MovePopup {
        window: Window,
        button: u32,
        offset: Point<f64, Logical>,
    },
    /// Cursor-to-window anchor in the coordinate space of the window's live
    /// presentation. Field windows use source coordinates; screen-sized
    /// cluster cards use output pixels.
    MoveWindow {
        id: Option<halley_core::field::NodeId>,
        window: Window,
        /// Origin and membership state for a window pulled from an active
        /// cluster. Ordinary Field moves leave this unset.
        cluster_drag: Option<ClusterWindowDrag>,
        /// Stable windowed size used while the client is acknowledging the
        /// restore configure from a maximized title-bar drag.
        drag_size: Option<Size<i32, Logical>>,
        /// The physical button that owns this move.
        button: u32,
        /// Client title-bar moves must forward their release to retire the
        /// client's implicit pointer grab. Compositor-only moves must not send
        /// an orphan release for a press they intercepted.
        client_owned: bool,
        anchor: WindowGrabAnchor,
        /// `Some` for the monitor-local `drag-pan` window drag. Ordinary
        /// `move-window` grabs leave this unset and may cross outputs.
        edge_pan: Option<WindowEdgePan>,
        last_world: Vec2,
        last_update: Duration,
        velocity: Vec2,
    },
    /// A node press that is still eligible to become a single-click restore.
    PendingNode {
        id: halley_core::field::NodeId,
        surface: WlSurface,
        press_screen: Point<f64, Logical>,
        screen_offset: Vec2,
    },
    /// A collapsed marker being carried without restoring its client window.
    MoveNode {
        id: halley_core::field::NodeId,
        surface: WlSurface,
        screen_offset: Vec2,
        last_world: Vec2,
        last_update: Duration,
        velocity: Vec2,
    },
    /// A cluster-core press that remains a click until it crosses the shared
    /// landmark drag threshold.
    PendingClusterCore {
        id: halley_core::cluster::ClusterId,
        output: String,
        press_screen: Point<f64, Logical>,
        screen_offset: Vec2,
    },
    /// A collapsed cluster core carried as one cluster-owned landmark.
    MoveClusterCore {
        id: halley_core::cluster::ClusterId,
        screen_offset: Vec2,
    },
    /// Camera pan drag (`pan-field`, `pan-field-always`, or `move-window`
    /// falling back on empty background). The output is captured at press
    /// time so crossing a boundary mid-drag never pans both monitors, and the
    /// owning button is remembered so only its release ends the drag.
    Pan {
        output: String,
        button: u32,
    },
    /// Mod+right-click-drag on a window.
    ResizeWindow(ResizeState),
}

pub(crate) fn screen_grip_offset(
    pointer: (f64, f64),
    visual_location: Point<i32, Logical>,
) -> Vec2 {
    Vec2 {
        x: visual_location.x as f32 - pointer.0 as f32,
        y: visual_location.y as f32 - pointer.1 as f32,
    }
}

pub(crate) fn world_location_from_screen_grip(
    pointer: (f64, f64),
    screen_offset: Vec2,
    camera: &Camera,
    output_geometry: Rectangle<i32, Logical>,
) -> Point<i32, Logical> {
    WindowGrabAnchor::Screen(screen_offset).world_location(pointer, camera, output_geometry)
}

impl Grab {
    /// Start a camera pan only when no compositor drag owns the pointer.
    /// A second chord must not replace a pending move, resize, or pan and lose
    /// the original operation's release and cleanup state.
    pub fn try_begin_pan(&mut self, output: String, button: u32) -> bool {
        if !matches!(self, Self::None) {
            return false;
        }
        *self = Self::Pan { output, button };
        true
    }

    /// Whether a collapsed landmark is being pressed or moved. Both nodes and
    /// cluster cores suppress hover presentation while the pointer owns them.
    pub fn landmark_active(&self) -> bool {
        matches!(
            self,
            Self::PendingNode { .. }
                | Self::MoveNode { .. }
                | Self::PendingClusterCore { .. }
                | Self::MoveClusterCore { .. }
        )
    }
}

pub fn belongs_to_surface(grab: &Grab, surface: &WlSurface) -> bool {
    let root = crate::wayland::compositor::root_surface(surface);
    let window = match grab {
        Grab::PendingWindowMove(pending) => Some(&pending.window),
        Grab::MoveWindow { window, .. } | Grab::MovePopup { window, .. } => Some(window),
        Grab::ResizeWindow(resize) => Some(&resize.window),
        Grab::None
        | Grab::Pan { .. }
        | Grab::PendingNode { .. }
        | Grab::MoveNode { .. }
        | Grab::PendingClusterCore { .. }
        | Grab::MoveClusterCore { .. } => None,
    };
    window.is_some_and(|window| {
        window.wl_surface().is_some_and(|candidate| {
            crate::wayland::compositor::root_surface(candidate.as_ref()) == root
        })
    }) || matches!(
        grab,
        Grab::PendingNode {
            surface: candidate,
            ..
        } | Grab::MoveNode {
            surface: candidate,
            ..
        } if crate::wayland::compositor::root_surface(candidate) == root
    )
}

/// Which edges a resize drag moves. The opposite edges stay anchored, so
/// dragging the left edge grows the window leftward rather than sliding it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeHandle {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeHandle {
    pub fn cursor_icon(self) -> smithay::input::pointer::CursorIcon {
        use smithay::input::pointer::CursorIcon;
        match self {
            Self::Left => CursorIcon::WResize,
            Self::Right => CursorIcon::EResize,
            Self::Top => CursorIcon::NResize,
            Self::Bottom => CursorIcon::SResize,
            Self::TopLeft => CursorIcon::NwResize,
            Self::TopRight => CursorIcon::NeResize,
            Self::BottomLeft => CursorIcon::SwResize,
            Self::BottomRight => CursorIcon::SeResize,
        }
    }

    pub fn moves_left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    pub fn moves_right(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }

    pub fn moves_top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    pub fn moves_bottom(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }
}

/// Selects a resize edge or corner from a point inside a rendered window
/// frame. The band is measured inward from the outer frame so it cannot steal
/// input from a different window visible immediately behind this one.
pub fn border_resize_handle(
    frame: Rectangle<i32, Logical>,
    point: Point<f64, Logical>,
    band: f64,
) -> Option<ResizeHandle> {
    if !frame.to_f64().contains(point) {
        return None;
    }

    let band = band.max(0.0);
    let left = frame.loc.x as f64;
    let top = frame.loc.y as f64;
    let right = (frame.loc.x + frame.size.w) as f64;
    let bottom = (frame.loc.y + frame.size.h) as f64;
    let to_left = point.x - left;
    let to_right = right - point.x;
    let to_top = point.y - top;
    let to_bottom = bottom - point.y;
    let near_left = to_left <= band;
    let near_right = to_right <= band;
    let near_top = to_top <= band;
    let near_bottom = to_bottom <= band;

    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some(ResizeHandle::TopLeft),
        (_, true, true, _) => Some(ResizeHandle::TopRight),
        (true, _, _, true) => Some(ResizeHandle::BottomLeft),
        (_, true, _, true) => Some(ResizeHandle::BottomRight),
        (true, _, _, _) => Some(ResizeHandle::Left),
        (_, true, _, _) => Some(ResizeHandle::Right),
        (_, _, true, _) => Some(ResizeHandle::Top),
        (_, _, _, true) => Some(ResizeHandle::Bottom),
        _ => None,
    }
}

/// Everything a resize drag needs, captured once at grab-start. Both the
/// rect and the cursor are in world coordinates, so the math below stays
/// independent of pan and zoom.
#[derive(Clone)]
pub struct ResizeState {
    pub window: Window,
    pub handle: ResizeHandle,
    pub button: u32,
    pub start_rect: Rectangle<i32, Logical>,
    pub start_cursor: Vec2,
    pub start_screen: (f64, f64),
    pub screen_to_source_scale: Vec2,
    pub target_size: Size<i32, Logical>,
    pub preview_size: Vec2,
    pub last_smooth_tick: Duration,
}

fn smooth_resize_value(
    preview: f32,
    target: f32,
    dt: Duration,
    enabled: bool,
    duration_ms: u32,
) -> f32 {
    if !enabled {
        return target;
    }
    let dt_secs = dt.as_secs_f32().clamp(0.0, 0.25);
    if dt_secs <= f32::EPSILON {
        return preview;
    }
    let duration_secs = duration_ms.max(1) as f32 / 1_000.0;
    let alpha = (1.0 - 0.1f32.powf(dt_secs / duration_secs)).clamp(0.0, 1.0);
    let next = preview + (target - preview) * alpha;
    if (target - next).abs() < 0.5 {
        target
    } else {
        next
    }
}

pub fn advance_resize_preview(
    state: &mut ResizeState,
    now: Duration,
    enabled: bool,
    duration_ms: u32,
) -> bool {
    let before = resize_preview_size(state);
    let dt = now.saturating_sub(state.last_smooth_tick);
    state.last_smooth_tick = now;
    state.preview_size.x = smooth_resize_value(
        state.preview_size.x,
        state.target_size.w as f32,
        dt,
        enabled,
        duration_ms,
    );
    state.preview_size.y = smooth_resize_value(
        state.preview_size.y,
        state.target_size.h as f32,
        dt,
        enabled,
        duration_ms,
    );
    resize_preview_size(state) != before
}

pub fn resize_preview_size(state: &ResizeState) -> Size<i32, Logical> {
    Size::from((
        state.preview_size.x.round().max(1.0) as i32,
        state.preview_size.y.round().max(1.0) as i32,
    ))
}

pub fn resize_cursor_from_screen(state: &ResizeState, screen: (f64, f64)) -> Vec2 {
    source_cursor_from_screen(
        state.start_cursor,
        state.start_screen,
        state.screen_to_source_scale,
        screen,
    )
}

fn source_cursor_from_screen(
    start_cursor: Vec2,
    start_screen: (f64, f64),
    screen_to_source_scale: Vec2,
    screen: (f64, f64),
) -> Vec2 {
    Vec2 {
        x: start_cursor.x + (screen.0 - start_screen.0) as f32 * screen_to_source_scale.x,
        y: start_cursor.y + (screen.1 - start_screen.1) as f32 * screen_to_source_scale.y,
    }
}

pub fn resize_screen_to_source_scale(
    source: Rectangle<i32, Logical>,
    visual: Rectangle<i32, Logical>,
) -> Vec2 {
    Vec2 {
        x: source.size.w.max(1) as f32 / visual.size.w.max(1) as f32,
        y: source.size.h.max(1) as f32 / visual.size.h.max(1) as f32,
    }
}

/// Left/top-edge anchoring state, kept *outside* `Grab` because it has to
/// outlive the drag itself.
///
/// Halley needs two states because it sends resize configures inline from
/// the motion handler, so the last serial is already known by the time the
/// button comes up.
pub struct ResizeAnchor {
    pub window: Window,
    pub handle: ResizeHandle,
    pub phase: ResizePhase,
    /// Serial of the most recent configure actually sent to this client
    /// during the drag, or `None` if the drag never changed the size.
    pub last_configure: Option<Serial>,
    /// The compositor's own copy of the window's size, and the *only* usable
    /// "before" side of the anchoring correction.
    ///
    /// It cannot be re-read from the surface at commit time: Smithay swaps
    /// pending cached state into current (`PrivateSurfaceData::commit` in
    /// `wayland/compositor/tree.rs`) *before* dispatching
    /// `CompositorHandler::commit`, so by the time this code runs
    /// `Window::geometry()` already reports the newly committed size and the
    /// correction would always work out to zero. Keeping the compositor's
    /// previous size provides the required before/after pair.
    pub last_size: Size<i32, Logical>,
}

pub enum ResizePhase {
    /// Button still held - every commit re-anchors.
    Ongoing,
    /// Button released, but the client still owes us a commit for the last
    /// configure we sent. Dropping the anchor here instead would let that
    /// final, in-flight resize land unanchored and snap the window sideways
    /// by whatever the client rounded off - exactly the jump anchoring
    /// exists to prevent, just moved to the end of the drag.
    WaitingForLastCommit(Serial),
}

/// Records a configure sent mid-drag, so the release below knows which commit
/// to wait for. A `None` serial means `send_pending_configure` found nothing
/// pending, which leaves the previous serial standing as the latest one.
pub fn note_resize_configure(anchor: &mut Option<ResizeAnchor>, serial: Option<Serial>) {
    if let Some(anchor) = anchor
        && serial.is_some()
    {
        anchor.last_configure = serial;
    }
}

/// Drops the anchor if it belonged to a window that just went away - a
/// released drag can otherwise sit in `WaitingForLastCommit` for a commit
/// that is never coming, since the client closed instead of answering.
pub fn forget_resize_anchor(anchor: &mut Option<ResizeAnchor>, surface: &WlSurface) {
    let belongs_to_surface = anchor.as_ref().is_some_and(|resize| {
        resize
            .window
            .wl_surface()
            .is_some_and(|candidate| candidate.as_ref() == surface)
    });
    if belongs_to_surface {
        *anchor = None;
    }
}

/// The drag ended. Keeps the anchor alive for one more round trip if a
/// configure is still unanswered, and drops it outright if the drag never
/// asked the client for anything.
pub fn release_resize_anchor(anchor: &mut Option<ResizeAnchor>) {
    let Some(resize) = anchor.as_mut() else {
        return;
    };
    match released_phase(resize.last_configure) {
        Some(phase) => resize.phase = phase,
        None => *anchor = None,
    }
}

/// Which phase a released drag moves to, or `None` to retire the anchor
/// immediately because no configure is outstanding.
fn released_phase(last_configure: Option<Serial>) -> Option<ResizePhase> {
    Some(ResizePhase::WaitingForLastCommit(last_configure?))
}

/// Whether the commit just processed is the one a released drag was waiting
/// for. `committed` is the serial the client had acked as of that commit;
/// `is_no_older_than` (rather than equality) because a client is free to skip
/// straight to a newer configure, which acks every older one with it.
fn anchor_is_retired(phase: &ResizePhase, committed: Option<Serial>) -> bool {
    match phase {
        ResizePhase::Ongoing => false,
        ResizePhase::WaitingForLastCommit(waiting) => {
            committed.is_some_and(|committed| committed.is_no_older_than(waiting))
        }
    }
}

/// Floor on interactive resize, matching old halley's own `.max(96.0)` /
/// `.max(72.0)` clamps. A client is free to commit something larger (a
/// terminal quantizing to whole cells, say) - nothing here fights it, since
/// this only bounds what gets *requested*.
pub const MIN_RESIZE_W: i32 = 96;
pub const MIN_RESIZE_H: i32 = 72;

/// Picks which edge or corner a press grabs, by which cell of a 3x3 grid
/// over the window it landed in: corners grab that corner, edge strips grab
/// that edge, and a press in the dead center falls back to the single
/// nearest edge. Ported from old halley's `handle_from_press_position`
/// (`halley-wl/src/input/pointer/resize/handles.rs`) so mod+right-drag picks
/// the same handle there and here.
pub fn handle_from_press_position(rect: Rectangle<i32, Logical>, point: Vec2) -> ResizeHandle {
    let left = rect.loc.x as f32;
    let top = rect.loc.y as f32;
    let width = (rect.size.w as f32).max(1.0);
    let height = (rect.size.h as f32).max(1.0);
    let fx = ((point.x - left) / width).clamp(0.0, 1.0);
    let fy = ((point.y - top) / height).clamp(0.0, 1.0);

    let near = |f: f32| f < 1.0 / 3.0;
    let far = |f: f32| f >= 2.0 / 3.0;

    match (near(fx), far(fx), near(fy), far(fy)) {
        (true, _, true, _) => ResizeHandle::TopLeft,
        (_, true, true, _) => ResizeHandle::TopRight,
        (true, _, _, true) => ResizeHandle::BottomLeft,
        (_, true, _, true) => ResizeHandle::BottomRight,
        (_, _, true, _) => ResizeHandle::Top,
        (_, _, _, true) => ResizeHandle::Bottom,
        (true, _, _, _) => ResizeHandle::Left,
        (_, true, _, _) => ResizeHandle::Right,
        // Dead center - no zone won, so resize whichever edge is closest.
        _ => {
            let to_left = point.x - left;
            let to_right = left + width - point.x;
            let to_top = point.y - top;
            let to_bottom = top + height - point.y;
            let min = to_left.min(to_right).min(to_top).min(to_bottom);
            if min == to_left {
                ResizeHandle::Left
            } else if min == to_right {
                ResizeHandle::Right
            } else if min == to_top {
                ResizeHandle::Top
            } else {
                ResizeHandle::Bottom
            }
        }
    }
}

/// The size to request, given where the cursor has been dragged to. Measured
/// against the grab-start rect rather than accumulated per event, so the
/// window can't drift away from the cursor over a long drag.
pub fn resize_target_size(
    handle: ResizeHandle,
    start_rect: Rectangle<i32, Logical>,
    start_cursor: Vec2,
    world_cursor: Vec2,
) -> Size<i32, Logical> {
    let dx = (world_cursor.x - start_cursor.x).round() as i32;
    let dy = (world_cursor.y - start_cursor.y).round() as i32;

    let mut width = start_rect.size.w;
    let mut height = start_rect.size.h;
    if handle.moves_left() {
        width -= dx;
    }
    if handle.moves_right() {
        width += dx;
    }
    if handle.moves_top() {
        height -= dy;
    }
    if handle.moves_bottom() {
        height += dy;
    }

    Size::from((width.max(MIN_RESIZE_W), height.max(MIN_RESIZE_H)))
}

/// Repositions a window after its client commits a resize, preserving the
/// edge opposite the grab. Using committed sizes here is important: clients
/// can respond asynchronously or quantize a requested size, so anchoring
/// against the request makes left/top resizes visibly jump.
///
/// The correction is `pos += previous_size - committed_size` for the
/// left/top edges only, evaluated once per commit against the size the
/// client actually landed on.
pub fn resize_location_after_commit(
    handle: ResizeHandle,
    current_location: Point<i32, Logical>,
    previous_size: Size<i32, Logical>,
    committed_size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    let x = if handle.moves_left() {
        current_location.x + previous_size.w - committed_size.w
    } else {
        current_location.x
    };
    let y = if handle.moves_top() {
        current_location.y + previous_size.h - committed_size.h
    } else {
        current_location.y
    };
    Point::from((x, y))
}

/// Call from `CompositorHandler::commit`, after dispatching to Smithay.
/// Applies the anchoring correction against the size this code last saw,
/// re-arms that size for the next commit, then retires the anchor if this was
/// the commit a released drag was waiting on.
///
/// Not scoped to the surface that actually committed: a window's geometry
/// only changes on its own commit, so for any other surface the size is
/// unchanged and the correction works out to zero.
///
/// The retirement check runs *after* the correction so the final commit of
/// a drag is still anchored.
pub fn finish_resize_commit(anchor: &mut Option<ResizeAnchor>, space: &mut Space<Window>) {
    let Some(resize) = anchor.as_mut() else {
        return;
    };

    if let Some(committed) = space.element_geometry(&resize.window)
        && let Some(location) = space.element_location(&resize.window)
    {
        let location =
            resize_location_after_commit(resize.handle, location, resize.last_size, committed.size);
        resize.last_size = committed.size;
        let window = resize.window.clone();
        space.relocate_element(&window, location);
    }

    let retire = anchor.as_ref().is_some_and(|resize| {
        anchor_is_retired(&resize.phase, committed_configure_serial(&resize.window))
    });
    if retire {
        *anchor = None;
    }
}

/// Serial of the configure the client had acked as of the commit currently
/// being processed - i.e. which resize request this frame is the answer to.
fn committed_configure_serial(window: &Window) -> Option<Serial> {
    window
        .toplevel()?
        .with_cached_state(|state| state.last_acked.as_ref().map(|configure| configure.serial))
}

/// Converts a screen-space (physical-pixel) position into world (`Space`)
/// coordinates, given the camera's live center/zoom state and the output's
/// physical size - the exact inverse of `render::camera_rect`'s transform.
/// Needed because the pointer's tracked position is always in screen
/// coordinates, but window positions and grab math need to be in world
/// coordinates now that panning is real.
#[cfg(test)]
fn screen_to_world(screen: (f64, f64), camera: &Camera, output_size: Size<i32, Physical>) -> Vec2 {
    let output_center_x = output_size.w as f32 / 2.0;
    let output_center_y = output_size.h as f32 / 2.0;
    let scale = crate::input::zoom::scale(camera);
    Vec2 {
        x: camera.center.x + (screen.0 as f32 - output_center_x) / scale,
        y: camera.center.y + (screen.1 as f32 - output_center_y) / scale,
    }
}

/// Multi-output form of `screen_to_world`. The pointer is global in
/// Smithay's mapped output layout, while each camera is local to its output.
/// Re-basing that output's pan/zoom into the global layout keeps render and
/// hit-test transforms exact inverses.
pub fn screen_to_world_on_output(
    screen: (f64, f64),
    camera: &Camera,
    output_geometry: Rectangle<i32, Logical>,
) -> Vec2 {
    let local_center = Vec2 {
        x: output_geometry.size.w as f32 / 2.0,
        y: output_geometry.size.h as f32 / 2.0,
    };
    let pan = Vec2 {
        x: camera.center.x - local_center.x,
        y: camera.center.y - local_center.y,
    };
    let output_center = Vec2 {
        x: output_geometry.loc.x as f32 + output_geometry.size.w as f32 / 2.0,
        y: output_geometry.loc.y as f32 + output_geometry.size.h as f32 / 2.0,
    };
    let local_screen = Vec2 {
        x: screen.0 as f32 - output_geometry.loc.x as f32,
        y: screen.1 as f32 - output_geometry.loc.y as f32,
    };
    let scale = crate::input::zoom::scale(camera);

    Vec2 {
        x: output_center.x + pan.x + (local_screen.x - local_center.x) / scale,
        y: output_center.y + pan.y + (local_screen.y - local_center.y) / scale,
    }
}

/// Converts a screen-space motion delta into a world-space delta - used
/// while panning, where only the delta matters (not an absolute position).
/// Scaled by the same factor `screen_to_world` uses, so panning speed stays
/// 1:1 with cursor motion regardless of the current zoom level (matches old
/// halley's own reason for scaling pan deltas by view-size/output-size).
pub fn screen_delta_to_world(dx: f64, dy: f64, camera: &Camera) -> Vec2 {
    let scale = crate::input::zoom::scale(camera);
    Vec2 {
        x: dx as f32 / scale,
        y: dy as f32 / scale,
    }
}

pub fn screen_offset_to_world(offset: Vec2, camera: &Camera) -> Vec2 {
    let scale = crate::input::zoom::scale(camera);
    Vec2 {
        x: offset.x / scale,
        y: offset.y / scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_pan_chord_preserves_the_original_button_and_output() {
        let mut grab = Grab::None;
        assert!(grab.try_begin_pan("DP-1".into(), 0x112));
        assert!(!grab.try_begin_pan("DP-2".into(), 0x110));
        assert!(matches!(grab, Grab::Pan { ref output, button: 0x112 } if output == "DP-1"));
        // Once the original owner finishes, a new chord may capture another
        // output and button instead of inheriting the rejected chord's state.
        grab = Grab::None;
        assert!(grab.try_begin_pan("DP-2".into(), 0x110));
        assert!(matches!(grab, Grab::Pan { ref output, button: 0x110 } if output == "DP-2"));
    }

    #[test]
    fn pan_chord_does_not_replace_a_compositor_landmark_drag() {
        let mut grab = Grab::MoveClusterCore {
            id: halley_core::cluster::ClusterId::new(7),
            screen_offset: Vec2 { x: 12.0, y: 34.0 },
        };
        assert!(!grab.try_begin_pan("DP-2".into(), 0x112));
        assert!(matches!(grab, Grab::MoveClusterCore { id, screen_offset }
            if id == halley_core::cluster::ClusterId::new(7)
                && screen_offset == Vec2 { x: 12.0, y: 34.0 }));
    }

    #[cfg(feature = "xwayland")]
    #[test]
    #[ignore = "requires an X server in DISPLAY to resolve Smithay's atoms"]
    fn pan_chord_preserves_pending_move_window_move_and_resize_cleanup_state() {
        use smithay::xwayland::{X11Surface, xwm::Atoms};
        use std::sync::{Arc, Weak, atomic::AtomicBool};

        // A real Smithay Window with no live X window or compositor session.
        let (connection, _) = x11rb::connect(None).expect("connect to DISPLAY");
        let atoms = Atoms::new(&connection).unwrap().reply().unwrap();
        let rect = Rectangle::new((10, 20).into(), (640, 480).into());
        let window = Window::new_x11_window(X11Surface::new(
            None,
            0,
            false,
            Weak::new(),
            atoms,
            None,
            rect,
            Arc::new(AtomicBool::new(false)),
        ));
        let button = 0x110;
        let serial = Serial::from(42);
        let offset = Vec2 { x: 12.0, y: 34.0 };
        let now = Duration::from_millis(100);
        let mut grabs = [
            Grab::PendingWindowMove(PendingWindowMove {
                window: window.clone(),
                serial,
                button,
                press_screen: (15.0, 25.0).into(),
                output: "DP-1".into(),
                visual_geometry: rect,
                maximized: false,
                client_owned: true,
            }),
            Grab::MoveWindow {
                id: Some(halley_core::field::NodeId::new(5)),
                window: window.clone(),
                cluster_drag: None,
                drag_size: None,
                button,
                client_owned: true,
                anchor: WindowGrabAnchor::Source(offset),
                edge_pan: None,
                last_world: offset,
                last_update: now,
                velocity: offset,
            },
            Grab::ResizeWindow(ResizeState {
                window: window.clone(),
                handle: ResizeHandle::BottomRight,
                button,
                start_rect: rect,
                start_cursor: offset,
                start_screen: (15.0, 25.0),
                screen_to_source_scale: Vec2 { x: 1.0, y: 1.0 },
                target_size: (700, 500).into(),
                preview_size: offset,
                last_smooth_tick: now,
            }),
        ];
        for grab in &mut grabs {
            assert!(!grab.try_begin_pan("DP-2".into(), 0x112));
            match grab {
                Grab::PendingWindowMove(pending) => {
                    assert_eq!(pending.window, window);
                    assert_eq!(pending.button, button);
                    assert_eq!(pending.serial, serial);
                    assert_eq!(pending.output, "DP-1");
                    assert!(pending.client_owned);
                }
                Grab::MoveWindow {
                    window: owner,
                    button: owner_button,
                    client_owned,
                    anchor,
                    last_update,
                    ..
                } => {
                    assert_eq!(*owner, window);
                    assert_eq!(*owner_button, button);
                    assert!(*client_owned);
                    assert_eq!(*anchor, WindowGrabAnchor::Source(offset));
                    assert_eq!(*last_update, now);
                }
                Grab::ResizeWindow(resize) => {
                    assert_eq!(resize.window, window);
                    assert_eq!(resize.button, button);
                    assert_eq!(resize.handle, ResizeHandle::BottomRight);
                    assert_eq!(resize.start_rect, rect);
                    assert_eq!(resize.target_size, Size::from((700, 500)));
                }
                _ => panic!("the pan chord replaced the original operation"),
            }
            // Finishing the original operation allows the next pan chord.
            *grab = Grab::None;
            assert!(grab.try_begin_pan("DP-2".into(), 0x112));
        }
    }

    #[test]
    fn resize_handles_map_to_directional_cursor_icons() {
        use smithay::input::pointer::CursorIcon;
        assert_eq!(ResizeHandle::Left.cursor_icon(), CursorIcon::WResize);
        assert_eq!(ResizeHandle::Right.cursor_icon(), CursorIcon::EResize);
        assert_eq!(ResizeHandle::Top.cursor_icon(), CursorIcon::NResize);
        assert_eq!(ResizeHandle::Bottom.cursor_icon(), CursorIcon::SResize);
        assert_eq!(ResizeHandle::TopLeft.cursor_icon(), CursorIcon::NwResize);
        assert_eq!(ResizeHandle::TopRight.cursor_icon(), CursorIcon::NeResize);
        assert_eq!(ResizeHandle::BottomLeft.cursor_icon(), CursorIcon::SwResize);
        assert_eq!(
            ResizeHandle::BottomRight.cursor_icon(),
            CursorIcon::SeResize
        );
    }

    #[test]
    fn smooth_resize_low_pass_advances_without_overshooting() {
        let first = smooth_resize_value(800.0, 1200.0, Duration::from_millis(16), true, 90);
        let second = smooth_resize_value(first, 1200.0, Duration::from_millis(16), true, 90);
        assert!(first > 800.0 && first < 1200.0);
        assert!(second > first && second < 1200.0);
        assert_eq!(
            smooth_resize_value(first, 1200.0, Duration::ZERO, false, 90),
            1200.0
        );
    }

    fn camera_at_rest() -> Camera {
        Camera::new(
            Vec2 { x: 640.0, y: 400.0 },
            Vec2 {
                x: 1280.0,
                y: 800.0,
            },
        )
    }

    #[test]
    fn window_edge_pan_waits_for_dwell_and_resets_on_direction_change() {
        let start = Duration::from_secs(5);
        let mut pan = WindowEdgePan::new("DP-1".into(), start);
        pan.update_contact(Vec2 { x: 1.0, y: 0.0 }, start);
        assert!(!pan.ready(start + WINDOW_EDGE_PAN_DWELL - Duration::from_millis(1)));
        assert!(pan.ready(start + WINDOW_EDGE_PAN_DWELL));

        let changed = start + Duration::from_secs(1);
        pan.update_contact(Vec2 { x: -1.0, y: 0.0 }, changed);
        assert!(!pan.ready(changed));
        assert!(pan.ready(changed + WINDOW_EDGE_PAN_DWELL));

        pan.update_contact(Vec2 { x: 0.0, y: 0.0 }, changed + Duration::from_secs(1));
        assert_eq!(pan.contact_since, None);
        assert!(!pan.ready(changed + Duration::from_secs(2)));
    }

    #[test]
    fn window_edge_pan_clamps_complete_frame_at_right_edge() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1280, 800).into());
        let placement = window_edge_pan_placement(
            (1279.0, 400.0),
            WindowGrabAnchor::Source(Vec2 {
                x: -100.0,
                y: -50.0,
            }),
            Size::from((200, 100)),
            (3, 3, 32, 3),
            &camera_at_rest(),
            output,
        )
        .expect("decorated window fits in the camera view");

        assert_eq!(placement.location, Point::from((1077, 350)));
        assert_eq!(
            placement.center,
            Vec2 {
                x: 1177.0,
                y: 400.0
            }
        );
        assert_eq!(placement.contact, Vec2 { x: 1.0, y: 0.0 });
    }

    #[test]
    fn window_edge_pan_keeps_titlebar_visible_at_top_edge() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1280, 800).into());
        let placement = window_edge_pan_placement(
            (640.0, 0.0),
            WindowGrabAnchor::Source(Vec2 {
                x: -100.0,
                y: -50.0,
            }),
            Size::from((200, 100)),
            (3, 3, 32, 3),
            &camera_at_rest(),
            output,
        )
        .expect("decorated window fits in the camera view");

        assert_eq!(placement.location, Point::from((540, 32)));
        assert_eq!(placement.center, Vec2 { x: 640.0, y: 82.0 });
        assert_eq!(placement.contact, Vec2 { x: 0.0, y: -1.0 });
        assert_eq!(placement.location.y - 32, output.loc.y);
    }

    #[test]
    fn window_edge_pan_rejects_frame_larger_than_view() {
        let output = Rectangle::<i32, Logical>::new((0, 0).into(), (1280, 800).into());
        assert!(
            window_edge_pan_placement(
                (640.0, 400.0),
                WindowGrabAnchor::Source(Vec2 { x: 0.0, y: 0.0 }),
                Size::from((1276, 100)),
                (3, 3, 32, 3),
                &camera_at_rest(),
                output,
            )
            .is_none()
        );
    }

    #[test]
    fn screen_to_world_is_identity_at_rest() {
        let camera = camera_at_rest();
        let output_size = Size::<i32, Physical>::from((1280, 800));
        // Output center maps to camera center (world origin here).
        assert_eq!(
            screen_to_world((640.0, 400.0), &camera, output_size),
            Vec2 { x: 640.0, y: 400.0 }
        );
        // A screen point 100px right/50px down of center is the same world
        // offset when nothing is panned or zoomed.
        let world = screen_to_world((740.0, 450.0), &camera, output_size);
        assert_eq!(world, Vec2 { x: 740.0, y: 450.0 });
    }

    #[test]
    fn screen_to_world_accounts_for_pan() {
        let mut camera = camera_at_rest();
        camera.center = Vec2 { x: 740.0, y: 400.0 };
        let output_size = Size::<i32, Physical>::from((1280, 800));
        // Output center now maps to the panned camera center, not the
        // original world origin.
        assert_eq!(
            screen_to_world((640.0, 400.0), &camera, output_size),
            Vec2 { x: 740.0, y: 400.0 }
        );
    }

    #[test]
    fn screen_to_world_accounts_for_zoom() {
        let mut camera = camera_at_rest();
        // Zoomed out to half scale (view_size double base_size).
        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        let output_size = Size::<i32, Physical>::from((1280, 800));
        // At 0.5x scale, a 100px screen offset is a 200px world offset.
        let world = screen_to_world((740.0, 400.0), &camera, output_size);
        assert_eq!(world, Vec2 { x: 840.0, y: 400.0 });
    }

    #[test]
    fn secondary_output_coordinates_are_global_at_rest() {
        let secondary = Rectangle::<i32, Logical>::new((1280, 0).into(), (1920, 1200).into());
        let camera = Camera::new(
            Vec2 { x: 960.0, y: 600.0 },
            Vec2 {
                x: 1920.0,
                y: 1200.0,
            },
        );

        assert_eq!(
            screen_to_world_on_output((1280.0, 0.0), &camera, secondary),
            Vec2 { x: 1280.0, y: 0.0 }
        );
        assert_eq!(
            screen_to_world_on_output((1600.0, 600.0), &camera, secondary),
            Vec2 {
                x: 1600.0,
                y: 600.0
            }
        );
    }

    #[test]
    fn secondary_output_coordinates_use_its_own_pan_and_zoom() {
        let mut camera = Camera::new(
            Vec2 { x: 960.0, y: 600.0 },
            Vec2 {
                x: 1920.0,
                y: 1200.0,
            },
        );
        camera.center = Vec2 {
            x: 1060.0,
            y: 650.0,
        };
        camera.view_size = Vec2 {
            x: 3840.0,
            y: 2400.0,
        };
        let secondary = Rectangle::<i32, Logical>::new((1280, 0).into(), (1920, 1200).into());

        assert_eq!(
            screen_to_world_on_output((2340.0, 650.0), &camera, secondary),
            Vec2 {
                x: 2540.0,
                y: 750.0
            }
        );
    }

    fn resize_rect() -> Rectangle<i32, Logical> {
        Rectangle::new((100, 100).into(), (300, 300).into())
    }

    #[test]
    fn press_position_picks_the_grabbed_corner() {
        let rect = resize_rect();
        // 3x3 grid over (100,100)-(400,400): thirds fall at 200 and 300.
        assert_eq!(
            handle_from_press_position(rect, Vec2 { x: 120.0, y: 120.0 }),
            ResizeHandle::TopLeft
        );
        assert_eq!(
            handle_from_press_position(rect, Vec2 { x: 380.0, y: 380.0 }),
            ResizeHandle::BottomRight
        );
        assert_eq!(
            handle_from_press_position(rect, Vec2 { x: 380.0, y: 120.0 }),
            ResizeHandle::TopRight
        );
        // Middle column, top row -> the top edge, not a corner.
        assert_eq!(
            handle_from_press_position(rect, Vec2 { x: 250.0, y: 120.0 }),
            ResizeHandle::Top
        );
    }

    #[test]
    fn border_band_picks_edges_and_corners_but_not_the_center_or_outside() {
        let rect = resize_rect();
        let point = |x, y| Point::<f64, Logical>::from((x, y));

        assert_eq!(
            border_resize_handle(rect, point(102.0, 250.0), 8.0),
            Some(ResizeHandle::Left)
        );
        assert_eq!(
            border_resize_handle(rect, point(398.0, 250.0), 8.0),
            Some(ResizeHandle::Right)
        );
        assert_eq!(
            border_resize_handle(rect, point(250.0, 102.0), 8.0),
            Some(ResizeHandle::Top)
        );
        assert_eq!(
            border_resize_handle(rect, point(250.0, 398.0), 8.0),
            Some(ResizeHandle::Bottom)
        );
        assert_eq!(
            border_resize_handle(rect, point(103.0, 103.0), 8.0),
            Some(ResizeHandle::TopLeft)
        );
        assert_eq!(
            border_resize_handle(rect, point(397.0, 397.0), 8.0),
            Some(ResizeHandle::BottomRight)
        );
        assert_eq!(border_resize_handle(rect, point(250.0, 250.0), 8.0), None);
        assert_eq!(border_resize_handle(rect, point(99.0, 250.0), 8.0), None);
    }

    #[test]
    fn border_band_expands_to_the_rendered_border_width() {
        let rect = resize_rect();
        let point = Point::<f64, Logical>::from((115.0, 250.0));
        assert_eq!(border_resize_handle(rect, point, 8.0), None);
        assert_eq!(
            border_resize_handle(rect, point, 16.0),
            Some(ResizeHandle::Left)
        );
    }

    #[test]
    fn dead_center_press_falls_back_to_the_nearest_edge() {
        // Exact center of a square window is equidistant - resolves to the
        // first edge checked rather than panicking or picking a corner.
        let handle = handle_from_press_position(resize_rect(), Vec2 { x: 250.0, y: 250.0 });
        assert_eq!(handle, ResizeHandle::Left);
    }

    #[test]
    fn dragging_bottom_right_grows_without_moving_the_window() {
        let rect = resize_rect();
        let start = Vec2 { x: 400.0, y: 400.0 };
        let size = resize_target_size(
            ResizeHandle::BottomRight,
            rect,
            start,
            Vec2 { x: 450.0, y: 430.0 },
        );
        assert_eq!(size, Size::from((350, 330)));
        // Right/bottom drags anchor the top-left, so the window stays put.
        assert_eq!(
            resize_location_after_commit(ResizeHandle::BottomRight, rect.loc, rect.size, size,),
            Point::from((100, 100))
        );
    }

    #[test]
    fn output_local_floating_resize_keeps_screen_and_source_deltas_aligned() {
        let source = Rectangle::new((100, 50).into(), (2_000, 1_000).into());
        let visual = Rectangle::new((0, 0).into(), (1_000, 500).into());
        let scale = resize_screen_to_source_scale(source, visual);
        assert_eq!(scale, Vec2 { x: 2.0, y: 2.0 });

        let cursor = source_cursor_from_screen(
            Vec2 {
                x: 2_100.0,
                y: 1_050.0,
            },
            (1_000.0, 500.0),
            scale,
            (900.0, 450.0),
        );
        assert_eq!(
            cursor,
            Vec2 {
                x: 1_900.0,
                y: 950.0
            }
        );
        assert_eq!(
            resize_target_size(
                ResizeHandle::BottomRight,
                source,
                Vec2 {
                    x: 2_100.0,
                    y: 1_050.0
                },
                cursor
            ),
            Size::from((1_800, 900))
        );
    }

    #[test]
    fn dragging_top_left_moves_the_window_to_keep_the_far_corner_fixed() {
        let rect = resize_rect();
        let start = Vec2 { x: 100.0, y: 100.0 };
        // Drag the top-left corner up and left by 50 - the window grows by
        // 50 in each axis and its origin moves back by the same amount, so
        // the bottom-right corner stays at (400, 400).
        let size = resize_target_size(
            ResizeHandle::TopLeft,
            rect,
            start,
            Vec2 { x: 50.0, y: 50.0 },
        );
        assert_eq!(size, Size::from((350, 350)));
        let loc = resize_location_after_commit(ResizeHandle::TopLeft, rect.loc, rect.size, size);
        assert_eq!(loc, Point::from((50, 50)));
        assert_eq!((loc.x + size.w, loc.y + size.h), (400, 400));
    }

    #[test]
    fn resize_clamps_to_the_minimum_and_stops_anchoring_past_it() {
        let rect = resize_rect();
        let start = Vec2 { x: 100.0, y: 100.0 };
        // Drag far past the opposite corner - size floors instead of going
        // negative or inverting the rect, and the anchored corner holds.
        let size = resize_target_size(
            ResizeHandle::TopLeft,
            rect,
            start,
            Vec2 {
                x: 9000.0,
                y: 9000.0,
            },
        );
        assert_eq!(size, Size::from((MIN_RESIZE_W, MIN_RESIZE_H)));
        let loc = resize_location_after_commit(ResizeHandle::TopLeft, rect.loc, rect.size, size);
        assert_eq!((loc.x + size.w, loc.y + size.h), (400, 400));
    }

    #[test]
    fn commit_size_is_authoritative_for_left_edge_anchoring() {
        let previous_size = Size::from((300, 300));
        // A terminal may commit 344 px after receiving a 350 px request.
        let committed_size = Size::from((344, 300));
        let location = resize_location_after_commit(
            ResizeHandle::Left,
            Point::from((100, 100)),
            previous_size,
            committed_size,
        );

        assert_eq!(location, Point::from((56, 100)));
        assert_eq!(location.x + committed_size.w, 400);
    }

    #[test]
    fn anchoring_diffs_against_the_tracked_size_not_the_committed_one() {
        // Regression: the "before" size used to be re-read from the surface
        // inside the commit handler, but Smithay has already swapped the new
        // geometry in by then - so every correction came out zero and a
        // left-edge drag dragged the *right* edge instead. Walk a drag as a
        // run of commits and hold the compositor's own tracked size.
        let mut location = Point::from((100, 100));
        let mut tracked = Size::from((300, 300));

        for committed in [
            Size::from((280, 300)),
            Size::from((264, 300)),
            Size::from((248, 300)),
        ] {
            location = resize_location_after_commit(
                ResizeHandle::BottomLeft,
                location,
                tracked,
                committed,
            );
            tracked = committed;
            // The un-dragged right edge holds at its original 400 throughout.
            assert_eq!(location.x + committed.w, 400);
            assert_eq!(location.y, 100);
        }

        // Feeding the committed size in as the "before" side - the bug - moves
        // nothing, leaving the right edge to walk in with the left one.
        let stuck = resize_location_after_commit(
            ResizeHandle::BottomLeft,
            Point::from((100, 100)),
            Size::from((248, 300)),
            Size::from((248, 300)),
        );
        assert_eq!(stuck, Point::from((100, 100)));
        assert_ne!(stuck.x + 248, 400);
    }

    #[test]
    fn releasing_a_drag_keeps_anchoring_until_the_last_configure_is_answered() {
        // Button up with a configure still in flight: the anchor has to
        // survive, or the client's final commit lands unanchored and the
        // window snaps sideways at the end of every left/top drag.
        let phase = released_phase(Some(Serial::from(7)))
            .expect("an outstanding configure keeps the anchor alive");

        assert!(!anchor_is_retired(&phase, None));
        assert!(!anchor_is_retired(&phase, Some(Serial::from(6))));
        assert!(anchor_is_retired(&phase, Some(Serial::from(7))));
        // Clients may skip ahead; a newer ack covers the one being waited on.
        assert!(anchor_is_retired(&phase, Some(Serial::from(9))));
    }

    #[test]
    fn releasing_a_drag_that_never_configured_retires_the_anchor() {
        assert!(released_phase(None).is_none());
    }

    #[test]
    fn an_ongoing_drag_is_never_retired_by_a_commit() {
        assert!(!anchor_is_retired(
            &ResizePhase::Ongoing,
            Some(Serial::from(9000))
        ));
    }

    #[test]
    fn screen_delta_to_world_scales_by_zoom() {
        let mut camera = camera_at_rest();
        assert_eq!(
            screen_delta_to_world(100.0, 50.0, &camera),
            Vec2 { x: 100.0, y: 50.0 }
        );

        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        assert_eq!(
            screen_delta_to_world(100.0, 50.0, &camera),
            Vec2 { x: 200.0, y: 100.0 }
        );
    }

    #[test]
    fn screen_grab_offset_stays_visually_fixed_across_zoom_levels() {
        let mut camera = camera_at_rest();
        let offset = Vec2 {
            x: -200.0,
            y: -80.0,
        };
        assert_eq!(
            screen_offset_to_world(offset, &camera),
            Vec2 {
                x: -200.0,
                y: -80.0
            }
        );

        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        assert_eq!(
            screen_offset_to_world(offset, &camera),
            Vec2 {
                x: -400.0,
                y: -160.0
            }
        );
    }

    #[test]
    fn screen_grip_keeps_its_visual_point_through_field_camera_conversion() {
        let pointer = (460.0, 280.0);
        let visual_location = Point::from((320, 180));
        let offset = screen_grip_offset(pointer, visual_location);
        let output = Rectangle::new((0, 0).into(), (1280, 800).into());
        let mut camera = camera_at_rest();

        assert_eq!(
            offset,
            Vec2 {
                x: -140.0,
                y: -100.0
            }
        );
        assert_eq!(
            world_location_from_screen_grip(pointer, offset, &camera, output),
            visual_location
        );

        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        assert_eq!(
            world_location_from_screen_grip(pointer, offset, &camera, output),
            Point::from((0, -40))
        );
    }

    #[test]
    fn source_grip_stays_inside_a_window_when_destination_zoom_shrinks_it() {
        let source_offset = Vec2 {
            x: -900.0,
            y: -400.0,
        };
        let anchor = WindowGrabAnchor::Source(source_offset);
        let output = Rectangle::new((1280, 0).into(), (1280, 800).into());
        let mut camera = camera_at_rest();
        camera.center = Vec2 { x: 740.0, y: 450.0 };
        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        let pointer = (2200.0, 500.0);

        let location = anchor.world_location(pointer, &camera, output);
        let visual_offset = anchor.screen_offset(&camera);
        let world = screen_to_world_on_output(pointer, &camera, output);

        assert_eq!(
            location,
            Point::from((
                (world.x + source_offset.x).round() as i32,
                (world.y + source_offset.y).round() as i32,
            ))
        );
        assert_eq!(
            visual_offset,
            Vec2 {
                x: -450.0,
                y: -200.0,
            }
        );
        // The same logical point remains under the cursor: a grip 900 units
        // into a 1000-unit-wide window becomes 450 px into its 500 px visual.
        assert!(-visual_offset.x < 1000.0 * crate::input::zoom::scale(&camera));
        assert!(-visual_offset.y < 500.0 * crate::input::zoom::scale(&camera));
    }

    #[test]
    fn source_and_screen_anchors_scale_differently_across_outputs() {
        let mut camera = camera_at_rest();
        camera.view_size = Vec2 {
            x: 2560.0,
            y: 1600.0,
        };
        let offset = Vec2 {
            x: -300.0,
            y: -120.0,
        };

        assert_eq!(
            WindowGrabAnchor::Source(offset).screen_offset(&camera),
            Vec2 {
                x: -150.0,
                y: -60.0,
            }
        );
        assert_eq!(
            WindowGrabAnchor::Screen(offset).screen_offset(&camera),
            offset
        );
    }
}
