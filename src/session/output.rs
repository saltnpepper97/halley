use std::collections::HashSet;

use smithay::output::{Mode, Output};
use smithay::utils::{Logical, Point, Transform};

use super::{Session, SessionDriver};

/// The compositor-visible state of one connected output.
///
/// This is deliberately separate from `halley_config::OutputConfig`: Wayland
/// clients can temporarily disable a head and request flipped transforms,
/// while Halley's persistent configuration format remains intentionally
/// narrower.
#[derive(Clone, Debug)]
pub struct OutputState {
    pub output: Output,
    pub enabled: bool,
    pub mode: Mode,
    pub location: Point<i32, Logical>,
    pub transform: Transform,
    pub scale: f64,
    pub adaptive_sync: bool,
    pub adaptive_sync_supported: bool,
}

#[derive(Clone, Debug)]
pub struct OutputConfiguration {
    pub output: Output,
    pub enabled: bool,
    pub mode: Mode,
    pub location: Point<i32, Logical>,
    pub transform: Transform,
    pub scale: f64,
    pub adaptive_sync: bool,
}

#[derive(Clone, Debug)]
pub struct OutputChange {
    pub before: OutputState,
    pub after: OutputState,
}

pub fn validate_complete_configuration(
    current: &[OutputState],
    requested: &[OutputConfiguration],
) -> Result<(), String> {
    if requested.len() != current.len() {
        return Err("configuration must include every connected output exactly once".into());
    }
    if !requested.iter().any(|config| config.enabled) {
        return Err("configuration must leave at least one output enabled".into());
    }

    let mut names = HashSet::new();
    for config in requested {
        let name = config.output.name();
        if !names.insert(name.clone()) {
            return Err(format!("output {name:?} is configured more than once"));
        }
        let Some(state) = current.iter().find(|state| state.output == config.output) else {
            return Err(format!("output {name:?} is not connected"));
        };
        if !config.output.modes().contains(&config.mode) {
            return Err(format!("output {name:?} requested an unadvertised mode"));
        }
        if halley_config::output::normalize_scale(config.scale).is_none() {
            return Err(format!(
                "output {name:?}: scale must be from 0.25 through 10.0"
            ));
        }
        if config.adaptive_sync && !state.adaptive_sync_supported {
            return Err(format!("output {name:?} does not support adaptive sync"));
        }
    }

    Ok(())
}

impl<D: SessionDriver> Session<D> {
    pub(crate) fn notify_output_management(&mut self) {
        self.sync_output_power();
        let states = self.driver.output_states();
        let display = self.wayland.display_handle.clone();
        self.wayland
            .wlr_output_management_state
            .notify_changes::<Self>(&display, &states);
    }

    pub(crate) fn apply_wayland_output_configuration(
        &mut self,
        configuration: &[OutputConfiguration],
    ) -> Result<(), String> {
        let result = self.driver.apply_output_configuration(configuration);
        self.sync_output_power();
        let changes = result?;
        if changes.is_empty() {
            return Ok(());
        }

        let fallback = changes
            .iter()
            .find(|change| change.after.enabled)
            .map(|change| change.after.output.clone())
            .or_else(|| {
                self.driver
                    .output_states()
                    .into_iter()
                    .find(|state| state.enabled)
                    .map(|state| state.output)
            })
            .ok_or_else(|| "configuration left no enabled output".to_string())?;

        for change in &changes {
            let output = &change.after.output;
            if change.after.enabled {
                self.wayland.space.map_output(output, change.after.location);
                smithay::desktop::layer_map_for_output(output).arrange();
                self.wayland.ensure_output_global::<Self>(output);

                let geometry = self
                    .wayland
                    .space
                    .output_geometry(output)
                    .expect("newly enabled output is mapped");
                if change.before.mode != change.after.mode
                    || change.before.transform != change.after.transform
                    || change.before.scale != change.after.scale
                    || !change.before.enabled
                {
                    if change.before.scale != change.after.scale && change.before.enabled {
                        self.cameras
                            .resize_for_display_scale(&output.name(), geometry.size.to_physical(1));
                    } else {
                        self.cameras
                            .reset(output.name(), geometry.size.to_physical(1));
                    }
                    let external = self.fullscreen.reconfigure_output(&self.wayland, output);
                    crate::xwayland::reconfigure_fullscreen(external);
                }
                self.request_output_redraw(output);
            } else if change.before.enabled {
                self.wayland.wlr_gamma_control_state.output_disabled(output);
                self.wayland.wlr_screencopy_state.output_disabled(output);
                self.clear_idle_inhibit_output(output);
                if let Err(err) = self.driver.set_gamma(output, None) {
                    eventline::warn!(
                        "output {:?}: failed to reset gamma while disabling: {err}",
                        output.name()
                    );
                }
                let windows: Vec<_> = self
                    .wayland
                    .space
                    .elements()
                    .filter(|window| {
                        crate::wayland::window_output_name(window)
                            .is_some_and(|name| name == output.name())
                    })
                    .cloned()
                    .collect();
                for window in windows {
                    crate::wayland::set_window_output(&window, &fallback);
                }
                self.wayland.space.unmap_output(output);
                self.wayland.disable_output_global::<Self>(output);
                self.cameras.remove(&output.name());
                self.shell.overlays.remove_output(&output.name());
                self.render
                    .backdrop_blur_renderer
                    .remove_output(&output.name());
            }
        }

        self.wayland.space.refresh();
        crate::wayland::display_scale::refresh(&self.wayland, self.driver.primary_output());
        crate::xwayland::sync_positions(self);
        self.xwayland.sync_desktop_geometry(&self.wayland.space);
        if !self.capture.update_layout(&self.wayland.space) {
            crate::capture::cancel_selected(self);
        }
        crate::wayland::session_lock::configure_surfaces(self);
        super::sync_keyboard_focus(self, smithay::utils::SERIAL_COUNTER.next_serial());
        super::pointer::update_client_state(self, self.start_time.elapsed().as_millis() as u32);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
    use smithay::utils::{Point, Size, Transform};

    use super::{OutputConfiguration, OutputState, validate_complete_configuration};

    fn output(name: &str) -> (Output, Mode) {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (600, 340).into(),
                subpixel: Subpixel::Unknown,
                make: "Test".into(),
                model: "Panel".into(),
                serial_number: "1".into(),
            },
        );
        let mode = Mode {
            size: Size::from((1920, 1080)),
            refresh: 60_000,
        };
        output.add_mode(mode);
        (output, mode)
    }

    fn state(output: Output, mode: Mode) -> OutputState {
        OutputState {
            output,
            enabled: true,
            mode,
            location: Point::from((0, 0)),
            transform: Transform::Normal,
            scale: 1.0,
            adaptive_sync: false,
            adaptive_sync_supported: true,
        }
    }

    fn configuration(state: &OutputState) -> OutputConfiguration {
        OutputConfiguration {
            output: state.output.clone(),
            enabled: state.enabled,
            mode: state.mode,
            location: state.location,
            transform: state.transform,
            scale: state.scale,
            adaptive_sync: state.adaptive_sync,
        }
    }

    #[test]
    fn requires_every_output_once_and_one_enabled() {
        let (left, left_mode) = output("DP-1");
        let (right, right_mode) = output("DP-2");
        let states = vec![state(left, left_mode), state(right, right_mode)];

        assert!(validate_complete_configuration(&states, &[]).is_err());
        let duplicate = vec![configuration(&states[0]), configuration(&states[0])];
        assert!(validate_complete_configuration(&states, &duplicate).is_err());

        let mut disabled = states.iter().map(configuration).collect::<Vec<_>>();
        disabled
            .iter_mut()
            .for_each(|config| config.enabled = false);
        assert!(validate_complete_configuration(&states, &disabled).is_err());
    }

    #[test]
    fn accepts_fractional_scale_but_rejects_invalid_scale_and_unsupported_vrr() {
        let (output, mode) = output("DP-1");
        let mut state = state(output, mode);
        state.adaptive_sync_supported = false;

        let mut config = configuration(&state);
        config.scale = 1.25;
        assert!(validate_complete_configuration(&[state.clone()], &[config.clone()]).is_ok());
        for scale in [0.0, -1.0, 0.1, 11.0, f64::NAN, f64::INFINITY] {
            config.scale = scale;
            assert!(validate_complete_configuration(&[state.clone()], &[config.clone()]).is_err());
        }

        config.scale = 1.0;
        config.adaptive_sync = true;
        assert!(validate_complete_configuration(&[state], &[config]).is_err());
    }
}
