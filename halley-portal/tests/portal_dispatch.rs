//! Exercise the real backend on a private bus/runtime, never the user's desktop.
use std::collections::HashMap;
use std::io::BufRead;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zbus::blocking::{Connection, Proxy};
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

type Vardict = HashMap<String, OwnedValue>;
const BACKEND: &str = "org.freedesktop.impl.portal.desktop.halley";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SESSION: &str = "/org/freedesktop/portal/desktop/session/test/one";
const REQUEST: &str = "/org/freedesktop/portal/desktop/request/test/one";

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Runtime(PathBuf);
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn connection(address: &str) -> Connection {
    zbus::blocking::connection::Builder::address(address)
        .unwrap()
        .method_timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}
fn path(value: &str) -> OwnedObjectPath {
    value.try_into().unwrap()
}
fn options(key: &str, value: u32) -> Vardict {
    HashMap::from([(key.into(), value.into())])
}
fn read_request(stream: &std::os::unix::net::UnixStream) -> halley_ipc::Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (bytes, fds) = halley_ipc::read_frame_with_fds(stream, 0).unwrap();
    assert!(fds.is_empty());
    halley_ipc::decode_request(&bytes).unwrap()
}
fn reply(stream: &std::os::unix::net::UnixStream, response: halley_ipc::Response) {
    let bytes = halley_ipc::encode_response(&response).unwrap();
    halley_ipc::write_frame_with_fds(stream, &bytes, &[]).unwrap();
}

#[test]
fn capture_dispatch_keeps_consent_ownership_and_cancellation_responsive() {
    let mut bus = Process(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    std::io::BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let runtime = Runtime(std::env::temp_dir().join(format!(
        "halley-portal-dispatch-{}-{unique}",
        std::process::id()
    )));
    std::fs::create_dir_all(runtime.0.join("halley")).unwrap();
    let listener = UnixListener::bind(runtime.0.join("halley/halley.sock")).unwrap();
    let _backend = Process(
        Command::new(env!("CARGO_BIN_EXE_xdg-desktop-portal-halley"))
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env("XDG_RUNTIME_DIR", &runtime.0)
            .env("PIPEWIRE_REMOTE", "nonexistent-test-pipewire")
            .env_remove("HALLEY_PORTAL_FRONTEND")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = connection(address);
    let other = connection(address);
    let bus_proxy = zbus::blocking::fdo::DBusProxy::new(&client).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !bus_proxy
        .name_has_owner(BusName::try_from(BACKEND).unwrap())
        .unwrap()
    {
        assert!(Instant::now() < deadline, "backend did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    // No trusted frontend name exists; ordinary callers still reach the chooser.
    assert!(
        !bus_proxy
            .name_has_owner(BusName::try_from("org.freedesktop.portal.Desktop").unwrap())
            .unwrap()
    );
    let cast = Proxy::new(
        &client,
        BACKEND,
        PATH,
        "org.freedesktop.impl.portal.ScreenCast",
    )
    .unwrap();
    let (code, _): (u32, Vardict) = cast
        .call(
            "CreateSession",
            &(path(REQUEST), path(SESSION), "test.app", Vardict::new()),
        )
        .unwrap();
    assert_eq!(code, 0);
    // Starting before consent/source selection must not create a stream.
    let (code, _): (u32, Vardict) = cast
        .call(
            "Start",
            &(path(REQUEST), path(SESSION), "test.app", "", Vardict::new()),
        )
        .unwrap();
    assert_eq!(code, 2);
    let foreign = Proxy::new(
        &other,
        BACKEND,
        PATH,
        "org.freedesktop.impl.portal.ScreenCast",
    )
    .unwrap();
    let denied: zbus::Result<(u32, Vardict)> = foreign.call(
        "SelectSources",
        &(
            path(REQUEST),
            path(SESSION),
            "test.app",
            options("types", 0),
        ),
    );
    assert!(
        matches!(denied, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied")
    );
    let wrong_app: zbus::Result<(u32, Vardict)> = cast.call(
        "Start",
        &(
            path(REQUEST),
            path(SESSION),
            "other.app",
            "",
            Vardict::new(),
        ),
    );
    assert!(
        matches!(wrong_app, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied")
    );

    let (entered_tx, entered_rx) = mpsc::channel();
    let compositor = std::thread::spawn(move || {
        let (pending, _) = listener.accept().unwrap();
        assert!(
            matches!(read_request(&pending), halley_ipc::Request::ChooseSource(request) if request.request_handle == REQUEST)
        );
        entered_tx.send(()).unwrap();
        let (cancel, _) = listener.accept().unwrap();
        assert!(
            matches!(read_request(&cancel), halley_ipc::Request::CancelSourceChooser { request_handle } if request_handle == REQUEST)
        );
        reply(&cancel, halley_ipc::Response::Ack);
        reply(
            &pending,
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Cancelled),
        );
        let (screenshot, _) = listener.accept().unwrap();
        assert!(matches!(
            read_request(&screenshot),
            halley_ipc::Request::Screenshot(_)
        ));
        reply(
            &screenshot,
            halley_ipc::Response::Screenshot(halley_ipc::ScreenshotResponse::Saved {
                path: "/tmp/test screenshot.png".into(),
            }),
        );
    });
    let selecting_client = client.clone();
    let selecting = std::thread::spawn(move || {
        let cast = Proxy::new(
            &selecting_client,
            BACKEND,
            PATH,
            "org.freedesktop.impl.portal.ScreenCast",
        )
        .unwrap();
        let result: (u32, Vardict) = cast
            .call(
                "SelectSources",
                &(
                    path(REQUEST),
                    path(SESSION),
                    "test.app",
                    options("types", halley_ipc::SOURCE_MONITOR),
                ),
            )
            .unwrap();
        result.0
    });
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("chooser request never reached compositor");
    // Call Properties.Get directly so the regression fails with a bounded timeout,
    // rather than waiting indefinitely on the client's property-cache initialization.
    let properties = Proxy::new(&client, BACKEND, PATH, "org.freedesktop.DBus.Properties").unwrap();
    let version: OwnedValue = properties
        .call(
            "Get",
            &("org.freedesktop.impl.portal.ScreenCast", "version"),
        )
        .unwrap();
    assert_eq!(u32::try_from(version).unwrap(), 6);
    let foreign_request = Proxy::new(
        &other,
        BACKEND,
        REQUEST,
        "org.freedesktop.impl.portal.Request",
    )
    .unwrap();
    let denied: zbus::Result<()> = foreign_request.call("Close", &());
    assert!(
        matches!(denied, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied")
    );
    let request = Proxy::new(
        &client,
        BACKEND,
        REQUEST,
        "org.freedesktop.impl.portal.Request",
    )
    .unwrap();
    request.call::<_, _, ()>("Close", &()).unwrap();
    assert_eq!(selecting.join().unwrap(), 1);

    let shot = Proxy::new(
        &client,
        BACKEND,
        PATH,
        "org.freedesktop.impl.portal.Screenshot",
    )
    .unwrap();
    let (code, results): (u32, Vardict) = shot
        .call(
            "Screenshot",
            &(path(REQUEST), "test.app", "", options("target", 1)),
        )
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        <&str>::try_from(&results["uri"]).unwrap(),
        "file:///tmp/test%20screenshot.png"
    );
    compositor.join().unwrap();
    let session = Proxy::new(
        &client,
        BACKEND,
        SESSION,
        "org.freedesktop.impl.portal.Session",
    )
    .unwrap();
    let foreign_session = Proxy::new(
        &other,
        BACKEND,
        SESSION,
        "org.freedesktop.impl.portal.Session",
    )
    .unwrap();
    let denied: zbus::Result<()> = foreign_session.call("Close", &());
    assert!(
        matches!(denied, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied")
    );
    session.call::<_, _, ()>("Close", &()).unwrap();
    let (code, _): (u32, Vardict) = cast
        .call(
            "CreateSession",
            &(path(REQUEST), path(SESSION), "test.app", Vardict::new()),
        )
        .unwrap();
    assert_eq!(code, 0, "closed session was not removed");
    session.call::<_, _, ()>("Close", &()).unwrap();
}

#[test]
fn closing_a_session_cancels_its_pending_source_chooser() {
    let mut bus = Process(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    std::io::BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let runtime = Runtime(std::env::temp_dir().join(format!(
        "halley-portal-session-close-{}-{unique}",
        std::process::id()
    )));
    std::fs::create_dir_all(runtime.0.join("halley")).unwrap();
    let listener = UnixListener::bind(runtime.0.join("halley/halley.sock")).unwrap();
    let _backend = Process(
        Command::new(env!("CARGO_BIN_EXE_xdg-desktop-portal-halley"))
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env("XDG_RUNTIME_DIR", &runtime.0)
            .env("PIPEWIRE_REMOTE", "nonexistent-test-pipewire")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = connection(address);
    let bus_proxy = zbus::blocking::fdo::DBusProxy::new(&client).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !bus_proxy
        .name_has_owner(BusName::try_from(BACKEND).unwrap())
        .unwrap()
    {
        assert!(Instant::now() < deadline, "backend did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let cast = Proxy::new(
        &client,
        BACKEND,
        PATH,
        "org.freedesktop.impl.portal.ScreenCast",
    )
    .unwrap();
    let (code, _): (u32, Vardict) = cast
        .call(
            "CreateSession",
            &(path(REQUEST), path(SESSION), "test.app", Vardict::new()),
        )
        .unwrap();
    assert_eq!(code, 0);
    let (entered_tx, entered_rx) = mpsc::channel();
    let compositor = std::thread::spawn(move || {
        let (pending, _) = listener.accept().unwrap();
        assert!(matches!(
            read_request(&pending),
            halley_ipc::Request::ChooseSource(_)
        ));
        listener.set_nonblocking(true).unwrap();
        entered_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut cancelled = false;
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((cancel, _)) => {
                    assert!(matches!(read_request(&cancel),
                        halley_ipc::Request::CancelSourceChooser { request_handle } if request_handle == REQUEST));
                    reply(&cancel, halley_ipc::Response::Ack);
                    cancelled = true;
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("accept cancel request: {err}"),
            }
        }
        reply(
            &pending,
            halley_ipc::Response::Source(halley_ipc::SourceChooserResponse::Cancelled),
        );
        cancelled
    });
    let selecting_client = client.clone();
    let selecting = std::thread::spawn(move || {
        let cast = Proxy::new(
            &selecting_client,
            BACKEND,
            PATH,
            "org.freedesktop.impl.portal.ScreenCast",
        )
        .unwrap();
        let (code, _): (u32, Vardict) = cast
            .call(
                "SelectSources",
                &(
                    path(REQUEST),
                    path(SESSION),
                    "test.app",
                    options("types", halley_ipc::SOURCE_MONITOR),
                ),
            )
            .unwrap();
        code
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let session = Proxy::new(
        &client,
        BACKEND,
        SESSION,
        "org.freedesktop.impl.portal.Session",
    )
    .unwrap();
    session.call::<_, _, ()>("Close", &()).unwrap();
    let cancelled = compositor.join().unwrap();
    assert_eq!(selecting.join().unwrap(), 1);
    assert!(
        cancelled,
        "Session.Close left its source chooser active in the compositor"
    );
}
