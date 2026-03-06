use crate::field::{Field, NodeId};

/// Current interaction target (separate from history).
/// Focus only applies to nodes that are present and experience-visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    focused: Option<NodeId>,
}

impl Focus {
    pub fn new() -> Self {
        Self { focused: None }
    }

    pub fn current(&self) -> Option<NodeId> {
        self.focused
    }

    pub fn is_focused(&self, id: NodeId) -> bool {
        self.focused == Some(id)
    }

    /// Set focus to `id` if it exists and is experience-visible.
    pub fn set(&mut self, field: &Field, id: NodeId) -> bool {
        if !field.is_visible(id) {
            return false;
        }
        self.focused = Some(id);
        true
    }

    pub fn clear(&mut self) {
        self.focused = None;
    }

    /// If the focused node is removed, clear focus.
    pub fn on_removed(&mut self, removed: NodeId) {
        if self.focused == Some(removed) {
            self.focused = None;
        }
    }

    /// If the focused node becomes hidden (collapse/hide/detach), clear focus.
    pub fn on_hidden(&mut self, field: &Field, id: NodeId) {
        if self.focused == Some(id) && !field.is_visible(id) {
            self.focused = None;
        }
    }
}

impl Default for Focus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Vec2;

    #[test]
    fn starts_empty() {
        let f = Focus::new();
        assert_eq!(f.current(), None);
    }

    #[test]
    fn can_focus_existing_node() {
        let mut field = Field::new();
        let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut focus = Focus::new();
        assert!(focus.set(&field, id));
        assert_eq!(focus.current(), Some(id));
        assert!(focus.is_focused(id));
    }

    #[test]
    fn cannot_focus_missing_node() {
        let field = Field::new();
        let mut focus = Focus::new();
        assert!(!focus.set(&field, NodeId::new(999)));
        assert_eq!(focus.current(), None);
    }

    #[test]
    fn cannot_focus_hidden_node() {
        let mut field = Field::new();
        let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(field.set_hidden(id, true));

        let mut focus = Focus::new();
        assert!(!focus.set(&field, id));
        assert_eq!(focus.current(), None);
    }

    #[test]
    fn clears_when_focused_node_removed() {
        let mut field = Field::new();
        let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut focus = Focus::new();
        assert!(focus.set(&field, id));

        field.remove(id);
        focus.on_removed(id);

        assert_eq!(focus.current(), None);
    }

    #[test]
    fn clears_when_focused_node_becomes_hidden() {
        let mut field = Field::new();
        let id = field.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let mut focus = Focus::new();
        assert!(focus.set(&field, id));

        assert!(field.set_hidden(id, true));
        focus.on_hidden(&field, id);

        assert_eq!(focus.current(), None);
    }
}
