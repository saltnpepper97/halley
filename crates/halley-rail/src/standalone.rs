use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use halley_api::{
    RailItemInfo, RailOutputSnapshot, RailRequest, RailVisibility, Request, Response,
};
use halley_config::{OverlayColorMode, RailConfig, RailPlacement, RailSizingMode, RuntimeTuning};
use image::{RgbaImage, imageops};
use resvg::{tiny_skia, usvg};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rusttype::{Font, PositionedGlyph, Scale, point};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{BTN_LEFT, BTN_RIGHT, PointerEvent, PointerEventKind, PointerHandler},
    },
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
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

const RAIL_NAMESPACE: &str = "halley-rail";
const DEFAULT_SIZE: (u32, u32) = (1, 1);
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(125);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(1);
const TOOLTIP_MAX_W: i32 = 260;
const OVERLAY_GAP: i32 = 10;
const TOOLTIP_PAD_X: i32 = 10;
const TOOLTIP_PAD_Y: i32 = 7;
const MENU_W: i32 = 156;
const MENU_ITEM_H: i32 = 32;
const MENU_PAD: i32 = 6;
const REVEAL_STRIP_THICK: i32 = 40;
const REVEAL_HIT_PAD: i32 = 20;
const REVEAL_HIDE_DELAY: Duration = Duration::from_millis(350);
const PIN_SVG: &[u8] = include_bytes!("../../halley-wl/src/render/assets/pin.svg");

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
    let config_path = default_rail_config_path();
    let config = load_rail_config(config_path.as_path());
    let font_renderer = FontRenderer::new("monospace")?;
    let config_mtime = fs::metadata(config_path.as_path())
        .and_then(|meta| meta.modified())
        .ok();
    let pool = SlotPool::new(4096, &shm).map_err(|err| format!("slot pool: {err}"))?;
    let mut app = StandaloneRail {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        config,
        config_path,
        config_mtime,
        next_config_poll: Instant::now(),
        next_status_poll: Instant::now(),
        layers: Vec::new(),
        snapshots: Vec::new(),
        revealed_outputs: HashSet::new(),
        pending_hide: None,
        hovered_item: None,
        menu: None,
        pointer: None,
        font_renderer,
        icon_cache: IconCache::default(),
        pin_icon: None,
        configured: false,
        exit: false,
    };

    event_queue
        .roundtrip(&mut app)
        .map_err(|err| format!("initial roundtrip 1: {err}"))?;
    event_queue
        .roundtrip(&mut app)
        .map_err(|err| format!("initial roundtrip 2: {err}"))?;

    app.refresh_halley_status();
    app.recreate_layers(&qh)?;

    while !app.exit {
        event_queue
            .dispatch_pending(&mut app)
            .map_err(|err| format!("event dispatch: {err}"))?;
        app.tick(&qh)?;
        wait_for_wayland_or_timeout(&mut event_queue, app.next_wayland_wait_timeout())?;
    }

    Ok(())
}

struct StandaloneRail {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    config: RailConfig,
    config_path: PathBuf,
    config_mtime: Option<SystemTime>,
    next_config_poll: Instant,
    next_status_poll: Instant,
    layers: Vec<LayerInstance>,
    snapshots: Vec<RailOutputSnapshot>,
    revealed_outputs: HashSet<Option<String>>,
    pending_hide: Option<PendingHide>,
    hovered_item: Option<HoverState>,
    menu: Option<MenuState>,
    pointer: Option<wl_pointer::WlPointer>,
    font_renderer: FontRenderer,
    icon_cache: IconCache,
    pin_icon: Option<IconRaster>,
    configured: bool,
    exit: bool,
}

struct LayerInstance {
    surface: LayerSurface,
    output_name: Option<String>,
    configured: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct HoverState {
    output_name: Option<String>,
    node_id: u64,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    FocusReveal,
    TogglePin,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
struct MenuState {
    output_name: Option<String>,
    node_id: u64,
    x: i32,
    y: i32,
    hovered: Option<MenuAction>,
}

#[derive(Clone, Debug, PartialEq)]
struct PendingHide {
    output_name: Option<String>,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ItemRect {
    node_id: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Default)]
struct IconCache {
    icons: HashMap<String, Option<IconRaster>>,
}

#[derive(Clone)]
struct IconRaster {
    width: u32,
    height: u32,
    pixels_rgba: Vec<u8>,
}

impl StandaloneRail {
    fn recreate_layers(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        self.layers.clear();
        self.configured = false;
        let output_names = self.output_names();
        let output_names = if output_names.is_empty() {
            vec![None]
        } else {
            output_names.into_iter().map(Some).collect()
        };

        for output_name in output_names {
            let surface = self.compositor.create_surface(qh);
            let output = self.find_output_by_name(output_name.as_deref());
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface,
                Layer::Top,
                Some(RAIL_NAMESPACE),
                output.as_ref(),
            );
            layer.set_anchor(anchor_for_placement(self.config.placement));
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_exclusive_zone(0);
            layer.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
            set_layer_margin(&layer, &self.config);
            layer.commit();
            self.layers.push(LayerInstance {
                surface: layer,
                output_name,
                configured: false,
            });
        }
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

    fn output_names(&self) -> Vec<String> {
        self.output_state
            .outputs()
            .into_iter()
            .filter_map(|output| self.output_state.info(&output))
            .filter_map(|info| info.name)
            .collect()
    }

    fn refresh_halley_status(&mut self) -> bool {
        self.next_status_poll = Instant::now() + STATUS_POLL_INTERVAL;
        let previous = self.snapshots.clone();
        let request = Request::Rail(RailRequest::Status { output: None });
        self.snapshots = match halley_ipc::send_request(&request) {
            Ok(Response::RailStatus(status)) => status.outputs,
            _ => Vec::new(),
        };
        self.retain_live_interaction_state();
        self.snapshots != previous
    }

    fn retain_live_interaction_state(&mut self) {
        let snapshots = self.snapshots.clone();
        self.revealed_outputs.retain(|output| {
            snapshot_for_output_in(&snapshots, output.as_deref())
                .is_some_and(snapshot_revealable_hidden)
        });
        if self.pending_hide.as_ref().is_some_and(|pending| {
            !snapshot_for_output_in(&snapshots, pending.output_name.as_deref())
                .is_some_and(snapshot_revealable_hidden)
        }) {
            self.pending_hide = None;
        }
        if self.hovered_item.as_ref().is_some_and(|hover| {
            !self.node_live_on_output(hover.output_name.as_deref(), hover.node_id)
        }) {
            self.hovered_item = None;
        }
        if self.menu.as_ref().is_some_and(|menu| {
            !self.node_live_on_output(menu.output_name.as_deref(), menu.node_id)
        }) {
            self.menu = None;
        }
    }

    fn node_live_on_output(&self, output_name: Option<&str>, node_id: u64) -> bool {
        self.snapshot_for_output(output_name)
            .is_some_and(|snapshot| {
                rail_items_renderable(snapshot, self.output_revealed(output_name))
                    && snapshot.items.iter().any(|item| item.node_id == node_id)
            })
    }

    fn output_revealed(&self, output_name: Option<&str>) -> bool {
        self.revealed_outputs
            .contains(&output_name.map(str::to_string))
    }

    fn set_output_revealed(&mut self, output_name: Option<String>, revealed: bool) -> bool {
        if revealed {
            self.revealed_outputs.insert(output_name)
        } else {
            self.revealed_outputs.remove(&output_name)
        }
    }

    fn reveal_output_now(&mut self, output_name: Option<String>) -> bool {
        let changed = self.set_output_revealed(output_name.clone(), true);
        if self
            .pending_hide
            .as_ref()
            .is_some_and(|pending| pending.output_name == output_name)
        {
            self.pending_hide = None;
            return true;
        }
        changed
    }

    fn start_pending_hide(&mut self, output_name: Option<String>) -> bool {
        self.pending_hide = Some(PendingHide {
            output_name,
            deadline: Instant::now() + REVEAL_HIDE_DELAY,
        });
        false
    }

    fn cancel_pending_hide(&mut self, output_name: Option<&str>) -> bool {
        if self
            .pending_hide
            .as_ref()
            .is_some_and(|pending| pending.output_name.as_deref() == output_name)
        {
            self.pending_hide = None;
        }
        false
    }

    fn maybe_reload_config(&mut self, qh: &QueueHandle<Self>) -> bool {
        let now = Instant::now();
        if now < self.next_config_poll {
            return false;
        }
        self.next_config_poll = now + CONFIG_POLL_INTERVAL;
        let next_mtime = fs::metadata(self.config_path.as_path())
            .and_then(|meta| meta.modified())
            .ok();
        if self.config_mtime == next_mtime {
            return false;
        }
        self.config = load_rail_config(self.config_path.as_path());
        self.config_mtime = next_mtime;
        if let Err(err) = self.recreate_layers(qh) {
            eprintln!("halley-rail recreate after config reload failed: {err}");
            self.exit = true;
        }
        true
    }

    fn tick(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        let mut needs_draw = self.maybe_reload_config(qh);
        if Instant::now() >= self.next_status_poll && self.refresh_halley_status() {
            needs_draw = true;
        }
        if self.hide_pending_output() {
            needs_draw = true;
        }
        if needs_draw && self.configured {
            self.draw()?;
        }
        Ok(())
    }

    fn hide_pending_output(&mut self) -> bool {
        let Some(pending) = self.pending_hide.clone() else {
            return false;
        };
        if Instant::now() < pending.deadline {
            return false;
        }
        self.pending_hide = None;
        let output_name = pending.output_name;
        let mut changed = self.set_output_revealed(output_name.clone(), false);
        if self
            .hovered_item
            .as_ref()
            .is_some_and(|hover| hover.output_name == output_name)
        {
            self.hovered_item = None;
            changed = true;
        }
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| menu.output_name == output_name)
        {
            self.menu = None;
            changed = true;
        }
        changed
    }

    fn next_wayland_wait_timeout(&self) -> Option<Duration> {
        if !self.configured || !self.layers.iter().any(|layer| layer.configured) {
            return None;
        }
        let now = Instant::now();
        let next = self
            .pending_hide
            .as_ref()
            .map(|pending| pending.deadline)
            .unwrap_or_else(|| self.next_status_poll.min(self.next_config_poll))
            .min(self.next_status_poll)
            .min(self.next_config_poll);
        Some(next.saturating_duration_since(now))
    }

    fn draw(&mut self) -> Result<(), String> {
        if !self.configured || !self.layers.iter().any(|layer| layer.configured) {
            return Ok(());
        }
        for index in 0..self.layers.len() {
            if !self.layers[index].configured {
                continue;
            }
            let output_name = self.layers[index].output_name.clone();
            let revealed = self.output_revealed(output_name.as_deref());
            let output_size = self.output_size(output_name.as_deref());
            let hover = self
                .hovered_item
                .as_ref()
                .filter(|hover| hover.output_name == output_name)
                .cloned();
            let menu = self
                .menu
                .as_ref()
                .filter(|menu| menu.output_name == output_name)
                .cloned();
            let snapshot = output_name
                .as_deref()
                .and_then(|name| {
                    self.snapshots
                        .iter()
                        .find(|snapshot| snapshot.output == name)
                })
                .or_else(|| self.snapshots.first())
                .cloned();
            let layout = rail_layout(
                &self.config,
                snapshot.as_ref(),
                &hover,
                &menu,
                revealed,
                output_size,
            );
            let stride = layout.width as i32 * 4;
            let needed = layout.height as usize * stride as usize;
            if self.pool.len() < needed {
                self.pool
                    .resize(needed)
                    .map_err(|err| format!("resize shm pool: {err}"))?;
            }
            let (buffer, canvas) = self
                .pool
                .create_buffer(
                    layout.width as i32,
                    layout.height as i32,
                    stride,
                    wl_shm::Format::Argb8888,
                )
                .map_err(|err| format!("create buffer: {err}"))?;
            canvas.fill(0);
            if let Some(snapshot) = snapshot.as_ref() {
                draw_rail(
                    canvas,
                    layout.width,
                    layout.height,
                    &layout,
                    &self.config,
                    snapshot,
                    &mut self.icon_cache,
                    &mut self.pin_icon,
                    &self.font_renderer,
                    hover.as_ref(),
                    menu.as_ref(),
                    revealed,
                );
            }
            let layer = &self.layers[index];
            layer.surface.set_size(layout.width, layout.height);
            set_layer_margin_for_layout(&layer.surface, &self.config, &layout);
            self.update_input_region(layer, &layout, snapshot.as_ref())?;
            layer.surface.wl_surface().damage_buffer(
                0,
                0,
                layout.width as i32,
                layout.height as i32,
            );
            buffer
                .attach_to(layer.surface.wl_surface())
                .map_err(|err| format!("attach buffer: {err}"))?;
            layer.surface.commit();
        }
        Ok(())
    }

    fn update_input_region(
        &self,
        layer: &LayerInstance,
        layout: &RailLayout,
        snapshot: Option<&RailOutputSnapshot>,
    ) -> Result<(), String> {
        let region =
            Region::new(&self.compositor).map_err(|err| format!("create input region: {err}"))?;
        if layout.input_full_surface {
            region.add(0, 0, layout.width as i32, layout.height as i32);
        } else if snapshot.is_some_and(|snapshot| {
            snapshot.visibility == RailVisibility::Visible || snapshot_revealable_hidden(snapshot)
        }) {
            region.add(
                layout.rail_x,
                layout.rail_y,
                layout.rail_width as i32,
                layout.rail_height as i32,
            );
        }
        if let Some(menu) = self.menu_for_layer(layer)
            && let Some(snapshot) = snapshot
            && let Some(item) = snapshot
                .items
                .iter()
                .find(|item| item.node_id == menu.node_id)
        {
            let (x, y, w, h) =
                menu_bounds(layout, &self.config, menu, menu_actions(item.pinned).len());
            region.add(x, y, w, h);
        }
        layer
            .surface
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
        Ok(())
    }

    fn menu_for_layer(&self, layer: &LayerInstance) -> Option<&MenuState> {
        self.menu
            .as_ref()
            .filter(|menu| menu.output_name == layer.output_name)
    }

    fn layer_index_for_surface(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.layers
            .iter()
            .position(|layer| layer.surface.wl_surface() == surface)
    }

    fn snapshot_for_output(&self, output_name: Option<&str>) -> Option<&RailOutputSnapshot> {
        output_name
            .and_then(|name| {
                self.snapshots
                    .iter()
                    .find(|snapshot| snapshot.output == name)
            })
            .or_else(|| self.snapshots.first())
    }

    fn output_size(&self, output_name: Option<&str>) -> Option<(i32, i32)> {
        self.find_output_by_name(output_name).and_then(|output| {
            self.output_state
                .info(&output)
                .and_then(|info| info.logical_size)
        })
    }

    fn update_hover_for_position(&mut self, layer_index: usize, x: i32, y: i32) -> bool {
        let output_name = self.layers[layer_index].output_name.clone();
        let snapshot = self.snapshot_for_output(output_name.as_deref());
        let revealed = self.output_revealed(output_name.as_deref());
        let layout = rail_layout(
            &self.config,
            snapshot,
            &self.hovered_item,
            &self.menu,
            revealed,
            self.output_size(output_name.as_deref()),
        );
        let next = hit_item(&layout, x, y).map(|node_id| HoverState {
            output_name,
            node_id,
            x,
            y,
        });
        if self.hovered_item != next {
            self.hovered_item = next;
            return true;
        }
        false
    }

    fn handle_press(&mut self, layer_index: usize, button: u32, x: i32, y: i32) -> bool {
        let output_name = self.layers[layer_index].output_name.clone();
        let snapshot = self.snapshot_for_output(output_name.as_deref());
        let revealed = self.output_revealed(output_name.as_deref());
        let layout = rail_layout(
            &self.config,
            snapshot,
            &self.hovered_item,
            &self.menu,
            revealed,
            self.output_size(output_name.as_deref()),
        );
        if button == BTN_LEFT {
            if let Some(action) = self.menu_hit(&layout, snapshot, x, y) {
                if let Some(menu) = self.menu.take() {
                    send_menu_action(menu.node_id, action);
                }
                self.refresh_halley_status();
                return true;
            }
            if self.menu.take().is_some() {
                return true;
            }
            if let Some(node_id) = hit_item(&layout, x, y) {
                send_rail_request(RailRequest::FocusReveal { node_id });
                return true;
            }
        } else if button == BTN_RIGHT {
            if let Some(node_id) = hit_item(&layout, x, y) {
                self.hovered_item = None;
                self.menu = Some(MenuState {
                    output_name,
                    node_id,
                    x,
                    y,
                    hovered: None,
                });
                return true;
            }
            if self.menu.take().is_some() {
                return true;
            }
        }
        false
    }

    fn menu_hit(
        &self,
        layout: &RailLayout,
        snapshot: Option<&RailOutputSnapshot>,
        x: i32,
        y: i32,
    ) -> Option<MenuAction> {
        let menu = self.menu.as_ref()?;
        let snapshot = snapshot?;
        let item = snapshot
            .items
            .iter()
            .find(|item| item.node_id == menu.node_id)?;
        let actions = menu_actions(item.pinned);
        let (mx, my, mw, mh) = menu_bounds(layout, &self.config, menu, actions.len());
        if x < mx || x > mx + mw || y < my || y > my + mh {
            return None;
        }
        let rel_y = y - my - MENU_PAD;
        if rel_y < 0 {
            return None;
        }
        actions
            .get((rel_y / MENU_ITEM_H) as usize)
            .map(|(action, _)| *action)
    }

    fn update_menu_hover(&mut self, layer_index: usize, x: i32, y: i32) -> bool {
        let output_name = self.layers[layer_index].output_name.clone();
        let snapshot = self.snapshot_for_output(output_name.as_deref());
        let revealed = self.output_revealed(output_name.as_deref());
        let layout = rail_layout(
            &self.config,
            snapshot,
            &self.hovered_item,
            &self.menu,
            revealed,
            self.output_size(output_name.as_deref()),
        );
        let next = self.menu_hit(&layout, snapshot, x, y);
        if let Some(menu) = self.menu.as_mut()
            && menu.hovered != next
        {
            menu.hovered = next;
            return true;
        }
        false
    }
}

fn hit_item(layout: &RailLayout, x: i32, y: i32) -> Option<u64> {
    layout
        .item_rects
        .iter()
        .find(|rect| x >= rect.x && x <= rect.x + rect.w && y >= rect.y && y <= rect.y + rect.h)
        .map(|rect| rect.node_id)
}

fn menu_bounds(
    layout: &RailLayout,
    config: &RailConfig,
    menu: &MenuState,
    action_count: usize,
) -> (i32, i32, i32, i32) {
    let h = action_count as i32 * MENU_ITEM_H + MENU_PAD * 2;
    match config.placement {
        RailPlacement::Left => (
            layout.rail_x + layout.rail_width as i32 + OVERLAY_GAP,
            safe_clamp(menu.y, 4, layout.height as i32 - h - 4),
            MENU_W,
            h,
        ),
        RailPlacement::Right => (
            layout.rail_x - OVERLAY_GAP - MENU_W,
            safe_clamp(menu.y, 4, layout.height as i32 - h - 4),
            MENU_W,
            h,
        ),
        RailPlacement::Up => (
            safe_clamp(menu.x, 4, layout.width as i32 - MENU_W - 4),
            layout.rail_y + layout.rail_height as i32 + OVERLAY_GAP,
            MENU_W,
            h,
        ),
        RailPlacement::Down => (
            safe_clamp(menu.x, 4, layout.width as i32 - MENU_W - 4),
            layout.rail_y - OVERLAY_GAP - h,
            MENU_W,
            h,
        ),
    }
}

fn send_menu_action(node_id: u64, action: MenuAction) {
    match action {
        MenuAction::FocusReveal => send_rail_request(RailRequest::FocusReveal { node_id }),
        MenuAction::TogglePin => send_rail_request(RailRequest::TogglePin { node_id }),
        MenuAction::Close => send_rail_request(RailRequest::Close { node_id }),
    }
}

fn send_rail_request(request: RailRequest) {
    let _ = halley_ipc::send_request(&Request::Rail(request));
}

#[derive(Clone)]
struct RailLayout {
    width: u32,
    height: u32,
    rail_x: i32,
    rail_y: i32,
    rail_width: u32,
    rail_height: u32,
    edge_aligned: bool,
    input_full_surface: bool,
    item_rects: Vec<ItemRect>,
}

fn rail_layout(
    config: &RailConfig,
    snapshot: Option<&RailOutputSnapshot>,
    _hover: &Option<HoverState>,
    _menu: &Option<MenuState>,
    revealed: bool,
    output_size: Option<(i32, i32)>,
) -> RailLayout {
    let trigger_only =
        config.enabled && !revealed && snapshot.is_some_and(snapshot_revealable_hidden);
    let visible_items = snapshot
        .filter(|snapshot| rail_items_renderable(snapshot, revealed) || trigger_only)
        .map(|snapshot| snapshot.items.len())
        .unwrap_or(0);
    if !config.enabled || visible_items == 0 {
        return RailLayout {
            width: 1,
            height: 1,
            rail_x: 0,
            rail_y: 0,
            rail_width: 1,
            rail_height: 1,
            edge_aligned: false,
            input_full_surface: false,
            item_rects: Vec::new(),
        };
    }
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let sep = if config.pinned_separator {
        let pinned = snapshot
            .map(|snapshot| snapshot.items.iter().filter(|item| item.pinned).count())
            .unwrap_or(0);
        if pinned > 0 && pinned < visible_items {
            1 + config.gap * 2
        } else {
            0
        }
    } else {
        0
    };
    let items = visible_items as i32;
    let content =
        config.padding * 2 + items * config.icon_size + (items - 1).max(0) * config.gap + sep;
    let cross = config.padding * 2 + config.icon_size;
    let (width, height) = match (vertical, config.sizing) {
        (true, RailSizingMode::GrowToContent) => (positive_or(config.width, cross), content),
        (false, RailSizingMode::GrowToContent) => (content, positive_or(config.height, cross)),
        (true, RailSizingMode::Fixed) => (
            positive_or(config.width, cross),
            positive_or(config.height, content),
        ),
        (false, RailSizingMode::Fixed) => (
            positive_or(config.width, content),
            positive_or(config.height, cross),
        ),
    };
    let rail_width = width.max(1) as u32;
    let rail_height = height.max(1) as u32;
    let overlay_w = if vertical {
        TOOLTIP_MAX_W.max(MENU_W) + OVERLAY_GAP * 2
    } else {
        0
    };
    let overlay_h = if !vertical { 140 } else { 0 };
    let rail_x = if matches!(config.placement, RailPlacement::Right) {
        overlay_w
    } else {
        0
    };
    let rail_y = if matches!(config.placement, RailPlacement::Down) {
        overlay_h
    } else {
        0
    };
    if trigger_only {
        return reveal_trigger_layout(config, rail_width, rail_height, output_size);
    }
    if revealed && snapshot.is_some_and(snapshot_revealable_hidden) {
        return revealed_sidebar_layout(
            config,
            snapshot,
            rail_width,
            rail_height,
            overlay_w,
            overlay_h,
            output_size,
        );
    }
    let min_menu_w = MENU_W + 8;
    let min_menu_h = menu_actions(false).len() as i32 * MENU_ITEM_H + MENU_PAD * 2 + 8;
    let canvas_width = if vertical {
        rail_width as i32 + overlay_w.max(0)
    } else {
        (rail_width as i32).max(min_menu_w)
    };
    let canvas_height = if vertical {
        (rail_height as i32).max(min_menu_h)
    } else {
        rail_height as i32 + overlay_h.max(0)
    };
    RailLayout {
        width: canvas_width.max(1) as u32,
        height: canvas_height.max(1) as u32,
        rail_x,
        rail_y,
        rail_width,
        rail_height,
        edge_aligned: false,
        input_full_surface: false,
        item_rects: item_rects(config, snapshot, rail_x, rail_y, rail_width, rail_height),
    }
}

fn item_rects(
    config: &RailConfig,
    snapshot: Option<&RailOutputSnapshot>,
    rail_x: i32,
    rail_y: i32,
    width: u32,
    height: u32,
) -> Vec<ItemRect> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if !rail_items_renderable(snapshot, true) {
        return Vec::new();
    }
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let pinned_count = snapshot.items.iter().take_while(|item| item.pinned).count();
    let mut cursor = config.padding;
    let mut rects = Vec::with_capacity(snapshot.items.len());
    for (index, item) in snapshot.items.iter().enumerate() {
        if index == pinned_count
            && pinned_count > 0
            && pinned_count < snapshot.items.len()
            && config.pinned_separator
        {
            cursor += config.gap + 1 + config.gap;
        }
        let (x, y) = if vertical {
            (
                rail_x + (width as i32 - config.icon_size) / 2,
                rail_y + cursor,
            )
        } else {
            (
                rail_x + cursor,
                rail_y + (height as i32 - config.icon_size) / 2,
            )
        };
        rects.push(ItemRect {
            node_id: item.node_id,
            x,
            y,
            w: config.icon_size.max(1),
            h: config.icon_size.max(1),
        });
        cursor += config.icon_size + config.gap;
    }
    rects
}

fn positive_or(value: i32, default: i32) -> i32 {
    if value > 0 { value } else { default.max(1) }
}

fn safe_clamp(value: i32, min: i32, max: i32) -> i32 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

fn snapshot_for_output_in<'a>(
    snapshots: &'a [RailOutputSnapshot],
    output_name: Option<&str>,
) -> Option<&'a RailOutputSnapshot> {
    output_name
        .and_then(|name| snapshots.iter().find(|snapshot| snapshot.output == name))
        .or_else(|| snapshots.first())
}

fn snapshot_revealable_hidden(snapshot: &RailOutputSnapshot) -> bool {
    matches!(
        snapshot.visibility,
        RailVisibility::HiddenFullscreen
            | RailVisibility::HiddenMaximized
            | RailVisibility::HiddenObstructed
    ) && !snapshot.items.is_empty()
}

fn rail_items_renderable(snapshot: &RailOutputSnapshot, revealed: bool) -> bool {
    snapshot.visibility == RailVisibility::Visible
        || (revealed && snapshot_revealable_hidden(snapshot))
}

fn reveal_trigger_layout(
    config: &RailConfig,
    normal_width: u32,
    normal_height: u32,
    output_size: Option<(i32, i32)>,
) -> RailLayout {
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let width = if vertical {
        REVEAL_STRIP_THICK
    } else {
        let output_w = output_size
            .map(|(width, _)| width)
            .unwrap_or(normal_width as i32);
        (normal_width as i32 + REVEAL_HIT_PAD * 2).min(output_w.max(1))
    };
    let height = if vertical {
        let output_h = output_size
            .map(|(_, height)| height)
            .unwrap_or(normal_height as i32);
        (normal_height as i32 + REVEAL_HIT_PAD * 2).min(output_h.max(1))
    } else {
        REVEAL_STRIP_THICK
    };
    RailLayout {
        width: width.max(1) as u32,
        height: height.max(1) as u32,
        rail_x: 0,
        rail_y: 0,
        rail_width: width.max(1) as u32,
        rail_height: height.max(1) as u32,
        edge_aligned: true,
        input_full_surface: true,
        item_rects: Vec::new(),
    }
}

fn revealed_sidebar_layout(
    config: &RailConfig,
    snapshot: Option<&RailOutputSnapshot>,
    rail_width: u32,
    rail_height: u32,
    overlay_w: i32,
    overlay_h: i32,
    output_size: Option<(i32, i32)>,
) -> RailLayout {
    let (output_w, output_h) = output_size.unwrap_or((rail_width as i32, rail_height as i32));
    let offset_x = config.offset_x.max(0);
    let offset_y = config.offset_y.max(0);
    let normal_rail_x = if matches!(config.placement, RailPlacement::Right) {
        overlay_w.max(0)
    } else {
        0
    };
    let normal_rail_y = if matches!(config.placement, RailPlacement::Down) {
        overlay_h.max(0)
    } else {
        0
    };
    let (normal_canvas_w, normal_canvas_h) =
        normal_canvas_size(config, rail_width, rail_height, overlay_w, overlay_h);
    let (width, height, rail_x, rail_y) = match config.placement {
        RailPlacement::Left => {
            let width = offset_x + rail_width as i32 + overlay_w.max(0);
            let height = (normal_canvas_h + REVEAL_HIT_PAD * 2).min(output_h.max(1));
            let rail_y = REVEAL_HIT_PAD + normal_rail_y;
            (width, height, offset_x, rail_y)
        }
        RailPlacement::Right => {
            let width = overlay_w.max(0) + rail_width as i32 + offset_x;
            let rail_x = width - offset_x - normal_canvas_w + normal_rail_x;
            let height = (normal_canvas_h + REVEAL_HIT_PAD * 2).min(output_h.max(1));
            let rail_y = REVEAL_HIT_PAD + normal_rail_y;
            (width, height, rail_x, rail_y)
        }
        RailPlacement::Up => {
            let height = offset_y + rail_height as i32 + overlay_h.max(0);
            let width = (normal_canvas_w + REVEAL_HIT_PAD * 2).min(output_w.max(1));
            let rail_x = REVEAL_HIT_PAD + normal_rail_x;
            (width, height, rail_x, offset_y)
        }
        RailPlacement::Down => {
            let height = overlay_h.max(0) + rail_height as i32 + offset_y;
            let width = (normal_canvas_w + REVEAL_HIT_PAD * 2).min(output_w.max(1));
            let rail_x = REVEAL_HIT_PAD + normal_rail_x;
            let rail_y = height - offset_y - normal_canvas_h + normal_rail_y;
            (width, height, rail_x, rail_y)
        }
    };
    RailLayout {
        width: width.max(1) as u32,
        height: height.max(1) as u32,
        rail_x,
        rail_y,
        rail_width,
        rail_height,
        edge_aligned: true,
        input_full_surface: true,
        item_rects: item_rects(config, snapshot, rail_x, rail_y, rail_width, rail_height),
    }
}

fn normal_canvas_size(
    config: &RailConfig,
    rail_width: u32,
    rail_height: u32,
    overlay_w: i32,
    overlay_h: i32,
) -> (i32, i32) {
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let min_menu_w = MENU_W + 8;
    let min_menu_h = menu_actions(false).len() as i32 * MENU_ITEM_H + MENU_PAD * 2 + 8;
    let width = if vertical {
        rail_width as i32 + overlay_w.max(0)
    } else {
        (rail_width as i32).max(min_menu_w)
    };
    let height = if vertical {
        (rail_height as i32).max(min_menu_h)
    } else {
        rail_height as i32 + overlay_h.max(0)
    };
    (width.max(1), height.max(1))
}

#[derive(Clone, Copy)]
struct Rgb {
    r: f32,
    g: f32,
    b: f32,
}

fn draw_rail(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    layout: &RailLayout,
    config: &RailConfig,
    snapshot: &RailOutputSnapshot,
    icon_cache: &mut IconCache,
    pin_icon: &mut Option<IconRaster>,
    font_renderer: &FontRenderer,
    hover: Option<&HoverState>,
    menu: Option<&MenuState>,
    revealed: bool,
) {
    if !rail_items_renderable(snapshot, revealed) || snapshot.items.is_empty() {
        return;
    }
    let background = resolve_fill(config.background_color);
    let foreground = resolve_text(config.foreground_color, background);
    let divider = resolve_text(config.divider_color, background);
    fill_rounded_rect_at(
        canvas,
        width,
        height,
        layout.rail_x,
        layout.rail_y,
        layout.rail_width as i32,
        layout.rail_height as i32,
        config.radius.max(0) as u32,
        background,
        0.88,
    );
    let vertical = matches!(config.placement, RailPlacement::Left | RailPlacement::Right);
    let pinned_count = snapshot.items.iter().take_while(|item| item.pinned).count();
    let mut cursor = config.padding;
    for (index, item) in snapshot.items.iter().enumerate() {
        if index == pinned_count
            && pinned_count > 0
            && pinned_count < snapshot.items.len()
            && config.pinned_separator
        {
            cursor += config.gap;
            if vertical {
                fill_rect(
                    canvas,
                    width,
                    height,
                    layout.rail_x + config.padding,
                    layout.rail_y + cursor,
                    (layout.rail_width as i32 - config.padding * 2).max(1),
                    1,
                    divider,
                    0.75,
                );
            } else {
                fill_rect(
                    canvas,
                    width,
                    height,
                    layout.rail_x + cursor,
                    layout.rail_y + config.padding,
                    1,
                    (layout.rail_height as i32 - config.padding * 2).max(1),
                    divider,
                    0.75,
                );
            }
            cursor += 1 + config.gap;
        }
        let Some(rect) = layout
            .item_rects
            .iter()
            .find(|rect| rect.node_id == item.node_id)
        else {
            cursor += config.icon_size + config.gap;
            continue;
        };
        if item.focused {
            draw_focus_indicator(canvas, width, height, *rect, vertical, divider);
        }
        draw_item_icon(
            canvas,
            width,
            height,
            *rect,
            item,
            icon_cache,
            font_renderer,
            foreground,
        );
        if item.pinned {
            draw_pin_badge(canvas, width, height, *rect, background, divider, pin_icon);
        }
        cursor += config.icon_size + config.gap;
    }
    if let Some(hover) = hover {
        draw_tooltip(
            canvas,
            width,
            height,
            layout,
            config,
            snapshot,
            hover,
            font_renderer,
            background,
            foreground,
        );
    }
    if let Some(menu) = menu {
        draw_menu(
            canvas,
            width,
            height,
            layout,
            config,
            snapshot,
            menu,
            font_renderer,
            background,
            foreground,
            divider,
        );
    }
}

fn draw_focus_indicator(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    rect: ItemRect,
    vertical: bool,
    color: Rgb,
) {
    if vertical {
        fill_rect(
            canvas,
            canvas_w,
            canvas_h,
            rect.x - 7,
            rect.y + 5,
            3,
            rect.h - 10,
            color,
            0.95,
        );
    } else {
        fill_rect(
            canvas,
            canvas_w,
            canvas_h,
            rect.x + 5,
            rect.y + rect.h + 4,
            rect.w - 10,
            3,
            color,
            0.95,
        );
    }
}

fn draw_item_icon(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    rect: ItemRect,
    item: &RailItemInfo,
    icon_cache: &mut IconCache,
    font_renderer: &FontRenderer,
    fallback_color: Rgb,
) {
    let icon = item
        .app_id
        .as_deref()
        .and_then(|app_id| icon_cache.icon_for(app_id, rect.w as u32));
    if let Some(icon) = icon {
        draw_raster(
            canvas, canvas_w, canvas_h, rect.x, rect.y, rect.w, rect.h, &icon, 1.0,
        );
        return;
    }
    let glyph = fallback_glyph(item);
    let font_px = (rect.h as f32 * 0.68).round().max(10.0) as u32;
    let (tw, th) = font_renderer.measure_px(glyph.as_str(), font_px);
    font_renderer.draw(
        canvas,
        canvas_w,
        canvas_h,
        rect.x + (rect.w - tw as i32) / 2,
        rect.y + (rect.h - th as i32) / 2,
        glyph.as_str(),
        font_px,
        1.0,
        fallback_color,
    );
}

fn fallback_glyph(item: &RailItemInfo) -> String {
    item.app_id
        .as_deref()
        .unwrap_or(item.title.as_str())
        .chars()
        .find(|ch| ch.is_ascii_alphanumeric())
        .unwrap_or('?')
        .to_ascii_uppercase()
        .to_string()
}

fn draw_pin_badge(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    rect: ItemRect,
    background: Rgb,
    glyph: Rgb,
    pin_icon: &mut Option<IconRaster>,
) {
    if pin_icon.is_none() {
        *pin_icon = load_pin_icon(glyph, 64);
    }
    let radius = (rect.w as f32 * 0.22).round().max(5.0) as i32;
    let cx = rect.x + rect.w - radius;
    let cy = rect.y + radius;
    fill_circle(canvas, canvas_w, canvas_h, cx, cy, radius, background, 0.94);
    if let Some(icon) = pin_icon.as_ref() {
        let side = (radius as f32 * 1.24).round().max(1.0) as i32;
        draw_raster(
            canvas,
            canvas_w,
            canvas_h,
            cx - side / 2,
            cy - side / 2,
            side,
            side,
            icon,
            1.0,
        );
    }
}

fn draw_tooltip(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    layout: &RailLayout,
    config: &RailConfig,
    snapshot: &RailOutputSnapshot,
    hover: &HoverState,
    font_renderer: &FontRenderer,
    background: Rgb,
    foreground: Rgb,
) {
    let Some(item) = snapshot
        .items
        .iter()
        .find(|item| item.node_id == hover.node_id)
    else {
        return;
    };
    let font_px = 14;
    let text = ellipsize_to_width(
        font_renderer,
        item.title.as_str(),
        font_px,
        TOOLTIP_MAX_W - TOOLTIP_PAD_X * 2,
    );
    let (tw, th) = font_renderer.measure_px(text.as_str(), font_px);
    let w = (tw as i32 + TOOLTIP_PAD_X * 2).min(TOOLTIP_MAX_W).max(1);
    let h = th as i32 + TOOLTIP_PAD_Y * 2;
    let (x, y) = match config.placement {
        RailPlacement::Left => (
            layout.rail_x + layout.rail_width as i32 + OVERLAY_GAP,
            safe_clamp(hover.y - h / 2, 4, canvas_h as i32 - h - 4),
        ),
        RailPlacement::Right => (
            layout.rail_x - OVERLAY_GAP - w,
            safe_clamp(hover.y - h / 2, 4, canvas_h as i32 - h - 4),
        ),
        RailPlacement::Up => (
            safe_clamp(hover.x - w / 2, 4, canvas_w as i32 - w - 4),
            layout.rail_y + layout.rail_height as i32 + OVERLAY_GAP,
        ),
        RailPlacement::Down => (
            safe_clamp(hover.x - w / 2, 4, canvas_w as i32 - w - 4),
            layout.rail_y - OVERLAY_GAP - h,
        ),
    };
    fill_rounded_rect_at(canvas, canvas_w, canvas_h, x, y, w, h, 10, background, 0.96);
    font_renderer.draw(
        canvas,
        canvas_w,
        canvas_h,
        x + TOOLTIP_PAD_X,
        y + TOOLTIP_PAD_Y,
        text.as_str(),
        font_px,
        1.0,
        foreground,
    );
}

fn draw_menu(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    layout: &RailLayout,
    config: &RailConfig,
    snapshot: &RailOutputSnapshot,
    menu: &MenuState,
    font_renderer: &FontRenderer,
    background: Rgb,
    foreground: Rgb,
    accent: Rgb,
) {
    let Some(item) = snapshot
        .items
        .iter()
        .find(|item| item.node_id == menu.node_id)
    else {
        return;
    };
    let actions = menu_actions(item.pinned);
    let (x, y, _, h) = menu_bounds(layout, config, menu, actions.len());
    fill_rounded_rect_at(
        canvas, canvas_w, canvas_h, x, y, MENU_W, h, 12, background, 0.98,
    );
    for (index, (action, label)) in actions.iter().enumerate() {
        let item_y = y + MENU_PAD + index as i32 * MENU_ITEM_H;
        if menu.hovered == Some(*action) {
            fill_rounded_rect_at(
                canvas,
                canvas_w,
                canvas_h,
                x + MENU_PAD,
                item_y,
                MENU_W - MENU_PAD * 2,
                MENU_ITEM_H,
                8,
                accent,
                0.22,
            );
        }
        font_renderer.draw(
            canvas,
            canvas_w,
            canvas_h,
            x + 12,
            item_y + 9,
            label,
            13,
            1.0,
            foreground,
        );
    }
}

fn menu_actions(pinned: bool) -> [(MenuAction, &'static str); 3] {
    [
        (MenuAction::FocusReveal, "Focus / Reveal"),
        (MenuAction::TogglePin, if pinned { "Unpin" } else { "Pin" }),
        (MenuAction::Close, "Close"),
    ]
}

impl IconCache {
    fn icon_for(&mut self, app_id: &str, target_px: u32) -> Option<IconRaster> {
        let key = format!("{}@{}", app_id.trim(), target_px.max(1));
        if !self.icons.contains_key(&key) {
            let icon = resolve_app_icon_path(app_id)
                .and_then(|path| load_icon_raster(path.as_path(), target_px.max(1)));
            self.icons.insert(key.clone(), icon);
        }
        self.icons.get(&key).cloned().flatten()
    }
}

fn resolve_app_icon_path(app_id: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    push_unique(&mut candidates, app_id.to_string());
    if let Some(tail) = app_id.rsplit(['.', '/']).next()
        && !tail.is_empty()
    {
        push_unique(&mut candidates, tail.to_string());
    }
    if let Some(icon_name) = desktop_entry_icon_name(app_id) {
        push_unique(&mut candidates, icon_name);
    }
    for candidate in candidates {
        if let Some(path) = find_best_icon_path(&candidate) {
            return Some(path);
        }
    }
    None
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn desktop_entry_icon_name(app_id: &str) -> Option<String> {
    for dir in desktop_entry_dirs() {
        let exact = dir.join(format!("{app_id}.desktop"));
        if exact.is_file()
            && let Some(icon_name) = parse_desktop_icon_name(&exact)
        {
            return Some(icon_name);
        }
    }
    let mut best_match = None;
    let app_id_lower = app_id.to_ascii_lowercase();
    for dir in desktop_entry_dirs() {
        walk_files(&dir, 2, &mut |path| {
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop")
                || best_match.is_some()
            {
                return;
            }
            let Some(entry) = parse_desktop_entry(path) else {
                return;
            };
            let stem_matches = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(app_id));
            let wm_class_matches = entry
                .startup_wm_class
                .as_deref()
                .is_some_and(|wm_class| wm_class.eq_ignore_ascii_case(&app_id_lower));
            if (stem_matches || wm_class_matches) && entry.icon.is_some() {
                best_match = entry.icon;
            }
        });
        if best_match.is_some() {
            break;
        }
    }
    best_match
}

#[derive(Default)]
struct DesktopEntry {
    icon: Option<String>,
    startup_wm_class: Option<String>,
}

fn parse_desktop_icon_name(path: &Path) -> Option<String> {
    parse_desktop_entry(path).and_then(|entry| entry.icon)
}

fn parse_desktop_entry(path: &Path) -> Option<DesktopEntry> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_desktop_entry = false;
    let mut entry = DesktopEntry::default();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_desktop_entry = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        if let Some(value) = line.strip_prefix("Icon=") {
            entry.icon = Some(unescape_desktop_value(value));
        } else if let Some(value) = line.strip_prefix("StartupWMClass=") {
            entry.startup_wm_class = Some(unescape_desktop_value(value));
        }
    }
    Some(entry)
}

fn unescape_desktop_value(value: &str) -> String {
    value
        .replace("\\s", " ")
        .replace("\\n", " ")
        .replace("\\t", " ")
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | ' '))
        .to_string()
}

fn find_best_icon_path(icon_name: &str) -> Option<PathBuf> {
    let direct_path = PathBuf::from(icon_name);
    if direct_path.is_file() {
        return Some(direct_path);
    }
    let mut best: Option<(i32, PathBuf)> = None;
    for root in icon_search_roots() {
        walk_files(&root, 8, &mut |path| {
            let Some(score) = icon_candidate_score(path, icon_name) else {
                return;
            };
            let replace = best.as_ref().is_none_or(|(best_score, best_path)| {
                score < *best_score || (score == *best_score && path < best_path.as_path())
            });
            if replace {
                best = Some((score, path.to_path_buf()));
            }
        });
    }
    best.map(|(_, path)| path)
}

fn icon_candidate_score(path: &Path, icon_name: &str) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    if stem != icon_name {
        return None;
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let format_score = match ext.as_str() {
        "svg" => 0,
        "png" => 40,
        "jpg" | "jpeg" => 60,
        _ => return None,
    };
    let size_score = icon_size_hint(path)
        .map(|size| (size as i32 - 64).abs())
        .unwrap_or(24);
    let theme_score = if path.to_string_lossy().contains("/hicolor/") {
        12
    } else {
        0
    };
    Some(format_score + size_score + theme_score)
}

fn icon_size_hint(path: &Path) -> Option<u32> {
    for component in path.components() {
        let part = component.as_os_str().to_str()?;
        if part.eq_ignore_ascii_case("scalable") {
            return Some(64);
        }
        if let Some((w, h)) = part.split_once('x')
            && let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>())
        {
            return Some(w.min(h));
        }
    }
    None
}

fn desktop_entry_dirs() -> Vec<PathBuf> {
    data_roots()
        .into_iter()
        .map(|root| root.join("applications"))
        .filter(|path| path.is_dir())
        .collect()
}

fn icon_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in data_roots() {
        let icons = root.join("icons");
        if icons.is_dir() {
            roots.push(icons);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        let legacy = home.join(".icons");
        if legacy.is_dir() {
            roots.push(legacy);
        }
    }
    let pixmaps = PathBuf::from("/usr/share/pixmaps");
    if pixmaps.is_dir() {
        roots.push(pixmaps);
    }
    roots
}

fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(home));
    } else if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if !dir.trim().is_empty() {
            roots.push(PathBuf::from(dir));
        }
    }
    roots
}

fn walk_files(root: &Path, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
    fn recurse(path: &Path, depth: usize, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < max_depth {
                    recurse(&path, depth + 1, max_depth, visit);
                }
            } else {
                visit(&path);
            }
        }
    }
    if root.is_dir() {
        recurse(root, 0, max_depth, visit);
    }
}

fn load_icon_raster(path: &Path, target_px: u32) -> Option<IconRaster> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "svg" => load_svg_icon(path, target_px),
        "png" | "jpg" | "jpeg" => load_raster_icon(path, target_px),
        _ => None,
    }
}

fn load_raster_icon(path: &Path, target_px: u32) -> Option<IconRaster> {
    let image = image::open(path).ok()?.to_rgba8();
    let normalized = normalize_icon_canvas(image, target_px);
    Some(IconRaster {
        width: normalized.width(),
        height: normalized.height(),
        pixels_rgba: normalized.into_vec(),
    })
}

fn normalize_icon_canvas(source: RgbaImage, target_px: u32) -> RgbaImage {
    let (src_w, src_h) = source.dimensions();
    if src_w == 0 || src_h == 0 {
        return RgbaImage::new(target_px, target_px);
    }
    let resized = imageops::thumbnail(&source, target_px, target_px);
    let mut canvas = RgbaImage::new(target_px, target_px);
    let dx = ((target_px - resized.width()) / 2) as i64;
    let dy = ((target_px - resized.height()) / 2) as i64;
    imageops::overlay(&mut canvas, &resized, dx, dy);
    canvas
}

fn load_svg_icon(path: &Path, target_px: u32) -> Option<IconRaster> {
    let mut options = usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    options.fontdb_mut().load_system_fonts();
    let data = fs::read(path).ok()?;
    let tree = usvg::Tree::from_data(&data, &options).ok()?;
    rasterize_svg_tree(&tree, target_px)
}

fn load_pin_icon(color: Rgb, target_px: u32) -> Option<IconRaster> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(PIN_SVG, &options).ok()?;
    let mut raster = rasterize_svg_tree(&tree, target_px)?;
    tint_alpha_mask(&mut raster, color);
    Some(raster)
}

fn rasterize_svg_tree(tree: &usvg::Tree, target_px: u32) -> Option<IconRaster> {
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(target_px, target_px)?;
    let scale_x = target_px as f32 / svg_size.width() as f32;
    let scale_y = target_px as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (target_px as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (target_px as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.data().to_vec();
    unpremultiply_rgba(&mut pixels);
    Some(IconRaster {
        width: target_px,
        height: target_px,
        pixels_rgba: pixels,
    })
}

fn unpremultiply_rgba(pixels: &mut [u8]) {
    for chunk in pixels.chunks_exact_mut(4) {
        let alpha = chunk[3] as u32;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        chunk[0] = ((chunk[0] as u32 * 255) / alpha).min(255) as u8;
        chunk[1] = ((chunk[1] as u32 * 255) / alpha).min(255) as u8;
        chunk[2] = ((chunk[2] as u32 * 255) / alpha).min(255) as u8;
    }
}

fn tint_alpha_mask(raster: &mut IconRaster, color: Rgb) {
    for chunk in raster.pixels_rgba.chunks_exact_mut(4) {
        chunk[0] = (color.r.clamp(0.0, 1.0) * 255.0).round() as u8;
        chunk[1] = (color.g.clamp(0.0, 1.0) * 255.0).round() as u8;
        chunk[2] = (color.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

fn resolve_fill(mode: OverlayColorMode) -> Rgb {
    match mode {
        OverlayColorMode::Auto | OverlayColorMode::Dark => Rgb {
            r: 0.15,
            g: 0.18,
            b: 0.22,
        },
        OverlayColorMode::Light => Rgb {
            r: 0.92,
            g: 0.95,
            b: 0.98,
        },
        OverlayColorMode::Fixed { r, g, b } => Rgb { r, g, b },
    }
}

fn resolve_text(mode: OverlayColorMode, background: Rgb) -> Rgb {
    match mode {
        OverlayColorMode::Auto => {
            if luminance(background) < 0.45 {
                Rgb {
                    r: 0.94,
                    g: 0.96,
                    b: 0.98,
                }
            } else {
                Rgb {
                    r: 0.08,
                    g: 0.10,
                    b: 0.12,
                }
            }
        }
        OverlayColorMode::Light => Rgb {
            r: 0.94,
            g: 0.96,
            b: 0.98,
        },
        OverlayColorMode::Dark => Rgb {
            r: 0.08,
            g: 0.10,
            b: 0.12,
        },
        OverlayColorMode::Fixed { r, g, b } => Rgb { r, g, b },
    }
}

fn luminance(color: Rgb) -> f32 {
    color.r * 0.2126 + color.g * 0.7152 + color.b * 0.0722
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_at(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius_px: u32,
    color: Rgb,
    alpha: f32,
) {
    let radius = (radius_px as f32).min(w.min(h).max(1) as f32 * 0.5);
    for py in y.max(0)..(y + h).min(canvas_h as i32) {
        for px in x.max(0)..(x + w).min(canvas_w as i32) {
            let local_x = px as f32 + 0.5 - x as f32 - w as f32 * 0.5;
            let local_y = py as f32 + 0.5 - y as f32 - h as f32 * 0.5;
            let dist = rounded_rect_sdf(local_x, local_y, w as f32, h as f32, radius);
            let coverage = sdf_alpha(dist);
            if coverage > 0.0 {
                let offset = ((py as u32 * canvas_w + px as u32) * 4) as usize;
                blend_argb8888(&mut canvas[offset..offset + 4], color, alpha * coverage);
            }
        }
    }
}

fn fill_rect(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Rgb,
    alpha: f32,
) {
    for py in y.max(0)..(y + h).min(canvas_h as i32) {
        for px in x.max(0)..(x + w).min(canvas_w as i32) {
            let offset = ((py as u32 * canvas_w + px as u32) * 4) as usize;
            blend_argb8888(&mut canvas[offset..offset + 4], color, alpha);
        }
    }
}

fn fill_circle(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    cx: i32,
    cy: i32,
    radius: i32,
    color: Rgb,
    alpha: f32,
) {
    let r = radius.max(1) as f32;
    for y in (cy - radius - 1).max(0)..(cy + radius + 1).min(canvas_h as i32) {
        for x in (cx - radius - 1).max(0)..(cx + radius + 1).min(canvas_w as i32) {
            let dx = x as f32 + 0.5 - cx as f32;
            let dy = y as f32 + 0.5 - cy as f32;
            let dist = (dx * dx + dy * dy).sqrt() - r;
            let coverage = sdf_alpha(dist);
            if coverage > 0.0 {
                let offset = ((y as u32 * canvas_w + x as u32) * 4) as usize;
                blend_argb8888(&mut canvas[offset..offset + 4], color, alpha * coverage);
            }
        }
    }
}

fn draw_raster(
    canvas: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    raster: &IconRaster,
    alpha: f32,
) {
    if w <= 0 || h <= 0 || raster.width == 0 || raster.height == 0 {
        return;
    }
    for dy in 0..h {
        let py = y + dy;
        if py < 0 || py >= canvas_h as i32 {
            continue;
        }
        let sy = ((dy as f32 / h as f32) * raster.height as f32)
            .floor()
            .clamp(0.0, (raster.height - 1) as f32) as u32;
        for dx in 0..w {
            let px = x + dx;
            if px < 0 || px >= canvas_w as i32 {
                continue;
            }
            let sx = ((dx as f32 / w as f32) * raster.width as f32)
                .floor()
                .clamp(0.0, (raster.width - 1) as f32) as u32;
            let src = ((sy * raster.width + sx) * 4) as usize;
            let a = raster.pixels_rgba[src + 3] as f32 / 255.0 * alpha;
            if a <= 0.0 {
                continue;
            }
            let color = Rgb {
                r: raster.pixels_rgba[src] as f32 / 255.0,
                g: raster.pixels_rgba[src + 1] as f32 / 255.0,
                b: raster.pixels_rgba[src + 2] as f32 / 255.0,
            };
            let dst = ((py as u32 * canvas_w + px as u32) * 4) as usize;
            blend_argb8888(&mut canvas[dst..dst + 4], color, a);
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

fn blend_argb8888(dst: &mut [u8], color: Rgb, a: f32) {
    let dst_b = dst[0] as f32 / 255.0;
    let dst_g = dst[1] as f32 / 255.0;
    let dst_r = dst[2] as f32 / 255.0;
    let dst_a = dst[3] as f32 / 255.0;
    let a = a.clamp(0.0, 1.0);
    let out_a = a + dst_a * (1.0 - a);
    let out_r = color.r * a + dst_r * (1.0 - a);
    let out_g = color.g * a + dst_g * (1.0 - a);
    let out_b = color.b * a + dst_b * (1.0 - a);
    dst[0] = (out_b.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[1] = (out_g.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[2] = (out_r.clamp(0.0, 1.0) * 255.0).round() as u8;
    dst[3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
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

    fn measure_px(&self, text: &str, font_px: u32) -> (u32, u32) {
        let bounds = self.layout_bounds(text, font_px);
        (bounds.w.max(1.0) as u32, bounds.h.max(1.0) as u32)
    }

    #[allow(clippy::too_many_arguments)]
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
        color: Rgb,
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
                let a = (coverage * alpha).clamp(0.0, 1.0);
                if a <= 0.0 {
                    return;
                }
                let offset = ((py as u32 * width + px as u32) * 4) as usize;
                blend_argb8888(&mut canvas[offset..offset + 4], color, a);
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
        }
        if matches!(weight, Weight::NORMAL)
            && let Some(stripped) = strip_font_suffix(family, &[" bold"])
        {
            family = stripped;
            weight = Weight::BOLD;
            continue;
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

fn ellipsize_to_width(
    font_renderer: &FontRenderer,
    text: &str,
    font_px: u32,
    max_width: i32,
) -> String {
    if max_width <= 0 {
        return String::new();
    }
    if font_renderer.measure_px(text, font_px).0 as i32 <= max_width {
        return text.to_string();
    }
    let ellipsis = "...";
    if font_renderer.measure_px(ellipsis, font_px).0 as i32 > max_width {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (0..=chars.len()).rev() {
        let candidate = chars.iter().take(keep).collect::<String>() + ellipsis;
        if font_renderer.measure_px(candidate.as_str(), font_px).0 as i32 <= max_width {
            return candidate;
        }
    }
    ellipsis.to_string()
}

fn wait_for_wayland_or_timeout(
    event_queue: &mut EventQueue<StandaloneRail>,
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

fn anchor_for_placement(placement: RailPlacement) -> Anchor {
    match placement {
        RailPlacement::Up => Anchor::TOP,
        RailPlacement::Down => Anchor::BOTTOM,
        RailPlacement::Left => Anchor::LEFT,
        RailPlacement::Right => Anchor::RIGHT,
    }
}

fn set_layer_margin(layer: &LayerSurface, config: &RailConfig) {
    let (top, right, bottom, left) = match config.placement {
        RailPlacement::Up => (config.offset_y, 0, 0, config.offset_x),
        RailPlacement::Down => (0, 0, config.offset_y, config.offset_x),
        RailPlacement::Left => (config.offset_y, 0, 0, config.offset_x),
        RailPlacement::Right => (config.offset_y, config.offset_x, 0, 0),
    };
    layer.set_margin(top, right, bottom, left);
}

fn set_layer_margin_for_layout(layer: &LayerSurface, config: &RailConfig, layout: &RailLayout) {
    if !layout.edge_aligned {
        set_layer_margin(layer, config);
        return;
    }
    let (top, right, bottom, left) = match config.placement {
        RailPlacement::Up => (0, 0, 0, config.offset_x),
        RailPlacement::Down => (0, 0, 0, config.offset_x),
        RailPlacement::Left => (config.offset_y, 0, 0, 0),
        RailPlacement::Right => (config.offset_y, 0, 0, 0),
    };
    layer.set_margin(top, right, bottom, left);
}

fn load_rail_config(path: &Path) -> RailConfig {
    RuntimeTuning::try_load_from_path(path.to_string_lossy().as_ref())
        .map(|tuning| tuning.rail)
        .unwrap_or_default()
}

fn default_rail_config_path() -> PathBuf {
    if let Ok(home) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join("halley/rail.rune");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Path::new(trimmed).join(".config/halley/rail.rune");
        }
    }
    PathBuf::from("rail.rune")
}

impl CompositorHandler for StandaloneRail {
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
            eprintln!("halley-rail draw failed: {err}");
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

impl OutputHandler for StandaloneRail {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layers(qh) {
            eprintln!("halley-rail recreate layer failed: {err}");
            self.exit = true;
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layers(qh) {
            eprintln!("halley-rail recreate layer failed: {err}");
            self.exit = true;
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        if let Err(err) = self.recreate_layers(qh) {
            eprintln!("halley-rail recreate layer failed: {err}");
            self.exit = true;
        }
    }
}

impl SeatHandler for StandaloneRail {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(err) => eprintln!("halley-rail pointer setup failed: {err}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for StandaloneRail {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let mut changed = false;
        for event in events {
            let Some(layer_index) = self.layer_index_for_surface(&event.surface) else {
                continue;
            };
            let x = event.position.0.round() as i32;
            let y = event.position.1.round() as i32;
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    let output_name = self.layers[layer_index].output_name.clone();
                    changed |= self.cancel_pending_hide(output_name.as_deref());
                    if self
                        .snapshot_for_output(output_name.as_deref())
                        .is_some_and(snapshot_revealable_hidden)
                    {
                        changed |= self.reveal_output_now(output_name);
                    }
                    changed |= self.update_menu_hover(layer_index, x, y);
                    changed |= self.update_hover_for_position(layer_index, x, y);
                }
                PointerEventKind::Motion { .. } => {
                    changed |= self.update_menu_hover(layer_index, x, y);
                    changed |= self.update_hover_for_position(layer_index, x, y);
                }
                PointerEventKind::Leave { .. } => {
                    let output_name = self.layers[layer_index].output_name.clone();
                    if self.output_revealed(output_name.as_deref()) {
                        changed |= self.start_pending_hide(output_name);
                    }
                    if self.hovered_item.take().is_some() {
                        changed = true;
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    changed |= self.handle_press(layer_index, button, x, y);
                }
                PointerEventKind::Release { .. } | PointerEventKind::Axis { .. } => {}
            }
        }
        if changed && let Err(err) = self.draw() {
            eprintln!("halley-rail draw failed: {err}");
            self.exit = true;
        }
    }
}

impl LayerShellHandler for StandaloneRail {
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
            eprintln!("halley-rail draw failed: {err}");
            self.exit = true;
        }
    }
}

impl ShmHandler for StandaloneRail {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for StandaloneRail {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(StandaloneRail);
delegate_output!(StandaloneRail);
delegate_seat!(StandaloneRail);
delegate_pointer!(StandaloneRail);
delegate_layer!(StandaloneRail);
delegate_shm!(StandaloneRail);
delegate_registry!(StandaloneRail);
