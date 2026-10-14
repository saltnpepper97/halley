use std::collections::HashMap;

#[derive(Clone)]
pub struct PortalSession {
    pub id: String,
    pub owner: String,
    pub app_id: String,
    pub selected: Option<halley_ipc::CaptureSource>,
    pub pending_source: Option<String>,
    pub cursor_mode: halley_ipc::CursorMode,
}

#[derive(Default)]
pub struct SessionStore {
    sessions: HashMap<String, PortalSession>,
    next_id: u64,
}

impl SessionStore {
    pub fn create(
        &mut self,
        handle: String,
        owner: String,
        app_id: String,
    ) -> Result<PortalSession, String> {
        if self.sessions.contains_key(&handle) || self.sessions.len() >= 64 {
            return Err("session already exists or session limit reached".into());
        }
        self.next_id = self.next_id.wrapping_add(1);
        let session = PortalSession {
            owner,
            app_id,
            id: format!("halley{}", self.next_id),
            selected: None,
            pending_source: None,
            cursor_mode: halley_ipc::CursorMode::Hidden,
        };
        self.sessions.insert(handle, session.clone());
        Ok(session)
    }

    pub fn get(&self, handle: &str) -> Option<&PortalSession> {
        self.sessions.get(handle)
    }

    pub fn get_mut(&mut self, handle: &str) -> Option<&mut PortalSession> {
        self.sessions.get_mut(handle)
    }

    pub fn remove(&mut self, handle: &str) -> Option<PortalSession> {
        self.sessions.remove(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_retain_identity_and_cannot_be_overwritten() {
        let mut sessions = SessionStore::default();
        sessions
            .create("/session/one".into(), ":1.2".into(), "app.a".into())
            .unwrap();
        assert!(
            sessions
                .create("/session/one".into(), ":1.3".into(), "app.b".into())
                .is_err()
        );
        let session = sessions.get("/session/one").unwrap();
        assert_eq!(session.owner, ":1.2");
        assert_eq!(session.app_id, "app.a");
    }
    #[test]
    fn session_limit_is_released_on_close() {
        let mut sessions = SessionStore::default();
        for index in 0..64 {
            sessions
                .create(format!("/session/{index}"), ":1.2".into(), "app".into())
                .unwrap();
        }
        assert!(
            sessions
                .create("/session/full".into(), ":1.2".into(), "app".into())
                .is_err()
        );
        sessions.remove("/session/0");
        assert!(
            sessions
                .create("/session/new".into(), ":1.2".into(), "app".into())
                .is_ok()
        );
    }
}
