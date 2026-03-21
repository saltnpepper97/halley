use super::*;
use std::collections::HashMap;

use crate::interaction::types::ResizeCtx;
use halley_ipc::{ModeInfo, OutputInfo, OutputStatus};

pub(crate) struct TtyDrmProbe {
    pub(crate) _card_path: std::path::PathBuf,
    pub(crate) dev: DrmDevice,
    pub(crate) notifier: smithay::backend::drm::DrmDeviceNotifier,
    pub(crate) renderer: Rc<RefCell<GlesRenderer>>,
    pub(crate) outputs: Vec<TtyDrmOutput>,
}

pub(crate) struct TtyDrmOutput {
    #[allow(dead_code)]
    pub(crate) connector: drm_control::connector::Handle,
    pub(crate) crtc: drm_control::crtc::Handle,
    pub(crate) connector_name: String,
    pub(crate) mode: drm_control::Mode,
    pub(crate) gbm_surface: Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
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
    let selected = select_tty_scanouts(&mut dev, tuning)?;
    let gbm = GbmDevice::new(dev_fd.device_fd()).map_err(|err| {
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
    let renderer_formats: Vec<Format> = renderer.dmabuf_formats().iter().copied().collect();
    let mut outputs = Vec::new();
    for (crtc, mode, connector, connector_name) in selected {
        let surface = dev
            .create_surface(crtc, mode, &[connector])
            .map_err(|err| {
                io::Error::other(format!(
                    "failed to create drm surface on {}:{}: {}",
                    card_path.display(),
                    connector_name,
                    err
                ))
            })?;
        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let gbm_surface = GbmBufferedSurface::new(
            surface,
            allocator,
            &[Fourcc::Xrgb8888, Fourcc::Argb8888],
            renderer_formats.clone(),
        )
        .map_err(|err| {
            io::Error::other(format!(
                "failed to create gbm buffered surface for {}:{}: {}",
                card_path.display(),
                connector_name,
                err
            ))
        })?;
        if let Err(err) = gbm_surface.surface().reset_state() {
            warn!(
                "failed to reset drm surface state for {}:{}: {}",
                card_path.display(),
                connector_name,
                err
            );
        }
        outputs.push(TtyDrmOutput {
            connector,
            crtc,
            connector_name,
            mode,
            gbm_surface: Rc::new(RefCell::new(gbm_surface)),
        });
    }
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
        _card_path: card_path.to_path_buf(),
        dev,
        notifier,
        renderer: Rc::new(RefCell::new(renderer)),
        outputs,
    })
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
                    let mode = info
                        .modes()
                        .first()
                        .copied()
                        .ok_or_else(|| io::Error::other(format!("connector {} has no modes", info)))?;
                    Ok((*conn, info.clone(), mode))
                })
                .collect::<Result<Vec<_>, io::Error>>()?
        } else {
            return Err(io::Error::other("viewport outputs are configured, but none are enabled").into());
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
                        // DRM vrefresh is an integer. Allow 2 Hz of slack so
                        // that e.g. a config of 59.94 matches DRM vrefresh=60,
                        // and 180.0 matches vrefresh=180 without rounding risk.
                        .is_none_or(|hz| (m.vrefresh() as f64 - hz).abs() < 2.0)
            }) else {
                warn!(
                    "configured viewport {} requests {}x{} @ {:?}Hz, but no matching DRM mode is available; skipping it",
                    wanted.connector,
                    wanted.width,
                    wanted.height,
                    wanted.refresh_rate
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
                    .map(|v| {
                        match v.refresh_rate {
                            Some(rate) => {
                                format!("{}={}x{}@{rate:.3}", v.connector, v.width, v.height)
                            }
                            None => format!("{}={}x{}", v.connector, v.width, v.height),
                        }
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

        // Collect the set of CRTCs this connector's encoder(s) can drive.
        // We must never assign a CRTC that isn't in possible_crtcs — the
        // kernel will reject the modeset with EINVAL even if the CRTC is
        // otherwise free.
        let possible_crtcs: std::collections::HashSet<drm_control::crtc::Handle> = {
            let mut set = std::collections::HashSet::new();
            // Check both the current encoder and all encoders the connector
            // can use, so we have the widest possible set to choose from.
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
                        set.insert(crtc);
                    }
                }
            }
            set
        };

        // Prefer the CRTC the current encoder is already driving (avoids a
        // full modeset on some drivers).
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

        // Otherwise pick any compatible, unused CRTC.
        if selected_crtc.is_none() {
            selected_crtc = possible_crtcs
                .iter()
                .copied()
                .find(|crtc| !used_crtcs.contains(crtc));
        }

        let Some(crtc) = selected_crtc else {
            return Err(io::Error::other(format!(
                "failed to find a usable CRTC for connector {} (possible CRTCs: {:?}, used: {:?})",
                selected_info,
                possible_crtcs,
                used_crtcs,
            ))
            .into());
        };

        // If the connector is already lit on this CRTC with the exact
        // requested resolution/refresh, prefer the live CRTC mode object over
        // an equivalent mode from the connector mode list. This reduces the
        // chance that Smithay sees a spurious mode mismatch and forces the
        // first frame down the blocking commit_pending()/modeset path.
        if let Some(enc) = selected_info.current_encoder()
            && let Ok(enc_info) = dev.get_encoder(enc)
            && enc_info.crtc() == Some(crtc)
            && let Ok(crtc_info) = dev.get_crtc(crtc)
            && let Some(current_mode) = crtc_info.mode()
        {
            let current_size = current_mode.size();
            let selected_size = selected_mode.size();
            let current_refresh = current_mode.vrefresh();
            let selected_refresh = selected_mode.vrefresh();
            if current_size == selected_size && current_refresh == selected_refresh {
                selected_mode = current_mode;
            }
        }

        used_crtcs.insert(crtc);
        selected.push((crtc, selected_mode, selected_conn, selected_info.to_string()));
    }

    if !configured.is_empty() {
        // Only disable connectors that the user explicitly configured but
        // that we failed to activate (wrong mode, not connected, etc.).
        // Previously this blanked ALL unselected connected connectors, which
        // caused DP-2 to be switched off whenever its mode-match failed —
        // even though the user had explicitly enabled it in their config.
        let configured_connectors: std::collections::HashSet<&str> =
            configured.iter().map(|v| v.connector.as_str()).collect();
        for (conn, info) in connected {
            if selected.iter().any(|(_, _, c, _)| *c == conn) {
                continue;
            }
            if !configured_connectors.contains(info.to_string().as_str()) {
                continue;
            }
            let enc = info
                .current_encoder()
                .or_else(|| info.encoders().first().copied());
            let Some(enc) = enc else {
                continue;
            };
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

pub(crate) fn find_tty_scanout_for_reload(
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
    select_tty_scanouts(dev, tuning)
}

pub(crate) fn collect_outputs_for_ipc(
    dev: &DrmDevice,
    active_modes: &HashMap<String, drm_control::Mode>,
    tuning: &RuntimeTuning,
    vrr_support: &HashMap<String, String>,
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
        let mut current_mode =
            active_mode.map(|mode| mode_info_from_drm_mode(mode, true, false));
        let mut modes = Vec::new();

        for mode in info.modes() {
            let current_match = active_mode.is_some_and(|active_mode| drm_mode_matches(*mode, active_mode));
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
    gbm_surface: &Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    renderer: &Rc<RefCell<GlesRenderer>>,
    st: &mut HalleyWlState,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> Result<(), Box<dyn Error>> {
    let previous_monitor = st.current_monitor.clone();
    let previous_layer_configure = st.suppress_layer_shell_configure;
    let _ = st.activate_monitor(output_name);
    let mut gbm_surface = gbm_surface.borrow_mut();
    let mut renderer = renderer.borrow_mut();
    let (mut dmabuf, _age) = gbm_surface.next_buffer()?;
    let mode = gbm_surface.pending_mode();
    let (w, h) = mode.size();
    let requested_vrr = st
        .tuning
        .tty_viewports
        .iter()
        .find(|viewport| viewport.connector == output_name)
        .map(|viewport| viewport.vrr.drm_enabled())
        .unwrap_or(false);
    if gbm_surface.vrr_enabled() != requested_vrr
        && let Err(err) = gbm_surface.use_vrr(requested_vrr)
    {
        warn!(
            "failed to set vrr={} for {}: {}",
            requested_vrr, output_name, err
        );
    }
    let local_cursor = cursor_screen.and_then(|(sx, sy)| {
        st.monitors.get(output_name).and_then(|monitor| {
            let inside = sx >= monitor.offset_x as f32
                && sx < (monitor.offset_x + monitor.width) as f32
                && sy >= monitor.offset_y as f32
                && sy < (monitor.offset_y + monitor.height) as f32;
            inside.then_some((sx - monitor.offset_x as f32, sy - monitor.offset_y as f32))
        })
    });
    {
        let draw_started_at = std::time::Instant::now();
        let mut target = renderer.bind(&mut dmabuf).map_err(|err| {
            io::Error::other(format!("failed to bind renderer to drm buffer: {}", err))
        })?;
        st.suppress_layer_shell_configure = output_name != previous_monitor;

        draw_debug_frame_to_target(
            &mut renderer,
            &mut target,
            (w as i32, h as i32).into(),
            st,
            resize_preview,
            hover_node,
            preview_hover_node,
            local_cursor,
            cursor_image,
            st.output_transform_for(output_name),
        )?;
        let _ = draw_started_at;
    }
    st.suppress_layer_shell_configure = previous_layer_configure;
    let _ = st.activate_monitor(previous_monitor.as_str());
    gbm_surface
        .queue_buffer(None, None, ())
        .map_err(|err| io::Error::other(format!("failed to queue drm frame: {}", err)))?;
    Ok(())
}
