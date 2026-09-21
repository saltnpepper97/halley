use std::io;

use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::Modifier;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::DrmNode;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use super::TtyDrmOutput;
use crate::backend::dmabuf::{SurfaceDmabufFeedback, scanout_formats};

pub fn frame_flags() -> FrameFlags {
    // Keep cursor elements in the primary composition. Client cursor surfaces
    // can switch size and storage while moving across a window, and the AMD
    // cursor-plane path has produced stale black damage during those switches.
    // Niri exposes the same policy as its `disable-cursor-plane` workaround.
    FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY
}

pub fn frame_flags_for_scene(has_framebuffer_effect: bool) -> FrameFlags {
    let mut flags = frame_flags();
    if has_framebuffer_effect {
        // Direct scan-out of a window skips the backdrop blur behind it, so
        // the surface flickers between the client buffer and the composed
        // frosted scene. Disable primary-plane scan-out while any
        // framebuffer effect is in the output list.
        flags.remove(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT);
        flags.remove(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY);
    }
    flags
}

pub fn surface_feedback(
    output: &TtyDrmOutput,
    renderer_formats: FormatSet,
    render_node: DrmNode,
    scanout_node: DrmNode,
) -> Result<SurfaceDmabufFeedback, io::Error> {
    let primary_plane_formats =
        output.with_compositor(|compositor| compositor.surface().plane_info().formats.clone());
    let mut primary_scanout_formats = scanout_formats(&renderer_formats, &primary_plane_formats);
    if render_node != scanout_node {
        // Cross-device DMA-BUF import is not guaranteed for vendor-specific
        // modifiers. Advertise linear scanout for hybrid systems and leave
        // the render GPU's full format set in the main tranche.
        primary_scanout_formats.retain(|format| format.modifier == Modifier::Linear);
    }
    let builder = DmabufFeedbackBuilder::new(render_node.dev_id(), renderer_formats);
    let scanout = builder
        .clone()
        .add_preference_tranche(
            scanout_node.dev_id(),
            Some(TrancheFlags::Scanout),
            primary_scanout_formats,
        )
        .build()?;

    // Both nodes belong to the same GPU in this backend, so scan-out-friendly
    // allocations are also the preferred render allocations.
    Ok(SurfaceDmabufFeedback {
        render: scanout.clone(),
        scanout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scanout_policy_uses_only_the_primary_plane() {
        let flags = frame_flags();
        assert!(flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
        assert!(!flags.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT));
        assert!(!flags.contains(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT));
    }

    #[test]
    fn framebuffer_effects_disable_primary_scanout() {
        let flags = frame_flags_for_scene(true);
        assert!(!flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT));
        assert!(!flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
        assert!(frame_flags_for_scene(false).contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
    }
}
