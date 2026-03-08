use super::*;
pub(super) struct TtyDrmProbe {
    pub(super) _card_path: std::path::PathBuf,
    pub(super) _dev: DrmDevice,
    pub(super) notifier: smithay::backend::drm::DrmDeviceNotifier,
    pub(super) crtc: drm_control::crtc::Handle,
    pub(super) mode: drm_control::Mode,
    pub(super) gbm_surface: Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    pub(super) renderer: Rc<RefCell<GlesRenderer>>,
}

use crate::interaction::types::ResizeCtx;

pub(super) fn probe_tty_drm_device(
    seat: &str,
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
        match probe_tty_drm_device_path(card.as_path(), tuning) {
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

#[cfg(feature = "session-libseat")]
pub(super) fn probe_tty_drm_device_via_session(
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

pub(super) fn probe_tty_drm_device_path(
    card_path: &Path,
    tuning: &RuntimeTuning,
) -> Result<TtyDrmProbe, Box<dyn Error>> {
    use rustix::fs::{Mode, OFlags, open};
    let raw_fd = open(
        card_path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )
    .map_err(|err| {
        io::Error::other(format!(
            "failed to open drm device {}: {}",
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
    let (crtc, mode, connector, connector_name) = select_tty_scanout(&mut dev, tuning)?;
    let surface = dev
        .create_surface(crtc, mode, &[connector])
        .map_err(|err| {
            io::Error::other(format!(
                "failed to create drm surface on {}: {}",
                card_path.display(),
                err
            ))
        })?;
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
    let allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let gbm_surface = GbmBufferedSurface::new(
        surface,
        allocator,
        &[Fourcc::Xrgb8888, Fourcc::Argb8888],
        renderer_formats,
    )
    .map_err(|err| {
        io::Error::other(format!(
            "failed to create gbm buffered surface for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    info!(
        "tty drm device ready: card={} atomic={} crtcs={} connector={} mode={}x{}",
        card_path.display(),
        dev.is_atomic(),
        dev.crtcs().len(),
        connector_name,
        mode.size().0,
        mode.size().1
    );
    Ok(TtyDrmProbe {
        _card_path: card_path.to_path_buf(),
        _dev: dev,
        notifier,
        crtc,
        mode,
        gbm_surface: Rc::new(RefCell::new(gbm_surface)),
        renderer: Rc::new(RefCell::new(renderer)),
    })
}

#[cfg(feature = "session-libseat")]
pub(super) fn probe_tty_drm_device_path_via_session(
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
    let (crtc, mode, connector, connector_name) = select_tty_scanout(&mut dev, tuning)?;
    let surface = dev
        .create_surface(crtc, mode, &[connector])
        .map_err(|err| {
            io::Error::other(format!(
                "failed to create drm surface on {}: {}",
                card_path.display(),
                err
            ))
        })?;
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
    let allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let gbm_surface = GbmBufferedSurface::new(
        surface,
        allocator,
        &[Fourcc::Xrgb8888, Fourcc::Argb8888],
        renderer_formats,
    )
    .map_err(|err| {
        io::Error::other(format!(
            "failed to create gbm buffered surface for {}: {}",
            card_path.display(),
            err
        ))
    })?;
    info!(
        "tty drm device ready: card={} atomic={} crtcs={} connector={} mode={}x{}",
        card_path.display(),
        dev.is_atomic(),
        dev.crtcs().len(),
        connector_name,
        mode.size().0,
        mode.size().1
    );
    Ok(TtyDrmProbe {
        _card_path: card_path.to_path_buf(),
        _dev: dev,
        notifier,
        crtc,
        mode,
        gbm_surface: Rc::new(RefCell::new(gbm_surface)),
        renderer: Rc::new(RefCell::new(renderer)),
    })
}

pub(super) fn select_tty_scanout(
    dev: &mut DrmDevice,
    tuning: &RuntimeTuning,
) -> Result<
    (
        drm_control::crtc::Handle,
        drm_control::Mode,
        drm_control::connector::Handle,
        String,
    ),
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

    let configured = &tuning.tty_viewports;
    let selected = if configured.is_empty() {
        let (conn, info) = &connected[0];
        let mode = info
            .modes()
            .first()
            .copied()
            .ok_or_else(|| io::Error::other(format!("connector {} has no modes", info)))?;
        (*conn, info.clone(), mode)
    } else {
        if configured.len() > 1 {
            warn!(
                "multiple viewport outputs configured ({}); tty backend is single-output and will use the first connected match",
                configured.len()
            );
        }
        let mut found: Option<(
            drm_control::connector::Handle,
            drm_control::connector::Info,
            drm_control::Mode,
        )> = None;
        for wanted in configured {
            if let Some((conn, info)) = connected
                .iter()
                .find(|(_, info)| info.to_string() == wanted.connector)
            {
                let Some(mode) = info
                    .modes()
                    .iter()
                    .copied()
                    .find(|m| m.size() == (wanted.width as u16, wanted.height as u16))
                else {
                    return Err(io::Error::other(format!(
                        "configured viewport {} requests {}x{}, but no such mode is available",
                        wanted.connector, wanted.width, wanted.height
                    ))
                    .into());
                };
                found = Some((*conn, info.clone(), mode));
                break;
            }
        }
        found.ok_or_else(|| {
            io::Error::other(format!(
                "none of configured viewport connectors are connected: {}",
                configured
                    .iter()
                    .map(|v| v.connector.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?
    };

    let (selected_conn, selected_info, selected_mode) = selected;
    let mut selected_crtc: Option<drm_control::crtc::Handle> = None;
    if let Some(enc) = selected_info
        .current_encoder()
        .or_else(|| selected_info.encoders().first().copied())
    {
        if let Ok(enc_info) = dev.get_encoder(enc) {
            if let Some(existing_crtc) = enc_info.crtc() {
                selected_crtc = Some(existing_crtc);
            } else {
                let candidates = resources.filter_crtcs(enc_info.possible_crtcs());
                selected_crtc = candidates.first().copied();
            }
        }
    }
    if selected_crtc.is_none() {
        selected_crtc = resources.crtcs().first().copied();
    }
    let Some(crtc) = selected_crtc else {
        return Err(io::Error::other("failed to find a usable CRTC for selected connector").into());
    };

    if !configured.is_empty() {
        for (conn, info) in connected {
            if conn == selected_conn {
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

    Ok((
        crtc,
        selected_mode,
        selected_conn,
        selected_info.to_string(),
    ))
}

pub(super) fn queue_tty_drm_frame(
    gbm_surface: &Rc<RefCell<GbmBufferedSurface<GbmAllocator<DeviceFd>, ()>>>,
    renderer: &Rc<RefCell<GlesRenderer>>,
    st: &mut HalleyWlState,
    resize_preview: Option<ResizeCtx>,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> Result<(), Box<dyn Error>> {
    let mut gbm_surface = gbm_surface.borrow_mut();
    let mut renderer = renderer.borrow_mut();
    let (mut dmabuf, _age) = gbm_surface.next_buffer()?;
    let mode = gbm_surface.pending_mode();
    let (w, h) = mode.size();
    {
        let mut target = renderer.bind(&mut dmabuf).map_err(|err| {
            io::Error::other(format!("failed to bind renderer to drm buffer: {}", err))
        })?;

        draw_debug_frame_to_target(
            &mut renderer,
            &mut target,
            (w as i32, h as i32).into(),
            st,
            resize_preview,
            None,
            None,
            cursor_screen,
            cursor_image,
            Transform::Normal,
        )?;
    }
    gbm_surface
        .queue_buffer(None, None, ())
        .map_err(|err| io::Error::other(format!("failed to queue drm frame: {}", err)))?;
    Ok(())
}
