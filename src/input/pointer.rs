use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, InputBackend, InputEvent, PointerAxisEvent,
    PointerMotionEvent,
};
use smithay::desktop::space::SpaceElement;
use smithay::desktop::{LayerSurface, Space, Window, WindowSurfaceType, layer_map_for_output};
use smithay::input::pointer::AxisFrame;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::{RegionAttributes, SurfaceAttributes, with_states};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::Layer;

use crate::input::keybinds::WheelDirection;
use crate::presentation::camera::OutputCameras;
use crate::presentation::window::WindowPresentation;

#[derive(Debug)]
pub enum PointerTarget {
    Layer(LayerSurface),
    Window(Window),
    Decoration {
        window: Window,
        hit: crate::titlebar::Hit,
    },
    Background,
}

pub type SurfaceFocus = (WlSurface, Point<f64, Logical>);
type LayerHit = (LayerSurface, SurfaceFocus);

/// A pointer route in the coordinate system used by its focused surface.
///
/// Camera-transformed windows use world coordinates so their client-local
/// position remains unscaled. Screen-fixed layer surfaces use global output
/// layout coordinates. In both cases `location` and the focus origin share
/// one coordinate system, which is Smithay's required invariant.
#[derive(Debug)]
pub struct PointerRoute {
    pub output: Output,
    pub location: Point<f64, Logical>,
    pub focus: Option<SurfaceFocus>,
    pub target: PointerTarget,
    pub visual_geometry: Option<Rectangle<i32, Logical>>,
    /// True when this route landed in a client popup promoted above the desktop
    /// top layer. Desktop landmarks must not intercept that promoted surface.
    pub is_desktop_popup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowHitKind {
    Popup,
    Any,
}

#[derive(Clone, Copy)]
pub struct PointerRoutingContext<'a> {
    pub space: &'a Space<Window>,
    pub cameras: &'a OutputCameras,
    pub clusters: &'a crate::clusters::ClusterSystem,
    pub nodes: &'a crate::nodes::NodesState,
    pub window_animations: &'a crate::animation::WindowAnimations,
    pub primary: &'a Output,
    pub fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    pub maximize: &'a crate::presentation::maximize::FieldMaximizeManager,
    pub decorations: &'a halley_config::Decorations,
    pub font: &'a halley_config::Font,
    pub focused: Option<&'a WlSurface>,
    pub now: std::time::Duration,
}

/// Screen-space cursor tracking. Client-facing focus, buttons, implicit
/// grabs, and axes are owned by Smithay's `PointerHandle`; this type only
/// tracks where Halley draws the hardware/software cursor.
pub struct Pointer {
    position: (f64, f64),
}

const WHEEL_V120_PER_TICK: f64 = 120.0;

/// Accumulates high-resolution physical wheel deltas into discrete notches.
/// Each axis is independent, and reversing direction drops the unfinished
/// fraction from the previous direction.
#[derive(Debug, Default)]
pub struct WheelAccumulator {
    horizontal: f64,
    vertical: f64,
}

impl WheelAccumulator {
    pub fn accumulate(&mut self, axis: Axis, delta_v120: f64) -> i32 {
        let pending = match axis {
            Axis::Horizontal => &mut self.horizontal,
            Axis::Vertical => &mut self.vertical,
        };
        if *pending != 0.0 && delta_v120 != 0.0 && pending.signum() != delta_v120.signum() {
            *pending = 0.0;
        }
        *pending += delta_v120;
        let ticks = (*pending / WHEEL_V120_PER_TICK).trunc() as i32;
        *pending -= f64::from(ticks) * WHEEL_V120_PER_TICK;
        ticks
    }

    pub fn reset(&mut self, axis: Axis) {
        match axis {
            Axis::Horizontal => self.horizontal = 0.0,
            Axis::Vertical => self.vertical = 0.0,
        }
    }

    pub fn reset_all(&mut self) {
        self.horizontal = 0.0;
        self.vertical = 0.0;
    }
}

fn wheel_delta_v120<B, E>(event: &E, axis: Axis) -> f64
where
    B: InputBackend,
    E: PointerAxisEvent<B>,
{
    event
        .amount_v120(axis)
        .or_else(|| {
            event
                .amount(axis)
                .map(|amount| amount * WHEEL_V120_PER_TICK / 15.0)
        })
        .unwrap_or(0.0)
}

fn wheel_direction(axis: Axis, delta_v120: f64) -> Option<WheelDirection> {
    match (axis, delta_v120.total_cmp(&0.0)) {
        (_, std::cmp::Ordering::Equal) => None,
        (Axis::Horizontal, std::cmp::Ordering::Less) => Some(WheelDirection::Left),
        (Axis::Horizontal, std::cmp::Ordering::Greater) => Some(WheelDirection::Right),
        (Axis::Vertical, std::cmp::Ordering::Less) => Some(WheelDirection::Up),
        (Axis::Vertical, std::cmp::Ordering::Greater) => Some(WheelDirection::Down),
    }
}

/// Backend-independent result of applying configured wheel bindings to one
/// axis event. Sessions execute the returned actions and forward only the
/// axes left enabled here.
pub struct WheelBindingResult<T> {
    pub forward_horizontal: bool,
    pub forward_vertical: bool,
    pub actions: Vec<(WheelDirection, T)>,
}

impl<T> Default for WheelBindingResult<T> {
    fn default() -> Self {
        Self {
            forward_horizontal: true,
            forward_vertical: true,
            actions: Vec::new(),
        }
    }
}

/// Applies physical-wheel source filtering, direction matching, high-
/// resolution accumulation, and per-axis forwarding policy in one place.
/// The callback keeps this low-level policy independent of Halley's action
/// type and bind-table representation.
pub fn process_wheel_bindings<B, E, T, F>(
    event: &E,
    accumulator: &mut WheelAccumulator,
    bindings_enabled: bool,
    mut action_for_direction: F,
) -> WheelBindingResult<T>
where
    B: InputBackend,
    E: PointerAxisEvent<B>,
    T: Clone,
    F: FnMut(WheelDirection) -> Option<T>,
{
    let mut result = WheelBindingResult::default();
    if event.source() != AxisSource::Wheel || !bindings_enabled {
        accumulator.reset_all();
        return result;
    }

    for axis in [Axis::Horizontal, Axis::Vertical] {
        let delta = wheel_delta_v120(event, axis);
        let Some(direction) = wheel_direction(axis, delta) else {
            continue;
        };
        let Some(action) = action_for_direction(direction) else {
            accumulator.reset(axis);
            continue;
        };

        match axis {
            Axis::Horizontal => result.forward_horizontal = false,
            Axis::Vertical => result.forward_vertical = false,
        }
        let ticks = accumulator.accumulate(axis, delta);
        for _ in 0..ticks.unsigned_abs() {
            result.actions.push((direction, action.clone()));
        }
    }
    result
}

impl Pointer {
    pub fn new(initial: (f64, f64)) -> Self {
        Self { position: initial }
    }

    pub fn position(&self) -> (f64, f64) {
        self.position
    }

    pub fn set_position(&mut self, position: (f64, f64)) {
        self.position = position;
    }

    /// Absolute events (winit's host window mouse, or absolute-mode tty
    /// devices like touchpads/tablets) set the position directly; relative
    /// events (a typical tty/libinput mouse) accumulate a delta. Both are
    /// kept in Smithay's global logical output-layout coordinates and
    /// constrained to mapped output geometries.
    pub fn process_input_event<I: InputBackend>(
        &mut self,
        event: &InputEvent<I>,
        space: &Space<Window>,
    ) {
        if !matches!(
            event,
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. }
        ) {
            return;
        }

        let outputs: Vec<_> = space
            .outputs()
            .filter_map(|output| space.output_geometry(output))
            .collect();

        match event {
            InputEvent::PointerMotion { event } => {
                let delta = event.delta();
                self.position = clamp_to_outputs(
                    (self.position.0 + delta.x, self.position.1 + delta.y),
                    &outputs,
                );
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(bounds) = desktop_bounds(&outputs) else {
                    return;
                };
                let pos = event.position_transformed(bounds.size) + bounds.loc.to_f64();
                self.position = clamp_to_outputs((pos.x, pos.y), &outputs);
            }
            _ => {}
        }
    }
}

/// Finds the client surface visually under `screen_position`.
///
/// Window ownership is output-local, matching rendering and compositor grab
/// hit-testing: a window assigned to another output cannot intercept input
/// merely because its world-space geometry overlaps this camera's view.
pub fn route_to_client(
    context: PointerRoutingContext<'_>,
    screen_position: (f64, f64),
) -> Option<PointerRoute> {
    let output = context.space.output_under(screen_position).next()?;
    let output_geometry = context.space.output_geometry(output)?;
    let screen_location = Point::<f64, Logical>::from(screen_position);
    let output_local = screen_location - output_geometry.loc.to_f64();

    if let Some((layer, focus)) =
        layer_under(output, output_geometry.loc, output_local, [Layer::Overlay])
    {
        return Some(PointerRoute {
            output: output.clone(),
            location: screen_location,
            focus: Some(focus),
            target: PointerTarget::Layer(layer),
            visual_geometry: None,
            is_desktop_popup: false,
        });
    }

    // Window popup trees occupy the plane immediately below overlay surfaces
    // and above Layer::Top. Route them in the same order in which they render.
    if let Some(route) = window_under(&context, output, output_local, WindowHitKind::Popup) {
        return Some(route);
    }

    if !context
        .fullscreen
        .covers_top_matching(context.focused, output, context.now, |surface| {
            crate::presentation::surface_workspace_is_active(
                context.clusters,
                context.nodes,
                surface,
                &output.name(),
                context.now,
            )
        })
        && let Some((layer, focus)) =
            layer_under(output, output_geometry.loc, output_local, [Layer::Top])
    {
        return Some(PointerRoute {
            output: output.clone(),
            location: screen_location,
            focus: Some(focus),
            target: PointerTarget::Layer(layer),
            visual_geometry: None,
            is_desktop_popup: false,
        });
    }

    if let Some(route) = window_under(&context, output, output_local, WindowHitKind::Any) {
        return Some(route);
    }

    let camera = context.cameras.get(&output.name())?;
    let world =
        crate::input::grab::screen_to_world_on_output(screen_position, camera, output_geometry);
    let location = Point::<f64, Logical>::from((world.x as f64, world.y as f64));

    if let Some((layer, focus)) = layer_under(
        output,
        output_geometry.loc,
        output_local,
        [Layer::Bottom, Layer::Background],
    ) {
        return Some(PointerRoute {
            output: output.clone(),
            location: screen_location,
            focus: Some(focus),
            target: PointerTarget::Layer(layer),
            visual_geometry: None,
            is_desktop_popup: false,
        });
    }

    Some(PointerRoute {
        output: output.clone(),
        location,
        focus: None,
        target: PointerTarget::Background,
        visual_geometry: None,
        is_desktop_popup: false,
    })
}

fn layer_under(
    output: &Output,
    output_location: Point<i32, Logical>,
    output_local: Point<f64, Logical>,
    layers: impl IntoIterator<Item = Layer>,
) -> Option<LayerHit> {
    let map = layer_map_for_output(output);
    for layer_kind in layers {
        for layer in map.layers_on(layer_kind).rev() {
            let Some(geometry) = map.layer_geometry(layer) else {
                continue;
            };
            let Some((surface, surface_location)) =
                layer.surface_under(output_local - geometry.loc.to_f64(), WindowSurfaceType::ALL)
            else {
                continue;
            };
            let origin = output_location + geometry.loc + surface_location;
            return Some((layer.clone(), (surface, origin.to_f64())));
        }
    }
    None
}

/// Resolves normal and fullscreen windows in one front-to-back pass.
///
/// Presentation transforms differ, but stacking order does not. A separate
/// fullscreen-first pass lets a fullscreen surface underneath a newly
/// raised normal window steal axes and clicks through that window.
fn window_under(
    context: &PointerRoutingContext<'_>,
    output: &Output,
    output_local: Point<f64, Logical>,
    hit_kind: WindowHitKind,
) -> Option<PointerRoute> {
    let screen_position = (
        context.space.output_geometry(output)?.loc.x as f64 + output_local.x,
        context.space.output_geometry(output)?.loc.y as f64 + output_local.y,
    );
    let screen_location = Point::<f64, Logical>::from(screen_position);
    let exclusive = crate::presentation::window::cluster_exclusive_presentation(
        context.clusters,
        context.nodes,
        context.fullscreen,
        context.maximize,
        output,
        context.space.output_geometry(output)?,
        context.now,
    )
    .filter(|presentation| presentation.progress > 0.0);

    let mut windows = context
        .space
        .elements()
        .enumerate()
        .filter_map(|(stack_index, window)| {
            if !crate::wayland::window_is_on_output(window, output, context.primary) {
                return None;
            }
            let presentation = WindowPresentation::for_window(
                context.space,
                context.cameras,
                Some(context.clusters),
                Some(context.nodes),
                context.window_animations,
                context.fullscreen,
                context.maximize,
                context.decorations,
                context.font,
                window,
                output,
                context.now,
            )?;
            Some((stack_index, window, presentation))
        })
        .collect::<Vec<_>>();
    let exclusive_anchor = exclusive.and_then(|exclusive| {
        windows
            .iter()
            .filter(|(_, window, _)| {
                exclusive_member_for_window(context.space, context.nodes, window)
                    == Some(exclusive.member)
            })
            .map(|(stack_index, _, _)| *stack_index)
            .max()
    });
    let cluster_anchor = windows
        .iter()
        .filter_map(|(stack_index, _, presentation)| {
            presentation.cluster_depth().map(|_| *stack_index)
        })
        .max();
    windows.sort_by_key(|(stack_index, _, presentation)| {
        presentation_stack_key(*stack_index, presentation.cluster_depth(), cluster_anchor)
    });

    for (stack_index, window, presentation) in windows.into_iter().rev() {
        if !crate::wayland::window_is_on_output(window, output, context.primary) {
            continue;
        }
        if let Some(exclusive) = exclusive {
            let member = exclusive_member_for_window(context.space, context.nodes, window);
            let member_floating =
                member.is_some_and(|member| context.clusters.is_member_floating(member));
            if !exclusive_pointer_member_is_allowed(
                member,
                exclusive.member,
                member_floating,
                stack_index,
                exclusive_anchor,
            ) {
                continue;
            }
        }
        let visual_geometry = presentation.visual_geometry();
        let surface = window.wl_surface();
        let fullscreen = surface
            .as_ref()
            .is_some_and(|surface| context.fullscreen.suppresses_chrome(surface.as_ref()))
            || crate::xwayland::is_fullscreen(window);
        let chrome =
            crate::titlebar::WindowChrome::for_window(window, context.decorations, context.font);
        if hit_kind == WindowHitKind::Any && !fullscreen {
            let source_height = presentation.source_geometry().size.h.max(1);
            let visual_scale = visual_geometry.size.h as f32 / source_height as f32;
            let border_width =
                crate::render::window_decoration::scaled_metric(chrome.border_width, visual_scale);
            let titlebar_layout = chrome.has_server_titlebar().then(|| {
                let titlebar_height = crate::titlebar::rendered_metrics(
                    &context.decorations.titlebars,
                    context.font.size,
                    visual_scale,
                )
                .height;
                crate::titlebar::DecorationLayout::new(
                    visual_geometry,
                    border_width,
                    titlebar_height,
                    &context.decorations.titlebars,
                )
            });

            let node = surface
                .as_ref()
                .and_then(|surface| context.nodes.id_for_surface(surface.as_ref()));
            let border_resize_allowed = context.decorations.resize_using_border
                && chrome.mode != crate::titlebar::DecorationMode::Unmanaged
                && crate::window::accepts_compositor_grab(window)
                && surface.as_ref().is_none_or(|surface| {
                    !context
                        .fullscreen
                        .is_fullscreen_or_pending(surface.as_ref())
                        && !context.maximize.contains(surface.as_ref())
                })
                && node
                    .is_none_or(|node| context.clusters.active_layout_for_member(node).is_none());
            let outer = titlebar_layout.as_ref().map_or_else(
                || {
                    crate::titlebar::WindowChrome {
                        mode: chrome.mode,
                        border_width,
                        titlebar_height: None,
                    }
                    .outer_rect(visual_geometry)
                },
                |layout| layout.outer,
            );
            if let Some(hit) = decoration_hit_at(
                titlebar_layout.as_ref(),
                outer,
                screen_location,
                border_resize_allowed,
                f64::from(border_width.max(8)),
            ) {
                return Some(PointerRoute {
                    output: output.clone(),
                    location: presentation.source_from_screen(screen_location),
                    focus: None,
                    target: PointerTarget::Decoration {
                        window: window.clone(),
                        hit,
                    },
                    visual_geometry: Some(visual_geometry),
                    is_desktop_popup: false,
                });
            }
        }
        if visual_bounds_required(hit_kind, crate::xwayland::is_x11(window))
            && !presentation.contains_screen(screen_location)
        {
            continue;
        }
        let location = presentation.source_from_screen(screen_location);

        let Some(element_location) = context.space.element_location(window) else {
            continue;
        };
        let render_location = element_location - window.geometry().loc;
        let surface_type = match hit_kind {
            WindowHitKind::Popup if crate::xwayland::is_override_redirect(window) => {
                WindowSurfaceType::ALL
            }
            WindowHitKind::Popup => WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE,
            WindowHitKind::Any => WindowSurfaceType::ALL,
        };
        let Some(focus) = window_focus(window, location, render_location, surface_type) else {
            continue;
        };
        return Some(PointerRoute {
            output: output.clone(),
            location,
            focus: Some(focus),
            target: PointerTarget::Window(window.clone()),
            visual_geometry: Some(presentation.visual_geometry()),
            is_desktop_popup: hit_kind == WindowHitKind::Popup,
        });
    }
    None
}

fn visual_bounds_required(hit_kind: WindowHitKind, is_x11: bool) -> bool {
    // Native popup trees perform their own surface-local hit testing and may
    // legitimately extend beyond the toplevel's visual rectangle. X11
    // override-redirect windows are independent surfaces. Retain this bounds
    // gate as well as their surface-local input-region check below.
    hit_kind == WindowHitKind::Any || is_x11
}

const TITLEBAR_CONTROL_RESIZE_BAND: f64 = 8.0;

fn decoration_hit_at(
    titlebar: Option<&crate::titlebar::DecorationLayout<Logical>>,
    outer: Rectangle<i32, Logical>,
    point: Point<f64, Logical>,
    resize_allowed: bool,
    resize_band: f64,
) -> Option<crate::titlebar::Hit> {
    let titlebar_hit = titlebar.and_then(|layout| layout.hit(point));
    if let Some(crate::titlebar::Hit::Control(control)) = titlebar_hit {
        // Match conventional desktop frames: the outermost perimeter remains
        // available for resizing while the interior of the button stays easy
        // to click. Cap only this overlap so an unusually wide rendered border
        // cannot consume the whole control.
        if resize_allowed
            && let Some(handle) = crate::input::grab::border_resize_handle(
                outer,
                point,
                resize_band.min(TITLEBAR_CONTROL_RESIZE_BAND),
            )
        {
            return Some(crate::titlebar::Hit::Resize(handle));
        }
        return Some(crate::titlebar::Hit::Control(control));
    }
    if resize_allowed
        && let Some(handle) = crate::input::grab::border_resize_handle(outer, point, resize_band)
    {
        return Some(crate::titlebar::Hit::Resize(handle));
    }
    titlebar_hit
}

/// Resolves the client surface under a window-local point.
///
/// Managed X11 windows retain the compatibility fast path: their input shape
/// can lag compositor-owned geometry changes. Override-redirect windows are
/// client-positioned, so honor the surface-local input shape XWayland supplies.
/// Their transparent margins must not steal input from surfaces underneath.
fn window_focus(
    window: &Window,
    location: Point<f64, Logical>,
    render_location: Point<i32, Logical>,
    surface_type: WindowSurfaceType,
) -> Option<SurfaceFocus> {
    let local = location - render_location.to_f64();
    if crate::xwayland::is_x11(window) {
        if !surface_type.contains(WindowSurfaceType::TOPLEVEL) {
            return None;
        }
        let surface = window.wl_surface()?.into_owned();
        if crate::xwayland::is_override_redirect(window)
            && !with_states(&surface, |states| {
                let mut attributes = states.cached_state.get::<SurfaceAttributes>();
                popup_input_region_contains(attributes.current().input_region.as_ref(), local)
            })
        {
            return None;
        }
        return Some((surface, render_location.to_f64()));
    }
    if !window.is_in_input_region(&local) {
        return None;
    }
    window
        .surface_under(local, surface_type)
        .map(|(surface, surface_location)| (surface, (surface_location + render_location).to_f64()))
}

fn popup_input_region_contains(
    region: Option<&RegionAttributes>,
    local: Point<f64, Logical>,
) -> bool {
    region.is_none_or(|region| region.contains(local.to_i32_floor()))
}

fn exclusive_pointer_member_is_allowed(
    member: Option<halley_core::field::NodeId>,
    exclusive: halley_core::field::NodeId,
    member_floating: bool,
    stack_index: usize,
    exclusive_anchor: Option<usize>,
) -> bool {
    member == Some(exclusive)
        || (member_floating && exclusive_anchor.is_some_and(|exclusive| stack_index > exclusive))
}

fn presentation_stack_key(
    stack_index: usize,
    cluster_depth: Option<usize>,
    cluster_anchor: Option<usize>,
) -> (usize, u64) {
    (
        cluster_depth.and(cluster_anchor).unwrap_or(stack_index),
        cluster_depth.map_or(u64::MAX, |depth| depth as u64),
    )
}

fn exclusive_member_for_window(
    space: &Space<Window>,
    nodes: &crate::nodes::NodesState,
    window: &Window,
) -> Option<halley_core::field::NodeId> {
    window
        .wl_surface()
        .and_then(|surface| nodes.id_for_surface(surface.as_ref()))
        .or_else(|| {
            let owner = crate::wayland::window_presentation_owner(window)?;
            let owner = crate::xwayland::window_for_xid(space, owner)?;
            owner
                .wl_surface()
                .and_then(|surface| nodes.id_for_surface(surface.as_ref()))
        })
}

/// Converts one backend scroll event into the complete Smithay/Wayland axis
/// frame used by both sessions. This follows Smithay's Anvil example:
/// continuous values are preferred, wheel `v120` data is retained, and
/// finger-source zeroes become explicit stop events.
#[cfg(test)]
pub fn axis_frame<B, E>(event: &E) -> AxisFrame
where
    B: InputBackend,
    E: PointerAxisEvent<B>,
{
    axis_frame_filtered(event, true, true)
}

/// Builds a client-facing frame while omitting axes consumed by compositor
/// keybinds. This preserves a diagonal event's unbound axis.
pub fn axis_frame_filtered<B, E>(
    event: &E,
    include_horizontal: bool,
    include_vertical: bool,
) -> AxisFrame
where
    B: InputBackend,
    E: PointerAxisEvent<B>,
{
    let horizontal = event
        .amount(Axis::Horizontal)
        .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0);
    let vertical = event
        .amount(Axis::Vertical)
        .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0);

    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
    if include_horizontal && horizontal != 0.0 {
        frame = frame
            .relative_direction(Axis::Horizontal, event.relative_direction(Axis::Horizontal))
            .value(Axis::Horizontal, horizontal);
        if let Some(v120) = event.amount_v120(Axis::Horizontal) {
            frame = frame.v120(Axis::Horizontal, v120 as i32);
        }
    }
    if include_vertical && vertical != 0.0 {
        frame = frame
            .relative_direction(Axis::Vertical, event.relative_direction(Axis::Vertical))
            .value(Axis::Vertical, vertical);
        if let Some(v120) = event.amount_v120(Axis::Vertical) {
            frame = frame.v120(Axis::Vertical, v120 as i32);
        }
    }
    if event.source() == smithay::backend::input::AxisSource::Finger {
        if include_horizontal && event.amount(Axis::Horizontal) == Some(0.0) {
            frame = frame.stop(Axis::Horizontal);
        }
        if include_vertical && event.amount(Axis::Vertical) == Some(0.0) {
            frame = frame.stop(Axis::Vertical);
        }
    }
    frame
}

fn desktop_bounds(outputs: &[Rectangle<i32, Logical>]) -> Option<Rectangle<i32, Logical>> {
    outputs.iter().copied().reduce(Rectangle::merge)
}

fn clamp_to_outputs(position: (f64, f64), outputs: &[Rectangle<i32, Logical>]) -> (f64, f64) {
    let point = Point::<f64, Logical>::from(position);
    if outputs.iter().any(|output| output.to_f64().contains(point)) {
        return position;
    }

    outputs
        .iter()
        .map(|output| {
            // `Rectangle::contains` is upper-bound-exclusive. Keeping the
            // constrained point one logical pixel inside that edge ensures
            // the same geometry will select an output for cursor rendering.
            let min = output.loc.to_f64();
            let max = Point::<f64, Logical>::from((
                (output.loc.x + output.size.w - 1) as f64,
                (output.loc.y + output.size.h - 1) as f64,
            ));
            let constrained = Point::<f64, Logical>::from((
                point.x.clamp(min.x, max.x),
                point.y.clamp(min.y, max.y),
            ));
            let dx = point.x - constrained.x;
            let dy = point.y - constrained.y;
            (dx * dx + dy * dy, constrained)
        })
        .min_by(|(left, _), (right, _)| left.total_cmp(right))
        .map_or(position, |(_, constrained)| constrained.into())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use smithay::backend::input::{
        Axis, AxisRelativeDirection, AxisSource, Device, DeviceCapability, Event, InputBackend,
        PointerAxisEvent, UnusedEvent,
    };
    use smithay::utils::Rectangle;

    use super::{
        WheelAccumulator, WindowHitKind, axis_frame, axis_frame_filtered, clamp_to_outputs,
        decoration_hit_at, desktop_bounds, exclusive_pointer_member_is_allowed,
        presentation_stack_key, process_wheel_bindings, visual_bounds_required, wheel_delta_v120,
        wheel_direction,
    };
    use crate::input::keybinds::WheelDirection;

    #[test]
    fn x11_popups_cannot_capture_input_outside_their_visual_bounds() {
        assert!(visual_bounds_required(WindowHitKind::Popup, true));
        assert!(visual_bounds_required(WindowHitKind::Any, true));
        assert!(visual_bounds_required(WindowHitKind::Any, false));
        assert!(!visual_bounds_required(WindowHitKind::Popup, false));
    }

    #[test]
    fn shaped_x11_popup_passes_through_tall_transparent_margins() {
        use smithay::wayland::compositor::{RectangleKind, RegionAttributes};
        // Representative live ChatGPT voice input shape inside its 772x2849
        // bounding rectangle. Shape coordinates are local, not output-global.
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((225, 1296).into(), (293, 56).into()),
                ),
                (
                    RectangleKind::Add,
                    Rectangle::new((315, 1354).into(), (113, 121).into()),
                ),
            ],
        };
        for point in [
            (370.0, 100.0),
            (370.0, 2300.0),
            (100.0, 1400.0),
            (370.0, 1353.0),
        ] {
            assert!(!super::popup_input_region_contains(
                Some(&region),
                point.into()
            ));
        }
        for point in [(230.0, 1300.0), (370.0, 1400.0)] {
            assert!(super::popup_input_region_contains(
                Some(&region),
                point.into()
            ));
        }
    }

    #[test]
    fn popup_input_region_preserves_empty_default_and_subtracted_holes() {
        use smithay::wayland::compositor::{RectangleKind, RegionAttributes};
        let point = (20.0, 20.0).into();
        assert!(super::popup_input_region_contains(None, point));
        assert!(!super::popup_input_region_contains(
            Some(&RegionAttributes::default()),
            point
        ));
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((0, 0).into(), (100, 100).into()),
                ),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((10, 10).into(), (20, 20).into()),
                ),
            ],
        };
        assert!(!super::popup_input_region_contains(Some(&region), point));
        assert!(super::popup_input_region_contains(
            Some(&region),
            (40.0, 40.0).into()
        ));
        assert!(!super::popup_input_region_contains(
            Some(&region),
            (-0.1, 40.0).into()
        ));
    }

    #[test]
    fn titlebar_perimeter_resizes_while_control_interior_remains_clickable() {
        let config = halley_config::Titlebars {
            button_position: halley_config::TitlebarButtonPosition::Right,
            ..halley_config::Titlebars::default()
        };
        let client = Rectangle::new((0, 32).into(), (300, 200).into());
        let layout = crate::titlebar::DecorationLayout::new(client, 0, 32, &config);

        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((298.0, 2.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Resize(
                crate::input::grab::ResizeHandle::TopRight
            ))
        );
        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((280.0, 2.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Resize(
                crate::input::grab::ResizeHandle::Top
            ))
        );
        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((298.0, 16.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Resize(
                crate::input::grab::ResizeHandle::Right
            ))
        );
        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((280.0, 16.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Control(
                crate::titlebar::Control::Close
            ))
        );

        let left_config = halley_config::Titlebars {
            button_position: halley_config::TitlebarButtonPosition::Left,
            ..halley_config::Titlebars::default()
        };
        let left_layout = crate::titlebar::DecorationLayout::new(client, 0, 32, &left_config);
        assert_eq!(
            decoration_hit_at(
                Some(&left_layout),
                left_layout.outer,
                smithay::utils::Point::from((2.0, 2.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Resize(
                crate::input::grab::ResizeHandle::TopLeft
            ))
        );
        assert_eq!(
            decoration_hit_at(
                Some(&left_layout),
                left_layout.outer,
                smithay::utils::Point::from((16.0, 16.0)),
                true,
                8.0,
            ),
            Some(crate::titlebar::Hit::Control(
                crate::titlebar::Control::Close
            ))
        );
    }

    #[test]
    fn wide_resize_band_does_not_consume_titlebar_control_interior() {
        let config = halley_config::Titlebars {
            button_position: halley_config::TitlebarButtonPosition::Right,
            ..halley_config::Titlebars::default()
        };
        let client = Rectangle::new((0, 32).into(), (300, 200).into());
        let layout = crate::titlebar::DecorationLayout::new(client, 0, 32, &config);

        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((288.0, 16.0)),
                true,
                16.0,
            ),
            Some(crate::titlebar::Hit::Control(
                crate::titlebar::Control::Close
            ))
        );
    }

    #[test]
    fn disabled_border_resize_leaves_the_full_titlebar_behavior_intact() {
        let config = halley_config::Titlebars {
            button_position: halley_config::TitlebarButtonPosition::Right,
            ..halley_config::Titlebars::default()
        };
        let client = Rectangle::new((0, 32).into(), (300, 200).into());
        let layout = crate::titlebar::DecorationLayout::new(client, 0, 32, &config);

        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((298.0, 2.0)),
                false,
                8.0,
            ),
            Some(crate::titlebar::Hit::Control(
                crate::titlebar::Control::Close
            ))
        );
        assert_eq!(
            decoration_hit_at(
                Some(&layout),
                layout.outer,
                smithay::utils::Point::from((200.0, 2.0)),
                false,
                8.0,
            ),
            Some(crate::titlebar::Hit::Drag)
        );
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct TestDevice;

    impl Device for TestDevice {
        fn id(&self) -> String {
            "test-pointer".into()
        }

        fn name(&self) -> String {
            "test pointer".into()
        }

        fn has_capability(&self, capability: DeviceCapability) -> bool {
            capability == DeviceCapability::Pointer
        }

        fn usb_id(&self) -> Option<(u32, u32)> {
            None
        }

        fn syspath(&self) -> Option<PathBuf> {
            None
        }
    }

    struct TestBackend;

    impl InputBackend for TestBackend {
        type Device = TestDevice;
        type KeyboardKeyEvent = UnusedEvent;
        type PointerAxisEvent = TestAxisEvent;
        type PointerButtonEvent = UnusedEvent;
        type PointerMotionEvent = UnusedEvent;
        type PointerMotionAbsoluteEvent = UnusedEvent;
        type GestureSwipeBeginEvent = UnusedEvent;
        type GestureSwipeUpdateEvent = UnusedEvent;
        type GestureSwipeEndEvent = UnusedEvent;
        type GesturePinchBeginEvent = UnusedEvent;
        type GesturePinchUpdateEvent = UnusedEvent;
        type GesturePinchEndEvent = UnusedEvent;
        type GestureHoldBeginEvent = UnusedEvent;
        type GestureHoldEndEvent = UnusedEvent;
        type TouchDownEvent = UnusedEvent;
        type TouchUpEvent = UnusedEvent;
        type TouchMotionEvent = UnusedEvent;
        type TouchCancelEvent = UnusedEvent;
        type TouchFrameEvent = UnusedEvent;
        type TabletToolAxisEvent = UnusedEvent;
        type TabletToolProximityEvent = UnusedEvent;
        type TabletToolTipEvent = UnusedEvent;
        type TabletToolButtonEvent = UnusedEvent;
        type SwitchToggleEvent = UnusedEvent;
        type SpecialEvent = ();
    }

    struct TestAxisEvent {
        source: AxisSource,
        horizontal: Option<f64>,
        vertical: Option<f64>,
        horizontal_v120: Option<f64>,
        vertical_v120: Option<f64>,
        horizontal_direction: AxisRelativeDirection,
        vertical_direction: AxisRelativeDirection,
    }

    impl Event<TestBackend> for TestAxisEvent {
        fn time(&self) -> u64 {
            42_000
        }

        fn device(&self) -> TestDevice {
            TestDevice
        }
    }

    impl PointerAxisEvent<TestBackend> for TestAxisEvent {
        fn amount(&self, axis: Axis) -> Option<f64> {
            match axis {
                Axis::Horizontal => self.horizontal,
                Axis::Vertical => self.vertical,
            }
        }

        fn amount_v120(&self, axis: Axis) -> Option<f64> {
            match axis {
                Axis::Horizontal => self.horizontal_v120,
                Axis::Vertical => self.vertical_v120,
            }
        }

        fn source(&self) -> AxisSource {
            self.source
        }

        fn relative_direction(&self, axis: Axis) -> AxisRelativeDirection {
            match axis {
                Axis::Horizontal => self.horizontal_direction,
                Axis::Vertical => self.vertical_direction,
            }
        }
    }

    #[test]
    fn clamp_leaves_positions_on_either_configured_output_unchanged() {
        let outputs = configured_outputs();
        assert_eq!(clamp_to_outputs((100.0, 200.0), &outputs), (100.0, 200.0));
        assert_eq!(clamp_to_outputs((3000.0, 800.0), &outputs), (3000.0, 800.0));
    }

    #[test]
    fn clamp_crosses_the_shared_edge_between_configured_outputs() {
        let outputs = configured_outputs();
        assert_eq!(clamp_to_outputs((2559.0, 600.0), &outputs), (2559.0, 600.0));
        assert_eq!(clamp_to_outputs((2560.0, 600.0), &outputs), (2560.0, 600.0));
    }

    #[test]
    fn clamp_pins_to_the_shorter_secondary_output() {
        let outputs = configured_outputs();
        assert_eq!(
            clamp_to_outputs((3000.0, 1300.0), &outputs),
            (3000.0, 1199.0)
        );
        assert_eq!(clamp_to_outputs((5000.0, -50.0), &outputs), (4479.0, 0.0));
    }

    #[test]
    fn desktop_bounds_cover_both_configured_outputs() {
        assert_eq!(
            desktop_bounds(&configured_outputs()),
            Some(Rectangle::new((0, 0).into(), (4480, 1440).into()))
        );
    }

    #[test]
    fn wheel_axis_frame_keeps_v120_and_derives_continuous_values() {
        let event = TestAxisEvent {
            source: AxisSource::Wheel,
            horizontal: None,
            vertical: None,
            horizontal_v120: Some(-240.0),
            vertical_v120: Some(120.0),
            horizontal_direction: AxisRelativeDirection::Inverted,
            vertical_direction: AxisRelativeDirection::Identical,
        };

        let frame = axis_frame(&event);
        assert_eq!(frame.time, 42);
        assert_eq!(frame.source, Some(AxisSource::Wheel));
        assert_eq!(frame.axis, (-30.0, 15.0));
        assert_eq!(frame.v120, Some((-240, 120)));
        assert_eq!(
            frame.relative_direction,
            (
                AxisRelativeDirection::Inverted,
                AxisRelativeDirection::Identical
            )
        );
        assert_eq!(frame.stop, (false, false));
    }

    #[test]
    fn filtered_axis_frame_preserves_only_the_unbound_axis() {
        let event = TestAxisEvent {
            source: AxisSource::Wheel,
            horizontal: None,
            vertical: None,
            horizontal_v120: Some(-120.0),
            vertical_v120: Some(120.0),
            horizontal_direction: AxisRelativeDirection::Identical,
            vertical_direction: AxisRelativeDirection::Identical,
        };

        let frame = axis_frame_filtered(&event, true, false);
        assert_eq!(frame.axis, (-15.0, 0.0));
        assert_eq!(frame.v120, Some((-120, 0)));
    }

    #[test]
    fn wheel_accumulator_handles_partial_multiple_and_reversed_notches() {
        let mut accumulator = WheelAccumulator::default();
        assert_eq!(accumulator.accumulate(Axis::Vertical, 30.0), 0);
        assert_eq!(accumulator.accumulate(Axis::Vertical, 90.0), 1);
        assert_eq!(accumulator.accumulate(Axis::Vertical, 300.0), 2);
        assert_eq!(accumulator.accumulate(Axis::Vertical, -30.0), 0);
        assert_eq!(accumulator.accumulate(Axis::Vertical, -90.0), -1);
    }

    #[test]
    fn wheel_accumulator_keeps_axes_independent_and_resets() {
        let mut accumulator = WheelAccumulator::default();
        assert_eq!(accumulator.accumulate(Axis::Horizontal, 60.0), 0);
        assert_eq!(accumulator.accumulate(Axis::Vertical, 120.0), 1);
        assert_eq!(accumulator.accumulate(Axis::Horizontal, 60.0), 1);
        accumulator.accumulate(Axis::Vertical, 60.0);
        accumulator.reset(Axis::Vertical);
        assert_eq!(accumulator.accumulate(Axis::Vertical, 60.0), 0);
        accumulator.reset_all();
        assert_eq!(accumulator.accumulate(Axis::Horizontal, 60.0), 0);
    }

    #[test]
    fn wheel_helpers_derive_v120_and_map_logical_directions() {
        let event = TestAxisEvent {
            source: AxisSource::Wheel,
            horizontal: Some(-15.0),
            vertical: Some(30.0),
            horizontal_v120: None,
            vertical_v120: None,
            horizontal_direction: AxisRelativeDirection::Identical,
            vertical_direction: AxisRelativeDirection::Identical,
        };
        assert_eq!(wheel_delta_v120(&event, Axis::Horizontal), -120.0);
        assert_eq!(wheel_delta_v120(&event, Axis::Vertical), 240.0);
        assert_eq!(
            wheel_direction(Axis::Horizontal, -1.0),
            Some(WheelDirection::Left)
        );
        assert_eq!(
            wheel_direction(Axis::Horizontal, 1.0),
            Some(WheelDirection::Right)
        );
        assert_eq!(
            wheel_direction(Axis::Vertical, -1.0),
            Some(WheelDirection::Up)
        );
        assert_eq!(
            wheel_direction(Axis::Vertical, 1.0),
            Some(WheelDirection::Down)
        );
        assert_eq!(wheel_direction(Axis::Vertical, 0.0), None);
    }

    #[test]
    fn wheel_binding_policy_consumes_only_matches_and_emits_full_notches() {
        let wheel = |source, vertical_v120| TestAxisEvent {
            source,
            horizontal: None,
            vertical: None,
            horizontal_v120: None,
            vertical_v120: Some(vertical_v120),
            horizontal_direction: AxisRelativeDirection::Identical,
            vertical_direction: AxisRelativeDirection::Identical,
        };
        let mut accumulator = WheelAccumulator::default();

        let first = process_wheel_bindings(
            &wheel(AxisSource::Wheel, 60.0),
            &mut accumulator,
            true,
            |direction| (direction == WheelDirection::Down).then_some("zoom-out"),
        );
        assert!(first.forward_horizontal);
        assert!(!first.forward_vertical);
        assert!(first.actions.is_empty());

        let second = process_wheel_bindings(
            &wheel(AxisSource::Wheel, 60.0),
            &mut accumulator,
            true,
            |direction| (direction == WheelDirection::Down).then_some("zoom-out"),
        );
        assert_eq!(second.actions, vec![(WheelDirection::Down, "zoom-out")]);

        let unbound = process_wheel_bindings(
            &wheel(AxisSource::Wheel, -120.0),
            &mut accumulator,
            true,
            |_| None::<&str>,
        );
        assert!(unbound.forward_vertical);

        process_wheel_bindings(
            &wheel(AxisSource::Wheel, 60.0),
            &mut accumulator,
            true,
            |_| Some("zoom-out"),
        );
        let bypassed = process_wheel_bindings(
            &wheel(AxisSource::Wheel, 120.0),
            &mut accumulator,
            false,
            |_| Some("zoom-out"),
        );
        assert!(bypassed.forward_vertical);
        assert!(bypassed.actions.is_empty());
        let after_reset = process_wheel_bindings(
            &wheel(AxisSource::Wheel, 60.0),
            &mut accumulator,
            true,
            |_| Some("zoom-out"),
        );
        assert!(after_reset.actions.is_empty());
    }

    #[test]
    fn finger_axis_frame_marks_zero_axes_stopped() {
        let event = TestAxisEvent {
            source: AxisSource::Finger,
            horizontal: Some(0.0),
            vertical: Some(0.0),
            horizontal_v120: None,
            vertical_v120: None,
            horizontal_direction: AxisRelativeDirection::Identical,
            vertical_direction: AxisRelativeDirection::Identical,
        };

        let frame = axis_frame(&event);
        assert_eq!(frame.source, Some(AxisSource::Finger));
        assert_eq!(frame.axis, (0.0, 0.0));
        assert_eq!(frame.v120, None);
        assert_eq!(frame.stop, (true, true));
    }

    #[test]
    fn cluster_depth_replaces_stale_space_order_for_pointer_routing() {
        let anchor = Some(7);
        let rear = presentation_stack_key(7, Some(0), anchor);
        let middle = presentation_stack_key(2, Some(1), anchor);
        let front = presentation_stack_key(1, Some(2), anchor);

        assert!(front > middle);
        assert!(middle > rear);
        assert_eq!(front.0, 7);
    }

    #[test]
    fn floating_member_stays_above_a_later_mapped_layout_member() {
        let anchor = Some(8);
        let floating = presentation_stack_key(1, Some(usize::MAX), anchor);
        let later_layout_member = presentation_stack_key(8, Some(3), anchor);

        assert!(floating > later_layout_member);
    }

    #[test]
    fn raised_float_can_receive_input_above_cluster_fullscreen() {
        let fullscreen = halley_core::field::NodeId::new(1);
        let floating = halley_core::field::NodeId::new(2);

        assert!(exclusive_pointer_member_is_allowed(
            Some(fullscreen),
            fullscreen,
            false,
            4,
            Some(4),
        ));
        assert!(exclusive_pointer_member_is_allowed(
            Some(floating),
            fullscreen,
            true,
            5,
            Some(4),
        ));
        assert!(!exclusive_pointer_member_is_allowed(
            Some(floating),
            fullscreen,
            true,
            3,
            Some(4),
        ));
    }

    #[test]
    fn ordinary_windows_keep_their_space_stack_key() {
        assert_eq!(presentation_stack_key(4, None, Some(9)), (4, u64::MAX));
    }

    fn configured_outputs() -> [Rectangle<i32, smithay::utils::Logical>; 2] {
        [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1200).into()),
        ]
    }
}
