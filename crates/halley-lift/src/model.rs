use crate::mode::LensMode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LensResultKind {
    App,
    Cluster,
    Node,
    Action,
    Config,
    CreateCluster,
}

#[derive(Clone, Debug)]
pub enum LensAction {
    LaunchApp { app_id: String },
    OpenCluster { id: u64 },
    FocusNode { id: u64 },
    CreateCluster,
    ReloadConfig,
    OpenPath { path: String },
}

#[derive(Clone, Debug)]
pub struct LensResult {
    pub section: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon_name: Option<String>,
    pub kind: LensResultKind,
    pub score: f64,
    pub is_field_pinned: bool,
    pub shortcut_hint: Option<String>,
    pub action: LensAction,
}

#[derive(Clone, Debug, Default)]
pub struct ClusterDraft {
    pub name_hint: Option<String>,
    pub app_ids: Vec<String>,
    pub running_node_ids: Vec<u64>,
}

impl ClusterDraft {
    pub fn count(&self) -> usize {
        self.app_ids.len() + self.running_node_ids.len()
    }

    pub fn toggle_result(&mut self, result: &LensResult) -> bool {
        match &result.action {
            LensAction::LaunchApp { app_id } => toggle_string(&mut self.app_ids, app_id),
            LensAction::FocusNode { id } => toggle_u64(&mut self.running_node_ids, *id),
            _ => false,
        }
    }

    pub fn contains_result(&self, result: &LensResult) -> bool {
        match &result.action {
            LensAction::LaunchApp { app_id } => self.app_ids.iter().any(|id| id == app_id),
            LensAction::FocusNode { id } => self.running_node_ids.contains(id),
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

pub fn mode_allows(mode: LensMode, kind: &LensResultKind) -> bool {
    match mode {
        LensMode::General => true,
        LensMode::Apps => matches!(kind, LensResultKind::App),
        LensMode::Clusters => matches!(
            kind,
            LensResultKind::Cluster
                | LensResultKind::CreateCluster
                | LensResultKind::App
                | LensResultKind::Node
        ),
        LensMode::Nodes => matches!(kind, LensResultKind::Node),
        LensMode::Actions => matches!(kind, LensResultKind::Action),
        LensMode::Config => matches!(kind, LensResultKind::Config),
    }
}
