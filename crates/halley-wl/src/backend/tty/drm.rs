use super::*;
use std::collections::HashMap;
use std::ffi::CStr;

use crate::compositor::interaction::ResizeCtx;
use crate::protocol::wayland::portal;
use halley_ipc::{ModeInfo, OutputInfo, OutputStatus};

use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags, PrimaryPlaneElement};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType};
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{
    Element, Kind, RenderElement, UnderlyingStorage,
    memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    render_elements,
    surface::render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{GpuManager, MultiFrame, MultiRenderer};
use smithay::backend::renderer::{Bind, Offscreen, Texture};
use smithay::desktop::{PopupManager, utils::bbox_from_surface_tree};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::OutputModeSource;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::utils::{Physical, Scale, Size, Transform};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;

render_elements! {
    HalleyDirectScanoutElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
}

trait AsGlesFrame<'frame, 'buffer>
where
    Self: 'frame,
{
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

struct PrimaryGpuTextureElement(TextureRenderElement<GlesTexture>);

impl Element for PrimaryGpuTextureElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        self.0.id()
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        self.0.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> smithay::utils::Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.0.transform()
    }

    fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
        self.0.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<smithay::backend::renderer::utils::CommitCounter>,
    ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(
        &self,
        scale: Scale<f64>,
    ) -> smithay::backend::renderer::utils::OpaqueRegions<i32, Physical> {
        self.0.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl RenderElement<GlesRenderer> for PrimaryGpuTextureElement {
    fn draw(
        &self,
        frame: &mut smithay::backend::renderer::gles::GlesFrame<'_, '_>,
        src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
        dst: smithay::utils::Rectangle<i32, Physical>,
        damage: &[smithay::utils::Rectangle<i32, Physical>],
        opaque_regions: &[smithay::utils::Rectangle<i32, Physical>],
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        RenderElement::<GlesRenderer>::draw(&self.0, frame, src, dst, damage, opaque_regions)
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl<'render> RenderElement<TtyMultiRenderer<'render>> for PrimaryGpuTextureElement {
    fn draw(
        &self,
        frame: &mut TtyMultiFrame<'render, '_, '_>,
        src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
        dst: smithay::utils::Rectangle<i32, Physical>,
        damage: &[smithay::utils::Rectangle<i32, Physical>],
        opaque_regions: &[smithay::utils::Rectangle<i32, Physical>],
    ) -> Result<(), <TtyMultiRenderer<'render> as smithay::backend::renderer::RendererSuper>::Error>
    {
        RenderElement::<GlesRenderer>::draw(
            &self.0,
            frame.as_gles_frame(),
            src,
            dst,
            damage,
            opaque_regions,
        )?;
        Ok(())
    }

    fn underlying_storage(
        &self,
        _renderer: &mut TtyMultiRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// The DrmCompositor type for a single output in halley.
///
/// DrmCompositor handles the full atomic-KMS pipeline:
///   - allocates GBM buffers for rendering
///   - exports them as DRM framebuffers
///   - commits them to the CRTC atomically (non-blocking ALLOW_MODESET)
///   - tracks buffer age for damage-based re-rendering
///   - clear() disables the CRTC atomically without blocking
pub(crate) type HalleyDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,           // buffer allocator
    GbmFramebufferExporter<DrmDeviceFd>, // framebuffer exporter
    (),                                  // per-frame user data (unused)
    DrmDeviceFd,                         // raw DRM fd
>;

pub(crate) type TtyGpuBackend = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;
pub(crate) type TtyGpuManager = GpuManager<TtyGpuBackend>;
pub(crate) type TtyMultiRenderer<'render> =
    MultiRenderer<'render, 'render, TtyGpuBackend, TtyGpuBackend>;
pub(crate) type TtyMultiFrame<'render, 'frame, 'buffer> =
    MultiFrame<'render, 'render, 'frame, 'buffer, TtyGpuBackend, TtyGpuBackend>;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TtyFrameQueueReport {
    pub(crate) queued: bool,
    pub(crate) animation_redraw_active: bool,
    pub(crate) direct_scanout_active: bool,
    pub(crate) composed: bool,
    pub(crate) sync_wait: Option<Duration>,
}

const TTY_SYNC_WAIT_WARN_MS: u64 = 8;

fn queue_tty_frame_or_clear_on_failure(
    compositor: &mut HalleyDrmCompositor,
    output_name: &str,
) -> Result<(), io::Error> {
    match compositor.queue_frame(()) {
        Ok(()) => Ok(()),
        Err(err) => {
            let recovery = match compositor.clear() {
                Ok(()) => "cleared drm surface for retry".to_string(),
                Err(clear_err) => format!("failed to clear drm surface for retry: {clear_err}"),
            };
            compositor.reset_buffers();
            Err(io::Error::other(format!(
                "queue_frame failed for {output_name}: {err}; {recovery}"
            )))
        }
    }
}

fn tty_env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) struct TtyDrmProbe {
    pub(crate) devices: Vec<TtyDrmDevice>,
    pub(crate) gpu_manager: Rc<RefCell<TtyGpuManager>>,
    pub(crate) primary_render_node: DrmNode,
    pub(crate) primary_dev_fd: DrmDeviceFd,
    pub(crate) outputs: Vec<TtyDrmOutput>,
}

pub(crate) struct TtyDrmDevice {
    pub(crate) card_path: std::path::PathBuf,
    #[allow(dead_code)]
    pub(crate) node: DrmNode,
    pub(crate) render_node: DrmNode,
    pub(crate) dev: Rc<RefCell<DrmDevice>>,
    pub(crate) gbm: Rc<GbmDevice<DrmDeviceFd>>,
    pub(crate) notifier: Option<smithay::backend::drm::DrmDeviceNotifier>,
    /// The DrmDeviceFd kept alive so GbmDevice references stay valid.
    pub(crate) dev_fd: DrmDeviceFd,
}

pub(crate) struct TtyDrmOutput {
    #[allow(dead_code)]
    pub(crate) connector: drm_control::connector::Handle,
    pub(crate) crtc: drm_control::crtc::Handle,
    pub(crate) connector_name: String,
    pub(crate) mode: drm_control::Mode,
    #[allow(dead_code)]
    pub(crate) device_node: DrmNode,
    pub(crate) render_node: DrmNode,
    /// Atomic DRM compositor — replaces GbmBufferedSurface.
    pub(crate) compositor: Rc<RefCell<HalleyDrmCompositor>>,
}

pub(crate) struct TtyOutputCaptureBackend {
    pub(crate) gpu_manager: Rc<RefCell<TtyGpuManager>>,
    pub(crate) primary_render_node: DrmNode,
    pub(crate) outputs: Rc<RefCell<Vec<TtyDrmOutput>>>,
    pub(crate) pointer_state: Rc<RefCell<PointerState>>,
    pub(crate) dmabuf_formats: Vec<smithay::backend::allocator::Format>,
}

impl portal::OutputCaptureBackend for TtyOutputCaptureBackend {
    fn capture_dmabuf_formats(&self) -> Vec<smithay::backend::allocator::Format> {
        self.dmabuf_formats.clone()
    }

    fn capture_output_shm(
        &self,
        st: &mut Halley,
        output_name: &str,
        overlay_cursor: bool,
        logical_region: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
    ) -> Result<portal::ShmCaptureFrame, Box<dyn Error>> {
        let outputs = self
            .outputs
            .try_borrow()
            .map_err(|_| io::Error::other("tty outputs already borrowed during screencopy"))?;
        let output = outputs
            .iter()
            .find(|output| output.connector_name == output_name)
            .ok_or_else(|| io::Error::other(format!("unknown tty output {output_name}")))?;
        let (w, h) = output.mode.size();
        let physical_size: smithay::utils::Size<i32, smithay::utils::Physical> =
            (w as i32, h as i32).into();
        let ps = self
            .pointer_state
            .try_borrow()
            .map_err(|_| io::Error::other("pointer state already borrowed during screencopy"))?;
        let now = Instant::now();
        let resize_preview = ps.resize;
        let (hover_node, preview_hover_node) =
            resolve_hover_targets_for_monitor(st, &ps, now, output_name);
        let cursor_screen = overlay_cursor.then_some(ps.screen);
        drop(ps);

        let mut gpu_manager = self
            .gpu_manager
            .try_borrow_mut()
            .map_err(|_| io::Error::other("tty gpu manager already borrowed during screencopy"))?;
        let mut renderer = gpu_manager.single_renderer(&self.primary_render_node)?;
        portal::capture_output_via_renderer(
            renderer.as_mut(),
            st,
            output_name,
            physical_size,
            st.output_transform_for(output_name),
            resize_preview,
            hover_node,
            preview_hover_node,
            cursor_screen,
            overlay_cursor,
            logical_region,
        )
    }

    fn capture_output_dmabuf(
        &self,
        st: &mut Halley,
        output_name: &str,
        overlay_cursor: bool,
        logical_region: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
        dmabuf: &mut smithay::backend::allocator::dmabuf::Dmabuf,
    ) -> Result<crate::backend::interface::CaptureDmabufResult, Box<dyn Error>> {
        let outputs = self.outputs.try_borrow().map_err(|_| {
            io::Error::other("tty outputs already borrowed during dma-buf screencopy")
        })?;
        let output = outputs
            .iter()
            .find(|output| output.connector_name == output_name)
            .ok_or_else(|| io::Error::other(format!("unknown tty output {output_name}")))?;
        let (w, h) = output.mode.size();
        let physical_size: smithay::utils::Size<i32, smithay::utils::Physical> =
            (w as i32, h as i32).into();
        let ps = self.pointer_state.try_borrow().map_err(|_| {
            io::Error::other("pointer state already borrowed during dma-buf screencopy")
        })?;
        let now = Instant::now();
        let resize_preview = ps.resize;
        let (hover_node, preview_hover_node) =
            resolve_hover_targets_for_monitor(st, &ps, now, output_name);
        let cursor_screen = overlay_cursor.then_some(ps.screen);
        drop(ps);

        let mut gpu_manager = self.gpu_manager.try_borrow_mut().map_err(|_| {
            io::Error::other("tty gpu manager already borrowed during dma-buf screencopy")
        })?;
        let mut renderer = gpu_manager.single_renderer(&self.primary_render_node)?;
        portal::capture_output_into_dmabuf_via_renderer(
            renderer.as_mut(),
            st,
            output_name,
            physical_size,
            st.output_transform_for(output_name),
            resize_preview,
            hover_node,
            preview_hover_node,
            cursor_screen,
            overlay_cursor,
            logical_region,
            dmabuf,
        )
    }

    fn capture_window_png(
        &self,
        st: &mut Halley,
        output_name: &str,
        node_id: halley_core::field::NodeId,
        output_path: &std::path::Path,
    ) -> Result<(), Box<dyn Error>> {
        let mut gpu_manager = self.gpu_manager.try_borrow_mut().map_err(|_| {
            io::Error::other("tty gpu manager already borrowed during window capture")
        })?;
        let mut renderer = gpu_manager.single_renderer(&self.primary_render_node)?;
        crate::window::capture_window_to_png_via_renderer(
            renderer.as_mut(),
            st,
            output_name,
            node_id,
            output_path,
        )
    }
}

pub(crate) fn probe_tty_drm_device_via_session(
    seat: &str,
    session: Rc<RefCell<LibSeatSession>>,
    tuning: &RuntimeTuning,
) -> Result<TtyDrmProbe, Box<dyn Error>> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(card) = primary_gpu(seat)? {
        candidates.push(card);
    }
    for card in all_gpus(seat)? {
        if !candidates.iter().any(|existing| existing == &card) {
            candidates.push(card);
        }
    }
    if candidates.is_empty() {
        return Err(
            io::Error::other(format!("no drm card devices found for seat={}", seat)).into(),
        );
    }

    let gpu_manager = Rc::new(RefCell::new(GpuManager::new(GbmGlesBackend::default())?));
    let mut devices = Vec::new();
    let mut outputs = Vec::new();
    let mut primary_render_node = None;
    let mut primary_dev_fd = None;
    let mut last_err: Option<String> = None;
    let tried_paths = candidates
        .iter()
        .map(|card| card.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for card in candidates {
        match probe_tty_drm_device_path_into_manager(
            card.as_path(),
            session.clone(),
            tuning,
            gpu_manager.clone(),
        ) {
            Ok((device, mut device_outputs)) => {
                if primary_render_node.is_none() {
                    primary_render_node = Some(device.render_node);
                    primary_dev_fd = Some(device.dev_fd.clone());
                }
                outputs.append(&mut device_outputs);
                devices.push(device);
            }
            Err(err) => {
                warn!("tty drm probe failed for {}: {}", card.display(), err);
                last_err = Some(err.to_string());
            }
        }
    }

    if let (Some(primary_render_node), Some(primary_dev_fd)) = (primary_render_node, primary_dev_fd)
        && !outputs.is_empty()
    {
        info!(
            "tty drm multi-gpu ready: primary_render_node={} devices={} outputs={}",
            primary_render_node,
            devices.len(),
            outputs
                .iter()
                .map(|output| {
                    let (w, h) = output.mode.size();
                    format!("{}:{}x{}", output.connector_name, w, h)
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(TtyDrmProbe {
            devices,
            gpu_manager,
            primary_render_node,
            primary_dev_fd,
            outputs,
        });
    }

    Err(io::Error::other(format!(
        "failed to initialize tty drm device for seat={} (tried: {}): {}",
        seat,
        tried_paths,
        last_err.unwrap_or_else(|| "unknown error".to_string())
    ))
    .into())
}

fn probe_tty_drm_device_path_into_manager(
    card_path: &Path,
    mut session: Rc<RefCell<LibSeatSession>>,
    tuning: &RuntimeTuning,
    gpu_manager: Rc<RefCell<TtyGpuManager>>,
) -> Result<(TtyDrmDevice, Vec<TtyDrmOutput>), Box<dyn Error>> {
    use rustix::fs::OFlags;
    let raw_fd = session
        .open(card_path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY)
        .map_err(|err| {
            io::Error::other(format!(
                "failed to open drm device {} via session: {:?}",
                card_path.display(),
                err
            ))
        })?;
    let dev_fd = DrmDeviceFd::new(DeviceFd::from(raw_fd));
    let (mut dev, notifier) = DrmDevice::new(dev_fd.clone(), true).map_err(|err| {
        io::Error::other(format!(
            "failed to initialize drm device {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let gbm = GbmDevice::new(dev_fd.clone()).map_err(|err| {
        io::Error::other(format!(
            "failed to create gbm device for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let node = DrmNode::from_path(card_path).map_err(|err| {
        io::Error::other(format!(
            "failed to identify drm node for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|err| {
        io::Error::other(format!(
            "failed to create egl display for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let egl_device = EGLDevice::device_for_display(&display).map_err(|err| {
        io::Error::other(format!(
            "failed to get egl device for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    if egl_device.is_software() {
        return Err(io::Error::other(format!(
            "skipping software egl renderer for {}",
            card_path.display()
        ))
        .into());
    }
    let render_node = egl_device
        .try_get_render_node()
        .ok()
        .flatten()
        .or_else(|| node.node_with_type(NodeType::Render).and_then(Result::ok))
        .unwrap_or(node);
    gpu_manager
        .borrow_mut()
        .as_mut()
        .add_node(render_node, gbm.clone())
        .map_err(|err| {
            io::Error::other(format!(
                "failed to add gpu node {} for {}: {}",
                render_node,
                card_path.display(),
                err
            ))
        })?;
    {
        let mut gpu_manager = gpu_manager.borrow_mut();
        let mut renderer = gpu_manager.single_renderer(&render_node).map_err(|err| {
            io::Error::other(format!(
                "failed to create gles renderer for {}: {:?}",
                card_path.display(),
                err
            ))
        })?;
        log_tty_renderer_info(card_path, renderer.as_mut());
    }
    let outputs = build_tty_outputs(
        &mut dev,
        &gbm,
        dev_fd.clone(),
        &gpu_manager,
        render_node,
        tuning,
        card_path.display(),
    )
    .unwrap_or_else(|err| {
        debug!(
            "tty drm output probe found no usable outputs on {}: {}",
            card_path.display(),
            err
        );
        Vec::new()
    });
    info!(
        "tty drm device ready: card={} node={} render_node={} atomic={} crtcs={} outputs={}",
        card_path.display(),
        node,
        render_node,
        dev.is_atomic(),
        dev.crtcs().len(),
        outputs
            .iter()
            .map(|output| {
                let (w, h) = output.mode.size();
                format!("{}:{}x{}", output.connector_name, w, h)
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok((
        TtyDrmDevice {
            card_path: card_path.to_path_buf(),
            node,
            render_node,
            dev: Rc::new(RefCell::new(dev)),
            gbm: Rc::new(gbm),
            notifier: Some(notifier),
            dev_fd,
        },
        outputs,
    ))
}

pub(crate) fn current_tty_output_signature(outputs: &[TtyDrmOutput]) -> Vec<String> {
    let mut signature = outputs
        .iter()
        .map(|output| {
            let (w, h) = output.mode.size();
            format!(
                "{}:{:?}:{}x{}@{}",
                output.connector_name,
                output.crtc,
                w,
                h,
                output.mode.vrefresh()
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

pub(crate) fn rebuild_tty_outputs(
    dev: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    dev_fd: DrmDeviceFd,
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    render_node: DrmNode,
    tuning: &RuntimeTuning,
    card_path: &Path,
) -> Result<Vec<TtyDrmOutput>, Box<dyn Error>> {
    build_tty_outputs(
        dev,
        gbm,
        dev_fd,
        gpu_manager,
        render_node,
        tuning,
        card_path.display(),
    )
}

pub(crate) fn build_tty_dmabuf_output_feedbacks(
    outputs: &[TtyDrmOutput],
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    primary_render_node: DrmNode,
) -> HashMap<String, DmabufFeedback> {
    let primary_formats: Vec<Format> = match gpu_manager
        .borrow_mut()
        .single_renderer(&primary_render_node)
    {
        Ok(renderer) => renderer.dmabuf_formats().iter().copied().collect(),
        Err(err) => {
            warn!(
                "failed to query primary renderer dma-buf formats for feedback: {:?}",
                err
            );
            return HashMap::new();
        }
    };

    let primary_format_set: smithay::backend::allocator::format::FormatSet =
        primary_formats.iter().copied().collect();
    let mut feedbacks = HashMap::new();

    for output in outputs {
        let compositor = output.compositor.borrow();
        let surface = compositor.surface();
        let primary_plane_formats = surface.plane_info().formats.clone();
        let primary_or_overlay_plane_formats = primary_plane_formats
            .iter()
            .chain(
                surface
                    .planes()
                    .overlay
                    .iter()
                    .flat_map(|plane| plane.formats.iter()),
            )
            .copied()
            .collect::<smithay::backend::allocator::format::FormatSet>();

        let mut primary_scanout_formats = primary_plane_formats
            .intersection(&primary_format_set)
            .copied()
            .collect::<Vec<_>>();
        let mut primary_or_overlay_scanout_formats = primary_or_overlay_plane_formats
            .intersection(&primary_format_set)
            .copied()
            .collect::<Vec<_>>();

        if output.render_node != primary_render_node {
            primary_scanout_formats.retain(|format| format.modifier == Modifier::Linear);
            primary_or_overlay_scanout_formats.retain(|format| format.modifier == Modifier::Linear);
        }

        match DmabufFeedbackBuilder::new(primary_render_node.dev_id(), primary_formats.clone())
            .add_preference_tranche(
                output.render_node.dev_id(),
                Some(TrancheFlags::Scanout),
                primary_scanout_formats,
            )
            .add_preference_tranche(
                output.render_node.dev_id(),
                Some(TrancheFlags::Scanout),
                primary_or_overlay_scanout_formats,
            )
            .build()
        {
            Ok(feedback) => {
                feedbacks.insert(output.connector_name.clone(), feedback);
            }
            Err(err) => warn!(
                "failed to build dma-buf feedback for {}: {}",
                output.connector_name, err
            ),
        }
    }

    feedbacks
}

fn build_tty_outputs(
    dev: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    _dev_fd: DrmDeviceFd,
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    render_node: DrmNode,
    tuning: &RuntimeTuning,
    card_label: impl std::fmt::Display,
) -> Result<Vec<TtyDrmOutput>, Box<dyn Error>> {
    let selected = select_tty_scanouts(dev, tuning)?;

    // Formats the renderer supports — DrmCompositor uses these to choose
    // an internal buffer format and verify scanout compatibility.
    let render_formats: Vec<_> = gpu_manager
        .borrow_mut()
        .single_renderer(&render_node)
        .map(|renderer| renderer.dmabuf_formats().iter().copied().collect())
        .unwrap_or_default();

    let mut outputs = Vec::new();

    for (crtc, mode, connector, connector_name) in selected {
        let surface = dev
            .create_surface(crtc, mode, &[connector])
            .map_err(|err| {
                io::Error::other(format!(
                    "failed to create drm surface on {}:{}: {}",
                    card_label, connector_name, err
                ))
            })?;

        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

        // GbmFramebufferExporter wraps the GBM device so DrmCompositor can
        // export rendered GBM buffers as KMS framebuffers.
        let exporter = GbmFramebufferExporter::new(gbm.clone(), None);

        let color_formats = [Fourcc::Xrgb8888, Fourcc::Argb8888];
        let (mw, mh) = mode.size();

        let compositor = DrmCompositor::new(
            OutputModeSource::Static {
                size: Size::from((mw as i32, mh as i32)),
                scale: Scale::from((1.0, 1.0)),
                transform: Transform::Normal,
            },
            surface,
            None, // cursor plane: disabled for now
            allocator,
            exporter,
            color_formats,
            render_formats.iter().copied(),
            dev.cursor_size(),
            Some(gbm.clone()),
        )
        .map_err(|err| {
            io::Error::other(format!(
                "failed to create drm compositor for {}:{}: {}",
                card_label, connector_name, err
            ))
        })?;

        outputs.push(TtyDrmOutput {
            connector,
            crtc,
            connector_name,
            mode,
            device_node: DrmNode::from_file(dev.device_fd()).unwrap_or(render_node),
            render_node,
            compositor: Rc::new(RefCell::new(compositor)),
        });
    }

    Ok(outputs)
}

pub(crate) fn select_tty_scanouts(
    dev: &mut DrmDevice,
    tuning: &RuntimeTuning,
) -> Result<
    Vec<(
        drm_control::crtc::Handle,
        drm_control::Mode,
        drm_control::connector::Handle,
        String,
    )>,
    Box<dyn Error>,
> {
    let resources = dev
        .resource_handles()
        .map_err(|err| io::Error::other(format!("failed to query drm resources: {}", err)))?;
    let mut connected = Vec::new();
    for conn in resources.connectors() {
        let info = dev.get_connector(*conn, true).map_err(|err| {
            io::Error::other(format!("failed to query drm connector {:?}: {}", conn, err))
        })?;
        if info.state() == drm_control::connector::State::Connected {
            connected.push((*conn, info));
        }
    }
    if connected.is_empty() {
        return Err(io::Error::other("no connected drm connector with a usable mode found").into());
    }

    let default_scanouts =
        |connected: &Vec<(drm_control::connector::Handle, drm_control::connector::Info)>| {
            connected
                .iter()
                .map(
                    |(conn, info): &(
                        drm_control::connector::Handle,
                        drm_control::connector::Info,
                    )| {
                        let mode = info
                            .modes()
                            .iter()
                            .copied()
                            .find(|mode: &drm_control::Mode| {
                                mode.mode_type()
                                    .contains(drm_control::ModeTypeFlags::PREFERRED)
                            })
                            .or_else(|| info.modes().first().copied())
                            .ok_or_else(|| {
                                io::Error::other(format!("connector {} has no modes", info))
                            })?;
                        Ok((*conn, info.clone(), mode))
                    },
                )
                .collect::<Result<Vec<_>, io::Error>>()
        };

    let configured: Vec<_> = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .collect();
    let desired: Vec<_> = if configured.is_empty() {
        if tuning.tty_viewports.is_empty() {
            default_scanouts(&connected)?
        } else {
            warn!(
                "viewport outputs are configured, but none are enabled; falling back to detected outputs"
            );
            default_scanouts(&connected)?
        }
    } else {
        let mut found = Vec::new();
        for wanted in &configured {
            let Some((conn, info)) = connected
                .iter()
                .find(|(_, info)| info.to_string() == wanted.connector)
            else {
                warn!(
                    "configured viewport {} is not currently connected; skipping it",
                    wanted.connector
                );
                continue;
            };

            let Some(mode) = info.modes().iter().copied().find(|m| {
                m.size() == (wanted.width as u16, wanted.height as u16)
                    && wanted
                        .refresh_rate
                        .is_none_or(|hz: f64| (m.vrefresh() as f64 - hz).abs() < 2.0)
            }) else {
                warn!(
                    "configured viewport {} requests {}x{} @ {:?}Hz, but no matching DRM mode is available; skipping it",
                    wanted.connector, wanted.width, wanted.height, wanted.refresh_rate
                );
                continue;
            };

            found.push((*conn, info.clone(), mode));
        }
        if found.is_empty() {
            warn!(
                "none of the configured viewport outputs are usable right now: {}; falling back to detected outputs",
                configured
                    .iter()
                    .map(|v| match v.refresh_rate {
                        Some(rate) => {
                            format!("{}={}x{}@{rate:.3}", v.connector, v.width, v.height)
                        }
                        None => format!("{}={}x{}", v.connector, v.width, v.height),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            default_scanouts(&connected)?
        } else {
            if found.len() < configured.len() {
                warn!(
                    "using {} of {} configured viewport outputs; invalid outputs were skipped",
                    found.len(),
                    configured.len()
                );
            }
            found
        }
    };

    let mut used_crtcs = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for (selected_conn, selected_info, mut selected_mode) in desired {
        let mut selected_crtc: Option<drm_control::crtc::Handle> = None;

        let possible_crtcs: Vec<drm_control::crtc::Handle> = {
            let mut vec: Vec<drm_control::crtc::Handle> = Vec::new();
            let encoder_handles: Vec<_> = {
                let mut handles = Vec::new();
                if let Some(enc) = selected_info
                    .current_encoder()
                    .map(|enc: drm_control::encoder::Handle| enc)
                {
                    handles.push(enc);
                }
                for &enc in selected_info.encoders() {
                    if !handles.contains(&enc) {
                        handles.push(enc);
                    }
                }
                handles
            };
            for enc_handle in encoder_handles {
                if let Ok(enc_info) = dev.get_encoder(enc_handle) {
                    for crtc in resources.filter_crtcs(enc_info.possible_crtcs()) {
                        if !vec.contains(&crtc) {
                            vec.push(crtc);
                        }
                    }
                }
            }
            vec
        };

        if let Some(enc) = selected_info
            .current_encoder()
            .or_else(|| selected_info.encoders().first().copied())
            && let Ok(enc_info) = dev.get_encoder(enc)
            && let Some(existing_crtc) = enc_info.crtc()
            && !used_crtcs.contains(&existing_crtc)
            && possible_crtcs.contains(&existing_crtc)
        {
            selected_crtc = Some(existing_crtc);
        }

        if selected_crtc.is_none() {
            selected_crtc = possible_crtcs
                .iter()
                .copied()
                .find(|crtc| !used_crtcs.contains(crtc));
        }

        let Some(crtc) = selected_crtc else {
            return Err(io::Error::other(format!(
                "failed to find a usable CRTC for connector {} (possible CRTCs: {:?}, used: {:?})",
                selected_info, possible_crtcs, used_crtcs,
            ))
            .into());
        };

        // Prefer the live CRTC mode to avoid a spurious mode mismatch on
        // the first frame (which would force a blocking commit_pending).
        if let Some(enc) = selected_info
            .current_encoder()
            .map(|enc: drm_control::encoder::Handle| enc)
            && let Ok(enc_info) = dev.get_encoder(enc)
            && enc_info.crtc() == Some(crtc)
            && let Ok(crtc_info) = dev.get_crtc(crtc)
            && let Some(current_mode) = crtc_info.mode()
        {
            if current_mode.size() == selected_mode.size()
                && current_mode.vrefresh() == selected_mode.vrefresh()
            {
                selected_mode = current_mode;
            }
        }

        used_crtcs.insert(crtc);
        selected.push((
            crtc,
            selected_mode,
            selected_conn,
            selected_info.to_string(),
        ));
    }

    if !configured.is_empty() {
        let configured_connectors: std::collections::HashSet<&str> =
            configured.iter().map(|v| v.connector.as_str()).collect();
        for (conn, info) in &connected {
            if selected.iter().any(|(_, _, c, _)| c == conn) {
                continue;
            }
            if !configured_connectors.contains(info.to_string().as_str()) {
                continue;
            }
            let enc = info
                .current_encoder()
                .or_else(|| info.encoders().first().copied());
            let Some(enc) = enc else { continue };
            let Ok(enc_info) = dev.get_encoder(enc) else {
                continue;
            };
            let Some(other_crtc) = enc_info.crtc() else {
                continue;
            };
            if let Err(err) = dev.set_crtc(other_crtc, None, (0, 0), &[], None) {
                warn!("failed to disable unconfigured connector {}: {}", info, err);
            } else {
                debug!("disabled unconfigured connector {}", info);
            }
        }
    }

    Ok(selected)
}

pub(crate) fn collect_outputs_for_ipc(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    tuning: &RuntimeTuning,
    vrr_support: &HashMap<String, String>,
    direct_scanout: &HashMap<
        String,
        crate::compositor::fullscreen::state::FullscreenDirectScanoutState,
    >,
) -> Vec<OutputInfo> {
    let mut outputs = Vec::new();

    let Ok(resources) = dev.resource_handles() else {
        return outputs;
    };

    for conn in resources.connectors() {
        let Ok(info) = dev.get_connector(*conn, true) else {
            continue;
        };

        let status = match info.state() {
            drm_control::connector::State::Connected => OutputStatus::Connected,
            drm_control::connector::State::Disconnected => OutputStatus::Disconnected,
            drm_control::connector::State::Unknown => OutputStatus::Unknown,
        };

        let active_mode = active_modes.get(&info.to_string()).copied();
        let mut current_mode = active_mode.map(|mode| mode_info_from_drm_mode(mode, true, false));
        let mut modes = Vec::new();

        for mode in info.modes() {
            let current_match =
                active_mode.is_some_and(|active_mode| drm_mode_matches(*mode, active_mode));
            let mode_info = mode_info_from_drm_mode(
                *mode,
                current_match,
                mode.mode_type()
                    .contains(drm_control::ModeTypeFlags::PREFERRED),
            );

            if current_match {
                current_mode = Some(mode_info.clone());
            }

            modes.push(mode_info);
        }

        let output_name = info.to_string();
        let scanout = direct_scanout.get(output_name.as_str());
        outputs.push(OutputInfo {
            name: output_name.clone(),
            status,
            enabled: active_mode.is_some(),
            current_mode,
            modes,
            vrr_mode: tuning
                .tty_viewports
                .iter()
                .find(|viewport| viewport.connector == output_name)
                .map(|viewport| viewport.vrr.as_str().to_string()),
            vrr_support: vrr_support.get(output_name.as_str()).cloned(),
            direct_scanout_candidate_node: scanout
                .and_then(|state| state.candidate_node)
                .map(|id: halley_core::field::NodeId| id.as_u64()),
            direct_scanout_active_node: scanout
                .and_then(|state| state.active_node)
                .map(|id: halley_core::field::NodeId| id.as_u64()),
            direct_scanout_reason: scanout.and_then(|state| state.reason.clone()),
            logical: None,
        });
    }

    outputs
}

fn drm_mode_matches(a: drm_control::Mode, b: drm_control::Mode) -> bool {
    let (aw, ah) = a.size();
    let (bw, bh) = b.size();
    aw == bw && ah == bh && a.vrefresh() == b.vrefresh()
}

fn mode_info_from_drm_mode(mode: drm_control::Mode, current: bool, preferred: bool) -> ModeInfo {
    let (w, h) = mode.size();
    ModeInfo {
        width: w as u32,
        height: h as u32,
        refresh_hz: Some(mode.vrefresh() as f64),
        preferred,
        current,
    }
}

pub(crate) fn queue_tty_drm_frame(
    output_name: &str,
    output_device_node: DrmNode,
    compositor: &Rc<RefCell<HalleyDrmCompositor>>,
    gpu_manager: &Rc<RefCell<TtyGpuManager>>,
    primary_render_node: DrmNode,
    output_render_node: DrmNode,
    composed_frame_cache: &Rc<RefCell<HashMap<String, GlesTexture>>>,
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> Result<TtyFrameQueueReport, Box<dyn Error>> {
    use crate::render::draw_debug_frame_to_target;
    let previous_monitor = st.begin_temporary_render_monitor(output_name);
    let previous_layer_configure = st.input.interaction_state.suppress_layer_shell_configure;

    let result = (|| {
        let mut compositor = compositor.borrow_mut();

        let mode = compositor.pending_mode();
        let (w, h) = mode.size();
        let physical_size: Size<i32, Physical> = (w as i32, h as i32).into();
        let animation_redraw =
            crate::frame_loop::tty_output_animation_redraw_state(st, output_name, Instant::now());

        let local_cursor = cursor_screen.and_then(|(sx, sy)| {
            let (target_monitor, sx, sy) = st.monitor_for_screen_clamped(sx, sy)?;
            if target_monitor != output_name {
                return None;
            }
            let (_local_w, _local_h, local_sx, local_sy) =
                st.local_screen_in_monitor(output_name, sx, sy);
            Some((local_sx, local_sy))
        });

        st.input.interaction_state.suppress_layer_shell_configure = previous_monitor.is_some();

        let disable_direct_scanout =
            tty_env_flag("HALLEY_DISABLE_DIRECT_SCANOUT") || tty_env_flag("HALLEY_FORCE_COMPOSED");
        let allow_direct_scanout =
            !disable_direct_scanout && primary_render_node == output_render_node;
        match allow_direct_scanout.then(|| {
            fullscreen_direct_scanout_candidate(
                st,
                output_name,
                w as i32,
                h as i32,
                resize_preview,
                hover_node,
                preview_hover_node,
                local_cursor,
                cursor_image,
            )
        }) {
            None => st
                .model
                .fullscreen_state
                .clear_direct_scanout_for_monitor(output_name),
            Some(None) => st
                .model
                .fullscreen_state
                .clear_direct_scanout_for_monitor(output_name),
            Some(Some(Err((node_id, reason)))) => st
                .model
                .fullscreen_state
                .set_direct_scanout_status(output_name, Some(node_id), None, Some(reason)),
            Some(Some(Ok(candidate))) => {
                let mut gpu_manager = gpu_manager.borrow_mut();
                let mut renderer =
                    gpu_manager
                        .single_renderer(&primary_render_node)
                        .map_err(|err| {
                            io::Error::other(format!(
                                "failed to create primary renderer for {} direct scanout: {:?}",
                                output_name, err
                            ))
                        })?;
                let renderer_ref = renderer.as_mut();
                let mut elements = direct_scanout_cursor_elements(
                    renderer_ref,
                    local_cursor,
                    cursor_image,
                    &st.runtime.tuning.cursor,
                )?;
                elements.extend(
                    render_elements_from_surface_tree::<_, HalleyDirectScanoutElement>(
                        renderer_ref,
                        &candidate.surface,
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                    .into_iter()
                    .map(Into::into),
                );
                match compositor.render_frame(
                    renderer_ref,
                    &elements,
                    [0.0, 0.0, 0.0, 1.0],
                    FrameFlags::DEFAULT,
                ) {
                    Ok(render_res) => {
                        let direct_scanout_active =
                            matches!(render_res.primary_element, PrimaryPlaneElement::Element(_));
                        st.model.fullscreen_state.set_direct_scanout_status(
                            output_name,
                            Some(candidate.node_id),
                            direct_scanout_active.then_some(candidate.node_id),
                            (!direct_scanout_active).then_some(
                                "eligible fullscreen surface fell back to compositor primary plane"
                                    .to_string(),
                            ),
                        );
                        let mut sync_wait = None;
                        let queued = if !render_res.is_empty {
                            if render_res.needs_sync()
                                && let PrimaryPlaneElement::Swapchain(element) =
                                    &render_res.primary_element
                            {
                                let wait_started = Instant::now();
                                let wait_result = element.sync.wait();
                                let wait_duration = wait_started.elapsed();
                                sync_wait = Some(wait_duration);
                                if wait_duration >= Duration::from_millis(TTY_SYNC_WAIT_WARN_MS) {
                                    warn!(
                                        "slow tty drm sync wait: output={} path=direct duration={:?} device_node={} primary_render_node={} output_render_node={}",
                                        output_name,
                                        wait_duration,
                                        output_device_node,
                                        primary_render_node,
                                        output_render_node
                                    );
                                }
                                if let Err(err) = wait_result {
                                    warn!(
                                        "failed to wait for tty drm direct-scanout frame completion on {}: {:?}",
                                        output_name, err
                                    );
                                }
                            }
                            queue_tty_frame_or_clear_on_failure(&mut compositor, output_name)?;
                            true
                        } else {
                            false
                        };
                        return Ok(TtyFrameQueueReport {
                            queued,
                            animation_redraw_active: animation_redraw.active,
                            direct_scanout_active,
                            composed: false,
                            sync_wait,
                        });
                    }
                    Err(err) => {
                        st.model.fullscreen_state.set_direct_scanout_status(
                            output_name,
                            Some(candidate.node_id),
                            None,
                            Some(format!("direct scanout render attempt failed: {}", err)),
                        );
                    }
                }
            }
        }

        let force_overlay_full_repaint =
            crate::frame_loop::monitor_overlay_requires_full_repaint(st, output_name);
        let force_full_repaint = force_overlay_full_repaint || animation_redraw.force_full_repaint;
        let texture_buffer = {
            let mut gpu_manager = gpu_manager.borrow_mut();
            let mut renderer =
                gpu_manager
                    .single_renderer(&primary_render_node)
                    .map_err(|err| {
                        io::Error::other(format!(
                            "failed to create primary renderer for {} composition: {:?}",
                            output_name, err
                        ))
                    })?;
            let renderer_ref = renderer.as_mut();
            let mut texture = composed_frame_texture_for_output(
                output_name,
                renderer_ref,
                composed_frame_cache,
                w as i32,
                h as i32,
            )?;

            {
                let mut target = renderer_ref.bind(&mut texture).map_err(|err| {
                    io::Error::other(format!("bind failed for {}: {}", output_name, err))
                })?;

                draw_debug_frame_to_target(
                    renderer_ref,
                    &mut target,
                    physical_size,
                    st,
                    resize_preview,
                    hover_node,
                    preview_hover_node,
                    local_cursor,
                    cursor_image,
                    st.output_transform_for(output_name),
                )?;
            }

            TextureBuffer::from_texture(
                renderer_ref,
                texture,
                1,
                Transform::Normal,
                Some(Vec::new()),
            )
        };

        let element = PrimaryGpuTextureElement(TextureRenderElement::from_texture_buffer(
            (0.0, 0.0),
            &texture_buffer,
            Some(1.0),
            None,
            None,
            Kind::Unspecified,
        ));

        let elements = [element];
        if force_full_repaint {
            compositor.reset_buffers();
        }
        let mut gpu_manager = gpu_manager.borrow_mut();
        let mut renderer = gpu_manager
            .renderer(
                &primary_render_node,
                &output_render_node,
                compositor.format(),
            )
            .map_err(|err| {
                io::Error::other(format!(
                    "failed to create multi-gpu renderer for {}: {:?}",
                    output_name, err
                ))
            })?;
        let render_res = compositor
            .render_frame(
                &mut renderer,
                &elements,
                [0.0, 0.0, 0.0, 1.0],
                FrameFlags::empty(),
            )
            .map_err(|err| {
                io::Error::other(format!("render_frame failed for {}: {}", output_name, err))
            })?;

        let mut sync_wait = None;
        let queued = if !render_res.is_empty {
            if render_res.needs_sync()
                && let PrimaryPlaneElement::Swapchain(element) = &render_res.primary_element
            {
                let wait_started = Instant::now();
                let wait_result = element.sync.wait();
                let wait_duration = wait_started.elapsed();
                sync_wait = Some(wait_duration);
                if wait_duration >= Duration::from_millis(TTY_SYNC_WAIT_WARN_MS) {
                    warn!(
                        "slow tty drm sync wait: output={} path=composed duration={:?} device_node={} primary_render_node={} output_render_node={}",
                        output_name,
                        wait_duration,
                        output_device_node,
                        primary_render_node,
                        output_render_node
                    );
                }
                if let Err(err) = wait_result {
                    warn!(
                        "failed to wait for tty drm composed frame completion on {}: {:?}",
                        output_name, err
                    );
                }
            }
            queue_tty_frame_or_clear_on_failure(&mut compositor, output_name)?;
            true
        } else {
            false
        };

        Ok(TtyFrameQueueReport {
            queued,
            animation_redraw_active: animation_redraw.active,
            direct_scanout_active: false,
            composed: true,
            sync_wait,
        })
    })();

    st.input.interaction_state.suppress_layer_shell_configure = previous_layer_configure;
    st.end_temporary_render_monitor(previous_monitor);
    result
}

fn composed_frame_texture_for_output(
    output_name: &str,
    renderer: &mut GlesRenderer,
    composed_frame_cache: &Rc<RefCell<HashMap<String, GlesTexture>>>,
    width: i32,
    height: i32,
) -> Result<GlesTexture, Box<dyn Error>> {
    let buffer_size = Size::from((width, height));
    if let Some(texture) = composed_frame_cache
        .borrow()
        .get(output_name)
        .filter(|texture| texture.size() == buffer_size)
        .cloned()
    {
        return Ok(texture);
    }

    let texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        buffer_size,
    )
    .map_err(|err| {
        io::Error::other(format!(
            "failed to create tty drm intermediate texture for {}: {}",
            output_name, err
        ))
    })?;
    composed_frame_cache
        .borrow_mut()
        .insert(output_name.to_string(), texture.clone());
    Ok(texture)
}

fn log_tty_renderer_info(card_path: &Path, renderer: &mut GlesRenderer) {
    let egl_version = renderer.egl_context().display().get_egl_version();
    let gl_strings = renderer.with_context(|gl| unsafe {
        let gl_string = |name| {
            let ptr = gl.GetString(name);
            if ptr.is_null() {
                return "<unavailable>".to_string();
            }
            CStr::from_ptr(ptr.cast()).to_string_lossy().into_owned()
        };
        (
            gl_string(smithay::backend::renderer::gles::ffi::VENDOR),
            gl_string(smithay::backend::renderer::gles::ffi::RENDERER),
            gl_string(smithay::backend::renderer::gles::ffi::VERSION),
        )
    });

    match gl_strings {
        Ok((gl_vendor, gl_renderer, gl_version)) => info!(
            "tty renderer ready: card={} egl={}.{} gl_vendor={} gl_renderer={} gl_version={}",
            card_path.display(),
            egl_version.0,
            egl_version.1,
            gl_vendor,
            gl_renderer,
            gl_version,
        ),
        Err(err) => warn!(
            "tty renderer info unavailable for {}: {}",
            card_path.display(),
            err
        ),
    }
}

struct FullscreenDirectScanoutCandidate {
    node_id: halley_core::field::NodeId,
    surface: WlSurface,
}

fn direct_scanout_cursor_elements(
    renderer: &mut GlesRenderer,
    local_cursor: Option<(f32, f32)>,
    cursor_image: Option<&CursorImageStatus>,
    cursor_config: &halley_config::CursorConfig,
) -> Result<Vec<HalleyDirectScanoutElement>, Box<dyn Error>> {
    let Some((sx, sy)) = local_cursor else {
        return Ok(Vec::new());
    };
    let cursor_status = cursor_image
        .cloned()
        .unwrap_or_else(CursorImageStatus::default_named);
    match cursor_status {
        CursorImageStatus::Hidden => Ok(Vec::new()),
        CursorImageStatus::Surface(surface) => {
            let scale = smithay::wayland::compositor::with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<smithay::wayland::compositor::SurfaceAttributes>()
                    .current()
                    .buffer_scale as f64
            });
            let (hotspot_x, hotspot_y) = crate::render::cursor_surface_hotspot(&surface);
            let loc = (sx.round() as i32 - hotspot_x, sy.round() as i32 - hotspot_y);
            Ok(
                render_elements_from_surface_tree::<_, HalleyDirectScanoutElement>(
                    renderer,
                    &surface,
                    loc,
                    scale,
                    1.0,
                    Kind::Cursor,
                )
                .into_iter()
                .map(Into::into)
                .collect(),
            )
        }
        CursorImageStatus::Named(icon) => {
            let Some(sprite) =
                crate::render::themed_cursor_sprite_with_fallback(cursor_config, icon)
            else {
                return Ok(Vec::new());
            };
            let loc = (
                sx.round() as i32 - sprite.hotspot_x,
                sy.round() as i32 - sprite.hotspot_y,
            );
            let buffer = MemoryRenderBuffer::from_slice(
                &sprite.pixels_bgra,
                Fourcc::Argb8888,
                (sprite.width as i32, sprite.height as i32),
                1,
                Transform::Normal,
                None,
            );
            let element = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                (loc.0 as f64, loc.1 as f64),
                &buffer,
                Some(1.0),
                None,
                None,
                Kind::Cursor,
            )?;
            Ok(vec![element.into()])
        }
    }
}

fn fullscreen_root_surface_for_node(
    st: &Halley,
    node_id: halley_core::field::NodeId,
) -> Option<WlSurface> {
    st.platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .find_map(|top| {
            let wl = top.wl_surface();
            (st.model.surface_to_node.get(&wl.id()).copied() == Some(node_id)).then(|| wl.clone())
        })
}

fn monitor_has_blocking_layer_shell_surfaces(st: &Halley, monitor: &str) -> bool {
    crate::compositor::monitor::layer_shell::layer_shell_placements_for_monitor(st, monitor)
        .into_iter()
        .any(|placement| {
            matches!(
                placement.layer,
                smithay::wayland::shell::wlr_layer::Layer::Top
                    | smithay::wayland::shell::wlr_layer::Layer::Overlay
            )
        })
}

fn fullscreen_direct_scanout_candidate(
    st: &Halley,
    output_name: &str,
    output_w: i32,
    output_h: i32,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    local_cursor: Option<(f32, f32)>,
    cursor_image: Option<&CursorImageStatus>,
) -> Option<Result<FullscreenDirectScanoutCandidate, (halley_core::field::NodeId, String)>> {
    let node_id = *st
        .model
        .fullscreen_state
        .fullscreen_active_node
        .get(output_name)?;
    let blocked = |reason: &str| Err((node_id, reason.to_string()));

    if st.output_transform_for(output_name) != Transform::Normal {
        return Some(blocked("output transform is not normal"));
    }
    if st
        .model
        .fullscreen_state
        .fullscreen_motion
        .contains_key(&node_id)
        || st
            .model
            .fullscreen_state
            .fullscreen_scale_anim
            .contains_key(&node_id)
    {
        return Some(blocked("fullscreen transition is still animating"));
    }
    if st.input.interaction_state.resize_active == Some(node_id)
        || st.input.interaction_state.drag_authority_node == Some(node_id)
        || resize_preview.is_some_and(|rz| rz.node_id == node_id)
    {
        return Some(blocked("interactive move or resize is active"));
    }
    if crate::frame_loop::monitor_overlay_requires_full_repaint(st, output_name) {
        return Some(blocked("monitor overlays are active"));
    }
    if hover_node.is_some() || preview_hover_node.is_some() {
        return Some(blocked("hover UI is active"));
    }
    if st.should_draw_focus_ring_preview(Instant::now()) {
        return Some(blocked("focus preview is active"));
    }
    if local_cursor.is_some() && matches!(cursor_image, Some(CursorImageStatus::Surface(_))) {
        return Some(blocked(
            "client surface cursor requires composited fullscreen fallback",
        ));
    }
    if monitor_has_blocking_layer_shell_surfaces(st, output_name) {
        return Some(blocked(
            "top/overlay layer-shell surfaces are present on the output",
        ));
    }
    if st.monitor_has_visible_overlap_policy_window(output_name) {
        return Some(blocked(
            "overlap-policy window is visible above fullscreen on the output",
        ));
    }
    let Some(surface) = fullscreen_root_surface_for_node(st, node_id) else {
        return Some(blocked("fullscreen node has no live toplevel surface"));
    };
    if PopupManager::popups_for_surface(&surface).next().is_some() {
        return Some(blocked("fullscreen surface has popups"));
    }

    let bbox = bbox_from_surface_tree(&surface, (0, 0));
    if bbox.loc.x != 0 || bbox.loc.y != 0 {
        return Some(blocked("surface bbox is offset from the output origin"));
    }
    if (bbox.size.w - output_w).abs() > 1 || (bbox.size.h - output_h).abs() > 1 {
        return Some(blocked("surface bbox does not match the output mode size"));
    }

    Some(Ok(FullscreenDirectScanoutCandidate { node_id, surface }))
}
