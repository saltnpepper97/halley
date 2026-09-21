use std::collections::HashMap;
use std::error::Error;
use std::os::fd::OwnedFd;
use std::time::Duration;

mod dmabuf;
mod gamma;
mod output;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{FrameFlags, PrimaryPlaneElement};
use smithay::backend::drm::exporter::gbm::{GbmFramebufferExporter, NodeFilter};
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmNode, NodeType, VrrSupport,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{
    Element, Kind, RenderElement, RenderElementPresentationState, RenderElementState,
    RenderElementStates, UnderlyingStorage,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{GpuManager, MultiFrame, MultiRenderer};
use smithay::backend::renderer::utils::{DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Bind, Frame, Offscreen, Renderer, RendererSuper, Texture};
use smithay::backend::session::Session;
use smithay::backend::session::libseat::{LibSeatSession, LibSeatSessionNotifier};
use smithay::backend::udev;
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::drm::control::{Mode, connector, crtc};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Rectangle, Scale};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use self::output::{
    OutputState as HardwareOutputState, connector_name, connector_output_info, default_mode,
    drm_output_mode, output_diff, output_target,
};
use super::{FrameSubmission, RenderOutcome, RenderRequest, RenderStatus, Renderable};
use crate::render::scene::SceneElement;

/// Candidate scan-out formats for the primary plane, most preferred first -
/// Smithay walks this list when it builds each surface's swapchain.
///
/// Opaque `Xrgb8888` has to come first. Offering only `Argb8888` (which this
/// did originally) hands the display engine an alpha-capable primary plane on
/// every CRTC, and the per-pixel blending that implies feeds straight into the
/// bandwidth/pipe validation a modeset has to satisfy. That was survivable
/// while one monitor was lit and pathological once the second joined: the
/// second connector's atomic commit blocked the compositor for ~25s on every
/// DPMS wake. Both reference implementations (old halley's
/// `SUPPORTED_COLOR_FORMATS` and niri's identically named constant) list these
/// same four in this same order; there is no reason to differ.
const SUPPORTED_COLOR_FORMATS: [Fourcc; 4] = [
    Fourcc::Xrgb8888,
    Fourcc::Xbgr8888,
    Fourcc::Argb8888,
    Fourcc::Abgr8888,
];

type TtyDrmOutputManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    FrameSubmission,
    DrmDeviceFd,
>;

type TtyGpuBackend = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;
type TtyGpuManager = GpuManager<TtyGpuBackend>;
type TtyMultiRenderer<'render> = MultiRenderer<'render, 'render, TtyGpuBackend, TtyGpuBackend>;
type TtyMultiFrame<'render, 'frame, 'buffer> =
    MultiFrame<'render, 'render, 'frame, 'buffer, TtyGpuBackend, TtyGpuBackend>;

trait AsGlesFrame<'frame, 'buffer> {
    fn as_gles_frame(
        &mut self,
    ) -> &mut smithay::backend::renderer::gles::GlesFrame<'frame, 'buffer>;
}

impl<'frame, 'buffer> AsGlesFrame<'frame, 'buffer>
    for smithay::backend::renderer::gles::GlesFrame<'frame, 'buffer>
{
    fn as_gles_frame(
        &mut self,
    ) -> &mut smithay::backend::renderer::gles::GlesFrame<'frame, 'buffer> {
        self
    }
}

impl<'render, 'frame, 'buffer> AsGlesFrame<'frame, 'buffer>
    for TtyMultiFrame<'render, 'frame, 'buffer>
{
    fn as_gles_frame(
        &mut self,
    ) -> &mut smithay::backend::renderer::gles::GlesFrame<'frame, 'buffer> {
        self.as_mut()
    }
}

/// A primary-GPU texture that can be presented through Smithay's multi-GPU
/// renderer. The desktop scene stays GlesRenderer-native; only the final
/// composed texture crosses to an output GPU.
struct PrimaryGpuTextureElement(
    TextureRenderElement<smithay::backend::renderer::gles::GlesTexture>,
);

impl Element for PrimaryGpuTextureElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        self.0.id()
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.0.current_commit()
    }

    fn geometry(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
        self.0.geometry(scale)
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.0.transform()
    }

    fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
        self.0.src()
    }

    fn damage_since(
        &self,
        scale: smithay::utils::Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> DamageSet<i32, smithay::utils::Physical> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(
        &self,
        scale: smithay::utils::Scale<f64>,
    ) -> OpaqueRegions<i32, smithay::utils::Physical> {
        self.0.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl smithay::backend::renderer::element::RenderElement<GlesRenderer> for PrimaryGpuTextureElement {
    fn draw(
        &self,
        frame: &mut smithay::backend::renderer::gles::GlesFrame<'_, '_>,
        src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
        dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
        damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        opaque_regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        smithay::backend::renderer::element::RenderElement::<GlesRenderer>::draw(
            &self.0,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> smithay::backend::renderer::element::RenderElement<TtyMultiRenderer<'render>>
    for PrimaryGpuTextureElement
{
    fn draw(
        &self,
        frame: &mut TtyMultiFrame<'render, '_, '_>,
        src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
        dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
        damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        opaque_regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), <TtyMultiRenderer<'render> as RendererSuper>::Error> {
        smithay::backend::renderer::element::RenderElement::<GlesRenderer>::draw(
            &self.0,
            frame.as_gles_frame(),
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
        .map_err(Into::into)
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyMultiRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Recreate the per-scene element states while a secondary output is fed by a
/// single composed texture. `DrmOutput::render_frame` can only report the
/// synthetic texture element in that path, but presentation and DMA-BUF
/// feedback still need the original Wayland surface ids and visibility.
fn scene_element_states<E: Element>(
    elements: &[E],
    scale: Scale<f64>,
    damage: Rectangle<i32, smithay::utils::Physical>,
) -> RenderElementStates {
    let mut states = RenderElementStates::default();
    let mut opaque_regions = Vec::new();

    for element in elements {
        let geometry = element.geometry(scale);
        let visible_regions = geometry.subtract_rects(opaque_regions.iter().copied());
        if visible_regions.is_empty() {
            states.states.insert(
                element.id().clone(),
                RenderElementState {
                    visible_area: 0,
                    presentation_state: RenderElementPresentationState::Skipped,
                    needs_capture: false,
                },
            );
            continue;
        }

        let visible_area = visible_regions
            .iter()
            .filter_map(|region| region.intersection(damage))
            .map(|region| {
                (region.size.w.max(0) as usize).saturating_mul(region.size.h.max(0) as usize)
            })
            .sum();
        states.states.insert(
            element.id().clone(),
            RenderElementState {
                visible_area,
                presentation_state: if visible_area == 0 {
                    RenderElementPresentationState::Skipped
                } else {
                    RenderElementPresentationState::Rendering { reason: None }
                },
                needs_capture: false,
            },
        );
        opaque_regions.extend(element.opaque_regions(scale).into_iter().map(|mut region| {
            region.loc += geometry.loc;
            region
        }));
    }

    states
}

fn composed_texture_sizes(
    mode_size: smithay::utils::Size<i32, smithay::utils::Buffer>,
    transform: smithay::utils::Transform,
    output_scale: Scale<f64>,
) -> (
    smithay::utils::Size<i32, smithay::utils::Buffer>,
    smithay::utils::Size<i32, smithay::utils::Logical>,
) {
    let texture_size = transform.transform_size(mode_size);
    let logical_size = texture_size
        .to_f64()
        .to_logical(output_scale, smithay::utils::Transform::Normal)
        .to_i32_ceil();
    (texture_size, logical_size)
}

struct TtyGpu {
    render_node: DrmNode,
    session_fd: Option<OwnedFd>,
    drm_fd: DrmDeviceFd,
    drm_output_manager: TtyDrmOutputManager,
    notifier: Option<DrmDeviceNotifier>,
}

pub(super) type TtyDrmOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    FrameSubmission,
    DrmDeviceFd,
>;

/// One connected output plus whether it currently has a frame queued and
/// awaiting its VBlank. DRM only produces a VBlank in response to a page
/// flip it was actually asked to do - a scene that settles into "nothing
/// changed" naturally queues no further frame and so never gets another
/// VBlank at all, which was the only thing driving redraws. `pending` lets
/// `render()` be called eagerly (e.g. on every input event, not just on
/// VBlank) without ever double-submitting a commit for an output that's
/// still waiting on its last one.
struct DrmOutputEntry {
    gpu_index: usize,
    render_node: DrmNode,
    crtc: crtc::Handle,
    connector: connector::Info,
    current_mode: Mode,
    configured_vrr: halley_config::Vrr,
    vrr_support: VrrSupport,
    vrr_active: bool,
    vrr_failure_warned_for: Option<bool>,
    direct_scanout_active: Option<bool>,
    output: Output,
    drm_output: TtyDrmOutput,
    dmabuf_feedback: Option<super::dmabuf::SurfaceDmabufFeedback>,
    pending: bool,
    dpms_enabled: bool,
    enabled: bool,
    gamma: gamma::GammaState,
}

#[derive(Debug)]
pub struct AppliedDpmsChange {
    pub output: Output,
    pub enabled: bool,
}

#[derive(Debug)]
pub struct AppliedDpms {
    pub changes: Vec<AppliedDpmsChange>,
    pub error: Option<String>,
}

pub struct AppliedOutputChange {
    pub output: Output,
    pub mode_changed: bool,
    pub size_changed: bool,
    pub layout_changed: bool,
}

/// The tty (DRM/KMS) backend - real hardware output, no host compositor
/// involved. Wraps exactly what rendering needs, mirroring how
/// `WinitBackend` wraps exactly `WinitGraphicsBackend<GlesRenderer>`.
///
/// One entry per connected connector at startup (no hotplug) - every
/// connected output must be initialized, not just the first, or its CRTC is
/// left in stale, un-negotiated state during another CRTC's atomic modeset
/// commit, which caused a real system freeze on this project's AMD hardware.
///
/// `session` is kept only for later VT-switch handling (`pause`/`resume`) -
/// `render()` never reaches into it, matching `Renderable`'s narrow contract.
pub struct TtyBackend {
    session: LibSeatSession,
    gpu_manager: TtyGpuManager,
    gpus: Vec<TtyGpu>,
    drm_outputs: Vec<DrmOutputEntry>,
    /// The `wl_output` where newly mapped windows begin.
    primary_output: Output,
    /// Every successfully initialized output, in connector-scan order - each
    /// one has its real name and configured mode/position/transform. Driving
    /// code advertises and maps all of them into Smithay's `Space`.
    outputs: Vec<Output>,
    /// IPC inventory for every physically connected connector, including
    /// connectors which had no usable CRTC/mode or failed initialization.
    /// Connected outputs are discovered at startup; active entries are
    /// updated as configuration changes are applied.
    ipc_output_info: Vec<halley_ipc::OutputInfo>,
    primary_render_node: DrmNode,
    primary_formats: FormatSet,
    composed_frame_cache: HashMap<String, smithay::backend::renderer::gles::GlesTexture>,
}

impl TtyBackend {
    /// Opens the seat and probes every GPU, then initializes each connected
    /// connector with a usable CRTC/mode pair. Notifiers are returned rather than
    /// owned by `TtyBackend` - whatever drives the event loop inserts them,
    /// exactly like `session::winit` owns `winit_source` today rather than
    /// `WinitBackend` doing so itself.
    pub fn new(
        outputs_config: &[halley_config::OutputConfig],
    ) -> Result<(TtyBackend, LibSeatSessionNotifier, Vec<DrmDeviceNotifier>), Box<dyn Error>> {
        let (mut session, session_notifier) = LibSeatSession::new()?;
        let candidates = udev::all_gpus(session.seat())?;
        let mut gpu_manager =
            GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))?;
        let mut gpus = Vec::new();
        let mut drm_outputs = Vec::new();
        let mut outputs = Vec::new();
        let mut ipc_output_info = Vec::new();
        let mut primary_output = None;
        let mut primary_render_node = None;

        for gpu_path in candidates {
            let path_label = gpu_path.display().to_string();
            let fd = match session.open(
                &gpu_path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            ) {
                Ok(fd) => fd,
                Err(err) => {
                    eventline::warn!("tty: failed to open GPU {path_label}: {err}");
                    continue;
                }
            };
            let mut session_fd = Some(fd);
            let mut added_render_node = None;
            let probe =
                (|| -> Result<(TtyGpu, Vec<DrmOutputEntry>, Vec<Output>), Box<dyn Error>> {
                    let node = DrmNode::from_path(&gpu_path)?;
                    let render_node = node
                        .node_with_type(NodeType::Render)
                        .and_then(Result::ok)
                        .unwrap_or(node);
                    let drm_fd = DrmDeviceFd::new(DeviceFd::from(
                        session_fd
                            .as_ref()
                            .expect("session fd remains owned during GPU probe")
                            .try_clone()?,
                    ));
                    let (drm, notifier) = DrmDevice::new(drm_fd.clone(), false)?;
                    let mut scanner: DrmScanner = DrmScanner::new();
                    let connected = scanner
                        .scan_connectors(&drm)?
                        .into_iter()
                        .filter_map(|event| match event {
                            DrmScanEvent::Connected { connector, crtc } => Some((connector, crtc)),
                            DrmScanEvent::Disconnected { .. } => None,
                            DrmScanEvent::Changed { .. } => None,
                        })
                        .collect::<Vec<_>>();
                    if connected.is_empty() {
                        return Err("no connected connector found".into());
                    }

                    let gbm = GbmDevice::new(drm_fd.clone())?;
                    gpu_manager.as_mut().add_node(render_node, gbm.clone())?;
                    added_render_node = Some(render_node);
                    let renderer_formats = {
                        let renderer = gpu_manager.single_renderer(&render_node)?;
                        renderer
                            .dmabuf_formats()
                            .iter()
                            .copied()
                            .collect::<Vec<_>>()
                    };
                    let allocator = GbmAllocator::new(
                        gbm.clone(),
                        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
                    );
                    let exporter = GbmFramebufferExporter::new(gbm.clone(), NodeFilter::All);
                    let mut manager = TtyDrmOutputManager::new(
                        drm,
                        allocator,
                        exporter,
                        Some(gbm),
                        SUPPORTED_COLOR_FORMATS,
                        renderer_formats,
                    );
                    let gpu_index = gpus.len();
                    let mut entries = Vec::new();
                    let mut gpu_outputs = Vec::new();
                    for (connector, crtc) in connected {
                        let name = connector_name(&connector);
                        let configured = outputs_config.iter().find(|cfg| cfg.name == name);
                        let target = output_target(&connector, configured);
                        let offset = target.offset;
                        let vrr = target.vrr;
                        if connector.modes().is_empty() {
                            eventline::warn!(
                                "output {name:?}: connected connector advertises no modes"
                            );
                            ipc_output_info.push(connector_output_info(
                                name, &connector, None, offset, vrr, false, false,
                            ));
                            continue;
                        }
                        let Some(crtc) = crtc else {
                            eventline::warn!(
                                "output {name:?}: connected connector has no available CRTC"
                            );
                            ipc_output_info.push(connector_output_info(
                                name, &connector, None, offset, vrr, false, false,
                            ));
                            continue;
                        };
                        let mode = target.mode;
                        let output = Output::new(
                            name.clone(),
                            PhysicalProperties {
                                size: (0, 0).into(),
                                subpixel: Subpixel::Unknown,
                                make: "halley-next".into(),
                                model: "tty".into(),
                                serial_number: "unknown".into(),
                            },
                        );
                        output.change_current_state(
                            Some(drm_output_mode(&mode)),
                            Some(target.transform),
                            None,
                            Some(offset.into()),
                        );
                        output.set_preferred(drm_output_mode(&default_mode(&connector)));
                        let result = {
                            let mut renderer = gpu_manager.single_renderer(&render_node)?;
                            manager
                                .lock()
                                .initialize_output::<GlesRenderer, SolidColorRenderElement>(
                                    crtc,
                                    mode,
                                    &[connector.handle()],
                                    &output,
                                    None,
                                    renderer.as_mut(),
                                    &DrmOutputRenderElements::default(),
                                )
                        };
                        match result {
                            Ok(drm_output) => {
                                let vrr_support =
                                    query_vrr_support(&drm_output, connector.handle(), &name);
                                let requested_vrr = configured_vrr_target(vrr, false, vrr_support);
                                if let Err(err) = drm_output
                                    .with_compositor(|compositor| compositor.use_vrr(requested_vrr))
                                {
                                    eventline::warn!(
                                        "output {name:?}: failed to set initial VRR state to {requested_vrr}: {err}"
                                    );
                                }
                                let vrr_active = drm_output
                                    .with_compositor(|compositor| compositor.vrr_enabled());
                                warn_vrr_configuration(&name, vrr, vrr_support);
                                primary_output.get_or_insert_with(|| output.clone());
                                gpu_outputs.push(output.clone());
                                ipc_output_info.push(connector_output_info(
                                    name,
                                    &connector,
                                    Some(mode),
                                    offset,
                                    vrr,
                                    vrr_is_supported(vrr_support),
                                    vrr_active,
                                ));
                                let gamma = gamma::GammaState::new(manager.device(), crtc);
                                entries.push(DrmOutputEntry {
                                    gpu_index,
                                    render_node,
                                    crtc,
                                    connector,
                                    current_mode: mode,
                                    configured_vrr: vrr,
                                    vrr_support,
                                    vrr_active,
                                    vrr_failure_warned_for: None,
                                    direct_scanout_active: None,
                                    output,
                                    drm_output,
                                    dmabuf_feedback: None,
                                    pending: false,
                                    dpms_enabled: true,
                                    enabled: true,
                                    gamma,
                                });
                            }
                            Err(err) => {
                                eventline::error!("failed to initialize output {name:?}: {err}");
                                ipc_output_info.push(connector_output_info(
                                    name, &connector, None, offset, vrr, false, false,
                                ));
                            }
                        }
                    }
                    if entries.is_empty() {
                        return Err("no output could be initialized".into());
                    }
                    Ok((
                        TtyGpu {
                            render_node,
                            session_fd: Some(
                                session_fd
                                    .take()
                                    .expect("session fd is transferred to the live GPU"),
                            ),
                            drm_fd,
                            drm_output_manager: manager,
                            notifier: Some(notifier),
                        },
                        entries,
                        gpu_outputs,
                    ))
                })();
            match probe {
                Ok((gpu, entries, gpu_outputs)) => {
                    if primary_render_node.is_none() {
                        primary_render_node = Some(gpu.render_node);
                    }
                    outputs.extend(gpu_outputs);
                    drm_outputs.extend(entries);
                    gpus.push(gpu);
                }
                Err(err) => {
                    if let Some(render_node) = added_render_node {
                        gpu_manager.as_mut().remove_node(&render_node);
                    }
                    if let Some(fd) = session_fd.take() {
                        let _ = session.close(fd);
                    }
                    eventline::warn!("tty: GPU {path_label} probe failed: {err}");
                }
            }
        }

        let primary_render_node = primary_render_node.ok_or("no usable GPU found on seat")?;
        let primary_formats = {
            let renderer = gpu_manager.single_renderer(&primary_render_node)?;
            renderer.dmabuf_formats().clone()
        };
        for entry in &mut drm_outputs {
            entry.dmabuf_feedback = dmabuf::surface_feedback(
                &entry.drm_output,
                primary_formats.clone(),
                primary_render_node,
                entry.render_node,
            )
            .ok();
        }
        let primary_output = primary_output.ok_or("no output could be initialized")?;
        let drm_notifiers = gpus
            .iter_mut()
            .filter_map(|gpu| gpu.notifier.take())
            .collect::<Vec<_>>();
        if !gpus
            .iter()
            .any(|gpu| gpu.render_node == primary_render_node)
        {
            return Err("primary GPU device disappeared during setup".into());
        }
        let backend = TtyBackend {
            session,
            gpu_manager,
            gpus,
            drm_outputs,
            primary_output,
            outputs,
            ipc_output_info,
            primary_render_node,
            primary_formats,
            composed_frame_cache: HashMap::new(),
        };
        Ok((backend, session_notifier, drm_notifiers))
    }

    /// The `wl_output` used for primary-only window rendering and frame
    /// callbacks.
    pub fn primary_output(&self) -> &Output {
        &self.primary_output
    }

    /// Primary DRM device used for demand-driven explicit synchronization.
    pub fn drm_device_fd(&self) -> &DrmDeviceFd {
        self.gpus
            .iter()
            .find(|gpu| gpu.render_node == self.primary_render_node)
            .map(|gpu| &gpu.drm_fd)
            .expect("primary GPU remains present for backend lifetime")
    }

    /// Every initialized `wl_output`, for registering globals and mapping
    /// the configured layout into Smithay's `Space`.
    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.iter()
    }

    pub fn output_for_crtc(&self, gpu_index: usize, crtc: crtc::Handle) -> Option<&Output> {
        self.drm_outputs
            .iter()
            .find(|entry| entry.gpu_index == gpu_index && entry.crtc == crtc)
            .map(|entry| &entry.output)
    }

    pub fn output_dpms_enabled(&self, output: &Output) -> bool {
        self.drm_outputs
            .iter()
            .find(|entry| &entry.output == output)
            .is_none_or(|entry| entry.enabled && entry.dpms_enabled)
    }

    pub fn output_vrr_active(&self, output: &Output) -> bool {
        self.drm_outputs
            .iter()
            .find(|entry| &entry.output == output)
            .is_some_and(|entry| entry.vrr_active)
    }

    pub fn any_output_dpms_enabled(&self) -> bool {
        self.drm_outputs
            .iter()
            .any(|entry| entry.enabled && entry.dpms_enabled)
    }

    pub fn gamma_size(&self, output: &Output) -> Result<u32, String> {
        let entry = self
            .drm_outputs
            .iter()
            .find(|entry| entry.output == *output && entry.enabled)
            .ok_or_else(|| format!("output {:?} is not enabled", output.name()))?;
        entry
            .gamma
            .size(self.gpus[entry.gpu_index].drm_output_manager.device())
    }

    pub fn set_gamma(&mut self, output: &Output, ramp: Option<Vec<u16>>) -> Result<(), String> {
        let active = self.session.is_active();
        let index = self
            .drm_outputs
            .iter()
            .find(|entry| entry.output == *output)
            .map(|entry| entry.gpu_index)
            .ok_or_else(|| format!("output {:?} is not connected", output.name()))?;
        let entry_index = self
            .drm_outputs
            .iter()
            .position(|entry| entry.output == *output)
            .expect("output index was found above");
        let device = self.gpus[index].drm_output_manager.device();
        let entry = &mut self.drm_outputs[entry_index];
        if !entry.enabled && ramp.is_some() {
            return Err(format!("output {:?} is not enabled", output.name()));
        }
        entry.gamma.set(device, ramp, active)
    }

    pub fn output_states(&self) -> Vec<crate::session::output::OutputState> {
        self.drm_outputs
            .iter()
            .map(|entry| crate::session::output::OutputState {
                output: entry.output.clone(),
                enabled: entry.enabled,
                mode: drm_output_mode(&entry.current_mode),
                location: entry.output.current_location(),
                transform: entry.output.current_transform(),
                scale: entry.output.current_scale().fractional_scale(),
                adaptive_sync: entry.configured_vrr != halley_config::Vrr::Off,
                adaptive_sync_supported: vrr_is_supported(entry.vrr_support),
            })
            .collect()
    }

    pub fn test_output_configuration(
        &mut self,
        configuration: &[crate::session::output::OutputConfiguration],
    ) -> Result<(), String> {
        let current = self.output_states();
        crate::session::output::validate_complete_configuration(&current, configuration)?;
        for config in configuration {
            let name = config.output.name();
            let entry = self
                .drm_outputs
                .iter()
                .find(|entry| entry.output == config.output)
                .ok_or_else(|| format!("output {name:?} is not connected"))?;
            if !entry
                .connector
                .modes()
                .iter()
                .any(|mode| drm_output_mode(mode) == config.mode)
            {
                return Err(format!(
                    "output {name:?} requested an unadvertised DRM mode"
                ));
            }
        }
        Ok(())
    }

    pub fn apply_runtime_output_configuration(
        &mut self,
        configuration: &[crate::session::output::OutputConfiguration],
    ) -> Result<Vec<crate::session::output::OutputChange>, String> {
        self.test_output_configuration(configuration)?;
        let before = self.output_states();
        let snapshots = self
            .drm_outputs
            .iter()
            .map(|entry| {
                (
                    entry.current_mode,
                    entry.output.current_location(),
                    entry.output.current_transform(),
                    entry.configured_vrr,
                    entry.enabled,
                )
            })
            .collect::<Vec<_>>();

        let mut failure = None;
        for config in configuration {
            let index = self
                .drm_outputs
                .iter()
                .position(|entry| entry.output == config.output)
                .expect("configuration was validated against connected outputs");
            let target_mode = self.drm_outputs[index]
                .connector
                .modes()
                .iter()
                .find(|mode| drm_output_mode(mode) == config.mode)
                .copied()
                .expect("configuration mode was validated");

            if target_mode != self.drm_outputs[index].current_mode {
                let result = self.use_output_mode(index, target_mode);
                if let Err(err) = result {
                    failure = Some(format!(
                        "output {:?}: mode preflight failed: {err}",
                        config.output.name()
                    ));
                    break;
                }
            }

            if !config.enabled
                && self.drm_outputs[index].enabled
                && let Err(err) = self.drm_outputs[index]
                    .drm_output
                    .with_compositor(|compositor| compositor.clear())
            {
                failure = Some(format!(
                    "output {:?}: failed to disable connector: {err}",
                    config.output.name()
                ));
                break;
            }

            let entry = &mut self.drm_outputs[index];
            entry.current_mode = target_mode;
            entry.enabled = config.enabled;
            entry.configured_vrr = if config.adaptive_sync {
                halley_config::Vrr::On
            } else {
                halley_config::Vrr::Off
            };
            entry.output.change_current_state(
                Some(config.mode),
                Some(config.transform),
                None,
                Some(config.location),
            );
            let requested_vrr =
                configured_vrr_target(entry.configured_vrr, false, entry.vrr_support);
            set_entry_vrr(entry, requested_vrr);
            entry.pending = false;
        }

        if let Some(error) = failure {
            let mut rollback_errors = Vec::new();
            for (index, snapshot) in snapshots.iter().enumerate() {
                let (mode, location, transform, vrr, enabled) = *snapshot;
                let mode_restored = if self.drm_outputs[index].current_mode != mode {
                    match self.use_output_mode(index, mode) {
                        Ok(()) => true,
                        Err(err) => {
                            rollback_errors.push(format!(
                                "output {:?}: mode rollback failed: {err}",
                                self.drm_outputs[index].output.name()
                            ));
                            false
                        }
                    }
                } else {
                    true
                };
                let entry = &mut self.drm_outputs[index];
                if mode_restored {
                    entry.current_mode = mode;
                    entry.output.change_current_state(
                        Some(drm_output_mode(&mode)),
                        Some(transform),
                        None,
                        Some(location),
                    );
                }
                entry.configured_vrr = vrr;
                entry.enabled = enabled;
                set_entry_vrr(entry, configured_vrr_target(vrr, false, entry.vrr_support));
                entry.pending = false;
            }
            if rollback_errors.is_empty() {
                return Err(error);
            }
            return Err(format!("{error}; {}", rollback_errors.join("; ")));
        }

        let after = self.output_states();
        for entry in &mut self.drm_outputs {
            let name = entry.output.name();
            if let Some(info) = self
                .ipc_output_info
                .iter_mut()
                .find(|info| info.name == name)
            {
                info.current_mode = entry
                    .enabled
                    .then(|| {
                        entry
                            .connector
                            .modes()
                            .iter()
                            .position(|mode| *mode == entry.current_mode)
                    })
                    .flatten();
                let location = entry.output.current_location();
                info.offset_x = location.x;
                info.offset_y = location.y;
                info.vrr = crate::ipc::vrr_str(entry.configured_vrr).to_string();
                info.vrr_active = entry.vrr_active && entry.enabled && entry.dpms_enabled;
            }
        }

        Ok(before
            .into_iter()
            .zip(after)
            .filter(|(before, after)| {
                before.enabled != after.enabled
                    || before.mode != after.mode
                    || before.location != after.location
                    || before.transform != after.transform
                    || before.adaptive_sync != after.adaptive_sync
            })
            .map(|(before, after)| crate::session::output::OutputChange { before, after })
            .collect())
    }

    pub fn apply_dpms(
        &mut self,
        command: halley_ipc::DpmsCommand,
        output_name: Option<&str>,
    ) -> Result<AppliedDpms, String> {
        let targets = match output_name {
            Some(name) => {
                let index = self
                    .drm_outputs
                    .iter()
                    .position(|entry| entry.output.name() == name)
                    .ok_or_else(|| format!("unknown active output {name:?}"))?;
                vec![index]
            }
            None => (0..self.drm_outputs.len()).collect(),
        };
        let target_enabled = dpms_target_enabled(
            command,
            targets
                .iter()
                .map(|index| self.drm_outputs[*index].dpms_enabled),
        )
        .ok_or_else(|| "dpms request has no active output targets".to_string())?;
        if targets
            .iter()
            .all(|index| self.drm_outputs[*index].dpms_enabled == target_enabled)
        {
            return Err("dpms request made no change".to_string());
        }

        let mut changes = Vec::new();
        let mut failures = Vec::new();
        for index in targets {
            let entry = &mut self.drm_outputs[index];
            if entry.dpms_enabled == target_enabled {
                continue;
            }
            // Powering on touches no DRM state at all - the CRTC is re-enabled
            // as a side effect of the first ordinary frame's atomic commit, so
            // the enable and its scan-out buffer land together. Old halley and
            // niri both do exactly nothing here; `reset_state()` belongs on the
            // VT-switch path (see `resume()`), not this one, and calling it here
            // cost a ~25s event-loop stall on every wake.
            //
            // Nothing is needed to force that first frame to be non-empty
            // either: `clear()` wipes the surface's *current* mode and connector
            // set, so `commit_pending()` stays true, which makes the next
            // prepared frame a full (not partial) one, and a full frame's
            // `is_empty()` is false by construction.
            if !target_enabled {
                set_entry_vrr(entry, false);
                let clear = entry
                    .drm_output
                    .with_compositor(|compositor| compositor.clear());
                if let Err(err) = clear {
                    failures.push(format!("{}: {err}", entry.output.name()));
                    continue;
                }
                entry.direct_scanout_active = None;
            }
            entry.dpms_enabled = target_enabled;
            entry.pending = false;
            changes.push(AppliedDpmsChange {
                output: entry.output.clone(),
                enabled: target_enabled,
            });
        }

        if !changes.is_empty() {
            let names = changes
                .iter()
                .map(|change| change.output.name())
                .collect::<Vec<_>>()
                .join(", ");
            eventline::debug!(
                "tty dpms: {} outputs {names}",
                if target_enabled {
                    "powering on"
                } else {
                    "powered off"
                }
            );
        }
        Ok(AppliedDpms {
            changes,
            error: (!failures.is_empty()).then(|| {
                format!(
                    "failed to power {} outputs: {}",
                    if target_enabled { "on" } else { "off" },
                    failures.join(", ")
                )
            }),
        })
    }

    pub fn dmabuf_capabilities(&self) -> super::dmabuf::DmabufCapabilities {
        super::dmabuf::DmabufCapabilities::new(
            Some(self.primary_render_node.dev_id()),
            self.primary_formats.clone(),
        )
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        let mut renderer = match self.gpu_manager.single_renderer(&self.primary_render_node) {
            Ok(renderer) => renderer,
            Err(err) => {
                eventline::debug!("tty: primary renderer unavailable for DMA-BUF import: {err:?}");
                return false;
            }
        };
        match renderer.import_dmabuf(dmabuf, None) {
            Ok(_) => {
                dmabuf.set_node(Some(self.primary_render_node));
                true
            }
            Err(err) => {
                eventline::debug!("tty: failed to import DMA-BUF: {err}");
                false
            }
        }
    }

    pub fn dmabuf_feedback(
        &self,
        output: &Output,
    ) -> Option<&super::dmabuf::SurfaceDmabufFeedback> {
        self.drm_outputs
            .iter()
            .find(|entry| &entry.output == output)
            .and_then(|entry| entry.dmabuf_feedback.as_ref())
    }

    pub fn refresh_interval_for_output(&self, output: &Output) -> Duration {
        self.drm_outputs
            .iter()
            .find(|entry| &entry.output == output)
            .and_then(|entry| entry.output.current_mode())
            .map(|mode| mode.refresh)
            .filter(|&refresh| refresh > 0)
            .map(|refresh| Duration::from_secs_f64(1000.0 / refresh as f64))
            .unwrap_or_else(|| Duration::from_secs_f64(1.0 / 60.0))
    }

    fn use_output_mode(&mut self, index: usize, mode: Mode) -> Result<(), String> {
        let target_node = self.drm_outputs[index].render_node;
        if target_node == self.primary_render_node {
            let mut renderer = self
                .gpu_manager
                .single_renderer(&self.primary_render_node)
                .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
            self.drm_outputs[index]
                .drm_output
                .use_mode::<GlesRenderer, SolidColorRenderElement>(
                    mode,
                    renderer.as_mut(),
                    &DrmOutputRenderElements::default(),
                )
                .map_err(|err| format!("{err}"))
        } else {
            let output_format = self.drm_outputs[index].drm_output.format();
            let mut renderer = self
                .gpu_manager
                .renderer(&self.primary_render_node, &target_node, output_format)
                .map_err(|err| format!("multi-GPU renderer unavailable: {err:?}"))?;
            let render_elements =
                DrmOutputRenderElements::<TtyMultiRenderer<'_>, PrimaryGpuTextureElement>::default(
                );
            self.drm_outputs[index]
                .drm_output
                .use_mode::<TtyMultiRenderer<'_>, PrimaryGpuTextureElement>(
                    mode,
                    &mut renderer,
                    &render_elements,
                )
                .map_err(|err| format!("{err}"))
        }
    }

    /// Applies only effective per-output differences. In particular, a
    /// changed DP-1 block never calls `use_mode` or `change_current_state`
    /// for DP-2.
    pub fn apply_output_config(
        &mut self,
        outputs_config: &[halley_config::OutputConfig],
    ) -> Vec<AppliedOutputChange> {
        let mut changes = Vec::new();

        for index in 0..self.drm_outputs.len() {
            let (target, diff, enable_changed) = {
                let entry = &self.drm_outputs[index];
                let configured = outputs_config
                    .iter()
                    .find(|cfg| cfg.name == entry.output.name());
                let target = output_target(&entry.connector, configured);
                let location = entry.output.current_location();
                let current = HardwareOutputState {
                    mode: drm_output_mode(&entry.current_mode),
                    offset: (location.x, location.y),
                    transform: entry.output.current_transform(),
                    vrr: entry.configured_vrr,
                };
                let requested = HardwareOutputState {
                    mode: drm_output_mode(&target.mode),
                    offset: target.offset,
                    transform: target.transform,
                    vrr: target.vrr,
                };
                (target, output_diff(current, requested), !entry.enabled)
            };

            if !(diff.mode_changed
                || diff.offset_changed
                || diff.transform_changed
                || diff.vrr_changed
                || enable_changed)
            {
                continue;
            }

            if diff.mode_changed {
                let result = self.use_output_mode(index, target.mode);
                if let Err(err) = result {
                    let name = self.drm_outputs[index].output.name();
                    eventline::error!(
                        "output {name:?}: failed to apply configured mode, keeping previous state: {err}"
                    );
                    continue;
                }
            }

            let (name, output, connector, current_mode, configured_vrr, vrr_supported, vrr_active) = {
                let entry = &mut self.drm_outputs[index];
                entry.output.change_current_state(
                    diff.mode_changed.then(|| drm_output_mode(&target.mode)),
                    diff.transform_changed.then_some(target.transform),
                    None,
                    diff.offset_changed.then(|| target.offset.into()),
                );
                entry.current_mode = target.mode;
                entry.configured_vrr = target.vrr;
                entry.enabled = true;
                if diff.mode_changed {
                    entry.vrr_support = query_vrr_support(
                        &entry.drm_output,
                        entry.connector.handle(),
                        &entry.output.name(),
                    );
                }
                let requested_vrr = configured_vrr_target(target.vrr, false, entry.vrr_support);
                set_entry_vrr(entry, requested_vrr);
                if diff.mode_changed || diff.vrr_changed {
                    warn_vrr_configuration(&entry.output.name(), target.vrr, entry.vrr_support);
                }
                (
                    entry.output.name(),
                    entry.output.clone(),
                    entry.connector.clone(),
                    entry.current_mode,
                    entry.configured_vrr,
                    vrr_is_supported(entry.vrr_support),
                    entry.vrr_active,
                )
            };

            if !self.drm_outputs[index].dpms_enabled {
                let entry = &mut self.drm_outputs[index];
                if let Err(err) = entry
                    .drm_output
                    .with_compositor(|compositor| compositor.clear())
                {
                    eventline::error!(
                        "output {name:?}: failed to restore DPMS-off state after configuration: {err}"
                    );
                }
                entry.pending = false;
            }

            let info = connector_output_info(
                name.clone(),
                &connector,
                Some(current_mode),
                {
                    let location = output.current_location();
                    (location.x, location.y)
                },
                configured_vrr,
                vrr_supported,
                vrr_active,
            );
            if let Some(existing) = self
                .ipc_output_info
                .iter_mut()
                .find(|existing| existing.name == name)
            {
                *existing = info;
            }

            changes.push(AppliedOutputChange {
                output,
                mode_changed: diff.mode_changed,
                size_changed: enable_changed || diff.size_changed,
                layout_changed: enable_changed
                    || diff.size_changed
                    || diff.offset_changed
                    || diff.transform_changed,
            });
        }

        changes
    }

    /// Reacquire DRM master and resync KMS state after a VT switch back.
    /// Kept separate from `Renderable` (like `WinitBackend::request_redraw()`)
    /// since there's no shared shape with winit worth forcing into one trait
    /// method - the session-event closure only ever needs `&mut TtyBackend`,
    /// never the whole compositor state (the flaw old halley's
    /// `apply_tty_reload(..., st: &mut Halley, ...)` had).
    pub fn resume(&mut self) -> Result<(), Box<dyn Error>> {
        for gpu in &mut self.gpus {
            gpu.drm_output_manager.lock().activate(false)?;
        }
        self.recover_outputs_after_loss("VT activation");
        Ok(())
    }

    /// Discard scan-out state after system sleep, where logind keeps the
    /// session active and libseat therefore emits no activate event.
    pub fn recover_after_system_sleep(&mut self) {
        self.recover_outputs_after_loss("system resume");
    }

    fn recover_outputs_after_loss(&mut self, reason: &str) {
        // Any frame that was in flight before the VT switch away is gone -
        // without this, a stale `pending` would permanently block that
        // output from rendering again (its VBlank is never coming).
        for entry in &mut self.drm_outputs {
            entry.pending = false;
            entry.direct_scanout_active = None;
            entry.vrr_failure_warned_for = None;
            // The manager activation above refreshes each compositor's KMS
            // state. Drop its pre-switch scan-out buffers as well so the
            // first frame after returning cannot reuse storage owned by the
            // previous DRM-master epoch.
            entry.drm_output.reset_buffers();
            if let Err(err) = entry
                .gamma
                .restore_after_resume(self.gpus[entry.gpu_index].drm_output_manager.device())
            {
                eventline::warn!(
                    "output {:?}: failed to restore gamma after {reason}: {err}",
                    entry.output.name(),
                );
            }
            set_entry_vrr(
                entry,
                configured_vrr_target(entry.configured_vrr, false, entry.vrr_support),
            );
            if !entry.dpms_enabled
                && let Err(err) = entry
                    .drm_output
                    .with_compositor(|compositor| compositor.clear())
            {
                eventline::error!(
                    "output {:?}: failed to restore DPMS-off state after {reason}: {err}",
                    entry.output.name(),
                );
            }
        }
    }

    /// Drop DRM master before a VT switch away.
    pub fn pause(&mut self) {
        for gpu in &mut self.gpus {
            gpu.drm_output_manager.pause();
        }
    }

    /// Ask the active libseat session to switch virtual terminals.
    ///
    /// This stays TTY-backend-specific: nested sessions have no VT to own,
    /// and the session notifier remains the sole authority for the ensuing
    /// pause/resume lifecycle.
    pub fn change_vt(&mut self, vt: i32) -> Result<(), Box<dyn Error>> {
        self.session.change_vt(vt)?;
        Ok(())
    }

    /// A cheap clone of the session, for building a libinput context
    /// externally - matches the existing "notifiers aren't owned by the
    /// backend, whatever drives the loop inserts them" pattern, extended to
    /// input. `LibSeatSession` is `Clone` (internally `Weak`-based), so this
    /// isn't a real second session.
    pub fn session(&self) -> LibSeatSession {
        self.session.clone()
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(&mut GlesRenderer) -> T) -> T {
        let mut renderer = self
            .gpu_manager
            .single_renderer(&self.primary_render_node)
            .expect("primary GPU renderer remains available");
        f(renderer.as_mut())
    }

    /// Acknowledge a page-flip completion for one output, called from the
    /// `DrmEvent::VBlank(crtc)` handler - the DRM-path equivalent of
    /// `WinitBackend::request_redraw()`. Takes a `crtc::Handle` since with
    /// multiple outputs "which one flipped" is no longer implicit. Must be
    /// followed by a fresh `render()` call to queue that output's next frame.
    pub fn frame_submitted(
        &mut self,
        gpu_index: usize,
        crtc: crtc::Handle,
    ) -> Result<Option<FrameSubmission>, Box<dyn Error>> {
        if let Some(entry) = self
            .drm_outputs
            .iter_mut()
            .find(|e| e.gpu_index == gpu_index && e.crtc == crtc)
        {
            // The kernel event completed the page flip regardless of whether
            // Smithay can advance a queued follow-up frame. Clear our gate
            // first so an acknowledgement error cannot wedge this output
            // permanently in `pending`.
            entry.pending = false;
            return entry.drm_output.frame_submitted().map_err(Into::into);
        }
        Ok(None)
    }
}

impl Drop for TtyBackend {
    fn drop(&mut self) {
        for gpu in &mut self.gpus {
            if let Some(fd) = gpu.session_fd.take() {
                if let Err(err) = self.session.close(fd) {
                    eventline::warn!("tty: failed to close GPU session fd: {err}");
                }
            }
        }
    }
}

impl crate::ipc::OutputInfoSource for TtyBackend {
    fn output_info(&self) -> Vec<halley_ipc::OutputInfo> {
        let mut outputs = self.ipc_output_info.clone();
        for entry in &self.drm_outputs {
            if let Some(info) = outputs
                .iter_mut()
                .find(|info| info.name == entry.output.name())
            {
                info.vrr_supported = vrr_is_supported(entry.vrr_support);
                info.vrr_active = entry.vrr_active && entry.dpms_enabled && entry.enabled;
                if !entry.dpms_enabled || !entry.enabled {
                    info.current_mode = None;
                }
            }
        }
        outputs
    }
}

impl Renderable for TtyBackend {
    fn render(
        &mut self,
        output: &Output,
        request: RenderRequest<'_>,
    ) -> Result<RenderOutcome, Box<dyn Error>> {
        let primary_output = self.primary_output.clone();
        let entry_index = self
            .drm_outputs
            .iter()
            .position(|entry| &entry.output == output)
            .ok_or_else(|| format!("unknown tty output {:?}", output.name()))?;
        if !self.drm_outputs[entry_index].dpms_enabled || !self.drm_outputs[entry_index].enabled {
            return Ok(RenderOutcome::new(RenderStatus::Skipped, None));
        }
        // DRM rejects a second commit while the previous page flip is still
        // pending. The next VBlank clears this flag and schedules another
        // render, so skipping here does not lose scene changes.
        if self.drm_outputs[entry_index].pending {
            return Ok(RenderOutcome::new(RenderStatus::Skipped, None));
        }
        let requested_vrr = configured_vrr_target(
            self.drm_outputs[entry_index].configured_vrr,
            request.frame.vrr_auto_eligible,
            self.drm_outputs[entry_index].vrr_support,
        );
        set_entry_vrr(&mut self.drm_outputs[entry_index], requested_vrr);
        let entry_output = self.drm_outputs[entry_index].output.clone();
        let target_node = self.drm_outputs[entry_index].render_node;
        let is_cross_gpu = target_node != self.primary_render_node;
        let output_geometry = request
            .desktop
            .space
            .output_geometry(&entry_output)
            .ok_or_else(|| format!("tty output {:?} is not mapped", entry_output.name()))?;
        let space = request.desktop.space;
        let session_lock = request.desktop.session_lock;
        let clear = if request.desktop.session_lock.active() {
            crate::render::SESSION_LOCK_COLOR
        } else {
            request.frame.clear
        };
        let target_presentation_time = request.frame.target_presentation_time;
        let force_full_repaint = request.frame.force_full_repaint;
        let session_lock_generation = request.desktop.session_lock.frame_generation();
        let elements = {
            let mut renderer = self
                .gpu_manager
                .single_renderer(&self.primary_render_node)
                .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
            crate::render::scene::build(
                renderer.as_mut(),
                &entry_output,
                &primary_output,
                output_geometry,
                request,
            )?
        };
        if force_full_repaint {
            // Geometry animations (window open/close, zoom, camera motion)
            // can touch a different pixel set in every swapchain buffer. A
            // stale buffer-age history then presents old client pixels on
            // alternating page flips. Preserve the buffers themselves, but
            // make Smithay repaint the complete output for animated frames.
            self.drm_outputs[entry_index]
                .drm_output
                .with_compositor(|compositor| compositor.reset_buffer_ages());
        }
        let (element_states, direct_scanout, is_empty) = if !is_cross_gpu {
            let mut renderer = self
                .gpu_manager
                .single_renderer(&self.primary_render_node)
                .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
            let result =
                self.drm_outputs[entry_index]
                    .drm_output
                    .render_frame::<_, SceneElement>(
                        renderer.as_mut(),
                        &elements,
                        clear,
                        dmabuf::frame_flags_for_scene(elements.iter().any(
                            smithay::backend::renderer::element::Element::is_framebuffer_effect,
                        )),
                    )?;
            if result.needs_sync()
                && let PrimaryPlaneElement::Swapchain(element) = &result.primary_element
                && let Err(err) = element.sync.wait()
            {
                eventline::warn!(
                    "output {:?}: failed waiting for rendered frame completion: {err}",
                    entry_output.name()
                );
            }
            (
                result.states.clone(),
                matches!(&result.primary_element, PrimaryPlaneElement::Element(_)),
                result.is_empty,
            )
        } else {
            let mode_size = self.drm_outputs[entry_index].current_mode.size();
            let mode_size = smithay::utils::Size::<i32, smithay::utils::Buffer>::from((
                mode_size.0 as i32,
                mode_size.1 as i32,
            ));
            let output_scale = Scale::from(entry_output.current_scale().fractional_scale());
            // Scene elements are built in the output's transformed physical
            // space (the same space used by `DrmOutput::render_frame`). Keep
            // the intermediate texture in that space too; the final DRM
            // render applies the output transform exactly once when it sends
            // this texture to the secondary GPU's scanout buffer.
            let (texture_size, logical_size) =
                composed_texture_sizes(mode_size, entry_output.current_transform(), output_scale);
            let physical_size = smithay::utils::Size::<i32, smithay::utils::Physical>::from((
                texture_size.w,
                texture_size.h,
            ));
            let texture = {
                let mut renderer = self
                    .gpu_manager
                    .single_renderer(&self.primary_render_node)
                    .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
                let output_name = entry_output.name();
                let needs_new = self
                    .composed_frame_cache
                    .get(&output_name)
                    .is_none_or(|texture| texture.size() != texture_size);
                if needs_new {
                    let texture = <GlesRenderer as Offscreen<
                        smithay::backend::renderer::gles::GlesTexture,
                    >>::create_buffer(
                        renderer.as_mut(), Fourcc::Abgr8888, texture_size
                    )?;
                    self.composed_frame_cache
                        .insert(output_name.clone(), texture);
                }
                self.composed_frame_cache
                    .get(&output_name)
                    .cloned()
                    .ok_or_else(|| "cross-GPU composition texture disappeared".to_string())?
            };
            {
                let mut renderer = self
                    .gpu_manager
                    .single_renderer(&self.primary_render_node)
                    .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
                let mut texture = texture.clone();
                let gles = renderer.as_mut();
                let mut target = gles.bind(&mut texture)?;
                let mut frame = gles.render(
                    &mut target,
                    physical_size,
                    smithay::utils::Transform::Normal,
                )?;
                let damage = [
                    smithay::utils::Rectangle::<i32, smithay::utils::Physical>::from_size(
                        physical_size,
                    ),
                ];
                frame.clear(clear, &damage)?;
                let effect_cache = UserDataMap::new();
                for element in elements.iter().rev() {
                    let geometry = element.geometry(output_scale);
                    let Some(visible) = geometry.intersection(damage[0]) else {
                        continue;
                    };
                    let local_damage = [Rectangle::new(visible.loc - geometry.loc, visible.size)];
                    let cache = if element.is_framebuffer_effect() {
                        element.capture_framebuffer(
                            &mut frame,
                            element.src(),
                            geometry,
                            &effect_cache,
                        )?;
                        Some(&effect_cache)
                    } else {
                        None
                    };
                    element.draw(
                        &mut frame,
                        element.src(),
                        geometry,
                        &local_damage,
                        &[],
                        cache,
                    )?;
                }
                let _ = frame.finish()?;
            }
            let element_states =
                scene_element_states(&elements, output_scale, Rectangle::from_size(physical_size));
            let texture_buffer = {
                let mut renderer = self
                    .gpu_manager
                    .single_renderer(&self.primary_render_node)
                    .map_err(|err| format!("primary renderer unavailable: {err:?}"))?;
                TextureBuffer::from_texture(
                    renderer.as_mut(),
                    texture,
                    1,
                    smithay::utils::Transform::Normal,
                    Some(Vec::new()),
                )
            };
            let element = PrimaryGpuTextureElement(TextureRenderElement::from_texture_buffer(
                (0.0, 0.0),
                &texture_buffer,
                Some(1.0),
                Some(Rectangle::from_size(physical_size).to_f64().to_logical(1.0)),
                Some(logical_size),
                Kind::Unspecified,
            ));
            let mut renderer = self
                .gpu_manager
                .renderer(
                    &self.primary_render_node,
                    &target_node,
                    self.drm_outputs[entry_index].drm_output.format(),
                )
                .map_err(|err| format!("multi-GPU renderer unavailable: {err:?}"))?;
            let elements = [element];
            let result = self.drm_outputs[entry_index]
                .drm_output
                .render_frame::<_, PrimaryGpuTextureElement>(
                    &mut renderer,
                    &elements,
                    clear,
                    FrameFlags::empty(),
                )?;
            if result.needs_sync()
                && let PrimaryPlaneElement::Swapchain(element) = &result.primary_element
                && let Err(err) = element.sync.wait()
            {
                eventline::warn!(
                    "output {:?}: failed waiting for rendered frame completion: {err}",
                    entry_output.name()
                );
            }
            (element_states, false, result.is_empty)
        };
        if is_empty {
            return Ok(RenderOutcome::new(
                RenderStatus::Skipped,
                Some(element_states),
            ));
        }

        let presentation_feedback = crate::wayland::presentation::take_output_feedback(
            &entry_output,
            &primary_output,
            space,
            session_lock,
            &element_states,
        );
        let variable_refresh = self.drm_outputs[entry_index].vrr_active;
        self.drm_outputs[entry_index]
            .drm_output
            .queue_frame(FrameSubmission {
                target_presentation_time,
                presentation_feedback,
                session_lock_generation,
                variable_refresh,
            })?;
        self.drm_outputs[entry_index].pending = true;
        set_entry_direct_scanout(&mut self.drm_outputs[entry_index], direct_scanout);
        Ok(RenderOutcome::new(
            RenderStatus::Submitted,
            Some(element_states),
        ))
    }
}

fn set_entry_vrr(entry: &mut DrmOutputEntry, requested: bool) {
    let requested = requested && entry.dpms_enabled && vrr_is_supported(entry.vrr_support);
    if requested == entry.vrr_active {
        return;
    }
    let name = entry.output.name();
    let previous = entry.vrr_active;
    if let Err(err) = entry
        .drm_output
        .with_compositor(|compositor| compositor.use_vrr(requested))
        && entry.vrr_failure_warned_for != Some(requested)
    {
        eventline::warn!(
            "output {name:?}: failed to {} VRR: {err}",
            if requested { "enable" } else { "disable" }
        );
        entry.vrr_failure_warned_for = Some(requested);
    }
    entry.vrr_active = entry
        .drm_output
        .with_compositor(|compositor| compositor.vrr_enabled());
    if entry.vrr_active == requested {
        entry.vrr_failure_warned_for = None;
    } else if entry.vrr_failure_warned_for != Some(requested) {
        eventline::warn!(
            "output {name:?}: VRR {} request did not take effect",
            if requested { "enable" } else { "disable" }
        );
        entry.vrr_failure_warned_for = Some(requested);
    }
    if entry.vrr_active != previous {
        eventline::info!(
            "output {name:?}: VRR {}",
            if entry.vrr_active {
                "enabled"
            } else {
                "disabled"
            }
        );
    }
}

fn set_entry_direct_scanout(entry: &mut DrmOutputEntry, active: bool) {
    if entry.direct_scanout_active == Some(active) {
        return;
    }
    entry.direct_scanout_active = Some(active);
    eventline::info!(
        "output {:?}: render path is {}",
        entry.output.name(),
        if active {
            "direct scanout"
        } else {
            "composited"
        }
    );
}

fn query_vrr_support(
    drm_output: &TtyDrmOutput,
    connector: connector::Handle,
    name: &str,
) -> VrrSupport {
    match drm_output.with_compositor(|compositor| compositor.vrr_supported(connector)) {
        Ok(support) => support,
        Err(err) => {
            eventline::warn!("output {name:?}: failed to query VRR support: {err}");
            VrrSupport::NotSupported
        }
    }
}

fn vrr_is_supported(support: VrrSupport) -> bool {
    !matches!(support, VrrSupport::NotSupported)
}

fn warn_vrr_configuration(name: &str, configured: halley_config::Vrr, support: VrrSupport) {
    match (configured, support) {
        (halley_config::Vrr::Off, _) | (_, VrrSupport::Supported) => {}
        (_, VrrSupport::NotSupported) => {
            eventline::warn!(
                "output {name:?}: VRR is configured but the connector does not support it"
            );
        }
        (halley_config::Vrr::Auto, VrrSupport::RequiresModeset) => {
            eventline::warn!(
                "output {name:?}: automatic VRR switching would require modesets, \
                 so VRR remains disabled; use vrr \"on\" to enable it persistently"
            );
        }
        (halley_config::Vrr::On, VrrSupport::RequiresModeset) => {}
    }
}

fn configured_vrr_target(
    configured: halley_config::Vrr,
    auto_eligible: bool,
    support: VrrSupport,
) -> bool {
    match configured {
        halley_config::Vrr::Off => false,
        halley_config::Vrr::On => vrr_is_supported(support),
        halley_config::Vrr::Auto => auto_eligible && matches!(support, VrrSupport::Supported),
    }
}

fn dpms_target_enabled(
    command: halley_ipc::DpmsCommand,
    current: impl IntoIterator<Item = bool>,
) -> Option<bool> {
    match command {
        halley_ipc::DpmsCommand::Off => Some(false),
        halley_ipc::DpmsCommand::On => Some(true),
        halley_ipc::DpmsCommand::Toggle => {
            let current = current.into_iter().collect::<Vec<_>>();
            (!current.is_empty()).then(|| !current.iter().all(|enabled| *enabled))
        }
    }
}

#[cfg(test)]
mod dpms_tests {
    use smithay::backend::drm::VrrSupport;

    use super::{configured_vrr_target, dpms_target_enabled};

    #[test]
    fn toggle_turns_all_off_only_when_every_target_is_on() {
        assert_eq!(
            dpms_target_enabled(halley_ipc::DpmsCommand::Toggle, [true, true]),
            Some(false)
        );
        assert_eq!(
            dpms_target_enabled(halley_ipc::DpmsCommand::Toggle, [true, false]),
            Some(true)
        );
        assert_eq!(
            dpms_target_enabled(halley_ipc::DpmsCommand::Toggle, [false, false]),
            Some(true)
        );
        assert_eq!(
            dpms_target_enabled(halley_ipc::DpmsCommand::Toggle, []),
            None
        );
    }

    #[test]
    fn vrr_policy_keeps_on_and_off_absolute_and_gates_auto() {
        let supported = VrrSupport::Supported;
        assert!(!configured_vrr_target(
            halley_config::Vrr::Off,
            true,
            supported
        ));
        assert!(!configured_vrr_target(
            halley_config::Vrr::Off,
            false,
            supported
        ));
        assert!(configured_vrr_target(
            halley_config::Vrr::On,
            true,
            supported
        ));
        assert!(configured_vrr_target(
            halley_config::Vrr::On,
            false,
            supported
        ));
        assert!(configured_vrr_target(
            halley_config::Vrr::Auto,
            true,
            supported
        ));
        assert!(!configured_vrr_target(
            halley_config::Vrr::Auto,
            false,
            supported
        ));
    }

    #[test]
    fn vrr_policy_does_not_dynamically_modeset_for_auto() {
        assert!(!configured_vrr_target(
            halley_config::Vrr::Auto,
            true,
            VrrSupport::RequiresModeset
        ));
        assert!(configured_vrr_target(
            halley_config::Vrr::On,
            false,
            VrrSupport::RequiresModeset
        ));
        assert!(!configured_vrr_target(
            halley_config::Vrr::On,
            true,
            VrrSupport::NotSupported
        ));
    }
}

#[cfg(test)]
mod cross_gpu_composition_tests {
    use super::{composed_texture_sizes, scene_element_states};
    use smithay::backend::renderer::Color32F;
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;
    use smithay::backend::renderer::element::{Element, Kind, RenderElementPresentationState};
    use smithay::utils::{Rectangle, Scale};

    #[test]
    fn composed_states_keep_front_element_and_skip_occluded_surface() {
        let front = SolidColorRenderElement::new(
            smithay::backend::renderer::element::Id::new(),
            Rectangle::new((0, 0).into(), (100, 100).into()),
            0,
            Color32F::new(1.0, 1.0, 1.0, 1.0),
            Kind::Unspecified,
        );
        let back = SolidColorRenderElement::new(
            smithay::backend::renderer::element::Id::new(),
            Rectangle::new((0, 0).into(), (100, 100).into()),
            0,
            Color32F::new(0.0, 0.0, 0.0, 1.0),
            Kind::Unspecified,
        );
        let front_id = front.id().clone();
        let back_id = back.id().clone();
        let states = scene_element_states(
            &[front, back],
            Scale::from(1.0),
            Rectangle::from_size((100, 100).into()),
        );

        let front_state = states.element_render_state(front_id).unwrap();
        assert_eq!(front_state.visible_area, 10_000);
        assert_eq!(
            front_state.presentation_state,
            RenderElementPresentationState::Rendering { reason: None }
        );
        let back_state = states.element_render_state(back_id).unwrap();
        assert_eq!(back_state.visible_area, 0);
        assert_eq!(
            back_state.presentation_state,
            RenderElementPresentationState::Skipped
        );
    }

    #[test]
    fn composed_texture_geometry_accounts_for_fractional_scale_and_rotation() {
        let (texture_size, logical_size) = composed_texture_sizes(
            (1920, 1080).into(),
            smithay::utils::Transform::_90,
            Scale::from(1.5),
        );
        assert_eq!(texture_size, (1080, 1920).into());
        assert_eq!(logical_size, (720, 1280).into());
    }
}
