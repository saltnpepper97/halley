use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use halley_api::{
    Client, ClusterDraft as ApiClusterDraft, ClusterDraftApp, ClusterTarget, Event, EventTopic,
    NodeId, NodeKind, NodeSelector,
};

use crate::config::{LiftConfig, default_config_path, resolved_halley_config_path};
use crate::mode::LiftMode;
use crate::model::{ClusterDraft, LiftAction, LiftResult, LiftResultKind, mode_allows};

#[derive(Clone, Debug)]
pub struct SearchContext {
    pub mode: LiftMode,
    pub query: String,
    pub query_lower: String,
    pub max_results: usize,
    pub draft_count: usize,
}

#[derive(Default)]
pub struct ProviderIndex {
    apps: Vec<DesktopApp>,
    nodes: Vec<CachedNode>,
    clusters: Vec<CachedCluster>,
    live_loaded: bool,
    live_rx: Option<Receiver<(Vec<CachedNode>, Vec<CachedCluster>)>>,
    live_wake: Option<calloop::channel::Sender<()>>,
    terminal: String,
    terminal_icon_name: Option<String>,
    client: Option<Arc<Client>>,
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
    pub fn load(config: &LiftConfig) -> Self {
        let apps = load_desktop_apps();
        let terminal = config.terminal.trim().to_string();
        let terminal_icon_name = terminal_icon_name_for_apps(&apps, terminal.as_str());
        let client = Client::connect().map(Arc::new).ok();
        Self {
            apps,
            nodes: Vec::new(),
            clusters: Vec::new(),
            live_loaded: false,
            live_rx: None,
            live_wake: None,
            terminal,
            terminal_icon_name,
            client,
        }
    }

    pub fn needs_live_refresh(&self) -> bool {
        !self.live_loaded && self.live_rx.is_none()
    }

    pub fn set_live_waker(&mut self, wake: calloop::channel::Sender<()>) {
        self.live_wake = Some(wake);
    }

    pub fn start_live_refresh(&mut self) {
        if !self.needs_live_refresh() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let client = self.client.clone();
        let wake = self.live_wake.clone();
        thread::spawn(move || {
            let Some(client) = client else {
                let _ = tx.send((Vec::new(), Vec::new()));
                if let Some(wake) = wake.as_ref() {
                    let _ = wake.send(());
                }
                return;
            };
            let Ok(mut subscription) = client.subscribe([EventTopic::Nodes, EventTopic::Clusters])
            else {
                let _ = tx.send((load_nodes(&client), load_clusters(&client)));
                if let Some(wake) = wake.as_ref() {
                    let _ = wake.send(());
                }
                return;
            };
            let mut nodes = subscription.initial.nodes;
            let mut clusters = subscription.initial.clusters;
            send_live_update(&tx, wake.as_ref(), &nodes, &clusters);
            while let Ok(event) = subscription.events.next_event() {
                match event {
                    Event::NodeAdded { node, .. } | Event::NodeChanged { node, .. } => {
                        upsert(&mut nodes, node, |node| node.id);
                    }
                    Event::NodeRemoved { id, .. } => nodes.retain(|node| node.id != id),
                    Event::ClusterAdded { cluster, .. } | Event::ClusterChanged { cluster, .. } => {
                        upsert(&mut clusters, cluster, |cluster| cluster.id);
                    }
                    Event::ClusterRemoved { id, .. } => clusters.retain(|cluster| cluster.id != id),
                    _ => continue,
                }
                send_live_update(&tx, wake.as_ref(), &nodes, &clusters);
            }
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
        // Keep the receiver connected: the subscription continues to push
        // typed deltas for the lifetime of Lift.
        Some((self.nodes.len(), self.clusters.len()))
    }

    pub fn search(&self, ctx: &SearchContext) -> Vec<LiftResult> {
        let mut results = Vec::new();
        if matches!(
            ctx.mode,
            LiftMode::General | LiftMode::Apps | LiftMode::Clusters
        ) {
            results.extend(self.search_apps(ctx));
        }
        if matches!(
            ctx.mode,
            LiftMode::General | LiftMode::Nodes | LiftMode::Clusters
        ) {
            results.extend(self.search_nodes(ctx));
        }
        if matches!(ctx.mode, LiftMode::General | LiftMode::Clusters) {
            results.extend(self.search_clusters(ctx));
        }
        if matches!(ctx.mode, LiftMode::General | LiftMode::Actions) {
            results.extend(search_actions(ctx));
        }
        if matches!(ctx.mode, LiftMode::General | LiftMode::Config) {
            results.extend(search_config(ctx));
        }
        if ctx.mode == LiftMode::Term {
            results.extend(self.search_term(ctx));
        }

        if ctx.mode == LiftMode::Clusters && ctx.draft_count > 0 {
            results.push(create_cluster_result(ctx.query.as_str()));
        }

        results.retain(|result| mode_allows(ctx.mode, &result.kind));
        let empty_general = ctx.mode == LiftMode::General && ctx.query_lower.is_empty();
        results.sort_by(|a, b| {
            b.is_field_pinned
                .cmp(&a.is_field_pinned)
                .then_with(|| {
                    if empty_general {
                        provider_rank(&a.kind).cmp(&provider_rank(&b.kind))
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.section.cmp(&b.section))
                .then_with(|| a.title.cmp(&b.title))
        });
        let max_results = if matches!(ctx.mode, LiftMode::Apps | LiftMode::Clusters)
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

    fn search_apps(&self, ctx: &SearchContext) -> Vec<LiftResult> {
        self.apps
            .iter()
            .filter_map(|app| {
                let score = match_score(ctx.query_lower.as_str(), app.search_text.as_str())?;
                Some(LiftResult {
                    section: if ctx.mode == LiftMode::Clusters {
                        "Apps"
                    } else {
                        "Applications"
                    }
                    .into(),
                    title: app.name.clone(),
                    subtitle: Some(app.comment.clone().unwrap_or_else(|| "Application".into())),
                    icon_name: app.icon_name.clone(),
                    kind: LiftResultKind::App,
                    score,
                    is_field_pinned: false,
                    shortcut_hint: Some(
                        if ctx.mode == LiftMode::Clusters {
                            "Space stage"
                        } else {
                            "Enter launch"
                        }
                        .into(),
                    ),
                    action: LiftAction::LaunchApp {
                        app_id: app.id.clone(),
                    },
                })
            })
            .collect()
    }

    fn search_nodes(&self, ctx: &SearchContext) -> Vec<LiftResult> {
        self.nodes
            .iter()
            .filter_map(|node| {
                let mut score = match_score(ctx.query_lower.as_str(), node.search_text.as_str())?;
                if node.pinned {
                    score += 1000.0;
                }
                Some(LiftResult {
                    section: if ctx.mode == LiftMode::Clusters {
                        "Running Nodes"
                    } else {
                        "Nodes"
                    }
                    .into(),
                    title: node.title.clone(),
                    subtitle: Some(node.subtitle.clone()),
                    icon_name: None,
                    kind: LiftResultKind::Node,
                    score,
                    is_field_pinned: node.pinned,
                    shortcut_hint: Some(
                        if ctx.mode == LiftMode::Clusters {
                            "Space stage"
                        } else {
                            "Enter open"
                        }
                        .into(),
                    ),
                    action: LiftAction::FocusNode { id: node.id },
                })
            })
            .collect()
    }

    fn search_clusters(&self, ctx: &SearchContext) -> Vec<LiftResult> {
        self.clusters
            .iter()
            .filter_map(|cluster| {
                let score = match_score(ctx.query_lower.as_str(), cluster.search_text.as_str())?;
                Some(LiftResult {
                    section: "Existing Clusters".into(),
                    title: cluster.title.clone(),
                    subtitle: Some(cluster.subtitle.clone()),
                    icon_name: None,
                    kind: LiftResultKind::Cluster,
                    score: score + 20.0,
                    is_field_pinned: false,
                    shortcut_hint: Some("Enter open".into()),
                    action: LiftAction::OpenCluster { id: cluster.id },
                })
            })
            .collect()
    }

    fn search_term(&self, ctx: &SearchContext) -> Vec<LiftResult> {
        let command = ctx.query.trim();
        let (title, subtitle) = if command.is_empty() {
            (
                "Open terminal".to_string(),
                "Type a command to run".to_string(),
            )
        } else {
            (command.to_string(), "Run in terminal".to_string())
        };
        vec![LiftResult {
            section: "Terminal".into(),
            title,
            subtitle: Some(subtitle),
            icon_name: self.terminal_icon_name.clone(),
            kind: LiftResultKind::Term,
            score: 1.0,
            is_field_pinned: false,
            shortcut_hint: Some("Enter run".into()),
            action: LiftAction::RunInTerminal {
                command: command.to_string(),
            },
        }]
    }

    pub fn launch_app(&self, app_id: &str) -> Result<(), String> {
        let app = self
            .apps
            .iter()
            .find(|app| app.id == app_id)
            .ok_or_else(|| format!("app `{app_id}` not found"))?;
        launch_exec(app.exec.as_str(), app.terminal, self.terminal.as_str())
    }

    fn draft_app_launches(&self, app_ids: &[String]) -> Vec<ClusterDraftApp> {
        app_ids
            .iter()
            .filter_map(|app_id| {
                let app = self.apps.iter().find(|app| app.id == *app_id)?;
                Some(ClusterDraftApp {
                    app_id: app.id.clone(),
                    command: app_launch_command(app, self.terminal.as_str()),
                })
            })
            .collect()
    }
}

fn load_nodes(client: &Client) -> Vec<CachedNode> {
    let Ok(live_nodes) = client.nodes(None) else {
        return Vec::new();
    };
    cached_nodes(&live_nodes)
}

fn cached_nodes(live_nodes: &[halley_api::NodeInfo]) -> Vec<CachedNode> {
    let mut nodes = Vec::new();
    for node in live_nodes.iter().cloned() {
        if node.kind != NodeKind::Surface || !node.visible {
            continue;
        }
        let output = node.output.unwrap_or_else(|| "unknown output".into());
        let title = node.title;
        let app_id = node.app_id.unwrap_or_default();
        let app_label = if app_id.is_empty() {
            "window"
        } else {
            app_id.as_str()
        };
        let search_text = format!("{title} {app_id} {output}").to_ascii_lowercase();
        nodes.push(CachedNode {
            id: node.id.get(),
            title,
            subtitle: format!("{app_label} on {output}"),
            search_text,
            pinned: node.pinned,
        });
    }
    nodes
}

fn load_clusters(client: &Client) -> Vec<CachedCluster> {
    let Ok(live_clusters) = client.clusters(None) else {
        return Vec::new();
    };
    cached_clusters(&live_clusters)
}

fn cached_clusters(live_clusters: &[halley_api::ClusterSummary]) -> Vec<CachedCluster> {
    let mut clusters = Vec::new();
    for cluster in live_clusters.iter().cloned() {
        let title = cluster.name;
        let slot = cluster.slot.map(|s| s.to_string()).unwrap_or_default();
        let search_text = format!("{title} {slot} {}", cluster.output).to_ascii_lowercase();
        clusters.push(CachedCluster {
            id: cluster.id.get(),
            title,
            subtitle: format!("{} members on {}", cluster.member_count, cluster.output),
            search_text,
        });
    }
    clusters
}

fn send_live_update(
    sender: &mpsc::Sender<(Vec<CachedNode>, Vec<CachedCluster>)>,
    wake: Option<&calloop::channel::Sender<()>>,
    nodes: &[halley_api::NodeInfo],
    clusters: &[halley_api::ClusterSummary],
) {
    if sender
        .send((cached_nodes(nodes), cached_clusters(clusters)))
        .is_ok()
        && let Some(wake) = wake
    {
        let _ = wake.send(());
    }
}

fn upsert<T, K: Eq>(values: &mut Vec<T>, value: T, key: impl Fn(&T) -> K) {
    let value_key = key(&value);
    if let Some(existing) = values
        .iter_mut()
        .find(|existing| key(existing) == value_key)
    {
        *existing = value;
    } else {
        values.push(value);
    }
}

fn create_cluster_result(_query: &str) -> LiftResult {
    let title = "Create cluster".into();
    LiftResult {
        section: "Create".into(),
        title,
        subtitle: Some("Open Cluster Finalize popup".into()),
        icon_name: None,
        kind: LiftResultKind::CreateCluster,
        score: 0.0,
        is_field_pinned: false,
        shortcut_hint: Some("Ctrl+Enter".into()),
        action: LiftAction::CreateCluster,
    }
}

fn search_actions(ctx: &SearchContext) -> Vec<LiftResult> {
    let actions = [
        (
            "reload-config",
            "Reload Halley config",
            "Compositor action",
            LiftAction::ReloadConfig,
        ),
        (
            "show-halley-basics",
            "Show Halley basics",
            "Compositor action",
            LiftAction::ShowBasics,
        ),
    ];
    actions
        .into_iter()
        .filter_map(|(_id, title, subtitle, action)| {
            match_score(
                ctx.query_lower.as_str(),
                title.to_ascii_lowercase().as_str(),
            )
            .map(|score| LiftResult {
                section: "Actions".into(),
                title: title.into(),
                subtitle: Some(subtitle.into()),
                icon_name: None,
                kind: LiftResultKind::Action,
                score,
                is_field_pinned: false,
                shortcut_hint: Some("Enter".into()),
                action,
            })
        })
        .collect()
}

fn search_config(ctx: &SearchContext) -> Vec<LiftResult> {
    let configs = [
        (
            "lift config",
            "Lift config",
            default_config_path().display().to_string(),
        ),
        (
            "halley config compositor config",
            "Halley config",
            resolved_halley_config_path().display().to_string(),
        ),
    ];
    configs
        .into_iter()
        .filter_map(|(haystack, title, path)| {
            match_score(ctx.query_lower.as_str(), haystack).map(|score| LiftResult {
                section: "Config".into(),
                title: title.into(),
                subtitle: Some(path.clone()),
                icon_name: None,
                kind: LiftResultKind::Config,
                score,
                is_field_pinned: false,
                shortcut_hint: Some("Enter edit".into()),
                action: LiftAction::OpenConfig { path },
            })
        })
        .collect()
}

pub fn activate_result(index: &ProviderIndex, result: &LiftResult) -> Result<(), String> {
    match &result.action {
        LiftAction::LaunchApp { app_id } => index.launch_app(app_id),
        LiftAction::OpenCluster { id } => index
            .client()?
            .open_cluster(ClusterTarget::Id((*id).into()), None)
            .map_err(|error| error.to_string()),
        LiftAction::FocusNode { id } => index
            .client()?
            .focus_node(Some(NodeSelector::Id(NodeId::new(*id))), None)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        LiftAction::ReloadConfig => index
            .client()?
            .reload_config()
            .map_err(|error| error.to_string()),
        LiftAction::ShowBasics => index
            .client()?
            .show_basics()
            .map_err(|error| error.to_string()),
        LiftAction::OpenConfig { path } => launch_editor(path, index.terminal.as_str()),
        LiftAction::CreateCluster => Ok(()),
        LiftAction::RunInTerminal { command } => {
            // Run through the user's interactive shell so aliases/functions are loaded,
            // then exec back into that shell so short commands like `ls` stay visible.
            let full = terminal_launch_command(index.terminal.as_str(), command.as_str());
            launch_exec(full.as_str(), false, index.terminal.as_str())
        }
    }
}

fn terminal_launch_command(terminal_command: &str, command: &str) -> String {
    terminal_launch_command_with_shell(terminal_command, command, user_shell().as_str())
}

fn terminal_launch_command_with_shell(
    terminal_command: &str,
    command: &str,
    shell: &str,
) -> String {
    format!(
        "{} {}",
        terminal_command.trim(),
        terminal_shell_invocation(command, shell)
    )
}

fn terminal_shell_invocation(command: &str, shell: &str) -> String {
    let command = command.trim();
    let shell = shell.trim();
    let shell = if shell.is_empty() { "sh" } else { shell };
    let shell = shell_quote(shell);
    if command.is_empty() {
        return format!("{shell} -i");
    }
    let script = format!("{command}\nexec {shell} -i");
    format!("{shell} -ic {}", shell_quote(script.as_str()))
}

fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "sh".into())
}

pub fn materialize_cluster_draft(
    index: &ProviderIndex,
    draft: &ClusterDraft,
    _query: &str,
) -> Result<(), String> {
    let request = build_cluster_draft_request(index, draft);
    index
        .client()?
        .finalize_cluster_draft(request, None)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn build_cluster_draft_request(index: &ProviderIndex, draft: &ClusterDraft) -> ApiClusterDraft {
    ApiClusterDraft {
        name_hint: None,
        apps: index.draft_app_launches(&draft.app_ids),
        running_nodes: draft
            .running_node_ids
            .iter()
            .copied()
            .map(NodeId::new)
            .collect(),
    }
}

fn app_launch_command(app: &DesktopApp, terminal_command: &str) -> String {
    if app.terminal {
        format!("{} {}", terminal_command.trim(), app.exec)
    } else {
        app.exec.clone()
    }
}

fn terminal_icon_name_for_apps(apps: &[DesktopApp], terminal_command: &str) -> Option<String> {
    let terminal = command_program_name(terminal_command)?.to_ascii_lowercase();
    apps.iter()
        .filter_map(|app| {
            terminal_app_match_score(app, terminal.as_str())
                .map(|score| (score, app.icon_name.clone()))
        })
        .max_by_key(|(score, _)| *score)
        .and_then(|(_, icon_name)| icon_name)
}

fn terminal_app_match_score(app: &DesktopApp, terminal: &str) -> Option<i32> {
    let id = app.id.to_ascii_lowercase();
    let name = app.name.to_ascii_lowercase();
    let exec = command_program_name(app.exec.as_str()).map(|value| value.to_ascii_lowercase());

    if id == terminal || id.ends_with(&format!(".{terminal}")) {
        return Some(100);
    }
    if exec.as_deref() == Some(terminal) {
        return Some(90);
    }
    if name == terminal {
        return Some(80);
    }
    if id.contains(terminal) {
        return Some(60);
    }
    if name.contains(terminal) || app.search_text.contains(terminal) {
        return Some(40);
    }
    None
}

fn command_program_name(command: &str) -> Option<String> {
    shell_words(command).into_iter().find_map(|word| {
        if word == "env" || (word.contains('=') && !word.contains('/')) {
            return None;
        }
        let file_name = Path::new(&word).file_name()?.to_str()?.trim();
        (!file_name.is_empty()).then(|| file_name.to_string())
    })
}

fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(active), ch) if ch == active => quote = None,
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

impl ProviderIndex {
    fn client(&self) -> Result<&Client, String> {
        self.client
            .as_deref()
            .ok_or_else(|| "Halley compositor API is unavailable".into())
    }
}

fn provider_rank(kind: &LiftResultKind) -> u8 {
    match kind {
        LiftResultKind::App => 0,
        LiftResultKind::Node => 1,
        LiftResultKind::Cluster => 2,
        LiftResultKind::Action => 3,
        LiftResultKind::Config => 4,
        LiftResultKind::CreateCluster => 5,
        LiftResultKind::Term => 6,
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
        let pos = tail.find(ch)?;
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

fn launch_editor(path: &str, terminal_command: &str) -> Result<(), String> {
    let editor = std::env::var("EDITOR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "EDITOR is not set".to_string())?;
    launch_exec(
        editor_command(editor.as_str(), path).as_str(),
        true,
        terminal_command,
    )
}

fn editor_command(editor: &str, path: &str) -> String {
    format!("{} {}", editor.trim(), shell_quote(path))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_shell_invocation_keeps_shell_open_after_command() {
        assert_eq!(
            terminal_shell_invocation(" ls -la ", "/bin/zsh"),
            r#"'/bin/zsh' -ic 'ls -la
exec '\''/bin/zsh'\'' -i'"#
        );
    }

    #[test]
    fn terminal_shell_invocation_opens_shell_for_empty_command() {
        assert_eq!(terminal_shell_invocation("  ", "/bin/zsh"), "'/bin/zsh' -i");
    }

    #[test]
    fn terminal_launch_command_quotes_full_shell_payload() {
        assert_eq!(
            terminal_launch_command_with_shell("kitty -e", "printf 'hi' && true", "/bin/zsh"),
            r#"kitty -e '/bin/zsh' -ic 'printf '\''hi'\'' && true
exec '\''/bin/zsh'\'' -i'"#
        );
    }

    #[test]
    fn editor_command_uses_editor_and_quotes_path() {
        assert_eq!(
            editor_command("code --wait", "/tmp/halley's config.rune"),
            "code --wait '/tmp/halley'\\''s config.rune'"
        );
    }

    #[test]
    fn terminal_launch_command_can_wrap_editor_command() {
        assert_eq!(
            format!(
                "{} {}",
                "kitty -e",
                editor_command("nvim", "/tmp/halley config.rune")
            ),
            "kitty -e nvim '/tmp/halley config.rune'"
        );
    }

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
            live_wake: None,
            terminal: String::new(),
            terminal_icon_name: None,
            client: None,
        };
        let results = index.search(&SearchContext {
            mode: LiftMode::Clusters,
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

    fn provider_index_with_every_provider() -> ProviderIndex {
        ProviderIndex {
            apps: vec![app("kitty", "Kitty", "kitty", "kitty")],
            nodes: vec![CachedNode {
                id: 7,
                title: "notes.txt - editor".into(),
                subtitle: "editor on DP-1".into(),
                search_text: "notes.txt - editor editor DP-1".to_ascii_lowercase(),
                pinned: false,
            }],
            clusters: vec![CachedCluster {
                id: 3,
                title: "Release".into(),
                subtitle: "2 members on DP-1".into(),
                search_text: "release 3 DP-1".to_ascii_lowercase(),
            }],
            live_loaded: true,
            live_rx: None,
            live_wake: None,
            terminal: "kitty -e".into(),
            terminal_icon_name: None,
            client: None,
        }
    }

    /// Milestone 2: a fresh `Mod+D` press must make launching *and* retrieval
    /// immediately understandable. An empty query is exactly what Lift computes
    /// on open, so every documented provider stays represented, with the plain
    /// General-mode section names and their launch/open hints.
    #[test]
    fn empty_general_query_keeps_every_provider_available() {
        let index = provider_index_with_every_provider();
        let results = index.search(&SearchContext {
            mode: LiftMode::General,
            query: String::new(),
            query_lower: String::new(),
            max_results: 40,
            draft_count: 0,
        });

        for (kind, section) in [
            (LiftResultKind::App, "Applications"),
            (LiftResultKind::Node, "Nodes"),
            (LiftResultKind::Cluster, "Existing Clusters"),
            (LiftResultKind::Action, "Actions"),
            (LiftResultKind::Config, "Config"),
        ] {
            let result = results
                .iter()
                .find(|result| result.kind == kind)
                .unwrap_or_else(|| panic!("a fresh search must still offer {kind:?}"));
            assert_eq!(
                result.section, section,
                "{kind:?} keeps its General section"
            );
        }

        let app_result = results
            .iter()
            .find(|result| result.kind == LiftResultKind::App)
            .expect("application provider");
        assert_eq!(app_result.shortcut_hint.as_deref(), Some("Enter launch"));
        let node_result = results
            .iter()
            .find(|result| result.kind == LiftResultKind::Node)
            .expect("node provider");
        assert_eq!(node_result.shortcut_hint.as_deref(), Some("Enter open"));

        let placeholder = LiftConfig::default().placeholder;
        for provider in ["apps", "nodes", "clusters", "actions"] {
            assert!(
                placeholder.contains(provider),
                "the default empty state must name the {provider} provider: {placeholder:?}"
            );
        }
    }

    /// The default result cap must not let cluster score bonuses hide the
    /// primary application-launch provider on a fresh `Mod+D` press.
    #[test]
    fn empty_general_query_orders_providers_before_default_truncation() {
        let mut index = provider_index_with_every_provider();
        index.apps = (0..8)
            .map(|idx| {
                app(
                    format!("app-{idx}").as_str(),
                    format!("App {idx}").as_str(),
                    "true",
                    "application-x-executable",
                )
            })
            .collect();
        index.nodes = (0..3)
            .map(|idx| CachedNode {
                id: idx,
                title: format!("Node {idx}"),
                subtitle: "window on DP-1".into(),
                search_text: format!("node {idx}"),
                pinned: false,
            })
            .collect();
        index.clusters = (0..4)
            .map(|idx| CachedCluster {
                id: idx,
                title: format!("Cluster {idx}"),
                subtitle: "2 members on DP-1".into(),
                search_text: format!("cluster {idx}"),
            })
            .collect();

        let results = index.search(&SearchContext {
            mode: LiftMode::General,
            query: String::new(),
            query_lower: String::new(),
            max_results: LiftConfig::default().max_results,
            draft_count: 0,
        });

        assert_eq!(results.len(), 12);
        assert!(
            results[..8]
                .iter()
                .all(|result| result.kind == LiftResultKind::App),
            "applications must lead the default empty state: {results:#?}"
        );
        assert!(
            results[8..11]
                .iter()
                .all(|result| result.kind == LiftResultKind::Node),
            "running nodes must follow applications: {results:#?}"
        );
        assert_eq!(
            results[11].kind,
            LiftResultKind::Cluster,
            "advanced providers follow application launch and node retrieval"
        );
    }

    /// The documented prefixes keep their provider reachable and do not leak
    /// unrelated results: `app`, `node`, `cluster`, `action`, `config`, `term`.
    #[test]
    fn documented_modes_keep_each_provider_reachable() {
        let index = provider_index_with_every_provider();
        for (mode, kind) in [
            (LiftMode::Apps, LiftResultKind::App),
            (LiftMode::Nodes, LiftResultKind::Node),
            (LiftMode::Clusters, LiftResultKind::Cluster),
            (LiftMode::Actions, LiftResultKind::Action),
            (LiftMode::Config, LiftResultKind::Config),
            (LiftMode::Term, LiftResultKind::Term),
        ] {
            let results = index.search(&SearchContext {
                mode,
                query: String::new(),
                query_lower: String::new(),
                max_results: 40,
                draft_count: 0,
            });
            assert!(
                results.iter().any(|result| result.kind == kind),
                "{mode:?} must still offer its {kind:?} provider"
            );
            assert!(
                results.iter().all(|result| mode_allows(mode, &result.kind)),
                "{mode:?} must not leak unrelated providers"
            );
        }
    }

    /// Milestone 3: the one-time basics card can always be reopened by hand
    /// from Lift, with a self-describing action title.
    #[test]
    fn actions_provider_offers_the_manual_basics_card_reopen() {
        let index = provider_index_with_every_provider();
        let results = index.search(&SearchContext {
            mode: LiftMode::Actions,
            query: "halley basics".into(),
            query_lower: "halley basics".into(),
            max_results: 40,
            draft_count: 0,
        });

        let basics = results
            .iter()
            .find(|result| matches!(result.action, LiftAction::ShowBasics))
            .expect("the actions provider offers Show Halley basics");
        assert_eq!(basics.title, "Show Halley basics");
        assert_eq!(basics.kind, LiftResultKind::Action);
        assert_eq!(basics.section, "Actions");
        assert_eq!(basics.shortcut_hint.as_deref(), Some("Enter"));

        // The existing compositor actions stay offered.
        let reload = index.search(&SearchContext {
            mode: LiftMode::Actions,
            query: "reload".into(),
            query_lower: "reload".into(),
            max_results: 40,
            draft_count: 0,
        });
        assert!(
            reload
                .iter()
                .any(|result| matches!(result.action, LiftAction::ReloadConfig))
        );
    }

    fn app(id: &str, name: &str, exec: &str, icon: &str) -> DesktopApp {
        DesktopApp {
            id: id.into(),
            name: name.into(),
            comment: None,
            icon_name: Some(icon.into()),
            exec: exec.into(),
            terminal: false,
            search_text: format!("{id} {name}").to_ascii_lowercase(),
        }
    }

    #[test]
    fn terminal_icon_resolves_from_simple_terminal_command() {
        let apps = vec![app("kitty", "Kitty", "kitty", "kitty")];

        assert_eq!(
            terminal_icon_name_for_apps(&apps, "kitty -e"),
            Some("kitty".into())
        );
    }

    #[test]
    fn terminal_icon_resolves_from_path_command() {
        let apps = vec![app(
            "com.mitchellh.ghostty",
            "Ghostty",
            "ghostty",
            "ghostty",
        )];

        assert_eq!(
            terminal_icon_name_for_apps(&apps, "/usr/bin/ghostty -e"),
            Some("ghostty".into())
        );
    }

    #[test]
    fn terminal_icon_resolves_from_quoted_command() {
        let apps = vec![app(
            "org.wezfurlong.wezterm",
            "WezTerm",
            "wezterm start",
            "wezterm",
        )];

        assert_eq!(
            terminal_icon_name_for_apps(&apps, "'/usr/bin/wezterm' start --"),
            Some("wezterm".into())
        );
    }

    #[test]
    fn terminal_icon_missing_when_no_desktop_app_matches() {
        let apps = vec![app("kitty", "Kitty", "kitty", "kitty")];

        assert_eq!(terminal_icon_name_for_apps(&apps, "foot -e"), None);
    }

    #[test]
    fn cluster_draft_sends_selected_apps_only_as_launches() {
        let index = ProviderIndex {
            apps: vec![app("kitty", "Kitty", "kitty", "kitty")],
            nodes: Vec::new(),
            clusters: Vec::new(),
            live_loaded: true,
            live_rx: None,
            live_wake: None,
            terminal: "foot -e".into(),
            terminal_icon_name: None,
            client: None,
        };
        let draft = ClusterDraft {
            app_ids: vec!["kitty".into()],
            running_node_ids: vec![42],
        };

        let request = build_cluster_draft_request(&index, &draft);

        assert_eq!(request.name_hint, None);
        assert_eq!(request.apps.len(), 1);
        assert_eq!(request.apps[0].app_id, "kitty");
        assert_eq!(request.apps[0].command, "kitty");
        assert_eq!(request.running_nodes, vec![NodeId::new(42)]);
    }
}
