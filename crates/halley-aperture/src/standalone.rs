use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use halley_aperture::{ApertureConfig, ApertureMode, ApertureRuntime, ClockColor, Rect, Size};
use rusttype::{Font, PositionedGlyph, Scale, point};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
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
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};

const CLOCK_NAMESPACE: &str = "halley-aperture";
const NORMAL_RIGHT_MARGIN_PX: i32 = 18;
const DEFAULT_SIZE: (u32, u32) = (1, 1);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(125);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
    let font_renderer = FontRenderer::new(initial_config.clock.font_family.as_str())?;
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
        config_mtime: None,
        next_config_poll: Instant::now(),
        next_status_poll: Instant::now(),
        last_tick: Instant::now(),
        layer: None,
        configured: false,
        desired_output_name: None,
        attached_output_name: None,
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
        if app.runtime.overlay_active() || !app.configured {
            event_queue
                .blocking_dispatch(&mut app)
                .map_err(|err| format!("event dispatch: {err}"))?;
        } else {
            event_queue
                .dispatch_pending(&mut app)
                .map_err(|err| format!("event dispatch: {err}"))?;
            let now = Instant::now();
            let next_wake = app.next_status_poll.min(app.next_config_poll);
            if now < next_wake {
                std::thread::sleep((next_wake - now).min(Duration::from_millis(32)));
            }
            if let Err(err) = app.draw(&qh) {
                return Err(format!("draw failed: {err}"));
            }
        }
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
    config_mtime: Option<SystemTime>,
    next_config_poll: Instant,
    next_status_poll: Instant,
    last_tick: Instant,
    layer: Option<LayerSurface>,
    configured: bool,
    desired_output_name: Option<String>,
    attached_output_name: Option<String>,
    exit: bool,
}

impl StandaloneAperture {
    fn recreate_layer(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        self.layer = None;
        self.configured = false;

        let surface = self.compositor.create_surface(qh);
        let output = self.find_target_output();
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Top,
            Some(CLOCK_NAMESPACE),
            output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        layer.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
        layer.set_margin(0, NORMAL_RIGHT_MARGIN_PX, 0, 0);
        layer.commit();

        self.attached_output_name = self.desired_output_name.clone();
        self.layer = Some(layer);
        Ok(())
    }

    fn find_target_output(&self) -> Option<wl_output::WlOutput> {
        let target_name = self.desired_output_name.as_deref();
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

    fn refresh_halley_status(&mut self) {
        self.next_status_poll = Instant::now() + STATUS_POLL_INTERVAL;
        let request =
            halley_ipc::Request::Compositor(halley_ipc::CompositorRequest::ApertureStatus);
        let response = halley_ipc::send_request(&request);
        let (output, mode) = match response {
            Ok(halley_ipc::Response::ApertureStatus(status)) => {
                (status.output, map_ipc_mode(status.mode))
            }
            _ => (self.desired_output_name.clone(), ApertureMode::Normal),
        };

        self.desired_output_name = output;
        self.runtime.set_mode(mode);
    }

    fn maybe_reload_config(&mut self) {
        let now = Instant::now();
        if now < self.next_config_poll {
            return;
        }
        self.next_config_poll = now + CONFIG_POLL_INTERVAL;

        let next_mtime = fs::metadata(self.config_path.as_path())
            .and_then(|meta| meta.modified())
            .ok();
        if self.config_mtime == next_mtime {
            return;
        }

        let next_config = load_aperture_config(self.config_path.as_path());
        if let Ok(next_font_renderer) = FontRenderer::new(next_config.clock.font_family.as_str()) {
            self.font_renderer = next_font_renderer;
            self.runtime.apply_config(next_config);
            self.config_mtime = next_mtime;
        }
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        self.maybe_reload_config();
        if Instant::now() >= self.next_status_poll {
            self.refresh_halley_status();
        }

        if self.attached_output_name != self.desired_output_name {
            self.recreate_layer(qh)?;
            return Ok(());
        }

        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        self.runtime.update(dt, SystemTime::now());

        let snapshot = self.runtime.snapshot(
            Rect::new(0.0, 0.0, 4096.0, 512.0),
            Rect::new(0.0, 0.0, 4096.0, 512.0),
            1.0,
            |font_px, text| self.font_renderer.measure(text, font_px),
        );

        let (buffer_width, buffer_height, top_margin, text) = match snapshot {
            Some(snapshot) => (
                snapshot.bounds.w.ceil().max(1.0) as u32,
                snapshot.bounds.h.ceil().max(1.0) as u32,
                snapshot.bounds.y.round() as i32,
                Some(snapshot),
            ),
            None => (1, 1, 0, None),
        };

        let layer = self
            .layer
            .as_ref()
            .ok_or_else(|| "layer surface missing".to_string())?;
        layer.set_size(buffer_width, buffer_height);
        layer.set_margin(top_margin, NORMAL_RIGHT_MARGIN_PX, 0, 0);

        let stride = buffer_width as i32 * 4;
        let needed = buffer_height as usize * stride as usize;
        if self.pool.len() < needed {
            self.pool
                .resize(needed)
                .map_err(|err| format!("resize shm pool: {err}"))?;
        }
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

        if let Some(snapshot) = text {
            self.font_renderer.draw(
                canvas,
                buffer_width,
                buffer_height,
                snapshot.text.as_str(),
                snapshot.font_px,
                snapshot.alpha,
                self.runtime.config().clock.color,
            );
        }

        let request_next_frame = self.runtime.overlay_active();

        layer
            .wl_surface()
            .damage_buffer(0, 0, buffer_width as i32, buffer_height as i32);
        if request_next_frame {
            layer.wl_surface().frame(qh, layer.wl_surface().clone());
        }
        buffer
            .attach_to(layer.wl_surface())
            .map_err(|err| format!("attach buffer: {err}"))?;
        layer.commit();
        Ok(())
    }
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
        if let Err(err) = self.draw(qh) {
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
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for StandaloneAperture {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.configured = true;
        if let Err(err) = self.draw(qh) {
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
                let px = bb.min.x - bounds.min_x + x as i32;
                let py = bb.min.y - bounds.min_y + y as i32;
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

fn map_ipc_mode(mode: halley_ipc::ApertureMode) -> ApertureMode {
    match mode {
        halley_ipc::ApertureMode::Normal => ApertureMode::Normal,
        halley_ipc::ApertureMode::Collapsed => ApertureMode::Collapsed,
        halley_ipc::ApertureMode::Hidden => ApertureMode::Hidden,
    }
}

fn load_aperture_config(path: &Path) -> ApertureConfig {
    match fs::read_to_string(path) {
        Ok(raw) => ApertureConfig::parse_str(raw.as_str()).unwrap_or_default(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ApertureConfig::default(),
        Err(_) => ApertureConfig::default(),
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
