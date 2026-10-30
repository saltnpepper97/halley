pub mod encoder;
pub mod menu;
pub mod picker;
pub(crate) mod preview;
pub mod screencast;
mod screenshot;
pub mod source_chooser;

use std::path::PathBuf;

use smithay::desktop::{Space, Window};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use menu::{ScreenshotMenu, ScreenshotMode};
use picker::RegionPicker;
pub(crate) use screenshot::screenshot_directory;
pub(crate) use screenshot::{
    capture_cursor_surface_tree, capture_source_pixels, render_source_dmabuf, resolve_source_window,
};
pub(crate) use screenshot::{capture_monitor_region_pixels, render_monitor_region_dmabuf};
pub use screenshot::{save_area, save_region, save_window};
use source_chooser::{SourceChooser, SourceMode, SourcePhase};

use crate::session::{Session, SessionDriver};

pub(crate) fn window_chrome_visible<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
) -> bool {
    !crate::xwayland::is_fullscreen(window)
        && window
            .wl_surface()
            .is_none_or(|surface| !session.fullscreen.suppresses_chrome(surface.as_ref()))
}

pub(crate) fn window_capture_size<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
) -> smithay::utils::Size<i32, smithay::utils::Physical> {
    let client = window.geometry().size;
    let size = if window_chrome_visible(session, window) {
        crate::titlebar::outer_size_for_client(
            window,
            client,
            &session.settings.decorations,
            &session.settings.font,
        )
    } else {
        client
    };
    size.to_f64()
        .to_physical(window_display_scale(session, window))
        .to_i32_round()
}

pub(crate) fn window_display_scale<D: SessionDriver>(session: &Session<D>, window: &Window) -> f64 {
    window
        .wl_surface()
        .and_then(|surface| {
            crate::wayland::display_scale::output_for_surface(&session.wayland, surface.as_ref())
        })
        .unwrap_or_else(|| session.driver.primary_output().clone())
        .current_scale()
        .fractional_scale()
}

pub(crate) fn window_capture_client_offset<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
) -> Point<i32, Logical> {
    if !window_chrome_visible(session, window) {
        return (0, 0).into();
    }
    let outer = crate::titlebar::outer_rect_for_client(
        window,
        Rectangle::new((0, 0).into(), window.geometry().size),
        &session.settings.decorations,
        &session.settings.font,
    );
    (-outer.loc.x, -outer.loc.y).into()
}

pub(crate) fn window_capture_visual_geometry<D: SessionDriver>(
    session: &Session<D>,
    window: &Window,
    client: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    if !window_chrome_visible(session, window) {
        return client;
    }
    let native_client = Rectangle::<i32, Logical>::from_size(window.geometry().size);
    let native_outer = crate::titlebar::outer_rect_for_client(
        window,
        native_client,
        &session.settings.decorations,
        &session.settings.font,
    );
    let scale_x = client.size.w as f64 / native_client.size.w.max(1) as f64;
    let scale_y = client.size.h as f64 / native_client.size.h.max(1) as f64;
    let left = ((native_client.loc.x - native_outer.loc.x) as f64 * scale_x).round() as i32;
    let right = ((native_outer.size.w
        - native_client.size.w
        - (native_client.loc.x - native_outer.loc.x)) as f64
        * scale_x)
        .round() as i32;
    let top = ((native_client.loc.y - native_outer.loc.y) as f64 * scale_y).round() as i32;
    let bottom = ((native_outer.size.h
        - native_client.size.h
        - (native_client.loc.y - native_outer.loc.y)) as f64
        * scale_y)
        .round() as i32;
    Rectangle::new(
        (client.loc.x - left, client.loc.y - top).into(),
        (
            client
                .size
                .w
                .saturating_add(left)
                .saturating_add(right)
                .max(1),
            client
                .size
                .h
                .saturating_add(top)
                .saturating_add(bottom)
                .max(1),
        )
            .into(),
    )
}

#[derive(Default)]
pub struct CaptureState {
    picker: RegionPicker,
    source_chooser: SourceChooser,
    selection: Option<Selection>,
    pending: Option<PendingCapture>,
    region_output: Option<String>,
}

enum Selection {
    Menu,
    Area,
    Screen {
        geometry: Rectangle<i32, Logical>,
    },
    Window {
        surface: Option<WlSurface>,
        geometry: Option<Rectangle<i32, Logical>>,
    },
}

enum PendingCapture {
    Local {
        menu: ScreenshotMenu,
        reply: Option<crate::ipc::ReplySender>,
        silent: bool,
    },
    Screenshot {
        request_handle: String,
        reply: crate::ipc::ReplySender,
    },
    Source {
        request_handle: String,
        reply: crate::ipc::ReplySender,
    },
}

struct AcceptedCapture {
    target: AcceptedTarget,
    pending: PendingCapture,
}

enum AcceptedTarget {
    Area {
        region: Rectangle<i32, Logical>,
        output: Option<String>,
    },
    Screen(Rectangle<i32, Logical>),
    Window(WlSurface),
    Source(halley_ipc::CaptureSource),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Menu,
    Area,
    Screen,
    Window,
    Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePress {
    Consumed,
    ActivateScreenshot(ScreenshotMode),
    ActivateSource(SourceMode),
    Accept,
}

#[derive(Clone, Copy, Debug)]
pub enum CaptureOverlay<'a> {
    None,
    Region(Rectangle<i32, Logical>),
    Highlight(Rectangle<i32, Logical>),
    Menu {
        output_name: &'a str,
        selected: usize,
        hovered: Option<usize>,
        window_available: bool,
    },
    SourceMenu {
        output_name: &'a str,
        selected: usize,
        hovered: Option<usize>,
        monitor_available: bool,
        window_available: bool,
    },
}

impl CaptureState {
    pub fn is_active(&self) -> bool {
        self.selection.is_some() || self.source_chooser.is_active()
    }

    pub fn kind(&self) -> Option<CaptureKind> {
        if self.source_chooser.is_active() {
            return Some(CaptureKind::Source);
        }
        match self.selection.as_ref()? {
            Selection::Menu => Some(CaptureKind::Menu),
            Selection::Area => Some(CaptureKind::Area),
            Selection::Screen { .. } => Some(CaptureKind::Screen),
            Selection::Window { .. } => Some(CaptureKind::Window),
        }
    }

    pub fn menu_is_active(&self) -> bool {
        matches!(self.selection, Some(Selection::Menu))
            || self.source_chooser.phase() == Some(SourcePhase::Menu)
    }

    #[cfg(any(feature = "dbus", test))]
    pub fn source_accessibility(
        &self,
    ) -> (
        Option<SourcePhase>,
        usize,
        Option<&halley_ipc::CaptureSource>,
    ) {
        (
            self.source_chooser.phase(),
            self.source_chooser.selected(),
            self.source_chooser.selected_source(),
        )
    }

    #[cfg(feature = "dbus")]
    pub fn screenshot_accessibility(
        &self,
        nodes: &crate::nodes::NodesState,
        space: &Space<Window>,
    ) -> String {
        match &self.selection {
            Some(Selection::Menu) => {
                let mode = self.local_menu().map(ScreenshotMenu::selected_mode);
                let name = match mode {
                    Some(ScreenshotMode::Region) => "region",
                    Some(ScreenshotMode::Screen) => "monitor",
                    Some(ScreenshotMode::Window) => "window",
                    None => "unknown",
                };
                format!(
                    "Screenshot, {name}. Use arrows to choose, Enter to select, Escape to cancel."
                )
            }
            Some(Selection::Area) => match self.picker.region() {
                Some(rect) => format!(
                    "Screenshot region at {}, {}, width {}, height {}. Use arrows to move, Shift plus arrows to resize, Enter to capture, Escape to go back.",
                    rect.loc.x, rect.loc.y, rect.size.w, rect.size.h
                ),
                None => "Screenshot region".into(),
            },
            Some(Selection::Screen { geometry }) => {
                let name = space
                    .outputs()
                    .find(|output| space.output_geometry(output).as_ref() == Some(geometry))
                    .map(|output| output.name())
                    .unwrap_or_else(|| "unknown".into());
                format!(
                    "Screenshot monitor {name}. Use arrows to choose, Enter to capture, Escape to go back."
                )
            }
            Some(Selection::Window { surface, .. }) => {
                let name = surface
                    .as_ref()
                    .and_then(|surface| nodes.id_for_surface(surface))
                    .and_then(|id| nodes.record(id))
                    .map(|record| record.title.as_str())
                    .filter(|title| !title.is_empty())
                    .unwrap_or("No window selected");
                format!(
                    "Screenshot window, {name}. Use arrows to choose, Enter to capture, Escape to go back."
                )
            }
            None => "Screenshot chooser".into(),
        }
    }

    pub fn overlay(&self) -> CaptureOverlay<'_> {
        match self.selection.as_ref() {
            Some(Selection::Menu) => {
                let Some(menu) = self.local_menu() else {
                    return CaptureOverlay::None;
                };
                CaptureOverlay::Menu {
                    output_name: menu.output_name(),
                    selected: menu.selected(),
                    hovered: menu.hovered(),
                    window_available: menu.window_available(),
                }
            }
            Some(Selection::Area) => self
                .picker
                .region()
                .map(CaptureOverlay::Region)
                .unwrap_or(CaptureOverlay::None),
            Some(Selection::Screen { geometry, .. }) => CaptureOverlay::Highlight(*geometry),
            Some(Selection::Window { geometry, .. }) => geometry
                .map(CaptureOverlay::Highlight)
                .unwrap_or(CaptureOverlay::None),
            None if self.source_chooser.phase() == Some(SourcePhase::Menu) => {
                CaptureOverlay::SourceMenu {
                    output_name: self.source_chooser.output_name(),
                    selected: self.source_chooser.selected(),
                    hovered: self.source_chooser.hovered(),
                    monitor_available: self.source_chooser.monitor_available(),
                    window_available: self.source_chooser.window_available(),
                }
            }
            None => self
                .source_chooser
                .selection_geometry()
                .map(CaptureOverlay::Highlight)
                .unwrap_or(CaptureOverlay::None),
        }
    }

    pub fn begin_menu(
        &mut self,
        space: &Space<Window>,
        preferred_output: Option<&str>,
        window_available: bool,
    ) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let output = preferred_output
            .and_then(|name| space.outputs().find(|output| output.name() == name))
            .or_else(|| space.outputs().next());
        let Some(output) = output else {
            return false;
        };
        let Some(geometry) = space.output_geometry(output) else {
            return false;
        };
        self.region_output = None;
        self.selection = Some(Selection::Menu);
        self.pending = Some(PendingCapture::Local {
            menu: ScreenshotMenu::new(output.name(), geometry, window_available),
            reply: None,
            silent: false,
        });
        true
    }

    fn attach_local_reply(&mut self, reply: crate::ipc::ReplySender, silent: bool) {
        let Some(PendingCapture::Local {
            reply: local_reply,
            silent: local_silent,
            ..
        }) = self.pending.as_mut()
        else {
            unreachable!("local reply can only be attached to a local capture")
        };
        *local_reply = Some(reply);
        *local_silent = silent;
    }

    fn begin_area(
        &mut self,
        space: &Space<Window>,
        preferred_output: Option<&str>,
        pending: PendingCapture,
    ) -> Result<(), PendingCapture> {
        if self.pending.is_some() {
            return Err(pending);
        }
        let outputs = space
            .outputs()
            .filter_map(|output| space.output_geometry(output))
            .collect::<Vec<_>>();
        let active = preferred_output
            .and_then(|name| {
                space
                    .outputs()
                    .find(|output| output.name() == name)
                    .and_then(|output| space.output_geometry(output))
            })
            .or_else(|| {
                space
                    .outputs()
                    .next()
                    .and_then(|output| space.output_geometry(output))
            });
        let Some(active) = active else {
            return Err(pending);
        };
        self.region_output = None;
        self.picker.begin(outputs, active);
        self.selection = Some(Selection::Area);
        self.pending = Some(pending);
        Ok(())
    }

    fn begin_window(&mut self, pending: PendingCapture) -> Result<(), PendingCapture> {
        if self.pending.is_some() {
            return Err(pending);
        }
        self.region_output = None;
        self.selection = Some(Selection::Window {
            surface: None,
            geometry: None,
        });
        self.pending = Some(pending);
        Ok(())
    }

    fn begin_source(
        &mut self,
        space: &Space<Window>,
        preferred_output: Option<&str>,
        source_types: u32,
        pending: PendingCapture,
    ) -> Result<(), PendingCapture> {
        if self.pending.is_some() {
            return Err(pending);
        }
        let output = preferred_output
            .and_then(|name| space.outputs().find(|output| output.name() == name))
            .or_else(|| space.outputs().next());
        let Some(output) = output else {
            return Err(pending);
        };
        let Some(geometry) = space.output_geometry(output) else {
            return Err(pending);
        };
        self.region_output = None;
        self.source_chooser
            .begin(source_types, output.name(), geometry);
        self.pending = Some(pending);
        Ok(())
    }

    /// False when a requested region output disappeared; the caller cancels capture.
    pub fn update_layout(&mut self, space: &Space<Window>) -> bool {
        let outputs = space
            .outputs()
            .filter(|output| {
                self.region_output
                    .as_deref()
                    .is_none_or(|name| output.name() == name)
            })
            .filter_map(|output| space.output_geometry(output))
            .collect::<Vec<_>>();
        if outputs.is_empty() && self.region_output.is_some() {
            return false;
        }
        if !outputs.is_empty() {
            self.picker.update_outputs(outputs);
        }
        if self.source_chooser.is_active()
            && let Some(output) = space
                .outputs()
                .find(|output| output.name() == self.source_chooser.output_name())
            && let Some(geometry) = space.output_geometry(output)
        {
            self.source_chooser.update_output_geometry(geometry);
        }
        true
    }

    pub fn press(&mut self, position: (f64, f64)) -> Option<CapturePress> {
        match self.kind()? {
            CaptureKind::Menu => {
                let mode = self
                    .local_menu()
                    .and_then(|menu| menu.hit_test(Point::from(position)))
                    .map(|index| ScreenshotMode::ALL[index]);
                Some(
                    mode.map(CapturePress::ActivateScreenshot)
                        .unwrap_or(CapturePress::Consumed),
                )
            }
            CaptureKind::Area => {
                self.picker.press(Point::from(position));
                Some(CapturePress::Consumed)
            }
            CaptureKind::Source if self.source_chooser.phase() == Some(SourcePhase::Menu) => Some(
                self.source_chooser
                    .hit_test(Point::from(position))
                    .map(|index| CapturePress::ActivateSource(SourceMode::ALL[index]))
                    .unwrap_or(CapturePress::Consumed),
            ),
            CaptureKind::Screen | CaptureKind::Window | CaptureKind::Source => {
                Some(CapturePress::Accept)
            }
        }
    }

    pub fn motion(&mut self, position: (f64, f64)) -> bool {
        if matches!(self.selection, Some(Selection::Menu)) {
            return self
                .local_menu_mut()
                .is_some_and(|menu| menu.hover(Point::from(position)));
        }
        if self.source_chooser.phase() == Some(SourcePhase::Menu) {
            return self.source_chooser.hover_menu(Point::from(position));
        }
        match self.selection {
            Some(Selection::Area) => self.picker.motion(Point::from(position)),
            Some(Selection::Screen { .. } | Selection::Window { .. }) => true,
            Some(Selection::Menu) => unreachable!("handled above"),
            None => self.source_chooser.is_active(),
        }
    }

    pub fn release(&mut self) -> bool {
        match self.selection {
            Some(Selection::Menu) => true,
            Some(Selection::Area) => self.picker.release(),
            Some(Selection::Screen { .. } | Selection::Window { .. }) => true,
            None => self.source_chooser.is_active(),
        }
    }

    pub fn move_menu_selection(&mut self, delta: i32) -> bool {
        if self.source_chooser.phase() == Some(SourcePhase::Menu) {
            self.source_chooser.move_selection(delta)
        } else {
            matches!(self.selection, Some(Selection::Menu))
                && self
                    .local_menu_mut()
                    .is_some_and(|menu| menu.move_selection(delta))
        }
    }

    pub fn adjust_region(&mut self, dx: i32, dy: i32, resize: bool) -> bool {
        matches!(self.selection, Some(Selection::Area)) && self.picker.adjust(dx, dy, resize)
    }

    pub fn activate_selected_menu(&mut self, space: &Space<Window>) -> bool {
        if self.source_chooser.phase() == Some(SourcePhase::Menu) {
            return self.source_chooser.activate_selected();
        }
        let Some(mode) = self.local_menu().map(ScreenshotMenu::selected_mode) else {
            return false;
        };
        self.activate_menu(mode, space)
    }

    pub fn activate_source(&mut self, mode: SourceMode) -> bool {
        self.source_chooser.activate(mode)
    }

    pub fn activate_menu(&mut self, mode: ScreenshotMode, space: &Space<Window>) -> bool {
        if !matches!(self.selection, Some(Selection::Menu)) {
            return false;
        }
        let Some(menu) = self.local_menu().cloned() else {
            return false;
        };
        match mode {
            ScreenshotMode::Region => {
                let output = self.region_output.clone();
                return self.activate_region(space, output.as_deref());
            }
            ScreenshotMode::Screen => {
                self.selection = Some(Selection::Screen {
                    geometry: menu.output_geometry(),
                });
            }
            ScreenshotMode::Window if menu.window_available() => {
                self.selection = Some(Selection::Window {
                    surface: None,
                    geometry: None,
                });
            }
            ScreenshotMode::Window => return false,
        }
        true
    }

    fn activate_region(&mut self, space: &Space<Window>, output: Option<&str>) -> bool {
        if !matches!(self.selection, Some(Selection::Menu)) {
            return false;
        }
        let Some(menu) = self.local_menu() else {
            return false;
        };
        let active = menu.output_geometry();
        let outputs = space
            .outputs()
            .filter(|candidate| output.is_none_or(|name| candidate.name() == name))
            .filter_map(|output| space.output_geometry(output))
            .collect::<Vec<_>>();
        if outputs.is_empty() {
            return false;
        }
        self.region_output = output.map(str::to_owned);
        self.picker.begin(outputs, active);
        self.selection = Some(Selection::Area);
        true
    }

    pub fn return_to_menu(&mut self) -> bool {
        if self.source_chooser.is_active() {
            return self.source_chooser.return_to_menu();
        }
        if matches!(self.selection, Some(Selection::Menu)) || self.local_menu().is_none() {
            return false;
        }
        if matches!(self.selection, Some(Selection::Area)) {
            self.picker.cancel();
        }
        self.selection = Some(Selection::Menu);
        true
    }

    pub fn hover_screen(&mut self, geometry: Rectangle<i32, Logical>) -> bool {
        let Some(Selection::Screen {
            geometry: selected_geometry,
        }) = self.selection.as_mut()
        else {
            return false;
        };
        *selected_geometry = geometry;
        true
    }

    pub fn hover_window(
        &mut self,
        surface: Option<WlSurface>,
        geometry: Option<Rectangle<i32, Logical>>,
    ) -> bool {
        let Some(Selection::Window {
            surface: selected_surface,
            geometry: selected_geometry,
        }) = self.selection.as_mut()
        else {
            return false;
        };
        *selected_surface = surface;
        *selected_geometry = geometry;
        true
    }

    pub fn hover_source(
        &mut self,
        monitor: halley_ipc::CaptureSource,
        window: Option<(halley_ipc::CaptureSource, Rectangle<i32, Logical>)>,
        monitor_geometry: Rectangle<i32, Logical>,
    ) -> bool {
        self.source_chooser
            .hover_source(monitor, window, monitor_geometry)
    }

    fn accept(&mut self) -> Option<AcceptedCapture> {
        if self.source_chooser.is_active() {
            let source = self.source_chooser.take_selected()?;
            return Some(AcceptedCapture {
                target: AcceptedTarget::Source(source),
                pending: self.pending.take()?,
            });
        }
        let target = match self.selection.take()? {
            Selection::Menu => {
                self.selection = Some(Selection::Menu);
                return None;
            }
            Selection::Area => AcceptedTarget::Area {
                region: self.picker.accept()?,
                output: self.region_output.take(),
            },
            Selection::Screen { geometry, .. } => AcceptedTarget::Screen(geometry),
            Selection::Window {
                surface: Some(surface),
                ..
            } => AcceptedTarget::Window(surface),
            selection @ Selection::Window { surface: None, .. } => {
                self.selection = Some(selection);
                return None;
            }
        };
        self.region_output = None;
        Some(AcceptedCapture {
            target,
            pending: self.pending.take()?,
        })
    }

    fn cancel(&mut self) -> Option<PendingCapture> {
        self.region_output = None;
        if matches!(self.selection.take(), Some(Selection::Area)) {
            self.picker.cancel();
        }
        self.source_chooser.cancel();
        self.pending.take()
    }

    fn local_menu(&self) -> Option<&ScreenshotMenu> {
        match self.pending.as_ref() {
            Some(PendingCapture::Local { menu, .. }) => Some(menu),
            _ => None,
        }
    }

    fn local_menu_mut(&mut self) -> Option<&mut ScreenshotMenu> {
        match self.pending.as_mut() {
            Some(PendingCapture::Local { menu, .. }) => Some(menu),
            _ => None,
        }
    }
}

pub fn begin_local<D: SessionDriver>(
    session: &mut Session<D>,
    preferred_output: Option<&str>,
    window_available: bool,
) -> bool {
    if session.session_lock.active() {
        return false;
    }
    if !session
        .capture
        .begin_menu(&session.wayland.space, preferred_output, window_available)
    {
        return false;
    }
    begin_modal_capture(session);
    true
}

pub fn request_screenshot<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::ScreenshotRequest,
    reply: crate::ipc::ReplySender,
) {
    if session.session_lock.active() {
        let _ = reply.send(
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                message: "session is locked".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    match request.target {
        halley_ipc::ScreenshotTarget::Area => {
            let preferred = crate::wayland::focus::selected_output(&session.wayland)
                .map(|output| output.name());
            let pending = PendingCapture::Screenshot {
                request_handle: request.request_handle,
                reply,
            };
            if let Err(PendingCapture::Screenshot { reply, .. }) =
                session
                    .capture
                    .begin_area(&session.wayland.space, preferred.as_deref(), pending)
            {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "another capture is already active".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            }
            begin_modal_capture(session);
        }
        halley_ipc::ScreenshotTarget::Screen => {
            let Some(region) = desktop_bounds(&session.wayland.space) else {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "no active outputs".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            };
            let captured = save_region(session, region);
            queue_capture(session, captured, PendingCaptureReply::Ipc(reply));
        }
        halley_ipc::ScreenshotTarget::Window => {
            let pending = PendingCapture::Screenshot {
                request_handle: request.request_handle,
                reply,
            };
            if let Err(PendingCapture::Screenshot { reply, .. }) =
                session.capture.begin_window(pending)
            {
                let _ = reply.send(
                    halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Failed {
                        message: "another capture is already active".to_string(),
                    }),
                    Vec::new(),
                );
                return;
            }
            begin_modal_capture(session);
        }
    }
}

fn fail_capture(
    reply: crate::ipc::ReplySender,
    kind: halley_ipc::ServerErrorKind,
    message: impl Into<String>,
) {
    let _ = reply.send(
        halley_ipc::Response::ApiError(halley_ipc::ServerError::new(kind, message)),
        Vec::new(),
    );
}

fn capture_unavailable<D: SessionDriver>(
    session: &Session<D>,
) -> Option<(halley_ipc::ServerErrorKind, &'static str)> {
    if session.session_lock.active() {
        Some((
            halley_ipc::ServerErrorKind::InvalidRequest,
            "session is locked",
        ))
    } else if session.capture.is_active() {
        Some((
            halley_ipc::ServerErrorKind::Busy,
            "another capture is already active",
        ))
    } else {
        None
    }
}

fn requested_output_geometry<D: SessionDriver>(
    session: &Session<D>,
    name: &str,
) -> Option<Rectangle<i32, Logical>> {
    session
        .wayland
        .space
        .outputs()
        .find(|output| output.name() == name)
        .and_then(|output| session.wayland.space.output_geometry(output))
}

/// Validate before translating into desktop coordinates. Never silently clip a target.
fn output_relative_rect(
    output: Rectangle<i32, Logical>,
    rect: halley_ipc::CaptureRect,
) -> Result<Rectangle<i32, Logical>, &'static str> {
    if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
        return Err("capture rectangle requires nonnegative X/Y and positive WIDTH/HEIGHT");
    }
    if i64::from(rect.x) + i64::from(rect.width) > i64::from(output.size.w)
        || i64::from(rect.y) + i64::from(rect.height) > i64::from(output.size.h)
    {
        return Err("capture rectangle is outside the output's logical bounds");
    }
    let x = output
        .loc
        .x
        .checked_add(rect.x)
        .ok_or("capture X coordinate overflow")?;
    let y = output
        .loc
        .y
        .checked_add(rect.y)
        .ok_or("capture Y coordinate overflow")?;
    x.checked_add(rect.width)
        .ok_or("capture right edge overflow")?;
    y.checked_add(rect.height)
        .ok_or("capture bottom edge overflow")?;
    Ok(Rectangle::new(
        (x, y).into(),
        (rect.width, rect.height).into(),
    ))
}

pub fn request_capture<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::CaptureRequest,
    silent: bool,
    reply: crate::ipc::ReplySender,
) {
    use halley_ipc::{CaptureRequest, LocalCaptureMode, ServerErrorKind};
    if let Some((kind, message)) = capture_unavailable(session) {
        fail_capture(reply, kind, message);
        return;
    }
    let (mode, output) = match request {
        CaptureRequest::Window { node_id } => {
            session.nodes.sync_from_space(&session.wayland.space);
            let id = halley_core::field::NodeId::new(node_id);
            let Some((surface, output_name)) = session
                .nodes
                .record(id)
                .map(|record| (record.surface.clone(), record.output.clone()))
            else {
                fail_capture(
                    reply,
                    ServerErrorKind::NotFound,
                    format!("unknown node {node_id}"),
                );
                return;
            };
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = save_window(session, &surface);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(output_name, reply, silent),
            );
            return;
        }
        CaptureRequest::Region {
            output,
            rect: Some(rect),
        } => {
            let Some(name) = output else {
                fail_capture(
                    reply,
                    ServerErrorKind::InvalidRequest,
                    "capture rectangle requires an output",
                );
                return;
            };
            let Some(geometry) = requested_output_geometry(session, &name) else {
                fail_capture(
                    reply,
                    ServerErrorKind::NotFound,
                    format!("unknown or unavailable output {name:?}"),
                );
                return;
            };
            let region = match output_relative_rect(geometry, rect) {
                Ok(region) => region,
                Err(message) => {
                    fail_capture(reply, ServerErrorKind::InvalidRequest, message);
                    return;
                }
            };
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = screenshot::save_output_region(session, &name, region);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(name, reply, silent),
            );
            return;
        }
        CaptureRequest::Menu { output } => (LocalCaptureMode::Menu, output),
        CaptureRequest::Screen { output } => (LocalCaptureMode::Screen, output),
        CaptureRequest::Region { output, rect: None } => (LocalCaptureMode::Region, output),
    };
    request_local_capture_with_options(
        session,
        halley_ipc::LocalCaptureRequest { mode, output },
        silent,
        reply,
    );
}

pub fn request_local_capture<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::LocalCaptureRequest,
    reply: crate::ipc::ReplySender,
) {
    request_local_capture_with_options(session, request, false, reply);
}

fn request_local_capture_with_options<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::LocalCaptureRequest,
    silent: bool,
    reply: crate::ipc::ReplySender,
) {
    use halley_ipc::ServerErrorKind;
    if let Some((kind, message)) = capture_unavailable(session) {
        fail_capture(reply, kind, message);
        return;
    }
    let explicit_output = request.output.is_some();
    if let Some(name) = request.output.as_deref() {
        let Some(geometry) = requested_output_geometry(session, name) else {
            fail_capture(
                reply,
                ServerErrorKind::NotFound,
                format!("unknown or unavailable output {name:?}"),
            );
            return;
        };
        if request.mode == halley_ipc::LocalCaptureMode::Screen {
            session.shell.overlays.clear_screenshot();
            session.request_redraw();
            let captured = screenshot::save_output_region(session, name, geometry);
            queue_capture(
                session,
                captured,
                PendingCaptureReply::local(name.to_string(), reply, silent),
            );
            return;
        }
    }
    let preferred = request.output.or_else(|| {
        crate::wayland::focus::selected_output(&session.wayland).map(|output| output.name())
    });
    let window_available = session.wayland.space.elements().any(|window| {
        preferred.as_deref().is_none_or(|output| {
            crate::wayland::window_output_name(window)
                .map(|name| name == output)
                .unwrap_or_else(|| session.driver.primary_output().name() == output)
        })
    });
    if !session.capture.begin_menu(
        &session.wayland.space,
        preferred.as_deref(),
        window_available,
    ) {
        fail_capture(reply, ServerErrorKind::NotFound, "no available output");
        return;
    }
    session.capture.attach_local_reply(reply, silent);
    let mode = match request.mode {
        halley_ipc::LocalCaptureMode::Menu => None,
        halley_ipc::LocalCaptureMode::Region => Some(ScreenshotMode::Region),
        halley_ipc::LocalCaptureMode::Screen => Some(ScreenshotMode::Screen),
        halley_ipc::LocalCaptureMode::Window => Some(ScreenshotMode::Window),
    };
    let activated = match mode {
        Some(ScreenshotMode::Region) if explicit_output => session
            .capture
            .activate_region(&session.wayland.space, preferred.as_deref()),
        Some(mode) => session.capture.activate_menu(mode, &session.wayland.space),
        None => true,
    };
    if !activated {
        if let Some(PendingCapture::Local {
            reply: Some(reply), ..
        }) = session.capture.cancel()
        {
            fail_capture(
                reply,
                ServerErrorKind::NotFound,
                "requested capture mode is unavailable",
            );
        }
        return;
    }
    begin_modal_capture(session);
}

pub fn request_source<D: SessionDriver>(
    session: &mut Session<D>,
    request: halley_ipc::SourceChooserRequest,
    reply: crate::ipc::ReplySender,
) {
    if session.session_lock.active() {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "session is locked".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    let supported = request.source_types & (halley_ipc::SOURCE_MONITOR | halley_ipc::SOURCE_WINDOW);
    if supported == 0 {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "no supported source type was requested".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    let pending = PendingCapture::Source {
        request_handle: request.request_handle,
        reply,
    };
    let preferred =
        crate::wayland::focus::selected_output(&session.wayland).map(|output| output.name());
    if let Err(PendingCapture::Source { reply, .. }) = session.capture.begin_source(
        &session.wayland.space,
        preferred.as_deref(),
        supported,
        pending,
    ) {
        let _ = reply.send(
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Failed {
                message: "another capture is already active".to_string(),
            }),
            Vec::new(),
        );
        return;
    }
    begin_modal_capture(session);
}

pub fn cancel_portal<D: SessionDriver>(session: &mut Session<D>, request_handle: &str) -> bool {
    let matches = matches!(
        session.capture.pending.as_ref(),
        Some(PendingCapture::Screenshot { request_handle: active, .. }
            | PendingCapture::Source { request_handle: active, .. })
            if active == request_handle
    );
    if !matches {
        return false;
    }
    match session.capture.cancel() {
        Some(PendingCapture::Screenshot { reply, .. }) => {
            let _ = reply.send(
                halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled),
                Vec::new(),
            );
        }
        Some(PendingCapture::Source { reply, .. }) => {
            let _ = reply.send(
                halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Cancelled),
                Vec::new(),
            );
        }
        _ => {}
    }
    finish_modal_capture(session);
    true
}

/// Move between visible, shareable portal sources without moving the pointer.
pub fn move_source_selection<D: SessionDriver>(session: &mut Session<D>, delta: i32) -> bool {
    let phase = session.capture.source_chooser.phase();
    let mut candidates: Vec<(halley_ipc::CaptureSource, Rectangle<i32, Logical>)> = Vec::new();
    match phase {
        Some(SourcePhase::MonitorPick) => {
            for output in session.wayland.space.outputs() {
                if let Some(geometry) = session.wayland.space.output_geometry(output) {
                    candidates.push((
                        halley_ipc::CaptureSource::Monitor {
                            name: output.name(),
                            x: geometry.loc.x,
                            y: geometry.loc.y,
                            width: geometry.size.w,
                            height: geometry.size.h,
                        },
                        geometry,
                    ));
                }
            }
            candidates.sort_by(|a, b| match (&a.0, &b.0) {
                (
                    halley_ipc::CaptureSource::Monitor { name: a, .. },
                    halley_ipc::CaptureSource::Monitor { name: b, .. },
                ) => a.cmp(b),
                _ => std::cmp::Ordering::Equal,
            });
        }
        Some(SourcePhase::WindowPick) => candidates.extend(
            window_selection_candidates(session, true)
                .into_iter()
                .map(|candidate| (candidate.source, candidate.geometry)),
        ),
        _ => return false,
    }
    let current = session.capture.source_chooser.selected_source();
    let index = current.and_then(|source| {
        candidates
            .iter()
            .position(|(candidate, _)| candidate == source)
    });
    let Some(index) = next_source_index(candidates.len(), index, delta) else {
        return false;
    };
    let (source, geometry) = candidates.swap_remove(index);
    session
        .capture
        .source_chooser
        .select_source(source, geometry)
}

fn next_source_index(count: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match current {
        Some(index) if index < count => Some(if delta < 0 {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        }),
        _ => Some(if delta < 0 { count - 1 } else { 0 }),
    }
}

struct WindowSelectionCandidate {
    surface: WlSurface,
    source: halley_ipc::CaptureSource,
    geometry: Rectangle<i32, Logical>,
}

fn window_selection_candidates<D: SessionDriver>(
    session: &Session<D>,
    sharing: bool,
) -> Vec<WindowSelectionCandidate> {
    let mut records: Vec<_> = session
        .nodes
        .records()
        .filter(|record| {
            record.attached
                && !record.collapsed
                && (!sharing
                    || !session
                        .window_rules
                        .applied(&record.surface)
                        .block_screencast)
        })
        .collect();
    records.sort_by(|a, b| {
        (&a.output, &a.title, a.id.as_u64()).cmp(&(&b.output, &b.title, b.id.as_u64()))
    });
    let now = crate::frame_clock::monotonic_now();
    records
        .into_iter()
        .filter_map(|record| {
            let output = session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == record.output)?;
            let output_geometry = session.wayland.space.output_geometry(output)?;
            let presentation = crate::presentation::window::WindowPresentation::for_window(
                &session.wayland.space,
                &session.cameras,
                Some(&session.clusters),
                Some(&session.nodes),
                &session.window_animations,
                &session.fullscreen,
                &session.maximize,
                &session.settings.decorations,
                &session.settings.font,
                &record.window,
                output,
                now,
            )?;
            let geometry = window_capture_visual_geometry(
                session,
                &record.window,
                presentation.visual_geometry(),
            );
            geometry.intersection(output_geometry)?;
            let size = window_capture_size(session, &record.window);
            Some(WindowSelectionCandidate {
                surface: record.surface.clone(),
                geometry,
                source: halley_ipc::CaptureSource::Window {
                    window_id: record.id.as_u64(),
                    width: size.w,
                    height: size.h,
                },
            })
        })
        .collect()
}

/// Cycle full-monitor or window screenshots through the same visible
/// presentations used by pointer selection.
pub fn move_screenshot_selection<D: SessionDriver>(session: &mut Session<D>, delta: i32) -> bool {
    match &session.capture.selection {
        Some(Selection::Screen { geometry }) => {
            let mut outputs: Vec<_> = session
                .wayland
                .space
                .outputs()
                .filter_map(|output| {
                    session
                        .wayland
                        .space
                        .output_geometry(output)
                        .map(|geometry| (output.name(), geometry))
                })
                .collect();
            outputs.sort_by(|a, b| a.0.cmp(&b.0));
            let current = outputs
                .iter()
                .position(|(_, candidate)| candidate == geometry);
            let Some(index) = next_source_index(outputs.len(), current, delta) else {
                return false;
            };
            session.capture.hover_screen(outputs[index].1)
        }
        Some(Selection::Window { surface, .. }) => {
            let mut candidates = window_selection_candidates(session, false);
            let current = surface.as_ref().and_then(|surface| {
                candidates
                    .iter()
                    .position(|candidate| &candidate.surface == surface)
            });
            let Some(index) = next_source_index(candidates.len(), current, delta) else {
                return false;
            };
            let candidate = candidates.swap_remove(index);
            session
                .capture
                .hover_window(Some(candidate.surface), Some(candidate.geometry))
        }
        _ => false,
    }
}

pub fn accept_selected<D: SessionDriver>(session: &mut Session<D>) -> bool {
    let blocked = match session.capture.source_chooser.selected_source() {
        Some(halley_ipc::CaptureSource::Window { window_id, .. }) => {
            resolve_source_window(&session.wayland.space, &session.nodes, *window_id)
                .ok()
                .and_then(|window| {
                    window.wl_surface().map(|surface| {
                        session
                            .window_rules
                            .applied(surface.as_ref())
                            .block_screencast
                    })
                })
                .unwrap_or(false)
        }
        _ => false,
    };
    if blocked {
        let output = session.capture.source_chooser.output_name().to_string();
        session.shell.overlays.show_error(
            output,
            "This window is blocked from screen sharing.",
            3000,
            crate::frame_clock::monotonic_now(),
        );
        session.request_redraw();
        return false;
    }
    let Some(accepted) = session.capture.accept() else {
        return false;
    };
    if let AcceptedTarget::Source(mut source) = accepted.target {
        if let halley_ipc::CaptureSource::Monitor {
            name,
            width,
            height,
            ..
        } = &mut source
        {
            if let Some(output) = session
                .wayland
                .space
                .outputs()
                .find(|output| output.name() == *name)
            {
                let size = crate::render::output_physical_size(output);
                *width = size.w;
                *height = size.h;
            }
        }
        if let PendingCapture::Source { reply, .. } = accepted.pending {
            let _ = reply.send(
                halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Selected(source)),
                Vec::new(),
            );
        }
        finish_modal_capture(session);
        return true;
    }
    let captured = match accepted.target {
        AcceptedTarget::Area {
            region,
            output: Some(output),
        } => screenshot::save_output_region(session, &output, region),
        AcceptedTarget::Area {
            region,
            output: None,
        } => save_area(session, region),
        AcceptedTarget::Screen(region) => save_region(session, region),
        AcceptedTarget::Window(surface) => save_window(session, &surface),
        AcceptedTarget::Source(_) => unreachable!("handled above"),
    };
    queue_capture(
        session,
        captured,
        PendingCaptureReply::from(accepted.pending),
    );
    finish_modal_capture(session);
    true
}

/// Hands a finished capture to the encoder worker and remembers who to answer.
///
/// The GPU readback has to happen on the compositor thread; PNG encoding does
/// not, and doing it inline froze every output for its duration.
fn queue_capture<D: SessionDriver>(
    session: &mut Session<D>,
    captured: Result<screenshot::CapturedImage, Box<dyn std::error::Error>>,
    reply: PendingCaptureReply,
) {
    let submitted =
        captured.and_then(|image| {
            let directory = screenshot_directory(&session.settings.screenshot.directory);
            let encoder = session.screenshot_encoder.as_mut().ok_or_else(
                || -> Box<dyn std::error::Error> { "screenshot encoder is not running".into() },
            )?;
            encoder
                .submit(
                    directory,
                    image.width,
                    image.height,
                    image.pixels,
                    matches!(&reply, PendingCaptureReply::Local { .. }),
                )
                .map_err(|err| -> Box<dyn std::error::Error> { err.into() })
        });
    match submitted {
        Ok(id) => {
            session.pending_captures.insert(id, reply);
        }
        Err(err) => finish_capture_reply(session, reply, Err(err.to_string())),
    }
}

/// What to do once a queued screenshot finishes encoding.
pub enum PendingCaptureReply {
    Local {
        output_name: String,
        reply: Option<crate::ipc::ReplySender>,
    },
    Ipc(crate::ipc::ReplySender),
}

impl PendingCaptureReply {
    fn local(output_name: String, reply: crate::ipc::ReplySender, silent: bool) -> Self {
        if silent {
            Self::Ipc(reply)
        } else {
            Self::Local {
                output_name,
                reply: Some(reply),
            }
        }
    }
}

impl From<PendingCapture> for PendingCaptureReply {
    fn from(pending: PendingCapture) -> Self {
        match pending {
            PendingCapture::Local {
                menu,
                reply: Some(reply),
                silent,
            } => Self::local(menu.output_name().to_string(), reply, silent),
            PendingCapture::Local {
                menu, reply: None, ..
            } => Self::Local {
                output_name: menu.output_name().to_string(),
                reply: None,
            },
            PendingCapture::Screenshot { reply, .. } => Self::Ipc(reply),
            PendingCapture::Source { .. } => {
                unreachable!("source selection returned a screenshot target")
            }
        }
    }
}

/// Delivers a finished encode back to whoever asked for the screenshot.
pub fn finish_encode<D: SessionDriver>(
    session: &mut Session<D>,
    done: crate::capture::encoder::EncodeDone,
) {
    let Some(reply) = session.pending_captures.remove(&done.id) else {
        eventline::warn!("screenshot: completion for unknown job {}", done.id);
        return;
    };
    finish_capture_reply(session, reply, done.result);
}

fn finish_capture_reply<D: SessionDriver>(
    session: &mut Session<D>,
    reply: PendingCaptureReply,
    result: Result<encoder::EncodedCapture, String>,
) {
    match reply {
        PendingCaptureReply::Local { output_name, reply } => {
            if let Some(reply) = reply {
                reply_with_capture(
                    reply,
                    result
                        .as_ref()
                        .map(|capture| capture.path.clone())
                        .map_err(Clone::clone),
                );
            }
            match result {
                Ok(capture) => {
                    let path = capture.path;
                    eventline::info!("screenshot saved to {}", path.display());
                    let directory = path.parent().unwrap_or(path.as_path()).to_path_buf();
                    session.shell.overlays.show_screenshot_saved(
                        output_name,
                        &directory,
                        session.settings.overlays.notifications.success_duration_ms,
                        crate::frame_clock::monotonic_now(),
                    );
                    session
                        .shell
                        .overlays
                        .attach_screenshot_preview(capture.preview);
                    session.request_redraw();
                }
                Err(err) => eventline::error!("screenshot failed: {err}"),
            }
        }
        PendingCaptureReply::Ipc(reply) => {
            reply_with_capture(reply, result.map(|capture| capture.path))
        }
    }
}

pub fn cancel_selected<D: SessionDriver>(session: &mut Session<D>) -> bool {
    let Some(pending) = session.capture.cancel() else {
        return false;
    };
    match pending {
        PendingCapture::Screenshot { reply, .. } => {
            let _ = reply.send(
                halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled),
                Vec::new(),
            );
        }
        PendingCapture::Source { reply, .. } => {
            let _ = reply.send(
                halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Cancelled),
                Vec::new(),
            );
        }
        PendingCapture::Local {
            reply: Some(reply), ..
        } => {
            let _ = reply.send(
                halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Cancelled),
                Vec::new(),
            );
        }
        PendingCapture::Local { reply: None, .. } => {}
    }
    finish_modal_capture(session);
    true
}

fn reply_with_capture(reply: crate::ipc::ReplySender, result: Result<PathBuf, String>) {
    let response = match result {
        Ok(path) => halley_ipc::ScreenshotResponse::Saved {
            path: path.to_string_lossy().into_owned(),
        },
        Err(message) => halley_ipc::ScreenshotResponse::Failed { message },
    };
    let _ = reply.send(halley_ipc::Response::Screenshot(response), Vec::new());
}

fn desktop_bounds(space: &Space<Window>) -> Option<Rectangle<i32, Logical>> {
    space
        .outputs()
        .filter_map(|output| space.output_geometry(output))
        .reduce(Rectangle::merge)
}

fn begin_modal_capture<D: SessionDriver>(session: &mut Session<D>) {
    session.shell.overlays.clear_screenshot();
    let moving_node = match &session.interactions.grab {
        crate::input::grab::Grab::MoveNode { id, .. } => Some(*id),
        _ => None,
    };
    let moving_window = match &session.interactions.grab {
        crate::input::grab::Grab::MoveWindow { id, .. } => *id,
        _ => None,
    };
    if (moving_node.is_some() || moving_window.is_some()) && session.nodes.physics.enabled {
        let _ = crate::nodes::tick_physics(session, crate::frame_clock::monotonic_now());
    }
    if let Some(id) = moving_node {
        session.nodes.clear_direct_motion(id);
    }
    if let Some(id) = moving_window
        && session.nodes.physics.enabled
    {
        session
            .nodes
            .lock_released_window(id, crate::frame_clock::monotonic_now());
    }
    crate::session::cancel_compositor_grab(session);
    crate::session::note_pointer_activity(session);
    let keyboard = session
        .seat
        .get_keyboard()
        .expect("keyboard capability added at seat setup");
    keyboard.set_focus(session, None, smithay::utils::SERIAL_COUNTER.next_serial());
    session.request_redraw();
}

fn finish_modal_capture<D: SessionDriver>(session: &mut Session<D>) {
    crate::session::note_pointer_activity(session);
    crate::session::sync_keyboard_focus(session, smithay::utils::SERIAL_COUNTER.next_serial());
    session.request_redraw();
}

#[cfg(test)]
mod tests;
