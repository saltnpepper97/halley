use smithay::desktop::{LayerSurface, Space, Window, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};

use super::super::WaylandState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyboardFocus {
    ExclusiveLayer(WlSurface),
    OnDemandLayer(WlSurface),
    Window(WlSurface),
}

impl KeyboardFocus {
    pub fn surface(&self) -> WlSurface {
        match self {
            Self::ExclusiveLayer(surface)
            | Self::OnDemandLayer(surface)
            | Self::Window(surface) => surface.clone(),
        }
    }
}

/// Resolves actual seat focus without conflating it with Halley's persistent
/// focused-window state. Higher visual layers win; on-demand lower layers
/// selected by a click still precede the remembered window, while exclusive
/// Bottom/Background surfaces only win when no window is focused.
pub fn current(
    wayland: &WaylandState,
    fullscreen: &super::super::fullscreen::FullscreenManager,
    clusters: &crate::clusters::ClusterSystem,
    nodes: &crate::nodes::NodesState,
    now: std::time::Duration,
) -> Option<KeyboardFocus> {
    if let Some(surface) = first_interactive(
        wayland,
        Layer::Overlay,
        KeyboardInteractivity::Exclusive,
        None,
    ) {
        return Some(KeyboardFocus::ExclusiveLayer(surface));
    }
    if let Some(surface) = selected_on_demand(wayland, Layer::Overlay) {
        return Some(KeyboardFocus::OnDemandLayer(surface));
    }
    let fullscreen_covers_top = wayland.space.outputs().any(|output| {
        fullscreen.covers_top_matching(wayland.focused_window.as_ref(), output, now, |surface| {
            crate::presentation::surface_workspace_is_active(
                clusters,
                nodes,
                surface,
                &output.name(),
                now,
            )
        })
    });
    if !fullscreen_covers_top {
        if let Some(surface) =
            first_interactive(wayland, Layer::Top, KeyboardInteractivity::Exclusive, None)
        {
            return Some(KeyboardFocus::ExclusiveLayer(surface));
        }
        if let Some(surface) = selected_on_demand(wayland, Layer::Top) {
            return Some(KeyboardFocus::OnDemandLayer(surface));
        }
    }

    for layer in [Layer::Bottom, Layer::Background] {
        if let Some(surface) = selected_on_demand(wayland, layer) {
            return Some(KeyboardFocus::OnDemandLayer(surface));
        }
    }

    if let Some(surface) = wayland.focused_window.clone() {
        return Some(KeyboardFocus::Window(surface));
    }

    for layer in [Layer::Bottom, Layer::Background] {
        if let Some(surface) =
            first_interactive(wayland, layer, KeyboardInteractivity::Exclusive, None)
        {
            return Some(KeyboardFocus::ExclusiveLayer(surface));
        }
    }

    None
}

pub fn select_layer(wayland: &mut WaylandState, selected: Option<LayerSurface>) {
    wayland.focused_layer = selected.filter(|layer| {
        retains_on_demand_focus(
            layer.layer_surface().alive(),
            wayland.unmapped_layers.contains(layer.wl_surface()),
            layer.cached_state().keyboard_interactivity,
        )
    });
}

/// Drops click-selected layer focus after the client makes that layer
/// ineligible. Protocol requests are dispatched in batches, so this cleanup
/// runs immediately before the batch's single seat-focus reconciliation.
pub fn refresh_selected_layer(wayland: &mut WaylandState) {
    let retain = wayland.focused_layer.as_ref().is_some_and(|layer| {
        retains_on_demand_focus(
            layer.layer_surface().alive(),
            wayland.unmapped_layers.contains(layer.wl_surface()),
            layer.cached_state().keyboard_interactivity,
        )
    });
    if !retain {
        wayland.focused_layer = None;
    }
}

/// Records the output chosen by click-to-focus. Resolution is deliberately
/// deferred until placement so unplugging an output cannot leave a stale
/// Smithay handle as the spawn target.
pub fn select_output(wayland: &mut WaylandState, output: &Output) {
    wayland.focused_output = Some(output.name());
}

/// Returns the click-focused output while it remains mapped, otherwise the
/// first mapped output. The fallback preserves startup behavior before the
/// first click and makes hot-unplug self-healing.
pub fn selected_output(wayland: &WaylandState) -> Option<&Output> {
    selected_output_in(&wayland.space, wayland.focused_output.as_deref())
}

/// Finds the nearest mapped output whose center lies in `direction` from the
/// selected output. Primary-axis distance wins, then cross-axis distance and
/// finally connector name keep irregular layouts deterministic.
pub fn adjacent_output(
    space: &Space<Window>,
    current: &Output,
    direction: halley_config::Direction,
) -> Option<Output> {
    let current_geometry = space.output_geometry(current)?;
    let current_center = output_center(current_geometry);
    space
        .outputs()
        .filter(|candidate| *candidate != current)
        .filter_map(|candidate| {
            let geometry = space.output_geometry(candidate)?;
            let center = output_center(geometry);
            let dx = center.0 - current_center.0;
            let dy = center.1 - current_center.1;
            let (primary, secondary) = output_direction_score(direction, dx, dy)?;
            Some((primary, secondary, candidate.name(), candidate.clone()))
        })
        .min_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.total_cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        })
        .map(|(_, _, _, output)| output)
}

fn output_center(geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>) -> (f64, f64) {
    (
        f64::from(geometry.loc.x) + f64::from(geometry.size.w) * 0.5,
        f64::from(geometry.loc.y) + f64::from(geometry.size.h) * 0.5,
    )
}

fn output_direction_score(
    direction: halley_config::Direction,
    dx: f64,
    dy: f64,
) -> Option<(f64, f64)> {
    match direction {
        halley_config::Direction::Left if dx < 0.0 => Some((-dx, dy.abs())),
        halley_config::Direction::Right if dx > 0.0 => Some((dx, dy.abs())),
        halley_config::Direction::Up if dy < 0.0 => Some((-dy, dx.abs())),
        halley_config::Direction::Down if dy > 0.0 => Some((dy, dx.abs())),
        _ => None,
    }
}

/// Resolves the output for a newly created surface. A client-selected output
/// wins while it remains mapped; otherwise every surface type follows the
/// compositor's click-focused output.
pub fn output_for_new_surface(
    wayland: &WaylandState,
    requested: Option<Output>,
    fallback: &Output,
) -> Output {
    output_for_new_surface_in(
        &wayland.space,
        wayland.focused_output.as_deref(),
        requested,
        fallback,
    )
}

fn selected_output_in<'a>(
    space: &'a Space<Window>,
    focused_output: Option<&str>,
) -> Option<&'a Output> {
    focused_output
        .and_then(|name| space.outputs().find(|output| output.name() == name))
        .or_else(|| space.outputs().next())
}

fn output_for_new_surface_in(
    space: &Space<Window>,
    focused_output: Option<&str>,
    requested: Option<Output>,
    fallback: &Output,
) -> Output {
    requested
        .filter(|candidate| space.outputs().any(|output| output == candidate))
        .or_else(|| selected_output_in(space, focused_output).cloned())
        .unwrap_or_else(|| fallback.clone())
}

pub fn forget_layer(wayland: &mut WaylandState, surface: &WlSurface) {
    if wayland
        .focused_layer
        .as_ref()
        .is_some_and(|layer| layer.wl_surface() == surface)
    {
        wayland.focused_layer = None;
    }
}

fn selected_on_demand(wayland: &WaylandState, layer_kind: Layer) -> Option<WlSurface> {
    let selected = wayland.focused_layer.as_ref()?;
    if selected.layer() != layer_kind {
        return None;
    }
    first_interactive(
        wayland,
        layer_kind,
        KeyboardInteractivity::OnDemand,
        Some(selected),
    )
}

fn retains_on_demand_focus(
    alive: bool,
    unmapped: bool,
    interactivity: KeyboardInteractivity,
) -> bool {
    alive && !unmapped && interactivity == KeyboardInteractivity::OnDemand
}

fn first_interactive(
    wayland: &WaylandState,
    layer_kind: Layer,
    interactivity: KeyboardInteractivity,
    selected: Option<&LayerSurface>,
) -> Option<WlSurface> {
    wayland.space.outputs().find_map(|output| {
        let map = layer_map_for_output(output);
        map.layers_on(layer_kind).rev().find_map(|layer| {
            if wayland.unmapped_layers.contains(layer.wl_surface())
                || selected.is_some_and(|selected| selected != layer)
                || layer.cached_state().keyboard_interactivity != interactivity
            {
                return None;
            }
            Some(layer.wl_surface().clone())
        })
    })
}

#[cfg(test)]
mod tests {
    use smithay::output::{Mode, PhysicalProperties, Subpixel};
    use smithay::utils::{Physical, Size, Transform};

    use super::*;

    fn output(name: &str, size: Size<i32, Physical>) -> Output {
        let output = Output::new(
            name.to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "halley-next".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        let mode = Mode {
            size,
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output
    }

    #[test]
    fn clicked_output_wins_and_missing_output_falls_back() {
        let left = output("DP-1", Size::from((2560, 1440)));
        let right = output("DP-2", Size::from((1920, 1200)));
        let mut space = Space::<Window>::default();
        space.map_output(&left, (0, 0));
        space.map_output(&right, (2560, 0));

        assert_eq!(
            selected_output_in(&space, Some("DP-2")).map(Output::name),
            Some("DP-2".to_string())
        );
        assert_eq!(
            selected_output_in(&space, Some("unplugged")).map(Output::name),
            Some("DP-1".to_string())
        );
    }

    #[test]
    fn adjacent_output_follows_monitor_geometry_in_each_direction() {
        let center = output("center", Size::from((1000, 800)));
        let left = output("left", Size::from((800, 600)));
        let right = output("right", Size::from((1200, 900)));
        let above = output("above", Size::from((900, 700)));
        let below = output("below", Size::from((900, 700)));
        let mut space = Space::<Window>::default();
        space.map_output(&center, (0, 0));
        space.map_output(&left, (-800, 100));
        space.map_output(&right, (1000, -50));
        space.map_output(&above, (50, -700));
        space.map_output(&below, (50, 800));

        for (direction, expected) in [
            (halley_config::Direction::Left, "left"),
            (halley_config::Direction::Right, "right"),
            (halley_config::Direction::Up, "above"),
            (halley_config::Direction::Down, "below"),
        ] {
            assert_eq!(
                adjacent_output(&space, &center, direction).map(|output| output.name()),
                Some(expected.to_string())
            );
        }
    }

    #[test]
    fn new_surfaces_prefer_requested_then_clicked_then_fallback_output() {
        let left = output("DP-1", Size::from((2560, 1440)));
        let right = output("DP-2", Size::from((1920, 1200)));
        let stale = output("disconnected", Size::from((1024, 768)));
        let fallback = output("fallback", Size::from((800, 600)));
        let mut space = Space::<Window>::default();
        space.map_output(&left, (0, 0));
        space.map_output(&right, (2560, 0));

        assert_eq!(
            output_for_new_surface_in(&space, Some("DP-2"), Some(left), &fallback).name(),
            "DP-1"
        );
        assert_eq!(
            output_for_new_surface_in(&space, Some("DP-2"), None, &fallback).name(),
            "DP-2"
        );
        assert_eq!(
            output_for_new_surface_in(&space, Some("DP-2"), Some(stale), &fallback).name(),
            "DP-2"
        );
        assert_eq!(
            output_for_new_surface_in(&Space::default(), None, None, &fallback).name(),
            "fallback"
        );
    }

    #[test]
    fn on_demand_focus_requires_a_live_mapped_interactive_layer() {
        let cases = [
            (true, false, KeyboardInteractivity::OnDemand, true),
            (false, false, KeyboardInteractivity::OnDemand, false),
            (true, true, KeyboardInteractivity::OnDemand, false),
            (true, false, KeyboardInteractivity::None, false),
            (true, false, KeyboardInteractivity::Exclusive, false),
        ];

        for (alive, unmapped, interactivity, expected) in cases {
            assert_eq!(
                retains_on_demand_focus(alive, unmapped, interactivity),
                expected
            );
        }
    }
}
