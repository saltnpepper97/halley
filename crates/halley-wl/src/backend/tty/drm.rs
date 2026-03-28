use super::*;
use std::collections::HashMap;

use crate::interaction::types::ResizeCtx;
use halley_ipc::{ModeInfo, OutputInfo, OutputStatus};

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags, PrimaryPlaneElement};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{
    Kind,
    memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
    render_elements,
    surface::render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::desktop::{PopupManager, utils::bbox_from_surface_tree};
use smithay::input::pointer::CursorImageStatus;
use smithay::output::OutputModeSource;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Physical, Scale, Size, Transform};

type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;

render_elements! {
    HalleyDirectScanoutElement<=GlesRenderer>;
    Surface=SurfaceElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
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

pub(crate) struct TtyDrmProbe {
    pub(crate) card_path: std::path::PathBuf,
    pub(crate) dev: DrmDevice,
    pub(crate) gbm: GbmDevice<DrmDeviceFd>,
    pub(crate) notifier: smithay::backend::drm::DrmDeviceNotifier,
    pub(crate) renderer: Rc<RefCell<GlesRenderer>>,
    pub(crate) outputs: Vec<TtyDrmOutput>,
    /// The DrmDeviceFd kept alive so GbmDevice references stay valid.
    pub(crate) dev_fd: DrmDeviceFd,
}

pub(crate) struct TtyDrmOutput {
    #[allow(dead_code)]
    pub(crate) connector: drm_control::connector::Handle,
    pub(crate) crtc: drm_control::crtc::Handle,
    pub(crate) connector_name: String,
    pub(crate) mode: drm_control::Mode,
    /// Atomic DRM compositor — replaces GbmBufferedSurface.
    pub(crate) compositor: Rc<RefCell<HalleyDrmCompositor>>,
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

    let mut last_err: Option<String> = None;
    let tried_paths = candidates
        .iter()
        .map(|card| card.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for card in candidates {
        match probe_tty_drm_device_path_via_session(card.as_path(), session.clone(), tuning) {
            Ok(probe) => return Ok(probe),
            Err(err) => {
                warn!("tty drm probe failed for {}: {}", card.display(), err);
                last_err = Some(err.to_string());
            }
        }
    }

    Err(io::Error::other(format!(
        "failed to initialize tty drm device for seat={} (tried: {}): {}",
        seat,
        tried_paths,
        last_err.unwrap_or_else(|| "unknown error".to_string())
    ))
    .into())
}

pub(crate) fn probe_tty_drm_device_path_via_session(
    card_path: &Path,
    mut session: Rc<RefCell<LibSeatSession>>,
    tuning: &RuntimeTuning,
) -> Result<TtyDrmProbe, Box<dyn Error>> {
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
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|err| {
        io::Error::other(format!(
            "failed to create egl display for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let context = EGLContext::new(&display).map_err(|err| {
        io::Error::other(format!(
            "failed to create egl context for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let renderer = unsafe { GlesRenderer::new(context) }.map_err(|err| {
        io::Error::other(format!(
            "failed to create gles renderer for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    let outputs = build_tty_outputs(
        &mut dev,
        &gbm,
        dev_fd.clone(),
        &renderer,
        tuning,
        card_path.display(),
    )?;
    info!(
        "tty drm device ready: card={} atomic={} crtcs={} outputs={}",
        card_path.display(),
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
    Ok(TtyDrmProbe {
        card_path: card_path.to_path_buf(),
        dev,
        gbm,
        notifier,
        renderer: Rc::new(RefCell::new(renderer)),
        outputs,
        dev_fd,
    })
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

pub(crate) fn selected_tty_scanout_signature(
    dev: &mut DrmDevice,
    tuning: &RuntimeTuning,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut signature = select_tty_scanouts(dev, tuning)?
        .into_iter()
        .map(|(crtc, mode, _connector, connector_name)| {
            let (w, h) = mode.size();
            format!(
                "{}:{:?}:{}x{}@{}",
                connector_name,
                crtc,
                w,
                h,
                mode.vrefresh()
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    Ok(signature)
}

pub(crate) fn rebuild_tty_outputs(
    dev: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    dev_fd: DrmDeviceFd,
    renderer: &Rc<RefCell<GlesRenderer>>,
    tuning: &RuntimeTuning,
    card_path: &Path,
) -> Result<Vec<TtyDrmOutput>, Box<dyn Error>> {
    let renderer = renderer.borrow();
    build_tty_outputs(dev, gbm, dev_fd, &renderer, tuning, card_path.display())
}

fn build_tty_outputs(
    dev: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    _dev_fd: DrmDeviceFd,
    renderer: &GlesRenderer,
    tuning: &RuntimeTuning,
    card_label: impl std::fmt::Display,
) -> Result<Vec<TtyDrmOutput>, Box<dyn Error>> {
    let selected = select_tty_scanouts(dev, tuning)?;

    // Formats the renderer supports — DrmCompositor uses these to choose
    // an internal buffer format and verify scanout compatibility.
    let render_formats: Vec<_> = renderer.dmabuf_formats().iter().copied().collect();

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

    let configured: Vec<_> = tuning
        .tty_viewports
        .iter()
        .filter(|viewport| viewport.enabled)
        .collect();
    let desired: Vec<_> = if configured.is_empty() {
        if tuning.tty_viewports.is_empty() {
            connected
                .iter()
                .map(|(conn, info)| {
                    let mode = info.modes().first().copied().ok_or_else(|| {
                        io::Error::other(format!("connector {} has no modes", info))
                    })?;
                    Ok((*conn, info.clone(), mode))
                })
                .collect::<Result<Vec<_>, io::Error>>()?
        } else {
            return Err(
                io::Error::other("viewport outputs are configured, but none are enabled").into(),
            );
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
                        .is_none_or(|hz| (m.vrefresh() as f64 - hz).abs() < 2.0)
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
            return Err(io::Error::other(format!(
                "none of the configured viewport outputs are usable right now: {}",
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
            ))
            .into());
        }

        if found.len() < configured.len() {
            warn!(
                "using {} of {} configured viewport outputs; invalid outputs were skipped",
                found.len(),
                configured.len()
            );
        }
        found
    };

    let mut used_crtcs = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for (selected_conn, selected_info, mut selected_mode) in desired {
        let mut selected_crtc: Option<drm_control::crtc::Handle> = None;

        let possible_crtcs: Vec<drm_control::crtc::Handle> = {
            let mut vec: Vec<drm_control::crtc::Handle> = Vec::new();
            let encoder_handles: Vec<_> = {
                let mut handles = Vec::new();
                if let Some(enc) = selected_info.current_encoder() {
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
        if let Some(enc) = selected_info.current_encoder()
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
                info!("disabled unconfigured connector {}", info);
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
    direct_scanout: &HashMap<String, crate::state::FullscreenDirectScanoutState>,
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
    compositor: &Rc<RefCell<HalleyDrmCompositor>>,
    renderer: &Rc<RefCell<GlesRenderer>>,
    st: &mut Halley,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> Result<bool, Box<dyn Error>> {
    use crate::render::draw_debug_frame_to_target;
    let previous_monitor = st.begin_temporary_render_monitor(output_name);
    let previous_layer_configure = st.input.interaction_state.suppress_layer_shell_configure;

    let result = (|| {
        let mut compositor = compositor.borrow_mut();
        let mut renderer_ref = renderer.borrow_mut();

        let mode = compositor.pending_mode();
        let (w, h) = mode.size();
        let buffer_size = Size::from((w as i32, h as i32));
        let physical_size: Size<i32, Physical> = (w as i32, h as i32).into();

        let local_cursor = cursor_screen.and_then(|(sx, sy)| {
            let target_monitor = st.monitor_for_screen(sx, sy)?;
            if target_monitor != output_name {
                return None;
            }
            let (_local_w, _local_h, local_sx, local_sy) =
                st.local_screen_in_monitor(output_name, sx, sy);
            Some((local_sx, local_sy))
        });

        st.input.interaction_state.suppress_layer_shell_configure = previous_monitor.is_some();

        match fullscreen_direct_scanout_candidate(
            st,
            output_name,
            w as i32,
            h as i32,
            resize_preview,
            hover_node,
            preview_hover_node,
            local_cursor,
            cursor_image,
        ) {
            None => st.clear_fullscreen_direct_scanout_for_monitor(output_name),
            Some(Err((node_id, reason))) => st.set_fullscreen_direct_scanout_status(
                output_name,
                Some(node_id),
                None,
                Some(reason),
            ),
            Some(Ok(candidate)) => {
                let mut elements: Vec<HalleyDirectScanoutElement> =
                    render_elements_from_surface_tree::<_, HalleyDirectScanoutElement>(
                        &mut *renderer_ref,
                        &candidate.surface,
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                    .into_iter()
                    .map(Into::into)
                    .collect();
                elements.extend(direct_scanout_cursor_elements(
                    &mut *renderer_ref,
                    local_cursor,
                    cursor_image,
                )?);
                match compositor.render_frame(
                    &mut *renderer_ref,
                    &elements,
                    [0.0, 0.0, 0.0, 1.0],
                    FrameFlags::DEFAULT,
                ) {
                    Ok(render_res) => {
                        let direct_scanout_active =
                            matches!(render_res.primary_element, PrimaryPlaneElement::Element(_));
                        st.set_fullscreen_direct_scanout_status(
                            output_name,
                            Some(candidate.node_id),
                            direct_scanout_active.then_some(candidate.node_id),
                            (!direct_scanout_active).then_some(
                                "eligible fullscreen surface fell back to compositor primary plane"
                                    .to_string(),
                            ),
                        );
                        let queued = if !render_res.is_empty {
                            compositor.queue_frame(()).map_err(|err| {
                                io::Error::other(format!(
                                    "queue_frame failed for {}: {}",
                                    output_name, err
                                ))
                            })?;
                            true
                        } else {
                            false
                        };
                        return Ok(queued);
                    }
                    Err(err) => {
                        st.set_fullscreen_direct_scanout_status(
                            output_name,
                            Some(candidate.node_id),
                            None,
                            Some(format!("direct scanout render attempt failed: {}", err)),
                        );
                    }
                }
            }
        }

        let force_overlay_full_repaint = st.monitor_overlay_requires_full_repaint(output_name);
        let mut texture: GlesTexture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            &mut *renderer_ref,
            Fourcc::Abgr8888,
            buffer_size,
        )
        .map_err(|err| {
            io::Error::other(format!(
                "failed to create tty drm intermediate texture for {}: {}",
                output_name, err
            ))
        })?;

        {
            let mut target = renderer_ref.bind(&mut texture).map_err(|err| {
                io::Error::other(format!("bind failed for {}: {}", output_name, err))
            })?;

            draw_debug_frame_to_target(
                &mut renderer_ref,
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

        let texture_buffer = TextureBuffer::from_texture(
            &mut *renderer_ref,
            texture,
            1,
            Transform::Normal,
            Some(Vec::new()),
        );

        let element = TextureRenderElement::from_texture_buffer(
            (0.0, 0.0),
            &texture_buffer,
            Some(1.0),
            None,
            None,
            Kind::Unspecified,
        );

        let elements = [element];
        if force_overlay_full_repaint {
            compositor.reset_buffers();
        }
        let render_res = compositor
            .render_frame(
                &mut *renderer_ref,
                &elements,
                [0.0, 0.0, 0.0, 1.0],
                FrameFlags::empty(),
            )
            .map_err(|err| {
                io::Error::other(format!("render_frame failed for {}: {}", output_name, err))
            })?;

        let queued = if !render_res.is_empty {
            compositor.queue_frame(()).map_err(|err| {
                io::Error::other(format!("queue_frame failed for {}: {}", output_name, err))
            })?;
            true
        } else {
            false
        };

        Ok(queued)
    })();

    st.input.interaction_state.suppress_layer_shell_configure = previous_layer_configure;
    st.end_temporary_render_monitor(previous_monitor);
    result
}

struct FullscreenDirectScanoutCandidate {
    node_id: halley_core::field::NodeId,
    surface: WlSurface,
}

fn direct_scanout_cursor_elements(
    renderer: &mut GlesRenderer,
    local_cursor: Option<(f32, f32)>,
    cursor_image: Option<&CursorImageStatus>,
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
            let Some(sprite) = crate::render::themed_cursor_sprite_with_fallback(icon) else {
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
    st.layer_shell_placements_for_monitor(monitor)
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
    if st.monitor_overlay_requires_full_repaint(output_name) {
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
