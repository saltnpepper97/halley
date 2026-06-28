use std::collections::HashMap;

use halley_api::PortalSourceSelection;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum CursorMode {
    #[default]
    Hidden,
    Embedded,
    Metadata,
}

impl CursorMode {
    pub fn from_mask(mask: u32) -> Self {
        if mask & 2 != 0 {
            Self::Embedded
        } else if mask & 4 != 0 {
            Self::Metadata
        } else {
            Self::Hidden
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            Self::Hidden => 1,
            Self::Embedded => 2,
            Self::Metadata => 4,
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct PortalSession {
    pub session_handle: String,
    pub session_id: String,
    pub app_id: Option<String>,
    pub selected_output: Option<String>,
    pub cursor_mode: CursorMode,
    pub source_types: u32,
    /// Source picked from the chooser overlay (monitor or window). Drives how
    /// `Start` streams.
    pub selected_source: Option<PortalSourceSelection>,
    pub started: bool,
    pub closed: bool,
}

#[derive(Default)]
pub struct SessionStore {
    sessions: HashMap<String, PortalSession>,
    next_id: u64,
}

impl SessionStore {
    pub fn create(&mut self, session_handle: String, app_id: Option<String>) -> PortalSession {
        self.next_id = self.next_id.wrapping_add(1);
        let session_id = format!("halley{}", self.next_id);
        let session = PortalSession {
            session_handle: session_handle.clone(),
            session_id,
            app_id,
            selected_output: None,
            cursor_mode: CursorMode::Hidden,
            source_types: 1,
            selected_source: None,
            started: false,
            closed: false,
        };
        self.sessions.insert(session_handle, session.clone());
        session
    }

    pub fn get(&self, handle: &str) -> Option<&PortalSession> {
        self.sessions.get(handle)
    }

    pub fn get_mut(&mut self, handle: &str) -> Option<&mut PortalSession> {
        self.sessions.get_mut(handle)
    }

    pub fn close(&mut self, handle: &str) {
        if let Some(session) = self.sessions.get_mut(handle) {
            session.closed = true;
            session.started = false;
        }
        self.sessions.remove(handle);
    }
}
