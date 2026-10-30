//! A projection of backend DPMS state, independent of logical output layout.
use smithay::output::Output;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    backend::{ClientId, GlobalId},
};
use wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self as manager, ZwlrOutputPowerManagerV1 as Manager},
    zwlr_output_power_v1::{self as power, ZwlrOutputPowerV1 as Power},
};

pub struct GlobalData {
    allowed: fn(&Client) -> bool,
}

pub trait Handler {
    fn output_power_state(&mut self) -> &mut State;
    /// Refresh from the driver before requests as well as after external changes.
    fn refresh_output_power(&mut self);
    fn set_output_power(&mut self, output: &Output, on: bool) -> Result<(), String>;
}

pub struct State {
    _global: Option<GlobalId>,
    outputs: Vec<(Output, bool)>,
    controls: Vec<Control>,
}

struct Control {
    resource: Power,
    output: Output,
    sent: bool,
}

impl State {
    pub fn new<D>(display: &DisplayHandle, enabled: bool, allowed: fn(&Client) -> bool) -> Self
    where
        D: GlobalDispatch<Manager, GlobalData> + 'static,
    {
        Self {
            _global: enabled
                .then(|| display.create_global::<D, Manager, _>(1, GlobalData { allowed })),
            outputs: Vec::new(),
            controls: Vec::new(),
        }
    }

    /// Missing, disabled, or unsupported outputs permanently invalidate their
    /// objects. Identity is the Output, so reconnecting a name cannot revive one.
    pub fn sync(&mut self, outputs: Vec<(Output, bool)>) {
        self.controls.retain_mut(|control| {
            if !control.resource.is_alive() {
                return false;
            }
            let Some((_, on)) = outputs.iter().find(|(output, _)| *output == control.output) else {
                control.resource.failed();
                return false;
            };
            if control.sent != *on {
                control.resource.mode(mode(*on));
                control.sent = *on;
            }
            true
        });
        self.outputs = outputs;
    }
}

fn mode(on: bool) -> power::Mode {
    if on {
        power::Mode::On
    } else {
        power::Mode::Off
    }
}

impl<D> GlobalDispatch<Manager, GlobalData, D> for State
where
    D: Handler + GlobalDispatch<Manager, GlobalData> + Dispatch<Manager, ()> + Dispatch<Power, ()>,
{
    fn bind(
        _: &mut D,
        _: &DisplayHandle,
        _: &Client,
        resource: New<Manager>,
        _: &GlobalData,
        init: &mut DataInit<'_, D>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, data: &GlobalData) -> bool {
        (data.allowed)(&client)
    }
}

impl<D> Dispatch<Manager, (), D> for State
where
    D: Handler + Dispatch<Manager, ()> + Dispatch<Power, ()>,
{
    fn request(
        state: &mut D,
        _: &Client,
        _: &Manager,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, D>,
    ) {
        if let manager::Request::GetOutputPower { id, output } = request {
            state.refresh_output_power();
            let resource = init.init(id, ());
            let protocol = state.output_power_state();
            let current = Output::from_resource(&output).and_then(|output| {
                protocol
                    .outputs
                    .iter()
                    .find(|(known, _)| *known == output)
                    .cloned()
            });
            if let Some((output, on)) = current {
                resource.mode(mode(on));
                protocol.controls.push(Control {
                    resource,
                    output,
                    sent: on,
                });
            } else {
                resource.failed();
            }
        }
    }
}

impl<D> Dispatch<Power, (), D> for State
where
    D: Handler + Dispatch<Power, ()>,
{
    fn request(
        state: &mut D,
        _: &Client,
        resource: &Power,
        request: power::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, D>,
    ) {
        if let power::Request::SetMode { mode } = request {
            let on = match mode {
                WEnum::Value(power::Mode::On) => true,
                WEnum::Value(power::Mode::Off) => false,
                _ => {
                    resource.post_error(power::Error::InvalidMode, "nonexistent output power mode");
                    return;
                }
            };
            state.refresh_output_power();
            let output = state
                .output_power_state()
                .controls
                .iter()
                .find(|control| control.resource == *resource)
                .map(|control| control.output.clone());
            let Some(output) = output else {
                return;
            };
            // Always consult the driver, even for an idempotent request: a
            // paused seat must reject writes rather than bypassing its guard.
            let result = state.set_output_power(&output, on);
            state.refresh_output_power();
            if result.is_err() {
                let protocol = state.output_power_state();
                if let Some(index) = protocol
                    .controls
                    .iter()
                    .position(|c| c.resource == *resource)
                {
                    protocol.controls.remove(index).resource.failed();
                }
            }
        }
    }

    fn destroyed(state: &mut D, _: ClientId, resource: &Power, _: &()) {
        state
            .output_power_state()
            .controls
            .retain(|c| c.resource != *resource);
    }
}
