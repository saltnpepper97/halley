use std::cell::RefCell;
use std::time::Duration;

use halley_core::field::NodeId;
use smithay::desktop::{Space, Window, utils::surface_primary_scanout_output};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::SurfaceData;

/// Hidden surfaces are allowed one callback roughly every second so clients
/// can make progress without running their render loops at the output rate.
pub const FALLBACK_THROTTLE: Option<Duration> = Some(Duration::from_millis(995));

/// Returns the cluster member promoted into the output-local exclusive scene.
///
/// The promoted surface is genuinely visible, but its client render elements
/// can lose their primary-output association when the cluster scene wraps and
/// reorders them. Compute this once per output callback pass so only that live
/// member bypasses the render-state visibility gate.
pub fn cluster_exclusive_callback_member(
    space: &Space<Window>,
    clusters: &crate::clusters::ClusterSystem,
    nodes: &crate::nodes::NodesState,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    maximize: &crate::presentation::maximize::FieldMaximizeManager,
    output: &Output,
    now: Duration,
) -> Option<NodeId> {
    let output_geometry = space.output_geometry(output)?;
    crate::presentation::window::cluster_exclusive_presentation(
        clusters,
        nodes,
        fullscreen,
        maximize,
        output,
        output_geometry,
        now,
    )
    .map(|presentation| presentation.member)
}

/// Whether a window's callbacks must be backed by final render-element state.
///
/// Ordinary and hidden cluster members retain visibility throttling. The one
/// member explicitly promoted into the cluster-exclusive scene is already
/// known to be visible and must keep receiving output-rate callbacks.
pub fn requires_render_visibility(
    window_member: Option<NodeId>,
    cluster_exclusive_member: Option<NodeId>,
    compositor_snapshot: bool,
) -> bool {
    !compositor_snapshot
        && cluster_exclusive_member.is_none_or(|exclusive| window_member != Some(exclusive))
}

#[derive(Default)]
struct SurfaceFrameSequence {
    last_sent_at: RefCell<Option<(Output, u32)>>,
}

/// Returns the primary output only once per output refresh sequence.
///
/// `require_visible` is false for compositor-owned live previews: their
/// client surface is captured offscreen and therefore is intentionally absent
/// from the output's final render-element states.
pub fn callback_output(
    surface: &WlSurface,
    states: &SurfaceData,
    output: &Output,
    sequence: u32,
    require_visible: bool,
) -> Option<Output> {
    if require_visible && surface_primary_scanout_output(surface, states).as_ref() != Some(output) {
        return None;
    }

    let throttling = states.data_map.get_or_insert(SurfaceFrameSequence::default);
    let mut last_sent_at = throttling.last_sent_at.borrow_mut();
    if last_sent_at
        .as_ref()
        .is_some_and(|(last_output, last_sequence)| {
            last_output == output && *last_sequence == sequence
        })
    {
        return None;
    }
    *last_sent_at = Some((output.clone(), sequence));
    Some(output.clone())
}

#[cfg(test)]
pub fn already_sent_in_sequence(last: Option<(&str, u32)>, output: &str, sequence: u32) -> bool {
    last.is_some_and(|(last_output, last_sequence)| {
        last_output == output && last_sequence == sequence
    })
}

#[cfg(test)]
mod tests {
    use halley_core::field::NodeId;

    use super::{already_sent_in_sequence, requires_render_visibility};

    #[test]
    fn callback_sequence_deduplicates_only_the_same_output_cycle() {
        assert!(already_sent_in_sequence(Some(("DP-1", 7)), "DP-1", 7));
        assert!(!already_sent_in_sequence(Some(("DP-1", 7)), "DP-1", 8));
        assert!(!already_sent_in_sequence(Some(("DP-2", 7)), "DP-1", 7));
        assert!(!already_sent_in_sequence(None, "DP-1", 7));
    }

    #[test]
    fn cluster_exclusive_member_bypasses_render_visibility() {
        let exclusive = NodeId::new(7);

        assert!(!requires_render_visibility(
            Some(exclusive),
            Some(exclusive),
            false,
        ));
        assert!(requires_render_visibility(
            Some(NodeId::new(8)),
            Some(exclusive),
            false,
        ));
        assert!(requires_render_visibility(None, Some(exclusive), false));
        assert!(requires_render_visibility(Some(exclusive), None, false));
        assert!(!requires_render_visibility(None, None, true));
    }
}
