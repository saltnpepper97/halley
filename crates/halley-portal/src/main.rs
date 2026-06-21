mod compositor_client;
mod dbus;
mod pipewire_producer;
mod session;

use std::sync::Arc;

use eventline::{error, info};
use zbus::blocking::Connection;

use dbus::{ScreenCastInterface, ScreenCastState, ScreenshotInterface};
use pipewire_producer::PipewireProducer;

const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.halley";
const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--version" | "-V"))
    {
        println!("xdg-desktop-portal-halley {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        print_help();
        return;
    }

    if let Err(e) = pollster::block_on(eventline::setup(eventline::Setup {
        verbose: true,
        level: Some(eventline::LogLevel::Debug),
        console_level: None,
        file_level: None,
        file: None,
        journal_retention: None,
    })) {
        eprintln!("failed to configure logging: {e}");
    }

    info!("xdg-desktop-portal-halley starting");

    if let Err(e) = run() {
        error!("xdg-desktop-portal-halley: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!("xdg-desktop-portal-halley {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!(
        "Native xdg-desktop-portal ScreenCast and Screenshot backend for the Halley compositor."
    );
    println!("Ordinarily autostarted by xdg-desktop-portal; not run directly.");
    println!();
    println!("Options:");
    println!("  -h, --help     Show this help");
    println!("  -V, --version  Show portal backend version");
}

fn run() -> zbus::Result<()> {
    let mut screencast_state = ScreenCastState::new();

    let pipewire = Arc::new(PipewireProducer::new());
    screencast_state.set_pipewire(pipewire);

    let connection = Connection::session()?;
    info!("connected to session bus");

    screencast_state.set_connection(connection.clone());
    let shared_connection = screencast_state.connection_arc();

    connection
        .object_server()
        .at(OBJECT_PATH, ScreenCastInterface::new(screencast_state))?;
    connection
        .object_server()
        .at(OBJECT_PATH, ScreenshotInterface::new(shared_connection))?;

    connection.request_name(BUS_NAME)?;
    info!("acquired bus name {BUS_NAME}");
    info!("ScreenCast + Screenshot backend ready at {OBJECT_PATH}");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
