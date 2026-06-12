use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use halley_api::{
    ApiError, ClusterDraftAppLaunch, ClusterDraftRequest, ClusterDraftSource, ClusterRequest,
    ClusterTarget, CompositorRequest, NodeKind, NodeRequest, NodeSelector, Request, Response,
};

use crate::config::{LensConfig, default_config_path};
use crate::mode::LensMode;
use crate::model::{ClusterDraft, LensAction, LensResult, LensResultKind, mode_allows};

#[derive(Clone, Debug)]
pub struct SearchContext {
    pub mode: LensMode,
    pub query: String,
    pub query_lower: String,
    pub max_results: usize,
    pub draft_count: usize,
}

#[derive(Debug, Default)]
pub struct ProviderIndex {
    apps: Vec<DesktopApp>,
    nodes: Vec<CachedNode>,
    clusters: Vec<CachedCluster>,
    live_loaded: bool,
    live_rx: Option<Receiver<(Vec<CachedNode>, Vec<CachedCluster>)>>,
    terminal: String,
}

#[derive(Clone, Debug)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub icon_name: Option<String>,
    pub exec: String,
    pub terminal: bool,
    search_text: String,
}

#[derive(Clone, Debug)]
struct CachedNode {
    id: u64,
    title: String,
    subtitle: String,
    search_text: String,
    pinned: bool,
}

#[derive(Clone, Debug)]
struct CachedCluster {
    id: u64,
    title: String,
    subtitle: String,
    search_text: String,
}

impl ProviderIndex {
    pub fn load(config: &LensConfig) -> Self {
        Self {
            apps: load_desktop_apps(),
            nodes: Vec::new(),
            clusters: Vec::new(),
            live_loaded: false,
            live_rx: None,
            terminal: config.terminal.trim().to_string(),
        }
    }

    pub fn needs_live_refresh(&self) -> bool {
        !self.live_loaded && self.live_rx.is_none()
    }

    pub fn has_pending_live_refresh(&self) -> bool {
        self.live_rx.is_some()
    }

    pub fn start_live_refresh(&mut self) {
        if !self.needs_live_refresh() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send((load_nodes(), load_clusters()));
        });
        self.live_rx = Some(rx);
    }

    pub fn finish_live_refresh_if_ready(&mut self) -> Option<(usize, usize)> {
        let rx = self.live_rx.as_ref()?;
        let Ok((nodes, clusters)) = rx.try_recv() else {
            return None;
        };
        self.nodes = nodes;
        self.clusters = clusters;
        self.live_loaded = true;
        self.live_rx = None;
        Some((self.nodes.len(), self.clusters.len()))
    }

    pub fn search(&self, ctx: &SearchContext) -> Vec<LensResult> {
        let mut results = Vec::new();
        if matches!(
            ctx.mode,
            LensMode::General | LensMode::Apps | LensMode::Clusters
        ) {
            results.extend(self.search_apps(ctx));
        }
        if matches!(
            ctx.mode,
            LensMode::General | LensMode::Nodes | LensMode::Clusters
        ) {
            results.extend(self.search_nodes(ctx));
        }
        if matches!(ctx.mode, LensMode::General | LensMode::Clusters) {
            results.extend(self.search_clusters(ctx));
        }
        if matches!(ctx.mode, LensMode::General | LensMode::Actions) {
            results.extend(search_actions(ctx));
        }
        if matches!(ctx.mode, LensMode::General | LensMode::Config) {
            results.extend(search_config(ctx));
        }

        if ctx.mode == LensMode::Clusters && ctx.draft_count > 0 {
            results.push(create_cluster_result(ctx.query.as_str()));
        }

        results.retain(|result| mode_allows(ctx.mode, &result.kind));
        results.sort_by(|a, b| {
            b.is_field_pinned
                .cmp(&a.is_field_pinned)
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.section.cmp(&b.section))
                .then_with(|| a.title.cmp(&b.title))
        });
        let max_results = if matches!(ctx.mode, LensMode::Apps | LensMode::Clusters)
            && ctx.query_lower.is_empty()
        {
            usize::MAX
        } else {
            ctx.max_results
        };
        if results.len() > max_results {
            results.truncate(max_results);
        }
        results
    }

    fn search_apps(&self, ctx: &SearchContext) -> Vec<LensResult> {
        self.apps
            .iter()
            .filter_map(|app| {
                let score = match_score(ctx.query_lower.as_str(), app.search_text.as_str())?;
                Some(LensResult {
                    section: if ctx.mode == LensMode::Clusters {
                        "Apps"
                    } else {
                        "Applications"
                    }
                    .into(),
                    title: app.name.clone(),
                    subtitle: Some(app.comment.clone().unwrap_or_else(|| "Application".into())),
                    icon_name: app.icon_name.clone(),
                    kind: LensResultKind::App,
                    score,
                    is_field_pinned: false,
                    shortcut_hint: Some(
                        if ctx.mode == LensMode::Clusters {
                            "Space stage"
                        } else {
                            "Enter launch"
                        }
                        .into(),
                    ),
                    action: LensAction::LaunchApp {
                        app_id: app.id.clone(),
                    },
                })
            })
            .collect()
    }

    fn search_nodes(&self, ctx: &SearchContext) -> Vec<LensResult> {
        self.nodes
            .iter()
            .filter_map(|node| {
                let mut score = match_score(ctx.query_lower.as_str(), node.search_text.as_str())?;
                if node.pinned {
                    score += 1000.0;
                }
                Some(LensResult {
                    section: if ctx.mode == LensMode::Clusters {
                        "Running Nodes"
                    } else {
                        "Nodes"
                    }
                    .into(),
                    title: node.title.clone(),
                    subtitle: Some(node.subtitle.clone()),
                    icon_name: None,
                    kind: LensResultKind::Node,
                    score,
                    is_field_pinned: node.pinned,
                    shortcut_hint: Some(
                        if ctx.mode == LensMode::Clusters {
                            "Space stage"
                        } else {
                            "Enter open"
                        }
                        .into(),
                    ),
                    action: LensAction::FocusNode { id: node.id },
                })
            })
            .collect()
    }

    fn search_clusters(&self, ctx: &SearchContext) -> Vec<LensResult> {
        self.clusters
            .iter()
            .filter_map(|cluster| {
                let score = match_score(ctx.query_lower.as_str(), cluster.search_text.as_str())?;
                Some(LensResult {
                    section: "Existing Clusters".into(),
                    title: cluster.title.clone(),
                    subtitle: Some(cluster.subtitle.clone()),
                    icon_name: None,
                    kind: LensResultKind::Cluster,
                    score: score + 20.0,
                    is_field_pinned: false,
                    shortcut_hint: Some("Enter open".into()),
                    action: LensAction::OpenCluster { id: cluster.id },
                })
            })
            .collect()
    }

    pub fn launch_app(&self, app_id: &str) -> Result<(), String> {
        let app = self
            .apps
            .iter()
            .find(|app| app.id == app_id)
            .ok_or_else(|| format!("app `{app_id}` not found"))?;
        launch_exec(app.exec.as_str(), app.terminal, self.terminal.as_str())
    }

    fn draft_app_launches(&self, app_ids: &[String]) -> Vec<ClusterDraftAppLaunch> {
        app_ids
            .iter()
            .filter_map(|app_id| {
                let app = self.apps.iter().find(|app| app.id == *app_id)?;
                Some(ClusterDraftAppLaunch {
                    app_id: app.id.clone(),
                    command: app_launch_command(app, self.terminal.as_str()),
                })
            })
            .collect()
    }
}

fn load_nodes() -> Vec<CachedNode> {
    let Ok(Response::NodeList(list)) =
        halley_ipc::send_request(&Request::Node(NodeRequest::List { output: None }))
    else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    for group in list.outputs {
        let output = group.output;
        for node in group.nodes {
            if node.kind != NodeKind::Surface || !node.visible {
                continue;
            }
            let title = node.title;
            let app_id = node.app_id.unwrap_or_default();
            let app_label = if app_id.is_empty() {
                "window"
            } else {
                app_id.as_str()
            };
            let search_text = format!("{title} {app_id} {output}").to_ascii_lowercase();
            nodes.push(CachedNode {
                id: node.id,
                title,
                subtitle: format!("{app_label} on {output}"),
                search_text,
                pinned: node.pinned,
            });
        }
    }
    nodes
}

fn load_clusters() -> Vec<CachedCluster> {
    let Ok(Response::ClusterList(list)) =
        halley_ipc::send_request(&Request::Cluster(ClusterRequest::List { output: None }))
    else {
        return Vec::new();
    };
    let mut clusters = Vec::new();
    for group in list.outputs {
        let output = group.output;
        for cluster in group.clusters {
            let title = cluster
                .name
                .unwrap_or_else(|| format!("Cluster {}", cluster.id));
            let slot = cluster.slot.map(|s| s.to_string()).unwrap_or_default();
            let search_text = format!("{title} {slot} {output}").to_ascii_lowercase();
            clusters.push(CachedCluster {
                id: cluster.id,
                title,
                subtitle: format!("{} members on {}", cluster.member_count, output),
                search_text,
            });
        }
    }
    clusters
}

fn create_cluster_result(query: &str) -> LensResult {
    let title = if query.trim().is_empty() {
        "Create cluster".into()
    } else {
        format!("Create cluster: {}", query.trim())
    };
    LensResult {
        section: "Create".into(),
        title,
        subtitle: Some("Open Cluster Finalize popup".into()),
        icon_name: None,
        kind: LensResultKind::CreateCluster,
        score: 0.0,
        is_field_pinned: false,
        shortcut_hint: Some("Ctrl+Enter".into()),
        action: LensAction::CreateCluster,
    }
}

fn search_actions(ctx: &SearchContext) -> Vec<LensResult> {
    let actions = [
        (
            "reload-config",
            "Reload Halley config",
            "Compositor action",
            LensAction::ReloadConfig,
        ),
        (
            "open-lens-config",
            "Open Lens config",
            "Config file",
            LensAction::OpenPath {
                path: default_config_path().display().to_string(),
            },
        ),
    ];
    actions
        .into_iter()
        .filter_map(|(_id, title, subtitle, action)| {
            match_score(
                ctx.query_lower.as_str(),
                title.to_ascii_lowercase().as_str(),
            )
            .map(|score| LensResult {
                section: "Actions".into(),
                title: title.into(),
                subtitle: Some(subtitle.into()),
                icon_name: None,
                kind: LensResultKind::Action,
                score,
                is_field_pinned: false,
                shortcut_hint: Some("Enter".into()),
                action,
            })
        })
        .collect()
}

fn search_config(ctx: &SearchContext) -> Vec<LensResult> {
    let path = default_config_path();
    match_score(ctx.query_lower.as_str(), "lens config").map_or_else(Vec::new, |score| {
        vec![LensResult {
            section: "Config".into(),
            title: "Lens config".into(),
            subtitle: Some(path.display().to_string()),
            icon_name: None,
            kind: LensResultKind::Config,
            score,
            is_field_pinned: false,
            shortcut_hint: Some("Enter open".into()),
            action: LensAction::OpenPath {
                path: path.display().to_string(),
            },
        }]
    })
}

pub fn activate_result(index: &ProviderIndex, result: &LensResult) -> Result<(), String> {
    match &result.action {
        LensAction::LaunchApp { app_id } => index.launch_app(app_id),
        LensAction::OpenCluster { id } => expect_ok(halley_ipc::send_request(&Request::Cluster(
            ClusterRequest::Open {
                target: ClusterTarget::Id(*id),
                output: None,
            },
        ))),
        LensAction::FocusNode { id } => expect_ok(halley_ipc::send_request(&Request::Node(
            NodeRequest::Focus {
                selector: Some(NodeSelector::Id(*id)),
                output: None,
            },
        ))),
        LensAction::ReloadConfig => expect_ok(halley_ipc::send_request(&Request::Compositor(
            CompositorRequest::Reload,
        ))),
        LensAction::OpenPath { path } => launch_exec(
            format!("xdg-open {}", shell_quote(path)).as_str(),
            false,
            index.terminal.as_str(),
        ),
        LensAction::CreateCluster => Ok(()),
    }
}

pub fn materialize_cluster_draft(
    index: &ProviderIndex,
    draft: &ClusterDraft,
    query: &str,
) -> Result<(), String> {
    let name_hint = query.trim();
    let request = ClusterDraftRequest {
        name_hint: (!name_hint.is_empty()).then(|| name_hint.to_string()),
        app_ids: draft.app_ids.clone(),
        app_launches: index.draft_app_launches(&draft.app_ids),
        running_node_ids: draft.running_node_ids.clone(),
        source: ClusterDraftSource::HalleyLens,
    };
    expect_ok(halley_ipc::send_request(&Request::Cluster(
        ClusterRequest::OpenFinalizeDraft {
            draft: request,
            output: None,
        },
    )))?;
    Ok(())
}

fn app_launch_command(app: &DesktopApp, terminal_command: &str) -> String {
    if app.terminal {
        format!("{} {}", terminal_command.trim(), app.exec)
    } else {
        app.exec.clone()
    }
}

fn expect_ok(response: Result<Response, halley_ipc::CodecError>) -> Result<(), String> {
    match response {
        Ok(Response::Ok) | Ok(Response::Reloaded) => Ok(()),
        Ok(Response::Error(err)) => Err(format_api_error(&err)),
        Ok(other) => Err(format!("unexpected response: {other:?}")),
        Err(err) => Err(err.to_string()),
    }
}

fn format_api_error(err: &ApiError) -> String {
    match err {
        ApiError::InvalidRequest(message)
        | ApiError::NotFound(message)
        | ApiError::Ambiguous(message)
        | ApiError::Unsupported(message)
        | ApiError::Internal(message) => message.clone(),
    }
}

fn match_score(query_lower: &str, haystack_lower: &str) -> Option<f64> {
    if query_lower.is_empty() {
        return Some(1.0);
    }
    if haystack_lower == query_lower {
        return Some(300.0);
    }
    if haystack_lower.contains(query_lower) {
        return Some(200.0 - haystack_lower.find(query_lower).unwrap_or(0) as f64);
    }
    fuzzy_match(query_lower, haystack_lower).map(|score| 100.0 + score)
}

fn fuzzy_match(query: &str, haystack: &str) -> Option<f64> {
    let mut score = 0.0;
    let mut last = 0usize;
    for ch in query.chars() {
        let tail = &haystack[last..];
        let Some(pos) = tail.find(ch) else {
            return None;
        };
        score += 10.0 - pos.min(8) as f64;
        last += pos + ch.len_utf8();
    }
    Some(score)
}

fn load_desktop_apps() -> Vec<DesktopApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in desktop_dirs() {
        walk_desktop_files(&dir, 3, &mut |path| {
            if let Some(app) = parse_desktop_app(path)
                && seen.insert(app.id.clone())
            {
                apps.push(app);
            }
        });
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

fn parse_desktop_app(path: &Path) -> Option<DesktopApp> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut name = None;
    let mut comment = None;
    let mut icon_name = None;
    let mut startup_wm_class = None;
    let mut exec = None;
    let mut hidden = false;
    let mut no_display = false;
    let mut terminal = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(value) = line.strip_prefix("Name=") {
            name = Some(unescape(value));
        } else if let Some(value) = line.strip_prefix("Comment=") {
            comment = Some(unescape(value));
        } else if let Some(value) = line.strip_prefix("Icon=") {
            icon_name = Some(unescape(value));
        } else if let Some(value) = line.strip_prefix("StartupWMClass=") {
            startup_wm_class = Some(unescape(value));
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec = Some(clean_exec(value));
        } else if let Some(value) = line.strip_prefix("Hidden=") {
            hidden = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = line.strip_prefix("NoDisplay=") {
            no_display = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = line.strip_prefix("Terminal=") {
            terminal = value.eq_ignore_ascii_case("true");
        } else if let Some(value) = line.strip_prefix("Type=")
            && !value.eq_ignore_ascii_case("Application")
        {
            return None;
        }
    }
    if hidden || no_display {
        return None;
    }
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let name = name?;
    let exec = exec?;
    let search_text = format!(
        "{} {} {} {}",
        name,
        id,
        comment.as_deref().unwrap_or_default(),
        startup_wm_class.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    let icon_name = icon_name
        .or_else(|| startup_wm_class.clone())
        .or_else(|| Some(id.clone()));
    Some(DesktopApp {
        id,
        name,
        comment,
        icon_name,
        exec,
        terminal,
        search_text,
    })
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/share/applications"));
    }
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(std::env::split_paths(&data_dirs).map(|dir| dir.join("applications")));
    dirs
}

fn walk_desktop_files(dir: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_desktop_files(&path, depth - 1, f);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("desktop") {
            f(&path);
        }
    }
}

fn clean_exec(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|part| !part.starts_with('%'))
        .collect::<Vec<_>>()
        .join(" ")
}

fn unescape(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\s", " ")
        .replace("\\\\", "\\")
}

fn launch_exec(command: &str, terminal: bool, terminal_command: &str) -> Result<(), String> {
    let command = if terminal {
        format!("{} {command}", terminal_command.trim())
    } else {
        command.to_string()
    };
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_mode_empty_query_keeps_all_stageable_results() {
        let index = ProviderIndex {
            apps: (0..5)
                .map(|idx| DesktopApp {
                    id: format!("app-{idx}"),
                    name: format!("App {idx}"),
                    comment: None,
                    icon_name: None,
                    exec: "true".into(),
                    terminal: false,
                    search_text: format!("app {idx}"),
                })
                .collect(),
            nodes: (0..5)
                .map(|idx| CachedNode {
                    id: idx,
                    title: format!("Node {idx}"),
                    subtitle: "window on monitor".into(),
                    search_text: format!("node {idx}"),
                    pinned: false,
                })
                .collect(),
            clusters: Vec::new(),
            live_loaded: true,
            live_rx: None,
            terminal: String::new(),
        };
        let results = index.search(&SearchContext {
            mode: LensMode::Clusters,
            query: String::new(),
            query_lower: String::new(),
            max_results: 3,
            draft_count: 0,
        });

        assert_eq!(results.len(), 10);
        assert!(results.iter().any(|result| result.section == "Apps"));
        assert!(
            results
                .iter()
                .any(|result| result.section == "Running Nodes")
        );
    }
}
