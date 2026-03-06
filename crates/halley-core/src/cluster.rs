use crate::field::NodeId;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClusterId(u64);

impl ClusterId {
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveLayoutMode {
    TiledWeighted,
    Stacked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClusterMode {
    Expanded,
    Collapsed,
    Active(ActiveLayoutMode),
}

#[derive(Clone, Debug)]
pub struct ActiveState {
    pub layout: ActiveLayoutMode,

    /// Per-member weights. Missing => default 1.0 in tiler.
    pub weights: HashMap<NodeId, f32>,

    /// If set, treat this as the chosen majors set (size <= MAX_MAJOR).
    /// This prevents auto-selection from “jumping” unless you clear it.
    pub majors_override: Option<Vec<NodeId>>,

    // Stacked mode state (used later)
    pub stack_index: usize,
    pub stack_scroll: f32,
}

impl ActiveState {
    pub fn new(layout: ActiveLayoutMode) -> Self {
        Self {
            layout,
            weights: HashMap::new(),
            majors_override: None,
            stack_index: 0,
            stack_scroll: 0.0,
        }
    }

    pub fn weight_of(&self, id: NodeId) -> f32 {
        self.weights.get(&id).copied().unwrap_or(1.0).max(0.0001)
    }

    pub fn bump_weight(&mut self, id: NodeId, delta: f32) {
        let w = self.weight_of(id);
        self.weights.insert(id, (w + delta).max(0.0001));
        // user intent => lock majors unless you want auto-jumps
        if self.majors_override.is_none() {
            // leave it None by default; you can set it externally if desired
        }
    }
}

/// A cluster is a group of window nodes (members).
/// When collapsed, a Core node represents the cluster as the handle.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub id: ClusterId,
    pub members: Vec<NodeId>,

    /// When collapsed, which Core node represents this cluster.
    pub core: Option<NodeId>,

    pub mode: ClusterMode,

    /// Present only when mode is Active(..). Keep it Some for convenience.
    pub active: Option<ActiveState>,
}

impl Cluster {
    pub fn new(id: ClusterId, members: Vec<NodeId>) -> Self {
        Self {
            id,
            members,
            core: None,
            mode: ClusterMode::Expanded,
            active: None,
        }
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.members.contains(&id)
    }

    pub fn core_node(&self) -> Option<NodeId> {
        self.core
    }

    pub fn is_collapsed(&self) -> bool {
        matches!(self.mode, ClusterMode::Collapsed)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.mode, ClusterMode::Active(_))
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.mode = if collapsed {
            ClusterMode::Collapsed
        } else {
            ClusterMode::Expanded
        };
        if !collapsed {
            self.active = None;
        }
    }

    pub fn enter_active(&mut self, layout: ActiveLayoutMode) {
        self.mode = ClusterMode::Active(layout);
        self.active = Some(ActiveState::new(layout));
    }

    pub fn set_active_layout(&mut self, layout: ActiveLayoutMode) {
        if let Some(a) = self.active.as_mut() {
            a.layout = layout;
        }
        self.mode = ClusterMode::Active(layout);
    }

    pub fn bump_weight(&mut self, id: NodeId, delta: f32) {
        if let Some(a) = self.active.as_mut() {
            a.bump_weight(id, delta);
        }
    }
}
