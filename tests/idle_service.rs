#![cfg(feature = "dbus")]
#![allow(dead_code)]
#[path = "../src/idle_service.rs"]
mod idle_service;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, Proxy};
use zbus::names::BusName;

struct Bus(Child, String);
impl Bus {
    fn new() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self(child, address.trim().into())
    }
    fn connection(&self) -> Connection {
        zbus::blocking::connection::Builder::address(self.1.as_str())
            .unwrap()
            .method_timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn proxy<'a>(client: &'a Connection, path: &'a str) -> Proxy<'a> {
    Proxy::new(
        client,
        "org.freedesktop.ScreenSaver",
        path,
        "org.freedesktop.ScreenSaver",
    )
    .unwrap()
}
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(Instant::now() < deadline, "idle service did not update");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn inhibitors_are_owned_removed_on_disconnect_and_release_the_bus_name() {
    let bus = Bus::new();
    let (sender, changes) = calloop::channel::channel();
    let service = idle_service::Service::with_connection(bus.connection(), sender).unwrap();
    let first = bus.connection();
    let other = bus.connection();
    let p = proxy(&first, "/org/freedesktop/ScreenSaver");
    let legacy = proxy(&other, "/ScreenSaver");
    assert!(!service.is_inhibited());
    let cookie: u32 = p.call("Inhibit", &("firefox", "video playback")).unwrap();
    assert_ne!(cookie, 0);
    assert!(service.is_inhibited());
    assert!(changes.try_recv().is_ok());
    let result: zbus::Result<()> = legacy.call("UnInhibit", &(cookie,));
    assert!(
        matches!(result, Err(zbus::Error::MethodError(name,_,_)) if name.as_str()=="org.freedesktop.DBus.Error.AccessDenied")
    );
    assert!(service.is_inhibited());
    let second_cookie: u32 = legacy.call("Inhibit", &("other", "presentation")).unwrap();
    assert_ne!(cookie, second_cookie);
    p.call::<_, _, ()>("UnInhibit", &(cookie,)).unwrap();
    assert!(service.is_inhibited());
    drop(legacy);
    drop(other);
    wait_until(|| !service.is_inhibited());
    // The disconnect subscription exists before the service accepts callers.
    for _ in 0..10 {
        let short = bus.connection();
        let _: u32 = proxy(&short, "/ScreenSaver")
            .call("Inhibit", &("short", "quick work"))
            .unwrap();
        drop(short);
        wait_until(|| !service.is_inhibited());
    }
    let bus_proxy = zbus::blocking::fdo::DBusProxy::new(&first).unwrap();
    drop(service);
    wait_until(|| {
        !bus_proxy
            .name_has_owner(BusName::try_from("org.freedesktop.ScreenSaver").unwrap())
            .unwrap()
    });
}
#[test]
fn a_second_instance_cannot_take_over_or_queue_behind_an_existing_service() {
    let bus = Bus::new();
    let (sender, _) = calloop::channel::channel();
    let service = idle_service::Service::with_connection(bus.connection(), sender).unwrap();
    let (sender, _) = calloop::channel::channel();
    assert!(idle_service::Service::with_connection(bus.connection(), sender).is_err());
    let client = bus.connection();
    let p = proxy(&client, "/ScreenSaver");
    let cookie: u32 = p.call("Inhibit", &("app", "work")).unwrap();
    assert!(service.is_inhibited());
    p.call::<_, _, ()>("UnInhibit", &(cookie,)).unwrap();
    assert!(!service.is_inhibited());
    assert!(p.call::<_, _, ()>("UnInhibit", &(0u32,)).is_err());
}
