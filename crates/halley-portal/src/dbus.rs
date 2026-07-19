use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eventline::{debug, info, warn};
use zbus::blocking::Connection;
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use halley_api::{
    CaptureMode, PORTAL_CURSOR_MODE_EMBEDDED, PORTAL_CURSOR_MODE_HIDDEN,
    PORTAL_CURSOR_MODE_METADATA, PORTAL_SOURCE_TYPE_MONITOR, PORTAL_SOURCE_TYPE_WINDOW,
    PortalSourceSelection,
};

use crate::compositor_client::{CompositorClient, ScreenshotOutcome};
use crate::pipewire_producer::PipewireProducer;
use crate::session::{CursorMode, SessionStore};

const SCREENCAST_VERSION: u32 = 6;
const AVAILABLE_SOURCE_TYPES: u32 = PORTAL_SOURCE_TYPE_MONITOR | PORTAL_SOURCE_TYPE_WINDOW;
const AVAILABLE_CURSOR_MODES: u32 =
    PORTAL_CURSOR_MODE_HIDDEN | PORTAL_CURSOR_MODE_EMBEDDED | PORTAL_CURSOR_MODE_METADATA;

const SCREENSHOT_VERSION: u32 = 3;
const SCREENSHOT_TARGET_SCREEN: u32 = 1;
const SCREENSHOT_TARGET_WINDOW: u32 = 2;
const SCREENSHOT_TARGET_AREA: u32 = 4;
const SCREENSHOT_TARGET_ACTIVE_WINDOW: u32 = 8;
const AVAILABLE_SCREENSHOT_TARGETS: u32 =
    SCREENSHOT_TARGET_SCREEN | SCREENSHOT_TARGET_WINDOW | SCREENSHOT_TARGET_AREA;

type Vardict = HashMap<String, OwnedValue>;

fn owned_from_value(value: Value<'_>) -> fdo::Result<OwnedValue> {
    OwnedValue::try_from(value).map_err(|e| fdo::Error::Failed(e.to_string()))
}

pub struct ScreenCastState {
    sessions: Arc<Mutex<SessionStore>>,
    connection: Arc<Mutex<Option<Connection>>>,
    pipewire: Option<Arc<PipewireProducer>>,
}

impl ScreenCastState {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(SessionStore::default())),
            connection: Arc::new(Mutex::new(None)),
            pipewire: None,
        }
    }

    pub fn set_connection(&self, conn: Connection) {
        *self.connection.lock().unwrap() = Some(conn);
    }

    pub fn connection_arc(&self) -> Arc<Mutex<Option<Connection>>> {
        self.connection.clone()
    }

    pub fn set_pipewire(&mut self, pw: Arc<PipewireProducer>) {
        self.pipewire = Some(pw);
    }
}

pub struct ScreenCastInterface {
    state: ScreenCastState,
}

impl ScreenCastInterface {
    pub fn new(state: ScreenCastState) -> Self {
        Self { state }
    }

    fn pipewire(&self) -> Option<&Arc<PipewireProducer>> {
        self.state.pipewire.as_ref()
    }
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastInterface {
    fn create_session(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        _options: Vardict,
    ) -> fdo::Result<(u32, Vardict)> {
        let session_path = session_handle.to_string();
        let app_id = (!app_id.is_empty()).then(|| app_id.to_string());
        export_request_object(&self.state.connection, &handle)?;

        debug!(
            "ScreenCast.CreateSession session={} app_id={:?}",
            session_path, app_id
        );

        let session = self
            .state
            .sessions
            .lock()
            .map_err(|_| fdo::Error::Failed("session store lock poisoned".into()))?
            .create(session_path.clone(), app_id);

        export_session_object(
            &self.state.connection,
            &session_handle,
            self.state.sessions.clone(),
            self.state.pipewire.clone(),
        )?;

        let mut results = Vardict::new();
        results.insert(
            "session_id".into(),
            owned_from_value(Value::from(session.session_id.clone()))?,
        );

        debug!(
            "ScreenCast session created: {} (id={})",
            session_path, session.session_id
        );
        Ok((0, results))
    }

    fn select_sources(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        options: Vardict,
    ) -> fdo::Result<(u32, Vardict)> {
        let session_path = session_handle.to_string();
        export_request_object(&self.state.connection, &handle)?;

        let source_types = extract_u32(&options, "types").unwrap_or(PORTAL_SOURCE_TYPE_MONITOR);
        let cursor_mode = extract_u32(&options, "cursor_mode").unwrap_or(PORTAL_CURSOR_MODE_HIDDEN);

        debug!(
            "ScreenCast.SelectSources session={} app_id={} types={} cursor={}",
            session_path, app_id, source_types, cursor_mode
        );

        // Accept any combination of the source types we advertise. We do not
        // reject window-only requests anymore — that was what blocked Discord's
        // "share a window" path.
        let supported = source_types & (PORTAL_SOURCE_TYPE_MONITOR | PORTAL_SOURCE_TYPE_WINDOW);
        if supported == 0 {
            warn!("SelectSources: no supported source types requested ({source_types}), rejecting");
            return Ok((2, Vardict::new()));
        }

        let cursor = CursorMode::from_mask(cursor_mode);
        if cursor_mode & !AVAILABLE_CURSOR_MODES != 0 || cursor_mode == 0 {
            warn!("SelectSources: unsupported cursor mode {cursor_mode}, rejecting");
            return Ok((2, Vardict::new()));
        }

        let mut sessions = self
            .state
            .sessions
            .lock()
            .map_err(|_| fdo::Error::Failed("session store lock poisoned".into()))?;

        let Some(session) = sessions.get_mut(&session_path) else {
            warn!("SelectSources: unknown session {session_path}");
            return Ok((2, Vardict::new()));
        };

        session.source_types = supported;
        session.cursor_mode = cursor;
        let session_handle = session.session_handle.clone();
        drop(sessions);

        // Open the Halley-native source picker. This blocks until the user
        // confirms (Screen or Window) or cancels; xdg-desktop-portal fronts
        // wait on this D-Bus return, matching how KDE/GNOME portals behave.
        match crate::compositor_client::CompositorClient::choose_source(&session_handle, supported)
        {
            Ok(PortalSourceSelection::Monitor(output)) => {
                let mut sessions = self
                    .state
                    .sessions
                    .lock()
                    .map_err(|_| fdo::Error::Failed("session store lock poisoned".into()))?;
                if let Some(session) = sessions.get_mut(&session_path) {
                    session.selected_output = Some(output.name.clone());
                    session.selected_source = Some(PortalSourceSelection::Monitor(output));
                    debug!("SelectSources: user picked monitor for {session_path}");
                }
                Ok((0, Vardict::new()))
            }
            Ok(PortalSourceSelection::Window(window)) => {
                let node_id = window.node_id;
                let mut sessions = self
                    .state
                    .sessions
                    .lock()
                    .map_err(|_| fdo::Error::Failed("session store lock poisoned".into()))?;
                if let Some(session) = sessions.get_mut(&session_path) {
                    session.selected_source = Some(PortalSourceSelection::Window(window));
                    debug!(
                        "SelectSources: user picked window node {} for {}",
                        node_id, session_path
                    );
                }
                Ok((0, Vardict::new()))
            }
            Err(e) => {
                warn!("SelectSources: chooser failed for {session_path}: {e}");
                Ok((2, Vardict::new()))
            }
        }
    }

    fn start(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        _parent_window: &str,
        _options: Vardict,
    ) -> fdo::Result<(u32, Vardict)> {
        let session_path = session_handle.to_string();
        export_request_object(&self.state.connection, &handle)?;

        debug!(
            "ScreenCast.Start session={} app_id={}",
            session_path, app_id
        );

        let (selection, cursor_mode_u32) = {
            let sessions = self
                .state
                .sessions
                .lock()
                .map_err(|_| fdo::Error::Failed("session store lock poisoned".into()))?;
            let Some(session) = sessions.get(&session_path) else {
                warn!("Start: unknown session {session_path}");
                return Ok((2, Vardict::new()));
            };
            let Some(ref selection) = session.selected_source else {
                warn!("Start: no source selected for {session_path}");
                return Ok((2, Vardict::new()));
            };
            (selection.clone(), session.cursor_mode.as_u32())
        };

        let start_result = match &selection {
            PortalSourceSelection::Monitor(output) => {
                crate::compositor_client::CompositorClient::start(
                    &session_path,
                    &output.name,
                    cursor_mode_u32,
                )
            }
            PortalSourceSelection::Window(window) => {
                crate::compositor_client::CompositorClient::start_window(
                    &session_path,
                    window.node_id,
                    cursor_mode_u32,
                )
            }
        };

        match start_result {
            Ok(halley_api::PortalScreenCastResponse::Started {
                node_id: _,
                width,
                height,
                offset_x,
                offset_y,
                source_type,
                mapping_id,
                shm_path,
            }) => {
                // Create the PipeWire stream
                let pw = self.pipewire().ok_or_else(|| {
                    fdo::Error::Failed("PipeWire producer not initialized".into())
                })?;

                let (node_id, pipewire_serial) = pw
                    .create_stream(&session_path, width as u32, height as u32, &shm_path)
                    .map_err(|e| fdo::Error::Failed(format!("pipewire: {e}")))?;

                {
                    let mut sessions =
                        self.state.sessions.lock().map_err(|_| {
                            fdo::Error::Failed("session store lock poisoned".into())
                        })?;
                    if let Some(session) = sessions.get_mut(&session_path) {
                        session.started = true;
                    }
                }

                let mut stream_props = Vardict::new();
                stream_props.insert(
                    "position".into(),
                    owned_from_value(Value::from((offset_x, offset_y)))?,
                );
                stream_props.insert(
                    "size".into(),
                    owned_from_value(Value::from((width, height)))?,
                );
                stream_props.insert("source_type".into(), OwnedValue::from(source_type));
                stream_props.insert(
                    "mapping_id".into(),
                    owned_from_value(Value::from(mapping_id.clone()))?,
                );
                if let Some(serial) = pipewire_serial {
                    stream_props.insert("pipewire-serial".into(), OwnedValue::from(serial));
                }

                let streams = vec![(node_id, stream_props)];

                let mut results = Vardict::new();
                results.insert("streams".into(), owned_from_value(Value::from(streams))?);

                info!(
                    "Start: session {} streaming {} node_id={} serial={:?}",
                    session_path,
                    match &selection {
                        PortalSourceSelection::Monitor(o) => format!("output {}", o.name),
                        PortalSourceSelection::Window(w) => format!("window node {}", w.node_id),
                    },
                    node_id,
                    pipewire_serial
                );
                Ok((0, results))
            }
            Ok(other) => {
                warn!("Start: unexpected compositor response: {other:?}");
                Ok((2, Vardict::new()))
            }
            Err(e) => {
                warn!("Start: compositor error: {e}");
                Ok((2, Vardict::new()))
            }
        }
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        AVAILABLE_SOURCE_TYPES
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        AVAILABLE_CURSOR_MODES
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        SCREENCAST_VERSION
    }
}

// ---------------------------------------------------------------------------
// Screenshot interface
// ---------------------------------------------------------------------------

pub struct ScreenshotInterface {
    connection: Arc<Mutex<Option<Connection>>>,
}

impl ScreenshotInterface {
    pub fn new(connection: Arc<Mutex<Option<Connection>>>) -> Self {
        Self { connection }
    }
}

#[interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl ScreenshotInterface {
    fn screenshot(
        &self,
        handle: OwnedObjectPath,
        app_id: &str,
        _parent_window: &str,
        options: Vardict,
    ) -> fdo::Result<(u32, Vardict)> {
        export_request_object(&self.connection, &handle)?;

        let interactive = extract_bool(&options, "interactive").unwrap_or(false);
        let target = extract_u32(&options, "target");

        debug!(
            "Screenshot: app_id={} target={:?} interactive={}",
            app_id, target, interactive
        );

        let mode = match target {
            Some(SCREENSHOT_TARGET_SCREEN) => CaptureMode::Screen,
            Some(SCREENSHOT_TARGET_WINDOW) => CaptureMode::Window,
            Some(SCREENSHOT_TARGET_AREA) => CaptureMode::Region,
            Some(SCREENSHOT_TARGET_ACTIVE_WINDOW) => {
                warn!("Screenshot: active-window target not yet supported");
                return Ok((2, Vardict::new()));
            }
            None if interactive => CaptureMode::Menu,
            None => CaptureMode::Screen,
            Some(other) => {
                warn!("Screenshot: unsupported target {other}");
                return Ok((2, Vardict::new()));
            }
        };

        match CompositorClient::screenshot(mode) {
            Ok(ScreenshotOutcome::Saved(path)) => {
                let uri = path_to_file_uri(&path);
                let mut results = Vardict::new();
                results.insert("uri".into(), owned_from_value(Value::from(uri))?);
                debug!("Screenshot: captured for app_id={app_id}");
                Ok((0, results))
            }
            Ok(ScreenshotOutcome::Cancelled) => {
                debug!("Screenshot: user cancelled (app_id={app_id})");
                Ok((1, Vardict::new()))
            }
            Err(e) => {
                warn!("Screenshot: failed (app_id={app_id}): {e}");
                Ok((2, Vardict::new()))
            }
        }
    }

    fn pick_color(
        &self,
        handle: OwnedObjectPath,
        _app_id: &str,
        _parent_window: &str,
        _options: Vardict,
    ) -> fdo::Result<(u32, Vardict)> {
        export_request_object(&self.connection, &handle)?;
        debug!("PickColor: not supported");
        Ok((2, Vardict::new()))
    }

    #[zbus(property)]
    fn available_targets(&self) -> u32 {
        AVAILABLE_SCREENSHOT_TARGETS
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        SCREENSHOT_VERSION
    }
}

pub struct SessionInterface {
    session_handle: String,
    sessions: Arc<Mutex<SessionStore>>,
    pipewire: Option<Arc<PipewireProducer>>,
}

impl SessionInterface {
    pub fn new(
        session_handle: String,
        sessions: Arc<Mutex<SessionStore>>,
        pipewire: Option<Arc<PipewireProducer>>,
    ) -> Self {
        Self {
            session_handle,
            sessions,
            pipewire,
        }
    }
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionInterface {
    fn close(&self) -> fdo::Result<()> {
        debug!("Session.Close: {}", self.session_handle);
        if let Some(ref pw) = self.pipewire {
            pw.destroy_stream(&self.session_handle);
        }
        let _ = crate::compositor_client::CompositorClient::stop(&self.session_handle);
        if let Ok(mut store) = self.sessions.lock() {
            store.close(&self.session_handle);
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn closed(signal_emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

pub struct RequestInterface {
    handle: String,
}

impl RequestInterface {
    pub fn new(handle: String) -> Self {
        Self { handle }
    }
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl RequestInterface {
    fn close(&self) -> fdo::Result<()> {
        debug!("Request.Close: {}", self.handle);
        Ok(())
    }
}

fn export_session_object(
    connection: &Arc<Mutex<Option<Connection>>>,
    session_path: &OwnedObjectPath,
    sessions: Arc<Mutex<SessionStore>>,
    pipewire: Option<Arc<PipewireProducer>>,
) -> fdo::Result<()> {
    let guard = connection
        .lock()
        .map_err(|_| fdo::Error::Failed("connection lock poisoned".into()))?;
    let Some(ref conn) = *guard else {
        warn!("connection not yet set when exporting session object");
        return Ok(());
    };

    let iface = SessionInterface::new(session_path.to_string(), sessions, pipewire);
    match conn.object_server().at(session_path.clone(), iface) {
        Ok(_) => {
            debug!("exported Session object at {}", session_path);
            Ok(())
        }
        Err(e) => {
            warn!("failed to export Session at {}: {e}", session_path);
            Ok(())
        }
    }
}

fn export_request_object(
    connection: &Arc<Mutex<Option<Connection>>>,
    handle: &OwnedObjectPath,
) -> fdo::Result<()> {
    let guard = connection
        .lock()
        .map_err(|_| fdo::Error::Failed("connection lock poisoned".into()))?;
    let Some(ref conn) = *guard else {
        warn!("connection not yet set when exporting request object");
        return Ok(());
    };

    let iface = RequestInterface::new(handle.to_string());
    match conn.object_server().at(handle.clone(), iface) {
        Ok(_) => {
            debug!("exported Request object at {}", handle);
            Ok(())
        }
        Err(e) => {
            warn!("failed to export Request at {}: {e}", handle);
            Ok(())
        }
    }
}

fn extract_u32(dict: &Vardict, key: &str) -> Option<u32> {
    let ov = dict.get(key)?;
    match &**ov {
        Value::U32(v) => Some(*v),
        _ => None,
    }
}

fn extract_bool(dict: &Vardict, key: &str) -> Option<bool> {
    let ov = dict.get(key)?;
    match &**ov {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

/// Convert a filesystem path to a `file://` URI with percent-encoding for
/// characters outside the unreserved set.
fn path_to_file_uri(path: &str) -> String {
    let mut result = String::from("file://");
    for byte in path.bytes() {
        match byte {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
