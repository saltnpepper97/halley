use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use halley_aperture::{
    ApertureConfig, ApertureMode, AperturePlacement, ApertureRuntime, ClockColor,
    PeekBackgroundColor, PeekCorner, Rect, Size,
};
use halley_api::{ApertureMode as IpcApertureMode, CompositorRequest, Request, Response};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rusttype::{Font, PositionedGlyph, Scale, point};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, EventQueue, QueueHandle,
    backend::WaylandError,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};

const CLOCK_NAMESPACE: &str = "halley-aperture";
const NORMAL_RIGHT_MARGIN_PX: i32 = 10;
const DEFAULT_SIZE: (u32, u32) = (1, 1);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(125);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CLOCK_REDRAW_INTERVAL: Duration = Duration::from_secs(1);
const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const PEEK_PADDING_X_PX: u32 = 14;
const PEEK_PADDING_Y_PX: u32 = 14;
const MINIMAL_PADDING_X_PX: u32 = 10;
const MINIMAL_PADDING_Y_PX: u32 = 4;
const MINIMAL_TAB_CROP_PX: f32 = 7.0;

pub fn run() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|err| format!("wayland connect: {err}"))?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).map_err(|err| format!("registry init: {err}"))?;
    let qh = event_queue.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|err| format!("bind wl_compositor: {err}"))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|err| format!("bind layer shell: {err}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|err| format!("bind wl_shm: {err}"))?;
    let config_path = default_aperture_config_path();
    let initial_config = load_aperture_config(config_path.as_path());
    let font_renderer = load_font_renderer(initial_config.peek.clock.font_family.as_str())?;
    let config_signature = aperture_config_signature(config_path.as_path());
    let pool = SlotPool::new(4096, &shm).map_err(|err| format!("slot pool: {err}"))?;
    let mut app = StandaloneAperture {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        runtime: ApertureRuntime::new(initial_config),
        font_renderer,
        config_path,
        config_signature,
        next_config_poll: Instant::now(),
        next_status_poll: Instant::now(),
        next_clock_redraw: Instant::now(),
        layers: Vec::new(),
        configured: false,
        desired_output_name: None,
        status_modes: HashMap::new(),
        attached_output_names: Vec::new(),
        monitor_fallback_warned: false,
        exit: false,
    };

    event_queue
        .roundtrip(&mut app)
        .map_err(|err| format!("initial roundtrip 1: {err}"))?;
    event_queue
        .roundtrip(&mut app)
        .map_err(|err| format!("initial roundtrip 2: {err}"))?;

    app.refresh_halley_status();
    app.recreate_layer(&qh)?;

    while !app.exit {
        event_queue
            .dispatch_pending(&mut app)
            .map_err(|err| format!("event dispatch: {err}"))?;
        app.tick(&qh)?;
        wait_for_wayland_or_timeout(&mut event_queue, app.next_wayland_wait_timeout())?;
    }

    Ok(())
}

struct StandaloneAperture {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    runtime: ApertureRuntime,
    font_renderer: FontRenderer,
    config_path: PathBuf,
    config_signature: ConfigSignature,
    next_config_poll: Instant,
    next_status_poll: Instant,
    next_clock_redraw: Instant,
    layers: Vec<LayerInstance>,
    configured: bool,
    desired_output_name: Option<String>,
    status_modes: HashMap<String, ApertureMode>,
    attached_output_names: Vec<Option<String>>,
    monitor_fallback_warned: bool,
    exit: bool,
}

struct LayerInstance {
    surface: LayerSurface,
    output_name: Option<String>,
    runtime: ApertureRuntime,
    last_tick: Instant,
    configured: bool,
}

impl StandaloneAperture {
    fn recreate_layer(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        self.layers.clear();
        self.configured = false;
        let output_names = self.desired_layer_output_names();

        for output_name in output_names.iter().cloned() {
            let surface = self.compositor.create_surface(qh);
            let output = self.find_output_by_name(output_name.as_deref());
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface,
                Layer::Bottom,
                Some(CLOCK_NAMESPACE),
                output.as_ref(),
            );
            layer.set_anchor(anchor_for_corner(self.runtime.config().peek.corner));
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
            set_layer_margin(
                &layer,
                self.runtime.config().peek.corner,
                0,
                NORMAL_RIGHT_MARGIN_PX,
            );
            let mut runtime = ApertureRuntime::new(self.runtime.config().clone());
            runtime.jump_to_mode(self.mode_for_new_layer(output_name.as_deref()));
            layer.set_exclusive_zone(0);
            // Aperture is display-only (no pointer/keyboard handlers). An empty input region
            // makes the surface fully click-through, so windows overlapping the clock/tab
            // still receive hover and click events instead of the compositor routing every
            // pointer event in Aperture's rectangle to this surface.
            if let Ok(region) = Region::new(&self.compositor) {
                layer
                    .wl_surface()
                    .set_input_region(Some(region.wl_region()));
            }
            layer.commit();
            self.layers.push(LayerInstance {
                surface: layer,
                output_name,
                runtime,
                last_tick: Instant::now(),
                configured: false,
            });
        }

        self.attached_output_names = output_names;
        Ok(())
    }

    fn find_output_by_name(&self, target_name: Option<&str>) -> Option<wl_output::WlOutput> {
        self.output_state.outputs().into_iter().find(|output| {
            let Some(info) = self.output_state.info(output) else {
                return target_name.is_none();
            };
            match target_name {
                Some(target_name) => info.name.as_deref() == Some(target_name),
                None => true,
            }
        })
    }

    fn desired_layer_output_names(&mut self) -> Vec<Option<String>> {
        match self.runtime.config().placement {
            AperturePlacement::Cursor => vec![self.desired_output_name.clone()],
            AperturePlacement::All => {
                let mut names = self.output_names();
                if names.is_empty() {
                    names.push(None);
                }
                names
            }
            AperturePlacement::Monitor => {
                let monitor = self.runtime.config().monitor.clone();
                if let Some(monitor) = monitor.as_deref() {
                    if self.output_name_exists(monitor) {
                        self.monitor_fallback_warned = false;
                        return vec![Some(monitor.to_string())];
                    }
                }

                if !self.monitor_fallback_warned {
                    match monitor.as_deref() {
                        Some(monitor) => eprintln!(
                            "halley-aperture: monitor '{monitor}' not found; falling back to cursor placement"
                        ),
                        None => eprintln!(
                            "halley-aperture: monitor placement requested without monitor; falling back to cursor placement"
                        ),
                    }
                    self.monitor_fallback_warned = true;
                }
                vec![self.desired_output_name.clone()]
            }
        }
    }

    fn output_names(&self) -> Vec<Option<String>> {
        self.output_state
            .outputs()
            .into_iter()
            .filter_map(|output| self.output_state.info(&output))
            .filter_map(|info| info.name.clone().map(Some))
            .collect()
    }

    fn output_name_exists(&self, target_name: &str) -> bool {
        self.output_state.outputs().into_iter().any(|output| {
            self.output_state
                .info(&output)
                .is_some_and(|info| info.name.as_deref() == Some(target_name))
        })
    }

    fn refresh_halley_status(&mut self) -> bool {
        self.next_status_poll = Instant::now() + STATUS_POLL_INTERVAL;
        let previous_output = self.desired_output_name.clone();
        let previous_modes = self.status_modes.clone();
        let previous_mode = self.runtime.target_mode();
        let request = Request::Compositor(CompositorRequest::ApertureStatus);
        let response = halley_ipc::send_request(&request);
        let (output, mode, modes) = match response {
            Ok(Response::ApertureStatus(status)) => {
                let modes = status
                    .outputs
                    .into_iter()
                    .map(|output| (output.output, map_ipc_mode(output.mode)))
                    .collect();
                (status.output, map_ipc_mode(status.mode), modes)
            }
            _ => (
                self.desired_output_name.clone(),
                ApertureMode::Normal,
                HashMap::new(),
            ),
        };

        self.desired_output_name = output;
        self.status_modes = modes;
        self.runtime.set_mode(mode);
        let mode_changed = self.update_layer_modes();
        let changed = self.desired_output_name != previous_output
            || mode != previous_mode
            || self.status_modes != previous_modes
            || mode_changed;
        changed
    }

    fn update_layer_modes(&mut self) -> bool {
        let now = Instant::now();
        let modes = self.status_modes.clone();
        let fallback = self.runtime.target_mode();
        let placement = self.runtime.config().placement;
        let mut changed = false;
        for layer in &mut self.layers {
            let mode =
                mode_for_output_in(&modes, fallback, placement, layer.output_name.as_deref())
                    .unwrap_or_else(|| layer.runtime.target_mode());
            if layer.runtime.target_mode() != mode {
                if mode == ApertureMode::Minimal {
                    layer.runtime.jump_to_mode(mode);
                } else {
                    layer.runtime.set_mode(mode);
                }
                layer.last_tick = now;
                changed = true;
            }
        }
        changed
    }

    fn mode_for_new_layer(&self, output_name: Option<&str>) -> ApertureMode {
        mode_for_output_in(
            &self.status_modes,
            self.runtime.target_mode(),
            self.runtime.config().placement,
            output_name,
        )
        .unwrap_or_else(|| self.runtime.target_mode())
    }

    fn maybe_reload_config(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_config_poll {
            return false;
        }
        self.next_config_poll = now + CONFIG_POLL_INTERVAL;

        let next_signature = aperture_config_signature(self.config_path.as_path());
        if self.config_signature == next_signature {
            return false;
        }

        let next_config = load_aperture_config(self.config_path.as_path());
        match load_font_renderer(next_config.peek.clock.font_family.as_str()) {
            Ok(next_font_renderer) => {
                self.font_renderer = next_font_renderer;
                self.runtime.apply_config(next_config);
                for layer in &mut self.layers {
                    layer.runtime.apply_config(self.runtime.config().clone());
                }
                self.config_signature = next_signature;
                self.monitor_fallback_warned = false;
                true
            }
            Err(err) => {
                eprintln!("halley-aperture config reload skipped: {err}");
                self.config_signature = next_signature;
                false
            }
        }
    }

    fn tick(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        let mut needs_draw = self.maybe_reload_config();
        if Instant::now() >= self.next_status_poll && self.refresh_halley_status() {
            needs_draw = true;
        }

        let desired_outputs = self.desired_layer_output_names();
        if self.attached_output_names != desired_outputs {
            self.recreate_layer(qh)?;
            return Ok(());
        }

        let now = Instant::now();
        if now >= self.next_clock_redraw {
            self.next_clock_redraw = now + CLOCK_REDRAW_INTERVAL;
            needs_draw = true;
        }
        if self
            .layers
            .iter()
            .any(|layer| layer.runtime.animation_active())
        {
            needs_draw = true;
        }

        if needs_draw && self.configured {
            self.draw()?;
        }

        Ok(())
    }

    fn next_wayland_wait_timeout(&self) -> Option<Duration> {
        if !self.configured || !self.layers.iter().any(|layer| layer.configured) {
            return None;
        }

        let now = Instant::now();
        let mut next_wake = self
            .next_status_poll
            .min(self.next_config_poll)
            .min(self.next_clock_redraw);
        if self
            .layers
            .iter()
            .any(|layer| layer.runtime.animation_active())
        {
            next_wake = next_wake.min(now + ANIMATION_FRAME_INTERVAL);
        }

        Some(next_wake.saturating_duration_since(now))
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.configured || !self.layers.iter().any(|layer| layer.configured) {
            return Ok(());
        }

        let now = Instant::now();
        let system_now = SystemTime::now();
        for layer in self.layers.iter_mut().filter(|layer| layer.configured) {
            let dt = now.saturating_duration_since(layer.last_tick);
            layer.last_tick = now;
            layer.runtime.update(dt, system_now);

            let snapshot = layer.runtime.snapshot(
                Rect::new(0.0, 0.0, 4096.0, 512.0),
                Rect::new(0.0, 0.0, 4096.0, 512.0),
                1.0,
                |font_px, text| self.font_renderer.measure(text, font_px),
            );

            let (text_width, text_height, edge_margin, text) = match snapshot {
                Some(snapshot) => (
                    snapshot.bounds.w.ceil().max(1.0) as u32,
                    snapshot.bounds.h.ceil().max(1.0) as u32,
                    snapshot.bounds.y.round().max(0.0) as i32,
                    Some(snapshot),
                ),
                None => (1, 1, 0, None),
            };

            let has_text = text.is_some();
            let is_minimal = layer.runtime.target_mode() == ApertureMode::Minimal;
            let padding_x = peek_padding_x_for_mode(layer.runtime.target_mode());
            let padding_y = peek_padding_y_for_mode(layer.runtime.target_mode());
            // Minimal is the reserved bar: its height comes from config
            // (`clock-small.height-px`) so the compositor can reserve it exactly,
            // not from the measured text. `max` with the text box avoids clipping
            // during the shrink animation; it settles to the configured height.
            let small_height_px = layer.runtime.config().peek.clock.small_height_px.max(1);
            let (buffer_width, buffer_height) = if has_text {
                let text_box_h = text_height + padding_y * 2;
                let h = if is_minimal {
                    text_box_h.max(small_height_px)
                } else {
                    text_box_h
                };
                (text_width + padding_x * 2, h)
            } else {
                (1, 1)
            };
            // Center the clock vertically inside the Minimal bar; other states keep
            // their top padding.
            let text_offset_y = if is_minimal {
                ((buffer_height as i32 - text_height as i32) / 2).max(0)
            } else {
                padding_y as i32
            };
            let outer_margin = if has_text {
                edge_margin.saturating_sub(padding_y as i32)
            } else {
                0
            };

            let stride = buffer_width as i32 * 4;
            let needed = buffer_height as usize * stride as usize;
            if self.pool.len() < needed {
                self.pool
                    .resize(needed)
                    .map_err(|err| format!("resize shm pool: {err}"))?;
            }
            layer.surface.set_exclusive_zone(0);
            layer.surface.set_size(buffer_width, buffer_height);
            set_layer_margin(
                &layer.surface,
                layer.runtime.config().peek.corner,
                outer_margin,
                NORMAL_RIGHT_MARGIN_PX,
            );
            let (buffer, canvas) = self
                .pool
                .create_buffer(
                    buffer_width as i32,
                    buffer_height as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .map_err(|err| format!("create buffer: {err}"))?;
            canvas.fill(0);

            if let Some(snapshot) = text.as_ref() {
                if layer.runtime.target_mode() == ApertureMode::Minimal {
                    fill_clipped_top_tab_rect(
                        canvas,
                        buffer_width,
                        buffer_height,
                        layer.runtime.config().peek.radius_px,
                        layer.runtime.config().peek.background,
                        snapshot.alpha,
                    );
                } else {
                    fill_rounded_rect(
                        canvas,
                        buffer_width,
                        buffer_height,
                        layer.runtime.config().peek.radius_px,
                        layer.runtime.config().peek.background,
                        snapshot.alpha,
                    );
                }
                self.font_renderer.draw(
                    canvas,
                    buffer_width,
                    buffer_height,
                    padding_x as i32,
                    text_offset_y,
                    snapshot.text.as_str(),
                    snapshot.font_px,
                    snapshot.alpha,
                    layer.runtime.config().peek.clock.color,
                );
            }

            layer.surface.wl_surface().damage_buffer(
                0,
                0,
                buffer_width as i32,
                buffer_height as i32,
            );
            buffer
                .attach_to(layer.surface.wl_surface())
                .map_err(|err| format!("attach buffer: {err}"))?;
            layer.surface.commit();
        }
        Ok(())
    }
}

fn wait_for_wayland_or_timeout(
    event_queue: &mut EventQueue<StandaloneAperture>,
    timeout: Option<Duration>,
) -> Result<(), String> {
    event_queue
        .flush()
        .map_err(|err| format!("wayland flush: {err}"))?;

    let Some(read_guard) = event_queue.prepare_read() else {
        return Ok(());
    };

    let timeout = timeout.map(duration_to_timespec);
    let mut fds = [PollFd::new(event_queue, PollFlags::IN)];
    let ready = loop {
        match poll(&mut fds, timeout.as_ref()) {
            Ok(ready) => break ready,
            Err(rustix::io::Errno::INTR) => continue,
            Err(err) => return Err(format!("wayland poll: {err}")),
        }
    };
    let revents = fds[0].revents();
    if ready > 0 && revents.intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP) {
        match read_guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => return Err(format!("wayland read: {err}")),
        }
    } else {
        drop(read_guard);
    }

    Ok(())
}

fn duration_to_timespec(duration: Duration) -> Timespec {
    Timespec {
        tv_sec: duration.as_secs().min(i64::MAX as u64) as i64,
        tv_nsec: duration.subsec_nanos().into(),
    }
}

fn mode_for_output_in(
    modes: &HashMap<String, ApertureMode>,
    fallback: ApertureMode,
    placement: AperturePlacement,
    output_name: Option<&str>,
) -> Option<ApertureMode> {
    match placement {
        AperturePlacement::Cursor => Some(fallback),
        AperturePlacement::Monitor | AperturePlacement::All => output_name
            .and_then(|name| modes.get(name).copied())
            .or(Some(fallback)),
    }
}

fn anchor_for_corner(corner: PeekCorner) -> Anchor {
    match corner {
        PeekCorner::TopLeft => Anchor::TOP | Anchor::LEFT,
        PeekCorner::TopRight => Anchor::TOP | Anchor::RIGHT,
        PeekCorner::BottomLeft => Anchor::BOTTOM | Anchor::LEFT,
        PeekCorner::BottomRight => Anchor::BOTTOM | Anchor::RIGHT,
    }
}

fn set_layer_margin(layer: &LayerSurface, corner: PeekCorner, edge_margin: i32, side_margin: i32) {
    let (top, right, bottom, left) = match corner {
        PeekCorner::TopLeft => (edge_margin, 0, 0, side_margin),
        PeekCorner::TopRight => (edge_margin, side_margin, 0, 0),
        PeekCorner::BottomLeft => (0, 0, edge_margin, side_margin),
        PeekCorner::BottomRight => (0, side_margin, edge_margin, 0),
    };
    layer.set_margin(top, right, bottom, left);
}

fn peek_padding_x_for_mode(mode: ApertureMode) -> u32 {
    if mode == ApertureMode::Minimal {
        MINIMAL_PADDING_X_PX
    } else {
        PEEK_PADDING_X_PX
    }
}

fn peek_padding_y_for_mode(mode: ApertureMode) -> u32 {
    if mode == ApertureMode::Minimal {
        MINIMAL_PADDING_Y_PX
    } else {
        PEEK_PADDING_Y_PX
    }
}

fn fill_rounded_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    radius_px: u32,
    color: PeekBackgroundColor,
    alpha: f32,
) {
    let alpha = (color.a * alpha).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }

    let size = (width.max(1) as f32, height.max(1) as f32);
    let radius = (radius_px as f32).min(size.0.min(size.1) * 0.5);
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5 - size.0 * 0.5;
            let py = y as f32 + 0.5 - size.1 * 0.5;
            let dist = rounded_rect_sdf(px, py, size.0, size.1, radius);
            let coverage = sdf_alpha(dist);
            if coverage > 0.0 {
                let offset = ((y * width + x) * 4) as usize;
                blend_argb8888(
                    &mut canvas[offset..offset + 4],
                    color.r,
                    color.g,
                    color.b,
                    alpha * coverage,
                );
            }
        }
    }
}

fn fill_clipped_top_tab_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    radius_px: u32,
    color: PeekBackgroundColor,
    alpha: f32,
) {
    let alpha = (color.a * alpha).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }

    let rect_w = width.max(1) as f32;
    let rect_h = height.max(1) as f32 + MINIMAL_TAB_CROP_PX;
    let radius = (radius_px as f32).min(rect_w.min(rect_h) * 0.5);
    let center_y = -MINIMAL_TAB_CROP_PX + rect_h * 0.5;
    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5 - rect_w * 0.5;
            let py = y as f32 + 0.5 - center_y;
            let dist = rounded_rect_sdf(px, py, rect_w, rect_h, radius);
            let coverage = sdf_alpha(dist);
            if coverage > 0.0 {
                let offset = ((y * width + x) * 4) as usize;
                blend_argb8888(
                    &mut canvas[offset..offset + 4],
                    color.r,
                    color.g,
                    color.b,
                    alpha * coverage,
                );
            }
        }
    }
}

fn rounded_rect_sdf(px: f32, py: f32, width: f32, height: f32, radius: f32) -> f32 {
    let qx = px.abs() - (width * 0.5 - radius);
    let qy = py.abs() - (height * 0.5 - radius);
    let outside_x = qx.max(0.0);
    let outside_y = qy.max(0.0);
    (outside_x * outside_x + outside_y * outside_y).sqrt() + qx.max(qy).min(0.0) - radius
}

fn sdf_alpha(dist: f32) -> f32 {
    1.0 - smoothstep(-0.75, 0.75, dist)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl CompositorHandler for StandaloneAperture {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Err(err) = self.tick(qh) {
            eprintln!("draw failed: {err}");
            self.exit = true;
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for StandaloneAperture {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layer(qh) {
            eprintln!("recreate layer failed: {err}");
            self.exit = true;
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layer(qh) {
            eprintln!("recreate layer failed: {err}");
            self.exit = true;
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layer(qh) {
            eprintln!("recreate layer failed: {err}");
            self.exit = true;
        }
    }
}

impl LayerShellHandler for StandaloneAperture {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.layers
            .retain(|entry| entry.surface.wl_surface() != layer.wl_surface());
        self.configured = self.layers.iter().any(|entry| entry.configured);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        for entry in &mut self.layers {
            if entry.surface.wl_surface() == layer.wl_surface() {
                entry.configured = true;
            }
        }
        self.configured = self.layers.iter().any(|entry| entry.configured);
        if let Err(err) = self.draw() {
            eprintln!("draw failed: {err}");
            self.exit = true;
        }
    }
}

impl ShmHandler for StandaloneAperture {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for StandaloneAperture {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(StandaloneAperture);
delegate_output!(StandaloneAperture);
delegate_layer!(StandaloneAperture);
delegate_shm!(StandaloneAperture);
delegate_registry!(StandaloneAperture);

fn load_font_renderer(family: &str) -> Result<FontRenderer, String> {
    FontRenderer::new(family).or_else(|err| {
        eprintln!("halley-aperture font fallback: {err}; using monospace");
        FontRenderer::new("monospace")
            .map_err(|fallback_err| format!("{err}; fallback monospace failed: {fallback_err}"))
    })
}

struct FontRenderer {
    font: Font<'static>,
}

impl FontRenderer {
    fn new(family: &str) -> Result<Self, String> {
        let request = parse_font_request(family);
        let mut db = Database::new();
        db.load_system_fonts();
        let families = if request.family.trim().eq_ignore_ascii_case("serif") {
            vec![Family::Serif, Family::Monospace, Family::SansSerif]
        } else if matches!(
            request.family.trim().to_ascii_lowercase().as_str(),
            "sans-serif" | "sans_serif" | "sansserif" | "sans"
        ) {
            vec![Family::SansSerif, Family::Monospace, Family::Serif]
        } else if request.family.trim().eq_ignore_ascii_case("cursive") {
            vec![Family::Cursive, Family::SansSerif, Family::Monospace]
        } else if request.family.trim().eq_ignore_ascii_case("fantasy") {
            vec![Family::Fantasy, Family::SansSerif, Family::Monospace]
        } else if request.family.trim().eq_ignore_ascii_case("monospace")
            || request.family.trim().is_empty()
        {
            vec![Family::Monospace, Family::SansSerif]
        } else {
            vec![
                Family::Name(request.family),
                Family::Monospace,
                Family::SansSerif,
            ]
        };
        let id = db
            .query(&Query {
                families: families.as_slice(),
                weight: request.weight,
                stretch: Stretch::Normal,
                style: request.style,
            })
            .ok_or_else(|| format!("unable to resolve font family '{family}'"))?;
        let bytes = db
            .with_face_data(id, |data, _face_index| data.to_vec())
            .ok_or_else(|| format!("unable to load font data for '{family}'"))?;
        let font = Font::try_from_vec(bytes)
            .ok_or_else(|| format!("invalid font bytes for '{family}'"))?;
        Ok(Self { font })
    }

    fn measure(&self, text: &str, font_px: u32) -> Size {
        let bounds = self.layout_bounds(text, font_px);
        Size {
            w: bounds.w.max(1.0),
            h: bounds.h.max(1.0),
        }
    }

    fn draw(
        &self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        offset_x: i32,
        offset_y: i32,
        text: &str,
        font_px: u32,
        alpha: f32,
        color: ClockColor,
    ) {
        let scale = Scale::uniform(font_px as f32);
        let v_metrics = self.font.v_metrics(scale);
        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(0.0, v_metrics.ascent))
            .collect();

        let Some(bounds) = union_bounds(&glyphs) else {
            return;
        };

        for glyph in glyphs {
            let Some(bb) = glyph.pixel_bounding_box() else {
                continue;
            };
            glyph.draw(|x, y, coverage| {
                let px = offset_x + bb.min.x - bounds.min_x + x as i32;
                let py = offset_y + bb.min.y - bounds.min_y + y as i32;
                if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                    return;
                }
                let src_alpha = (coverage * alpha).clamp(0.0, 1.0);
                if src_alpha <= 0.0 {
                    return;
                }
                let offset = ((py as u32 * width + px as u32) * 4) as usize;
                blend_argb8888(
                    &mut canvas[offset..offset + 4],
                    color.r,
                    color.g,
                    color.b,
                    src_alpha,
                );
            });
        }
    }

    fn layout_bounds(&self, text: &str, font_px: u32) -> LayoutBounds {
        let scale = Scale::uniform(font_px as f32);
        let v_metrics = self.font.v_metrics(scale);
        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(0.0, v_metrics.ascent))
            .collect();
        union_bounds(&glyphs).unwrap_or(LayoutBounds {
            min_x: 0,
            min_y: 0,
            w: font_px as f32,
            h: font_px as f32,
        })
    }
}

#[derive(Clone, Copy)]
struct ParsedFontRequest<'a> {
    family: &'a str,
    style: Style,
    weight: Weight,
}

fn parse_font_request(requested: &str) -> ParsedFontRequest<'_> {
    let trimmed = requested.trim();
    let mut family = trimmed;
    let mut style = Style::Normal;
    let mut weight = Weight::NORMAL;

    loop {
        if matches!(style, Style::Normal) {
            if let Some(stripped) = strip_font_suffix(family, &[" italic"]) {
                family = stripped;
                style = Style::Italic;
                continue;
            }
            if let Some(stripped) = strip_font_suffix(family, &[" oblique"]) {
                family = stripped;
                style = Style::Oblique;
                continue;
            }
        }

        if matches!(weight, Weight::NORMAL) {
            let weight_suffixes = [
                (
                    &[
                        " extra bold",
                        " extra-bold",
                        " extrabold",
                        " ultra bold",
                        " ultra-bold",
                        " ultrabold",
                    ][..],
                    Weight::EXTRA_BOLD,
                ),
                (
                    &[
                        " semi bold",
                        " semi-bold",
                        " semibold",
                        " demi bold",
                        " demi-bold",
                        " demibold",
                    ][..],
                    Weight::SEMIBOLD,
                ),
                (
                    &[
                        " extra light",
                        " extra-light",
                        " extralight",
                        " ultra light",
                        " ultra-light",
                        " ultralight",
                    ][..],
                    Weight::EXTRA_LIGHT,
                ),
                (&[" bold"][..], Weight::BOLD),
                (&[" medium"][..], Weight::MEDIUM),
                (&[" light"][..], Weight::LIGHT),
                (&[" thin"][..], Weight::THIN),
                (&[" black", " heavy"][..], Weight::BLACK),
                (
                    &[" regular", " normal", " book", " roman"][..],
                    Weight::NORMAL,
                ),
            ];
            if let Some((stripped, parsed_weight)) =
                weight_suffixes
                    .iter()
                    .find_map(|(suffixes, parsed_weight)| {
                        strip_font_suffix(family, suffixes)
                            .map(|stripped| (stripped, *parsed_weight))
                    })
            {
                family = stripped;
                weight = parsed_weight;
                continue;
            }
        }

        break;
    }

    ParsedFontRequest {
        family: if family.trim().is_empty() {
            trimmed
        } else {
            family.trim()
        },
        style,
        weight,
    }
}

fn strip_font_suffix<'a>(value: &'a str, suffixes: &[&str]) -> Option<&'a str> {
    let folded = value.to_ascii_lowercase();
    suffixes.iter().find_map(|suffix| {
        folded
            .ends_with(suffix)
            .then(|| value[..value.len().saturating_sub(suffix.len())].trim_end())
    })
}

#[derive(Clone, Copy)]
struct LayoutBounds {
    min_x: i32,
    min_y: i32,
    w: f32,
    h: f32,
}

fn union_bounds(glyphs: &[PositionedGlyph<'_>]) -> Option<LayoutBounds> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for glyph in glyphs {
        let Some(bb) = glyph.pixel_bounding_box() else {
            continue;
        };
        min_x = min_x.min(bb.min.x);
        min_y = min_y.min(bb.min.y);
        max_x = max_x.max(bb.max.x);
        max_y = max_y.max(bb.max.y);
    }

    if min_x == i32::MAX {
        None
    } else {
        Some(LayoutBounds {
            min_x,
            min_y,
            w: (max_x - min_x).max(1) as f32,
            h: (max_y - min_y).max(1) as f32,
        })
    }
}

fn blend_argb8888(dst: &mut [u8], r: f32, g: f32, b: f32, a: f32) {
    let dst_b = dst[0] as f32 / 255.0;
    let dst_g = dst[1] as f32 / 255.0;
    let dst_r = dst[2] as f32 / 255.0;
    let dst_a = dst[3] as f32 / 255.0;

    let out_a = a + dst_a * (1.0 - a);
    let src_r = r * a;
    let src_g = g * a;
    let src_b = b * a;
    let out_r = src_r + dst_r * (1.0 - a);
    let out_g = src_g + dst_g * (1.0 - a);
    let out_b = src_b + dst_b * (1.0 - a);

    dst[0] = (out_b.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[1] = (out_g.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[2] = (out_r.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

fn map_ipc_mode(mode: IpcApertureMode) -> ApertureMode {
    match mode {
        IpcApertureMode::Normal => ApertureMode::Normal,
        IpcApertureMode::Collapsed => ApertureMode::Collapsed,
        IpcApertureMode::Minimal => ApertureMode::Minimal,
        IpcApertureMode::Hidden => ApertureMode::Hidden,
    }
}

fn load_aperture_config(path: &Path) -> ApertureConfig {
    if !path.exists() {
        return ApertureConfig::default();
    }
    ApertureConfig::parse_file(path).unwrap_or_default()
}

type ConfigSignature = Vec<(PathBuf, Option<SystemTime>)>;

fn aperture_config_signature(path: &Path) -> ConfigSignature {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    push_unique_path(&mut paths, &mut seen, path.to_path_buf());
    for dep in halley_config::gather_dependencies_for_file(path.to_string_lossy().as_ref()) {
        push_unique_path(&mut paths, &mut seen, dep);
    }
    paths
        .into_iter()
        .map(|path| {
            let mtime = fs::metadata(path.as_path())
                .and_then(|meta| meta.modified())
                .ok();
            (path, mtime)
        })
        .collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        paths.push(path);
    }
}

fn default_aperture_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join("halley/aperture.rune");
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".config/halley/aperture.rune");
        }
    }

    PathBuf::from("aperture.rune")
}
