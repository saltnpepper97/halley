use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::fdo;
use zbus::interface;
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const VERSION: u32 = 6;
const AVAILABLE_SOURCE_TYPES: u32 = halley_ipc::SOURCE_MONITOR | halley_ipc::SOURCE_WINDOW;
const CURSOR_HIDDEN: u32 = 1;
const CURSOR_EMBEDDED: u32 = 2;
const CURSOR_METADATA: u32 = 4;
const AVAILABLE_CURSOR_MODES: u32 = CURSOR_HIDDEN | CURSOR_EMBEDDED | CURSOR_METADATA;

type Vardict = HashMap<String, OwnedValue>;

#[derive(Clone)]
pub struct ScreenCastInterface {
    connection: Connection,
    sessions: Arc<Mutex<crate::session::SessionStore>>,
    producer: Arc<crate::pipewire::Producer>,
}

impl ScreenCastInterface {
    pub fn new(connection: Connection, producer: Arc<crate::pipewire::Producer>) -> Self {
        Self {
            connection,
            sessions: Arc::new(Mutex::new(crate::session::SessionStore::default())),
            producer,
        }
    }
    fn require_session(
        &self,
        handle: &OwnedObjectPath,
        owner: &str,
        app_id: &str,
    ) -> fdo::Result<()> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| fdo::Error::Failed("session lock poisoned".into()))?;
        let session = sessions
            .get(handle.as_str())
            .ok_or_else(|| fdo::Error::InvalidArgs("unknown session".into()))?;
        crate::auth::same_owner(owner, &session.owner)?;
        if session.app_id != app_id {
            return Err(fdo::Error::AccessDenied(
                "session belongs to another application".into(),
            ));
        }
        Ok(())
    }
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastInterface {
    async fn create_session(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        _options: Vardict,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(u32, Vardict)> {
        let this = self.clone();
        let sender = header.sender().map(|name| name.to_owned());
        let app_id = app_id.to_owned();
        // zbus dispatches even synchronous interface methods on its async executor.
        // Keep IPC waits and object cleanup off that executor.
        blocking::unblock(move || {
            let owner = crate::auth::sender(sender.as_ref())?;
            let _request = export_request(&this.connection, &handle, owner.as_str())?;
            let session_path = session_handle.to_string();
            let session = this
                .sessions
                .lock()
                .map_err(|_| fdo::Error::Failed("session store lock poisoned".to_string()))?
                .create(session_path.clone(), owner.to_string(), app_id.to_string())
                .map_err(fdo::Error::InvalidArgs)?;
            this.connection.object_server().at(
                session_handle,
                SessionInterface {
                    owner: owner.to_string(),
                    connection: this.connection.clone(),
                    handle: session_path,
                    sessions: this.sessions.clone(),
                    producer: this.producer.clone(),
                },
            )?;
            let mut results = Vardict::new();
            results.insert("session_id".to_string(), owned(Value::from(session.id))?);
            Ok((0, results))
        })
        .await
    }

    async fn select_sources(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        options: Vardict,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(u32, Vardict)> {
        let this = self.clone();
        let sender = header.sender().map(|name| name.to_owned());
        let app_id = app_id.to_owned();
        // zbus dispatches even synchronous interface methods on its async executor.
        // Keep IPC waits and object cleanup off that executor.
        blocking::unblock(move || {
            let owner = crate::auth::sender(sender.as_ref())?;
            let _request = export_request(&this.connection, &handle, owner.as_str())?;
            this.require_session(&session_handle, owner.as_str(), &app_id)?;
            let source_types = extract_u32(&options, "types").unwrap_or(halley_ipc::SOURCE_MONITOR);
            let supported = source_types & AVAILABLE_SOURCE_TYPES;
            if supported == 0 || source_types & !AVAILABLE_SOURCE_TYPES != 0 {
                return Ok((2, Vardict::new()));
            }
            let cursor_mode = match extract_u32(&options, "cursor_mode").unwrap_or(CURSOR_HIDDEN) {
                CURSOR_HIDDEN => halley_ipc::CursorMode::Hidden,
                CURSOR_EMBEDDED => halley_ipc::CursorMode::Embedded,
                CURSOR_METADATA => halley_ipc::CursorMode::Metadata,
                _ => return Ok((2, Vardict::new())),
            };
            let session_path = session_handle.to_string();
            {
                let mut sessions = this
                    .sessions
                    .lock()
                    .map_err(|_| fdo::Error::Failed("session store lock poisoned".to_string()))?;
                let Some(session) = sessions.get_mut(&session_path) else {
                    return Ok((2, Vardict::new()));
                };
                // A session owns at most one outstanding selection. Retain its
                // request handle so closing the session also dismisses consent.
                if session.pending_source.is_some() {
                    return Ok((2, Vardict::new()));
                }
                session.pending_source = Some(handle.to_string());
            }
            let response = crate::compositor::choose_source(handle.to_string(), supported);
            {
                let mut sessions = this
                    .sessions
                    .lock()
                    .map_err(|_| fdo::Error::Failed("session store lock poisoned".to_string()))?;
                if let Some(session) = sessions.get_mut(&session_path)
                    && session.pending_source.as_deref() == Some(handle.as_str())
                {
                    session.pending_source = None;
                }
            }
            match response {
                Ok(halley_ipc::SourceChooserResponse::Selected(source)) => {
                    let mut sessions = this.sessions.lock().map_err(|_| {
                        fdo::Error::Failed("session store lock poisoned".to_string())
                    })?;
                    let Some(session) = sessions.get_mut(&session_path) else {
                        return Ok((2, Vardict::new()));
                    };
                    session.selected = Some(source);
                    session.cursor_mode = cursor_mode;
                    Ok((0, Vardict::new()))
                }
                Ok(halley_ipc::SourceChooserResponse::Cancelled) => Ok((1, Vardict::new())),
                Ok(halley_ipc::SourceChooserResponse::Failed { message }) | Err(message) => {
                    eventline::warn!("source chooser failed: {message}");
                    Ok((2, Vardict::new()))
                }
            }
        })
        .await
    }

    async fn start(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: &str,
        _parent_window: &str,
        _options: Vardict,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(u32, Vardict)> {
        let this = self.clone();
        let sender = header.sender().map(|name| name.to_owned());
        let app_id = app_id.to_owned();
        // zbus dispatches even synchronous interface methods on its async executor.
        // Keep IPC waits and object cleanup off that executor.
        blocking::unblock(move || {
            let owner = crate::auth::sender(sender.as_ref())?;
            let _request = export_request(&this.connection, &handle, owner.as_str())?;
            let session_path = session_handle.to_string();
            this.require_session(&session_handle, owner.as_str(), &app_id)?;
            let (source, cursor_mode) = {
                let sessions = this
                    .sessions
                    .lock()
                    .map_err(|_| fdo::Error::Failed("session store lock poisoned".to_string()))?;
                let Some(session) = sessions.get(&session_path) else {
                    return Ok((2, Vardict::new()));
                };
                let Some(source) = session.selected.clone() else {
                    return Ok((2, Vardict::new()));
                };
                (source, session.cursor_mode)
            };
            let (node, serial) =
                match this
                    .producer
                    .create_stream(&session_path, source.clone(), cursor_mode)
                {
                    Ok(stream) => stream,
                    Err(err) => {
                        eventline::warn!("could not start PipeWire stream: {err}");
                        return Ok((2, Vardict::new()));
                    }
                };
            let mut properties = Vardict::new();
            properties.insert(
                "size".to_string(),
                owned(Value::from((source.width(), source.height())))?,
            );
            match &source {
                halley_ipc::CaptureSource::Monitor { name, x, y, .. } => {
                    properties.insert("position".to_string(), owned(Value::from((*x, *y)))?);
                    properties.insert(
                        "source_type".to_string(),
                        OwnedValue::from(halley_ipc::SOURCE_MONITOR),
                    );
                    properties.insert("mapping_id".to_string(), owned(Value::from(name.clone()))?);
                }
                halley_ipc::CaptureSource::Window { window_id, .. } => {
                    properties.insert(
                        "source_type".to_string(),
                        OwnedValue::from(halley_ipc::SOURCE_WINDOW),
                    );
                    properties.insert(
                        "mapping_id".to_string(),
                        owned(Value::from(format!("halley-window-{window_id}")))?,
                    );
                }
            }
            if let Some(serial) = serial {
                properties.insert("pipewire-serial".to_string(), OwnedValue::from(serial));
            }
            let streams = vec![(node, properties)];
            let mut results = Vardict::new();
            results.insert("streams".to_string(), owned(Value::from(streams))?);
            Ok((0, results))
        })
        .await
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
        VERSION
    }
}

#[derive(Clone)]
struct SessionInterface {
    owner: String,
    connection: Connection,
    handle: String,
    sessions: Arc<Mutex<crate::session::SessionStore>>,
    producer: Arc<crate::pipewire::Producer>,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionInterface {
    async fn close(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let this = self.clone();
        let sender = header.sender().map(|name| name.to_owned());
        // zbus dispatches even synchronous interface methods on its async executor.
        // Keep IPC waits and object cleanup off that executor.
        blocking::unblock(move || {
            crate::auth::same_owner(
                sender.as_ref().map(|name| name.as_str()).unwrap_or(""),
                &this.owner,
            )?;
            // Invalidate the session before dismissing the chooser. A reply
            // racing with Close must never revive consent or start a stream.
            let session = this
                .sessions
                .lock()
                .map_err(|_| fdo::Error::Failed("session store lock poisoned".to_string()))?
                .remove(&this.handle);
            if let Some(request_handle) = session.and_then(|session| session.pending_source) {
                let _ = crate::compositor::cancel_source(request_handle);
            }
            this.producer.destroy_stream(&this.handle);
            let _ = this
                .connection
                .object_server()
                .remove::<SessionInterface, _>(this.handle.as_str());
            Ok(())
        })
        .await
    }
}

#[derive(Clone)]
struct RequestInterface {
    owner: String,
    handle: String,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl RequestInterface {
    async fn close(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let this = self.clone();
        let sender = header.sender().map(|name| name.to_owned());
        // zbus dispatches even synchronous interface methods on its async executor.
        // Keep IPC waits and object cleanup off that executor.
        blocking::unblock(move || {
            crate::auth::same_owner(
                sender.as_ref().map(|name| name.as_str()).unwrap_or(""),
                &this.owner,
            )?;
            let _ = crate::compositor::cancel_source(this.handle.clone());
            Ok(())
        })
        .await
    }
}

struct RequestGuard {
    connection: Connection,
    handle: OwnedObjectPath,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let _ = self
            .connection
            .object_server()
            .remove::<RequestInterface, _>(self.handle.clone());
    }
}
fn export_request(
    connection: &Connection,
    handle: &OwnedObjectPath,
    owner: &str,
) -> fdo::Result<RequestGuard> {
    if !connection.object_server().at(
        handle.clone(),
        RequestInterface {
            handle: handle.to_string(),
            owner: owner.into(),
        },
    )? {
        return Err(fdo::Error::InvalidArgs("request already exists".into()));
    }
    Ok(RequestGuard {
        connection: connection.clone(),
        handle: handle.clone(),
    })
}

fn extract_u32(dict: &Vardict, key: &str) -> Option<u32> {
    match &**dict.get(key)? {
        Value::U32(value) => Some(*value),
        _ => None,
    }
}

fn owned(value: Value<'_>) -> fdo::Result<OwnedValue> {
    OwnedValue::try_from(value).map_err(|err| fdo::Error::Failed(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_capabilities_are_exact() {
        assert_eq!(
            AVAILABLE_SOURCE_TYPES,
            halley_ipc::SOURCE_MONITOR | halley_ipc::SOURCE_WINDOW
        );
        assert_eq!(
            AVAILABLE_CURSOR_MODES,
            CURSOR_HIDDEN | CURSOR_EMBEDDED | CURSOR_METADATA
        );
    }
}
