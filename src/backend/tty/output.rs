use smithay::output::Mode as OutputMode;
use smithay::reexports::drm::control::{Mode, ModeTypeFlags, connector};
use smithay::utils::Transform;

#[derive(Clone, Copy)]
pub(super) struct OutputTarget {
    pub mode: Mode,
    pub offset: (i32, i32),
    pub transform: Transform,
    pub vrr: halley_config::Vrr,
    pub scale: f64,
}

#[derive(Clone, Copy)]
pub(super) struct OutputState {
    pub mode: OutputMode,
    pub offset: (i32, i32),
    pub transform: Transform,
    pub vrr: halley_config::Vrr,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OutputDiff {
    pub mode_changed: bool,
    pub size_changed: bool,
    pub offset_changed: bool,
    pub transform_changed: bool,
    pub vrr_changed: bool,
    pub scale_changed: bool,
}

pub(super) fn output_diff(current: OutputState, target: OutputState) -> OutputDiff {
    OutputDiff {
        mode_changed: current.mode != target.mode,
        size_changed: current.transform.transform_size(current.mode.size)
            != target.transform.transform_size(target.mode.size),
        offset_changed: current.offset != target.offset,
        transform_changed: current.transform != target.transform,
        vrr_changed: current.vrr != target.vrr,
        scale_changed: current.scale != target.scale,
    }
}

pub(super) fn connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    )
}

pub(super) fn drm_output_mode(mode: &Mode) -> OutputMode {
    OutputMode::from(*mode)
}

fn configured_refresh_millihz(refresh_hz: f64) -> i32 {
    (refresh_hz * 1000.0).round() as i32
}

fn matching_mode_index(
    modes: impl IntoIterator<Item = (usize, i32, i32, i32)>,
    width: i32,
    height: i32,
    refresh_hz: Option<f64>,
) -> Option<usize> {
    let refresh_millihz = refresh_hz.map(configured_refresh_millihz);
    modes
        .into_iter()
        .filter(|(_, mode_width, mode_height, mode_refresh)| {
            *mode_width == width
                && *mode_height == height
                && refresh_millihz.is_none_or(|refresh| *mode_refresh == refresh)
        })
        .max_by_key(|(_, _, _, refresh)| *refresh)
        .map(|(index, _, _, _)| index)
}

pub(super) struct VrrInfo {
    pub configured: halley_config::Vrr,
    pub supported: bool,
    pub active: bool,
}

pub(super) fn connector_output_info(
    name: String,
    connector: &connector::Info,
    current_mode: Option<Mode>,
    offset: (i32, i32),
    vrr: VrrInfo,
    scale: f64,
) -> halley_ipc::OutputInfo {
    let modes = connector
        .modes()
        .iter()
        .map(|mode| {
            crate::ipc::mode_info(
                drm_output_mode(mode),
                mode.mode_type().contains(ModeTypeFlags::PREFERRED),
            )
        })
        .collect::<Vec<_>>();
    let current_mode =
        current_mode.and_then(|current| connector.modes().iter().position(|mode| *mode == current));

    halley_ipc::OutputInfo {
        name,
        modes,
        current_mode,
        offset_x: offset.0,
        offset_y: offset.1,
        vrr: crate::ipc::vrr_str(vrr.configured).to_string(),
        vrr_supported: vrr.supported,
        vrr_active: vrr.active,
        scale,
    }
}

pub(super) fn default_mode(connector: &connector::Info) -> Mode {
    connector
        .modes()
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| connector.modes().first())
        .copied()
        .expect("connector has at least one mode")
}

fn select_mode(
    connector: &connector::Info,
    configured: Option<&halley_config::OutputConfig>,
) -> Mode {
    let name = connector_name(connector);
    let Some(configured) = configured else {
        return default_mode(connector);
    };
    let (Some(width), Some(height)) = (configured.width, configured.height) else {
        return default_mode(connector);
    };

    let matched = matching_mode_index(
        connector.modes().iter().enumerate().map(|(index, mode)| {
            let output_mode = drm_output_mode(mode);
            (
                index,
                output_mode.size.w,
                output_mode.size.h,
                output_mode.refresh,
            )
        }),
        width,
        height,
        configured.rate,
    )
    .and_then(|index| connector.modes().get(index));

    match matched {
        Some(mode) => *mode,
        None => {
            let available = connector
                .modes()
                .iter()
                .map(|mode| {
                    let output_mode = drm_output_mode(mode);
                    format!(
                        "{}x{}@{:.3}",
                        output_mode.size.w,
                        output_mode.size.h,
                        output_mode.refresh as f64 / 1000.0,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            eventline::warn!(
                "output {name:?}: configured {}x{}{} not found on this connector; available modes: {available}",
                width,
                height,
                configured
                    .rate
                    .map(|hz| format!(" @ {hz}Hz"))
                    .unwrap_or_default(),
            );
            default_mode(connector)
        }
    }
}

fn transform_from_degrees(degrees: u16) -> Transform {
    match degrees {
        90 => Transform::_90,
        180 => Transform::_180,
        270 => Transform::_270,
        _ => Transform::Normal,
    }
}

pub(super) fn output_target(
    connector: &connector::Info,
    configured: Option<&halley_config::OutputConfig>,
) -> OutputTarget {
    OutputTarget {
        mode: select_mode(connector, configured),
        offset: configured.map_or((0, 0), |config| (config.offset_x, config.offset_y)),
        transform: configured.map_or(Transform::Normal, |config| {
            transform_from_degrees(config.transform)
        }),
        vrr: configured.map(|config| config.vrr).unwrap_or_default(),
        scale: configured.map_or(1.0, |config| config.scale),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_mode(width: i32, height: i32, refresh: i32) -> OutputMode {
        OutputMode {
            size: smithay::utils::Size::from((width, height)),
            refresh,
        }
    }

    fn state(
        mode: OutputMode,
        offset: (i32, i32),
        transform: Transform,
        vrr: halley_config::Vrr,
    ) -> OutputState {
        OutputState {
            mode,
            offset,
            transform,
            vrr,
            scale: 1.0,
        }
    }

    #[test]
    fn unchanged_output_has_no_reload_work() {
        let current = state(
            output_mode(1920, 1200, 74_930),
            (2560, 0),
            Transform::Normal,
            halley_config::Vrr::Auto,
        );

        assert_eq!(
            output_diff(current, current),
            OutputDiff {
                mode_changed: false,
                size_changed: false,
                offset_changed: false,
                transform_changed: false,
                vrr_changed: false,
                scale_changed: false,
            }
        );
    }

    #[test]
    fn scale_only_reload_keeps_the_hardware_mode() {
        let current = state(
            output_mode(3840, 2160, 60_000),
            (0, 0),
            Transform::Normal,
            halley_config::Vrr::Off,
        );
        let target = OutputState {
            scale: 1.5,
            ..current
        };
        let diff = output_diff(current, target);
        assert!(diff.scale_changed);
        assert!(!diff.mode_changed);
        assert!(!diff.transform_changed);
        assert!(!diff.size_changed);
    }

    #[test]
    fn refresh_only_change_does_not_reset_layout_or_size() {
        let diff = output_diff(
            state(
                output_mode(2560, 1440, 60_000),
                (0, 0),
                Transform::Normal,
                halley_config::Vrr::Off,
            ),
            state(
                output_mode(2560, 1440, 179_998),
                (0, 0),
                Transform::Normal,
                halley_config::Vrr::Off,
            ),
        );

        assert!(diff.mode_changed);
        assert!(!diff.size_changed);
        assert!(!diff.offset_changed);
        assert!(!diff.transform_changed);
    }

    #[test]
    fn quarter_turn_changes_a_rectangular_output_footprint() {
        let diff = output_diff(
            state(
                output_mode(1920, 1200, 60_000),
                (0, 0),
                Transform::Normal,
                halley_config::Vrr::Off,
            ),
            state(
                output_mode(1920, 1200, 60_000),
                (0, 0),
                Transform::_90,
                halley_config::Vrr::Off,
            ),
        );

        assert!(!diff.mode_changed);
        assert!(diff.size_changed);
        assert!(diff.transform_changed);
    }

    #[test]
    fn configured_refresh_matches_exact_integer_millihertz_only() {
        let modes = [
            (0, 2560, 1440, 179_997),
            (1, 2560, 1440, 179_998),
            (2, 2560, 1440, 180_000),
        ];

        assert_eq!(
            matching_mode_index(modes, 2560, 1440, Some(179.998)),
            Some(1)
        );
        assert_eq!(matching_mode_index(modes, 2560, 1440, Some(179.999)), None);
    }

    #[test]
    fn configured_refresh_rounds_to_integer_millihertz() {
        let modes = [(0, 2560, 1440, 179_998)];

        assert_eq!(
            matching_mode_index(modes, 2560, 1440, Some(179.9984)),
            Some(0)
        );
        assert_eq!(matching_mode_index(modes, 2560, 1440, Some(179.9986)), None);
    }

    #[test]
    fn omitted_refresh_selects_highest_rate_at_exact_resolution() {
        let modes = [
            (0, 1920, 1080, 60_000),
            (1, 2560, 1440, 143_912),
            (2, 2560, 1440, 179_998),
        ];

        assert_eq!(matching_mode_index(modes, 2560, 1440, None), Some(2));
        assert_eq!(matching_mode_index(modes, 3840, 2160, None), None);
    }

    #[test]
    fn transform_from_degrees_maps_known_values_and_falls_back_to_normal() {
        assert_eq!(transform_from_degrees(0), Transform::Normal);
        assert_eq!(transform_from_degrees(90), Transform::_90);
        assert_eq!(transform_from_degrees(180), Transform::_180);
        assert_eq!(transform_from_degrees(270), Transform::_270);
        assert_eq!(transform_from_degrees(45), Transform::Normal);
    }
}
