//! Manual native pointer-warp check. Left click alternates targets while the
//! implicit click grab is active; right click toggles client fullscreen.
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    os::fd::{AsFd, FromRawFd},
};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::pointer_warp::v1::client::wp_pointer_warp_v1 as warp;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct App {
    globals: HashMap<String, u32>,
    surface: Option<wl_surface::WlSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    viewport: Option<wp_viewport::WpViewport>,
    buffer: Option<wl_buffer::WlBuffer>,
    warp: Option<warp::WpPointerWarpV1>,
    enter: Option<u32>,
    expected: Option<(f64, f64)>,
    alternate: bool,
    fullscreen: bool,
    size: (i32, i32),
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            app.globals.insert(interface, name);
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        shell: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(
        app: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            app.viewport
                .as_ref()
                .unwrap()
                .set_destination(app.size.0, app.size.1);
            let surface = app.surface.as_ref().unwrap();
            surface.attach(app.buffer.as_ref(), 0, 0);
            surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            surface.commit();
        }
    }
}
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => {
                app.size = (width, height)
            }
            xdg_toplevel::Event::Close => app.done = true,
            _ => {}
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial, surface, ..
            } if Some(&surface) == app.surface.as_ref() => app.enter = Some(serial),
            wl_pointer::Event::Leave { .. } => app.enter = None,
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                let toplevel = app.toplevel.as_ref().unwrap();
                if button == 273 {
                    app.fullscreen = !app.fullscreen;
                    if app.fullscreen {
                        toplevel.set_fullscreen(None);
                    } else {
                        toplevel.unset_fullscreen();
                    }
                } else if button == 272
                    && let Some(serial) = app.enter
                {
                    let fraction = if app.alternate { 0.2 } else { 0.8 };
                    app.alternate = !app.alternate;
                    let target = (
                        f64::from(app.size.0) * fraction,
                        f64::from(app.size.1) * fraction,
                    );
                    app.expected = Some(target);
                    toplevel.set_title(format!(
                        "Warp requested: {:.0}, {:.0} | right click: fullscreen",
                        target.0, target.1
                    ));
                    app.warp.as_ref().unwrap().warp_pointer(
                        app.surface.as_ref().unwrap(),
                        pointer,
                        target.0,
                        target.1,
                        serial,
                    );
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } if app.expected.is_some_and(|(x, y)| {
                (surface_x - x).abs() < 0.1 && (surface_y - y).abs() < 0.1
            }) =>
            {
                app.expected = None;
                app.toplevel.as_ref().unwrap().set_title(format!("Warp received: {surface_x:.0}, {surface_y:.0} | left click: jump | right click: fullscreen"));
            }
            _ => {}
        }
    }
}
delegate_noop!(App: ignore wl_compositor::WlCompositor);
delegate_noop!(App: ignore wl_surface::WlSurface);
delegate_noop!(App: ignore wl_seat::WlSeat);
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
delegate_noop!(App: ignore wl_buffer::WlBuffer);
delegate_noop!(App: ignore warp::WpPointerWarpV1);
delegate_noop!(App: ignore wp_viewporter::WpViewporter);
delegate_noop!(App: ignore wp_viewport::WpViewport);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    let registry = connection.display().get_registry(&qh, ());
    let mut app = App {
        size: (480, 360),
        ..App::default()
    };
    queue.roundtrip(&mut app)?;
    let Some(&warp_id) = app.globals.get("wp_pointer_warp_v1") else {
        return Err("This compositor does not advertise pointer-warp-v1. Start the newly installed Halley session before testing.".into());
    };
    let compositor: wl_compositor::WlCompositor =
        registry.bind(app.globals["wl_compositor"], 6, &qh, ());
    let shell: xdg_wm_base::XdgWmBase = registry.bind(app.globals["xdg_wm_base"], 1, &qh, ());
    let shm: wl_shm::WlShm = registry.bind(app.globals["wl_shm"], 1, &qh, ());
    let viewporter: wp_viewporter::WpViewporter =
        registry.bind(app.globals["wp_viewporter"], 1, &qh, ());
    let seat: wl_seat::WlSeat = registry.bind(app.globals["wl_seat"], 8, &qh, ());
    let _pointer = seat.get_pointer(&qh, ());
    app.warp = Some(registry.bind(warp_id, 1, &qh, ()));

    // Two targets at 20% and 80% remain aligned when the viewport resizes.
    let mut pixels = Vec::with_capacity(480 * 360 * 4);
    for y in 0i32..360 {
        for x in 0i32..480 {
            let target = [(96, 72), (384, 288)].iter().any(|&(cx, cy)| {
                ((x - cx).abs() <= 1 && (y - cy).abs() <= 12)
                    || ((y - cy).abs() <= 1 && (x - cx).abs() <= 12)
            });
            let color: u32 = if target { 0x00f0ff80 } else { 0x00273347 };
            pixels.extend_from_slice(&color.to_ne_bytes());
        }
    }
    let fd = unsafe { libc::memfd_create(c"halley-pointer-warp-test".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &qh, ());
    app.buffer = Some(pool.create_buffer(0, 480, 360, 480 * 4, wl_shm::Format::Xrgb8888, &qh, ()));
    let surface = compositor.create_surface(&qh, ());
    app.viewport = Some(viewporter.get_viewport(&surface, &qh, ()));
    let xdg = shell.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_app_id("halley.pointer-warp-test".into());
    toplevel.set_title(
        "Pointer warp test | left click: jump between crosses | right click: fullscreen".into(),
    );
    app.toplevel = Some(toplevel);
    app.surface = Some(surface.clone());
    surface.commit();
    while !app.done {
        queue.blocking_dispatch(&mut app)?;
    }
    Ok(())
}
