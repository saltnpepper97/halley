//! Accessibility information for compositor-owned surfaces.
//!
//! The adapter runs away from the compositor event loop: a stalled accessibility
//! bus must never stall input or rendering.
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, mpsc};

use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Live, Node, NodeId, Role,
    TreeId, TreeInfo, TreeUpdate,
};

use crate::session::{Session, SessionDriver};

const ROOT: NodeId = NodeId(0);
const ANNOUNCEMENT: NodeId = NodeId(2);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    role: Role,
    label: String,
    focused: bool,
    identity: u64,
    announcement: Option<String>,
}

impl Snapshot {
    fn desktop() -> Self {
        Self {
            role: Role::Window,
            label: "Halley".into(),
            focused: false,
            identity: 0,
            announcement: None,
        }
    }

    fn tree(&self) -> TreeUpdate {
        // A new selection must emit a focus change, including when two windows
        // happen to have the same title. Keep the identity stable otherwise.
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.label.hash(&mut hash);
        self.identity.hash(&mut hash);
        let content_id = NodeId(hash.finish() | 1);
        let mut root = Node::new(Role::Window);
        root.set_label("Halley");
        root.set_children(vec![content_id, ANNOUNCEMENT]);
        let mut content = Node::new(self.role);
        content.set_label(self.label.clone());
        let mut announcement = Node::new(Role::Label);
        announcement.set_live(Live::Polite);
        announcement.set_value(self.announcement.clone().unwrap_or_default());
        TreeUpdate {
            tree_id: TreeId::ROOT,
            nodes: vec![
                (ROOT, root),
                (content_id, content),
                (ANNOUNCEMENT, announcement),
            ],
            tree: Some(TreeInfo {
                root: ROOT,
                toolkit_name: Some("Halley".into()),
                toolkit_version: None,
            }),
            focus: if self.focused { content_id } else { ROOT },
        }
    }
}

#[derive(Clone)]
struct Handler(Arc<Mutex<Snapshot>>);

impl ActivationHandler for Handler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.lock().ok().map(|snapshot| snapshot.tree())
    }
}
impl ActionHandler for Handler {
    fn do_action(&mut self, _request: ActionRequest) {}
}
impl DeactivationHandler for Handler {
    fn deactivate_accessibility(&mut self) {}
}

pub struct AccessibilityTree {
    current: Arc<Mutex<Snapshot>>,
    sender: mpsc::SyncSender<()>,
    last: Snapshot,
}

impl AccessibilityTree {
    pub fn start() -> std::io::Result<Self> {
        let last = Snapshot::desktop();
        let current = Arc::new(Mutex::new(last.clone()));
        let handler = Handler(current.clone());
        let (sender, receiver) = mpsc::sync_channel::<()>(1);
        let worker_current = current.clone();
        std::thread::Builder::new()
            .name("halley-accessibility".into())
            .spawn(move || {
                let mut adapter =
                    accesskit_unix::Adapter::new(handler.clone(), handler.clone(), handler);
                while receiver.recv().is_ok() {
                    let snapshot = match worker_current.lock() {
                        Ok(current) => current.clone(),
                        Err(_) => break,
                    };
                    let focused = snapshot.focused;
                    adapter.update_if_active(|| snapshot.tree());
                    adapter.update_window_focus_state(focused);
                }
            })?;
        Ok(Self {
            current,
            sender,
            last,
        })
    }

    pub fn refresh<D: SessionDriver>(&mut self, session: &Session<D>) {
        let snapshot = snapshot(session);
        if snapshot == self.last {
            return;
        }
        if let Ok(mut current) = self.current.lock() {
            *current = snapshot.clone();
        }
        self.last = snapshot.clone();
        let _ = self.sender.try_send(());
    }
}

fn snapshot<D: SessionDriver>(session: &Session<D>) -> Snapshot {
    let mut state = overlay_snapshot(session);
    if !session.session_lock.active() {
        state.announcement = session
            .shell
            .overlays
            .accessibility_notification(crate::frame_clock::monotonic_now())
            .map(str::to_owned);
    }
    state
}

fn overlay_snapshot<D: SessionDriver>(session: &Session<D>) -> Snapshot {
    if session.session_lock.active() {
        return Snapshot::desktop();
    }
    if session.shell.overlays.exit_modal_active() {
        return Snapshot {
            role: Role::AlertDialog,
            label: "Quit Halley? Press Enter to quit or Escape to cancel.".into(),
            focused: true,
            identity: 0,
            announcement: None,
        };
    }
    if let Some(name) = session.shell.overlays.cluster_delete_name() {
        return Snapshot {
            role: Role::AlertDialog,
            label: format!(
                "Delete {}? Its windows will return to the Field. Applications will remain open. Press Enter to delete or Escape to cancel.",
                name.trim()
            ),
            focused: true,
            identity: 0,
            announcement: None,
        };
    }
    if let Some(creation) = session.clusters.creation() {
        let focused = session.shell.cluster_composer.focused();
        let label = if creation.naming {
            format!(
                "Cluster name: {}. Type a name, press Enter to confirm or Escape to go back.",
                creation.name_buffer
            )
        } else if let Some(id) = focused {
            let title = session
                .nodes
                .record(id)
                .map(|record| record.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or("Untitled window");
            format!(
                "Cluster composer, {title}, {}. {} selected. Use arrows to choose, Space to toggle, Enter to continue, Escape to cancel.",
                if creation.selected.contains(&id) {
                    "selected"
                } else {
                    "not selected"
                },
                creation.selected.len()
            )
        } else {
            format!(
                "Cluster composer, {} selected. Use arrows to choose windows, Space to toggle, Enter to continue, Escape to cancel.",
                creation.selected.len()
            )
        };
        return Snapshot {
            role: Role::Group,
            label,
            focused: true,
            identity: focused.map_or(0, |id| id.as_u64()),
            announcement: None,
        };
    }
    if session.capture.is_active() {
        use crate::capture::source_chooser::SourcePhase;
        let (phase, selected, source) = session.capture.source_accessibility();
        let label = match phase {
            Some(SourcePhase::Menu) => format!("Share source, {}. Use arrow keys to choose, Enter to select, Escape to cancel.", if selected == 0 { "monitor" } else { "window" }),
            Some(SourcePhase::MonitorPick) => match source {
                Some(halley_ipc::CaptureSource::Monitor { name, .. }) => format!("Share monitor {name}. Use arrow keys to choose, Enter to share, Escape to go back or cancel."),
                _ => "Share monitor. Use arrow keys to choose, Enter to share, Escape to go back or cancel.".into(),
            },
            Some(SourcePhase::WindowPick) => match source {
                Some(halley_ipc::CaptureSource::Window { window_id, .. }) => {
                    let record = session.nodes.record(halley_core::field::NodeId::new(*window_id));
                    let name = record.map(|record| record.title.as_str()).filter(|title| !title.is_empty()).unwrap_or("Untitled window");
                    format!("Share window {name}. Use arrow keys to choose, Enter to share, Escape to go back or cancel.")
                }
                _ => "Share window. Use arrow keys to choose, Enter to share, Escape to go back or cancel.".into(),
            },
            None => session.capture.screenshot_accessibility(&session.nodes, &session.wayland.space),
        };
        return Snapshot {
            role: Role::Group,
            label,
            focused: true,
            identity: match source {
                Some(halley_ipc::CaptureSource::Window { window_id, .. }) => *window_id,
                _ => 0,
            },
            announcement: None,
        };
    }
    if session.shell.apogee.accepts_input() {
        let label = session
            .shell
            .apogee
            .selected()
            .map(|id| {
                if let Some(record) = session.nodes.record(id) {
                    let title = if record.title.is_empty() {
                        "Untitled window"
                    } else {
                        &record.title
                    };
                    format!(
                        "Apogee, {title}, monitor {}{}",
                        record.output,
                        if record.collapsed { ", collapsed" } else { "" }
                    )
                } else if let Some(metadata) = session
                    .clusters
                    .cluster_for_core(id)
                    .and_then(|id| session.clusters.metadata(id))
                {
                    format!(
                        "Apogee, cluster {}, monitor {}",
                        metadata.name, metadata.output
                    )
                } else {
                    "Apogee, unknown item".into()
                }
            })
            .unwrap_or_else(|| {
                "Apogee, no selection. Use arrow keys to select, Enter to open or Escape to cancel."
                    .into()
            });
        return Snapshot {
            role: Role::Group,
            label,
            focused: true,
            identity: session.shell.apogee.selected().map_or(0, |id| id.as_u64()),
            announcement: None,
        };
    }
    if session.shell.focus_cycle.is_open() {
        let label = session
            .shell
            .focus_cycle
            .session()
            .and_then(|cycle| cycle.preview())
            .and_then(|id| session.nodes.record(id))
            .map(|record| {
                format!(
                    "Recent windows, {}",
                    if record.title.is_empty() {
                        "Untitled window"
                    } else {
                        &record.title
                    }
                )
            })
            .unwrap_or_else(|| "Recent windows".into());
        return Snapshot {
            role: Role::Group,
            label,
            focused: true,
            identity: session
                .shell
                .focus_cycle
                .session()
                .and_then(|cycle| cycle.preview())
                .map_or(0, |id| id.as_u64()),
            announcement: None,
        };
    }
    if let Some(modifier) = session.shell.overlays.basics_modifier() {
        use crate::render::overlays::basics::{FOOTER, MODEL, MODEL_SUBTEXT, TITLE, chord_entries};
        let rows = chord_entries(modifier)
            .into_iter()
            .map(|(key, description)| format!("{key}: {description}"))
            .collect::<Vec<_>>()
            .join(". ");
        return Snapshot {
            role: Role::Group,
            label: format!("{TITLE}. {MODEL} {MODEL_SUBTEXT} {rows}. {FOOTER}"),
            focused: true,
            identity: 0,
            announcement: None,
        };
    }
    Snapshot::desktop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_focus_tracks_overlay_visibility() {
        let desktop = Snapshot::desktop();
        assert_eq!(desktop.tree().focus, ROOT);
        let overlay = Snapshot {
            role: Role::Group,
            label: "Apogee".into(),
            focused: true,
            identity: 10,
            announcement: None,
        };
        assert_ne!(overlay.tree().focus, ROOT);
        assert_eq!(overlay.tree().nodes.len(), 3);
        let another = Snapshot {
            identity: 11,
            ..overlay.clone()
        };
        assert_ne!(overlay.tree().focus, another.tree().focus);
        let with_notice = Snapshot {
            announcement: Some("Configuration failed".into()),
            ..overlay.clone()
        };
        assert_eq!(overlay.tree().focus, with_notice.tree().focus);
    }
}
