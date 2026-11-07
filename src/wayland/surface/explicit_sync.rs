//! Commit readiness follows the submitted acquire point, including after a
//! client destroys and recreates its synchronization object on one surface.

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::compositor::{add_blocker, with_states};
use smithay::wayland::drm_syncobj::{DrmSyncPointSource, DrmSyncobjCachedState};

/// Returns true when explicit synchronization owns this commit's readiness;
/// the caller must then skip implicit DMA-BUF waits. Registration errors must
/// reject the commit, rather than permit an early read or an orphaned blocker.
pub fn defer_until_acquired(
    surface: &WlSurface,
    register: impl FnOnce(Client, DrmSyncPointSource) -> bool,
) -> Result<bool, String> {
    // A recreated protocol object can leave upstream's stored association
    // empty. The pending point belongs to this commit and is authoritative;
    // never use that association to decide whether a submitted fence matters.
    let acquire_point = with_states(surface, |states| {
        states
            .cached_state
            .get::<DrmSyncobjCachedState>()
            .pending()
            .acquire_point
            .clone()
    });
    let Some(acquire_point) = acquire_point else {
        return Ok(false);
    };
    let Some(client) = surface.client() else {
        return Ok(true);
    };
    let (blocker, source) = acquire_point.generate_blocker().map_err(|err| {
        format!(
            "explicit synchronization setup failed: could not create acquire-point blocker: {err}"
        )
    })?;
    if !register(client, source) {
        return Err(
            "explicit synchronization setup failed: acquire-point source was not registered".into(),
        );
    }
    add_blocker(surface, blocker);
    Ok(true)
}
