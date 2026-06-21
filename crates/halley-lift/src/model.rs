use crate::mode::LiftMode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiftResultKind {
    App,
    Cluster,
    Node,
    Action,
    Config,
    CreateCluster,
    Term,
}

#[derive(Clone, Debug)]
pub enum LiftAction {
    LaunchApp { app_id: String },
    OpenCluster { id: u64 },
    FocusNode { id: u64 },
    CreateCluster,
    ReloadConfig,
    OpenConfig { path: String },
    RunInTerminal { command: String },
}

#[derive(Clone, Debug)]
pub struct LiftResult {
    pub section: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_name: Option<String>,
    pub kind: LiftResultKind,
    pub score: f64,
    pub is_field_pinned: bool,
    pub shortcut_hint: Option<String>,
    pub action: LiftAction,
}

#[derive(Clone, Debug, Default)]
pub struct ClusterDraft {
    pub app_ids: Vec<String>,
    pub running_node_ids: Vec<u64>,
}

impl ClusterDraft {
    pub fn count(&self) -> usize {
        self.app_ids.len() + self.running_node_ids.len()
    }

    pub fn toggle_result(&mut self, result: &LiftResult) -> bool {
        match &result.action {
            LiftAction::LaunchApp { app_id } => toggle_string(&mut self.app_ids, app_id),
            LiftAction::FocusNode { id } => toggle_u64(&mut self.running_node_ids, *id),
            _ => false,
        }
    }

    pub fn contains_result(&self, result: &LiftResult) -> bool {
        match &result.action {
            LiftAction::LaunchApp { app_id } => self.app_ids.iter().any(|id| id == app_id),
            LiftAction::FocusNode { id } => self.running_node_ids.contains(id),
            _ => false,
        }
    }
}

fn toggle_string(values: &mut Vec<String>, value: &str) -> bool {
    if let Some(index) = values.iter().position(|existing| existing == value) {
        values.remove(index);
    } else {
        values.push(value.to_string());
    }
    true
}

fn toggle_u64(values: &mut Vec<u64>, value: u64) -> bool {
    if let Some(index) = values.iter().position(|existing| *existing == value) {
        values.remove(index);
    } else {
        values.push(value);
    }
    true
}

pub fn mode_allows(mode: LiftMode, kind: &LiftResultKind) -> bool {
    match mode {
        LiftMode::General => true,
        LiftMode::Apps => matches!(kind, LiftResultKind::App),
        LiftMode::Clusters => matches!(
            kind,
            LiftResultKind::Cluster
                | LiftResultKind::CreateCluster
                | LiftResultKind::App
                | LiftResultKind::Node
        ),
        LiftMode::Nodes => matches!(kind, LiftResultKind::Node),
        LiftMode::Actions => matches!(kind, LiftResultKind::Action),
        LiftMode::Config => matches!(kind, LiftResultKind::Config),
        LiftMode::Term => matches!(kind, LiftResultKind::Term),
    }
}
