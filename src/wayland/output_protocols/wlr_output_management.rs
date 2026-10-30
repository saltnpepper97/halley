//! Writable `wlr-output-management-unstable-v1` implementation.
//!
//! Protocol objects and serial tracking live here. Backend validation and
//! hardware changes go through `SessionDriver`'s runtime output contract.

use std::collections::HashMap;
use std::sync::Mutex;

use smithay::output::{Mode, Output};
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::protocol::wl_output;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use wayland_protocols_wlr::output_management::v1::server::{
    zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

use crate::session::output::{OutputConfiguration, OutputState};
use crate::session::{Session, SessionDriver};

const VERSION: u32 = 4;

#[derive(Debug)]
struct HeadResources {
    head: ZwlrOutputHeadV1,
    modes: Vec<(Mode, ZwlrOutputModeV1)>,
}

#[derive(Debug)]
struct ClientData {
    manager: ZwlrOutputManagerV1,
    heads: HashMap<String, HeadResources>,
    configurations: Vec<ZwlrOutputConfigurationV1>,
}

#[derive(Debug)]
pub struct State {
    _global: GlobalId,
    serial: u32,
    clients: HashMap<ClientId, ClientData>,
}

impl State {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwlrOutputManagerV1, ()> + 'static,
    {
        Self {
            _global: display.create_global::<D, ZwlrOutputManagerV1, _>(VERSION, ()),
            serial: 1,
            clients: HashMap::new(),
        }
    }

    pub fn notify_changes<D>(&mut self, display: &DisplayHandle, states: &[OutputState])
    where
        D: Dispatch<ZwlrOutputHeadV1, HeadData> + Dispatch<ZwlrOutputModeV1, ModeData> + 'static,
    {
        self.serial = self.serial.wrapping_add(1).max(1);
        for client in self.clients.values_mut() {
            for configuration in &client.configurations {
                if let Some(data) = configuration.data::<ConfigurationData>() {
                    let mut builder = data.builder.lock().expect("configuration lock poisoned");
                    if !builder.used {
                        builder.used = true;
                        configuration.cancelled();
                    }
                }
            }

            for state in states {
                if let Some(resources) = client.heads.get(&state.output.name()) {
                    send_head_state(resources, state);
                } else if let Some(client_handle) = client.manager.client()
                    && let Some(resources) =
                        advertise_output::<D>(display, &client_handle, &client.manager, state)
                {
                    client.heads.insert(state.output.name(), resources);
                }
            }

            let connected = states
                .iter()
                .map(|state| state.output.name())
                .collect::<std::collections::HashSet<_>>();
            client.heads.retain(|name, resources| {
                if connected.contains(name) {
                    true
                } else {
                    resources.head.finished();
                    resources.modes.iter().for_each(|(_, mode)| mode.finished());
                    false
                }
            });
            client.manager.done(self.serial);
        }
    }
}

#[derive(Debug)]
pub struct HeadData {
    output: Output,
}

#[derive(Debug)]
pub struct ModeData {
    output: Output,
    mode: Mode,
}

#[derive(Debug)]
struct ConfiguredHead {
    configuration: Option<OutputConfiguration>,
    mode_set: bool,
    position_set: bool,
    transform_set: bool,
    scale_set: bool,
    adaptive_sync_set: bool,
    unsupported: bool,
}

#[derive(Debug, Default)]
struct ConfigurationBuilder {
    used: bool,
    heads: HashMap<String, ConfiguredHead>,
}

#[derive(Debug)]
pub struct ConfigurationData {
    serial: u32,
    builder: Mutex<ConfigurationBuilder>,
}

#[derive(Debug)]
pub struct ConfigurationHeadData {
    parent: ZwlrOutputConfigurationV1,
    output: Output,
}

impl<D: SessionDriver> GlobalDispatch<ZwlrOutputManagerV1, (), Session<D>> for Session<D> {
    fn bind(
        session: &mut Session<D>,
        display: &DisplayHandle,
        client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        let manager = data_init.init(resource, ());
        let states = session.driver.output_states();
        let heads = states
            .iter()
            .filter_map(|state| {
                advertise_output::<Session<D>>(display, client, &manager, state)
                    .map(|resources| (state.output.name(), resources))
            })
            .collect();
        let protocol = &mut session.wayland.wlr_output_management_state;
        manager.done(protocol.serial);
        protocol.clients.insert(
            client.id(),
            ClientData {
                manager,
                heads,
                configurations: Vec::new(),
            },
        );
    }
}

fn advertise_output<D>(
    display: &DisplayHandle,
    client: &Client,
    manager: &ZwlrOutputManagerV1,
    state: &OutputState,
) -> Option<HeadResources>
where
    D: Dispatch<ZwlrOutputHeadV1, HeadData> + Dispatch<ZwlrOutputModeV1, ModeData> + 'static,
{
    let head = client
        .create_resource::<ZwlrOutputHeadV1, _, D>(
            display,
            manager.version(),
            HeadData {
                output: state.output.clone(),
            },
        )
        .ok()?;
    manager.head(&head);
    head.name(state.output.name());
    head.description(state.output.description());

    let physical = state.output.physical_properties();
    if physical.size.w > 0 && physical.size.h > 0 {
        head.physical_size(physical.size.w, physical.size.h);
    }
    if head.version() >= 2 {
        if !physical.make.is_empty() {
            head.make(physical.make);
        }
        if !physical.model.is_empty() {
            head.model(physical.model);
        }
        if !physical.serial_number.is_empty() {
            head.serial_number(physical.serial_number);
        }
    }

    let mut modes = Vec::new();
    for mode in state.output.modes() {
        let resource = client
            .create_resource::<ZwlrOutputModeV1, _, D>(
                display,
                head.version().min(3),
                ModeData {
                    output: state.output.clone(),
                    mode,
                },
            )
            .ok()?;
        head.mode(&resource);
        resource.size(mode.size.w, mode.size.h);
        if mode.refresh > 0 {
            resource.refresh(mode.refresh);
        }
        if state.output.preferred_mode() == Some(mode) {
            resource.preferred();
        }
        modes.push((mode, resource));
    }

    let resources = HeadResources { head, modes };
    send_head_state(&resources, state);
    Some(resources)
}

fn send_head_state(resources: &HeadResources, state: &OutputState) {
    resources.head.enabled(i32::from(state.enabled));
    if !state.enabled {
        return;
    }
    if let Some((_, mode)) = resources.modes.iter().find(|(mode, _)| *mode == state.mode) {
        resources.head.current_mode(mode);
    }
    resources.head.position(state.location.x, state.location.y);
    resources.head.transform(wl_transform(state.transform));
    resources.head.scale(state.scale);
    if resources.head.version() >= 4 {
        resources.head.adaptive_sync(if state.adaptive_sync {
            zwlr_output_head_v1::AdaptiveSyncState::Enabled
        } else {
            zwlr_output_head_v1::AdaptiveSyncState::Disabled
        });
    }
}

fn wl_transform(transform: smithay::utils::Transform) -> wl_output::Transform {
    match transform {
        smithay::utils::Transform::Normal => wl_output::Transform::Normal,
        smithay::utils::Transform::_90 => wl_output::Transform::_90,
        smithay::utils::Transform::_180 => wl_output::Transform::_180,
        smithay::utils::Transform::_270 => wl_output::Transform::_270,
        smithay::utils::Transform::Flipped => wl_output::Transform::Flipped,
        smithay::utils::Transform::Flipped90 => wl_output::Transform::Flipped90,
        smithay::utils::Transform::Flipped180 => wl_output::Transform::Flipped180,
        smithay::utils::Transform::Flipped270 => wl_output::Transform::Flipped270,
    }
}

fn smithay_transform(transform: wl_output::Transform) -> smithay::utils::Transform {
    match transform {
        wl_output::Transform::Normal => smithay::utils::Transform::Normal,
        wl_output::Transform::_90 => smithay::utils::Transform::_90,
        wl_output::Transform::_180 => smithay::utils::Transform::_180,
        wl_output::Transform::_270 => smithay::utils::Transform::_270,
        wl_output::Transform::Flipped => smithay::utils::Transform::Flipped,
        wl_output::Transform::Flipped90 => smithay::utils::Transform::Flipped90,
        wl_output::Transform::Flipped180 => smithay::utils::Transform::Flipped180,
        wl_output::Transform::Flipped270 => smithay::utils::Transform::Flipped270,
        _ => smithay::utils::Transform::Normal,
    }
}

impl<D: SessionDriver> Dispatch<ZwlrOutputManagerV1, (), Session<D>> for Session<D> {
    fn request(
        session: &mut Session<D>,
        client: &Client,
        manager: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                let configuration = data_init.init(
                    id,
                    ConfigurationData {
                        serial,
                        builder: Mutex::new(ConfigurationBuilder::default()),
                    },
                );
                if let Some(client_data) = session
                    .wayland
                    .wlr_output_management_state
                    .clients
                    .get_mut(&client.id())
                {
                    client_data.configurations.push(configuration);
                }
            }
            zwlr_output_manager_v1::Request::Stop => manager.finished(),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        session: &mut Session<D>,
        client: smithay::reexports::wayland_server::backend::ClientId,
        _resource: &ZwlrOutputManagerV1,
        _data: &(),
    ) {
        session
            .wayland
            .wlr_output_management_state
            .clients
            .remove(&client);
    }
}

impl<D: SessionDriver> Dispatch<ZwlrOutputHeadV1, HeadData, Session<D>> for Session<D> {
    fn request(
        _session: &mut Session<D>,
        _client: &Client,
        _head: &ZwlrOutputHeadV1,
        request: zwlr_output_head_v1::Request,
        _data: &HeadData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Session<D>>,
    ) {
        match request {
            zwlr_output_head_v1::Request::Release => {}
            _ => unreachable!(),
        }
    }
}

impl<D: SessionDriver> Dispatch<ZwlrOutputModeV1, ModeData, Session<D>> for Session<D> {
    fn request(
        _session: &mut Session<D>,
        _client: &Client,
        _mode: &ZwlrOutputModeV1,
        request: zwlr_output_mode_v1::Request,
        _data: &ModeData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Session<D>>,
    ) {
        match request {
            zwlr_output_mode_v1::Request::Release => {}
            _ => unreachable!(),
        }
    }
}

impl<D: SessionDriver> Dispatch<ZwlrOutputConfigurationV1, ConfigurationData, Session<D>>
    for Session<D>
{
    fn request(
        session: &mut Session<D>,
        _client: &Client,
        configuration: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &ConfigurationData,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        let current_serial = session.wayland.wlr_output_management_state.serial;
        match request {
            zwlr_output_configuration_v1::Request::EnableHead { id, head } => {
                let Some(head_data) = head.data::<HeadData>() else {
                    data_init.init(
                        id,
                        ConfigurationHeadData {
                            parent: configuration.clone(),
                            output: session.driver.primary_output().clone(),
                        },
                    );
                    return;
                };
                let mut builder = data.builder.lock().expect("configuration lock poisoned");
                if builder.used {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyUsed,
                        "configuration has already been used",
                    );
                    return;
                }
                let name = head_data.output.name();
                if builder.heads.contains_key(&name) {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        "head is configured more than once",
                    );
                    return;
                }
                let state = session
                    .driver
                    .output_states()
                    .into_iter()
                    .find(|state| state.output == head_data.output);
                let initial = state.map(|state| OutputConfiguration {
                    output: state.output,
                    enabled: true,
                    mode: state.mode,
                    location: state.location,
                    transform: state.transform,
                    scale: state.scale,
                    adaptive_sync: state.adaptive_sync,
                });
                builder.heads.insert(
                    name,
                    ConfiguredHead {
                        configuration: initial,
                        mode_set: false,
                        position_set: false,
                        transform_set: false,
                        scale_set: false,
                        adaptive_sync_set: false,
                        unsupported: false,
                    },
                );
                drop(builder);
                data_init.init(
                    id,
                    ConfigurationHeadData {
                        parent: configuration.clone(),
                        output: head_data.output.clone(),
                    },
                );
            }
            zwlr_output_configuration_v1::Request::DisableHead { head } => {
                let Some(head_data) = head.data::<HeadData>() else {
                    return;
                };
                let mut builder = data.builder.lock().expect("configuration lock poisoned");
                if builder.used {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyUsed,
                        "configuration has already been used",
                    );
                    return;
                }
                let name = head_data.output.name();
                if builder.heads.contains_key(&name) {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        "head is configured more than once",
                    );
                    return;
                }
                builder.heads.insert(
                    name,
                    ConfiguredHead {
                        configuration: None,
                        mode_set: false,
                        position_set: false,
                        transform_set: false,
                        scale_set: false,
                        adaptive_sync_set: false,
                        unsupported: false,
                    },
                );
            }
            zwlr_output_configuration_v1::Request::Apply
            | zwlr_output_configuration_v1::Request::Test => {
                let apply = matches!(request, zwlr_output_configuration_v1::Request::Apply);
                let mut builder = data.builder.lock().expect("configuration lock poisoned");
                if builder.used {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyUsed,
                        "configuration has already been used",
                    );
                    return;
                }
                builder.used = true;
                if data.serial != current_serial {
                    configuration.cancelled();
                    return;
                }

                let states = session.driver.output_states();
                if states
                    .iter()
                    .any(|state| !builder.heads.contains_key(&state.output.name()))
                {
                    configuration.post_error(
                        zwlr_output_configuration_v1::Error::UnconfiguredHead,
                        "configuration omitted a connected head",
                    );
                    return;
                }
                if builder.heads.values().any(|head| head.unsupported) {
                    configuration.failed();
                    return;
                }
                let requested = states
                    .iter()
                    .filter_map(|state| {
                        let head = builder.heads.get(&state.output.name())?;
                        Some(head.configuration.clone().unwrap_or(OutputConfiguration {
                            output: state.output.clone(),
                            enabled: false,
                            mode: state.mode,
                            location: state.location,
                            transform: state.transform,
                            scale: state.scale,
                            adaptive_sync: false,
                        }))
                    })
                    .collect::<Vec<_>>();
                drop(builder);

                let result = if apply {
                    session.apply_wayland_output_configuration(&requested)
                } else {
                    session.driver.test_output_configuration(&requested)
                };
                match result {
                    Ok(()) => {
                        configuration.succeeded();
                        if apply {
                            session.notify_output_management();
                        }
                    }
                    Err(err) => {
                        eventline::warn!("output management: configuration failed: {err}");
                        configuration.failed();
                    }
                }
            }
            zwlr_output_configuration_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D: SessionDriver> Dispatch<ZwlrOutputConfigurationHeadV1, ConfigurationHeadData, Session<D>>
    for Session<D>
{
    fn request(
        _session: &mut Session<D>,
        _client: &Client,
        resource: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &ConfigurationHeadData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Session<D>>,
    ) {
        let Some(parent_data) = data.parent.data::<ConfigurationData>() else {
            return;
        };
        let mut builder = parent_data
            .builder
            .lock()
            .expect("configuration lock poisoned");
        if builder.used {
            data.parent.post_error(
                zwlr_output_configuration_v1::Error::AlreadyUsed,
                "configuration has already been used",
            );
            return;
        }
        let Some(head) = builder.heads.get_mut(&data.output.name()) else {
            return;
        };
        let Some(configuration) = head.configuration.as_mut() else {
            return;
        };

        match request {
            zwlr_output_configuration_head_v1::Request::SetMode { mode } => {
                if head.mode_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "mode has already been set",
                    );
                    return;
                }
                head.mode_set = true;
                let Some(mode_data) = mode.data::<ModeData>() else {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::InvalidMode,
                        "mode has no compositor data",
                    );
                    return;
                };
                if mode_data.output != data.output {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::InvalidMode,
                        "mode belongs to another head",
                    );
                    return;
                }
                configuration.mode = mode_data.mode;
            }
            zwlr_output_configuration_head_v1::Request::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                if head.mode_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "mode has already been set",
                    );
                    return;
                }
                head.mode_set = true;
                if width <= 0 || height <= 0 || refresh < 0 {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::InvalidCustomMode,
                        "custom mode dimensions or refresh are invalid",
                    );
                    return;
                }
                head.unsupported = true;
            }
            zwlr_output_configuration_head_v1::Request::SetPosition { x, y } => {
                if head.position_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "position has already been set",
                    );
                    return;
                }
                head.position_set = true;
                configuration.location = (x, y).into();
            }
            zwlr_output_configuration_head_v1::Request::SetTransform { transform } => {
                if head.transform_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "transform has already been set",
                    );
                    return;
                }
                head.transform_set = true;
                let WEnum::Value(transform) = transform else {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::InvalidTransform,
                        "unknown transform",
                    );
                    return;
                };
                configuration.transform = smithay_transform(transform);
            }
            zwlr_output_configuration_head_v1::Request::SetScale { scale } => {
                if head.scale_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "scale has already been set",
                    );
                    return;
                }
                head.scale_set = true;
                if !scale.is_finite() || scale <= 0.0 {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::InvalidScale,
                        "scale must be finite and positive",
                    );
                    return;
                }
                // Use the same range and 1/120 units as persistent output
                // config. Positive scales outside that range fail the whole
                // transaction rather than partially changing another head.
                if let Some(scale) = halley_config::output::normalize_scale(scale) {
                    configuration.scale = scale;
                } else {
                    head.unsupported = true;
                }
            }
            zwlr_output_configuration_head_v1::Request::SetAdaptiveSync { state } => {
                if head.adaptive_sync_set {
                    resource.post_error(
                        zwlr_output_configuration_head_v1::Error::AlreadySet,
                        "adaptive sync has already been set",
                    );
                    return;
                }
                head.adaptive_sync_set = true;
                configuration.adaptive_sync = match state {
                    WEnum::Value(zwlr_output_head_v1::AdaptiveSyncState::Enabled) => true,
                    WEnum::Value(zwlr_output_head_v1::AdaptiveSyncState::Disabled) => false,
                    _ => {
                        resource.post_error(
                            zwlr_output_configuration_head_v1::Error::InvalidAdaptiveSyncState,
                            "unknown adaptive-sync state",
                        );
                        return;
                    }
                };
            }
            _ => unreachable!(),
        }
    }
}
