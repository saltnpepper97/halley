//! Shared fixtures and focused fullscreen behavior tests.

use halley_config::{AnimationCurve, AnimationMotion, EasingMotion};

use super::*;

fn test_entry(active: bool) -> FullscreenWindow {
    FullscreenWindow {
        desired: active,
        active,
        presented: active,
        target_output: "DP-1".to_string(),
        restore: None,
        presentation_windowed: None,
        presentation_output: None,
        restore_presentation_output: None,
        fullscreen_size: (1920, 1080).into(),
        transition: None,
        pending_motion: (if active { 1.0 } else { 0.0 }, 0.0),
        external_pending: None,
        snapshot_serials: Vec::new(),
        origin: FullscreenOrigin::Client,
        native_pending: None,
        native: Some(NativeFullscreenState {
            client_requested: active,
            compositor_requested: false,
            protocol_desired: active,
            protocol_active: active,
        }),
        restore_kind: FullscreenRestoreKind::Windowed,
        preserve_stack: false,
        presentation_paused: false,
        navigation_released: false,
    }
}
fn entry_on_disabled_output() -> FullscreenWindow {
    let mut entry = test_entry(true);
    entry.target_output = "disabled".into();
    entry.restore = Some(WindowedPlacement {
        location: (2100, 200).into(),
        geometry: Rectangle::new((2110, 220).into(), (800, 600).into()),
        output: Some("disabled".into()),
    });
    entry.presentation_output = Some(Rectangle::new((120, 80).into(), (800, 600).into()));
    entry.restore_presentation_output = entry.presentation_output;
    entry.snapshot_serials.push(10.into());
    entry.native_pending = Some(NativePending {
        serial: 10.into(),
        size_retry_at: Some(Duration::from_secs(1)),
    });
    entry
}
fn pending_compositor_entry() -> FullscreenWindow {
    let mut entry = test_entry(false);
    entry.desired = true;
    entry.origin = FullscreenOrigin::Compositor;
    entry.restore = Some(WindowedPlacement {
        location: (400, 240).into(),
        geometry: Rectangle::new((400, 240).into(), (800, 600).into()),
        output: Some("DP-1".into()),
    });
    entry.native_pending = Some(NativePending {
        serial: Serial::from(20),
        size_retry_at: None,
    });
    entry
}

mod external_transactions;
mod navigation_motion;
mod ownership;
mod restore;
