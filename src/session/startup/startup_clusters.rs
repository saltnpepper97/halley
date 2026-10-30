use std::collections::HashMap;
use std::time::Duration;

use halley_core::cluster::ClusterId;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};

pub(super) const LAUNCH_ID_ENV: &str = "HALLEY_STARTUP_CLUSTER_LAUNCH";
const LAUNCH_CONTEXT_LIFETIME: Duration = Duration::from_secs(15);
const MAX_ANCESTORS: usize = 32;

#[derive(Clone, Debug)]
pub(super) struct ActivationAttribution {
    pub launch_id: String,
}

#[derive(Clone, Debug)]
pub(super) struct PendingLaunch {
    pub cluster: ClusterId,
    pub command: String,
}

#[derive(Clone, Debug)]
struct LaunchContext {
    cluster: ClusterId,
    command: String,
    expires_at: Duration,
    matched: bool,
}

#[derive(Default)]
pub(super) struct StartupClusters {
    next_launch: u64,
    pending: Vec<PendingLaunch>,
    contexts: HashMap<String, LaunchContext>,
    client_launches: HashMap<ClientId, String>,
    surface_launches: Vec<(WlSurface, String)>,
}

impl StartupClusters {
    pub fn queue(&mut self, cluster: ClusterId, command: String) {
        self.pending.push(PendingLaunch { cluster, command });
    }

    pub fn take_pending(&mut self) -> Vec<PendingLaunch> {
        std::mem::take(&mut self.pending)
    }

    pub fn begin_launch(&mut self, cluster: ClusterId, command: String, now: Duration) -> String {
        self.next_launch = self.next_launch.saturating_add(1);
        let launch_id = format!(
            "{}-{}-{}",
            std::process::id(),
            now.as_nanos(),
            self.next_launch
        );
        self.contexts.insert(
            launch_id.clone(),
            LaunchContext {
                cluster,
                command,
                expires_at: now + LAUNCH_CONTEXT_LIFETIME,
                matched: false,
            },
        );
        launch_id
    }

    pub fn remember_activation(
        &mut self,
        surface: WlSurface,
        client_id: Option<ClientId>,
        launch_id: &str,
        now: Duration,
    ) -> Option<ClusterId> {
        self.expire(now);
        let context = self.contexts.get_mut(launch_id)?;
        context.matched = true;
        let cluster = context.cluster;
        self.surface_launches.push((surface, launch_id.to_string()));
        if let Some(client_id) = client_id {
            self.client_launches
                .insert(client_id, launch_id.to_string());
        }
        Some(cluster)
    }

    pub fn cluster_for_wayland_surface(
        &mut self,
        surface: &WlSurface,
        display: &DisplayHandle,
        now: Duration,
    ) -> Option<ClusterId> {
        self.expire(now);
        if let Some(launch_id) = self
            .surface_launches
            .iter()
            .find_map(|(candidate, launch_id)| (candidate == surface).then_some(launch_id.clone()))
        {
            return self.match_launch(&launch_id);
        }

        if let Some(client) = surface.client() {
            let client_id = client.id();
            if let Some(launch_id) = self.client_launches.get(&client_id).cloned() {
                return self.match_launch(&launch_id);
            }
            if let Ok(credentials) = client.get_credentials(display)
                && let Ok(pid) = u32::try_from(credentials.pid)
                && let Some(launch_id) = launch_id_from_process(pid)
            {
                self.client_launches.insert(client_id, launch_id.clone());
                return self.match_launch(&launch_id);
            }
        }
        None
    }

    pub fn cluster_for_pid(&mut self, pid: u32, now: Duration) -> Option<ClusterId> {
        self.expire(now);
        let launch_id = launch_id_from_process(pid)?;
        self.match_launch(&launch_id)
    }

    fn match_launch(&mut self, launch_id: &str) -> Option<ClusterId> {
        let context = self.contexts.get_mut(launch_id)?;
        context.matched = true;
        Some(context.cluster)
    }

    fn expire(&mut self, now: Duration) {
        let expired = self
            .contexts
            .iter()
            .filter_map(|(id, context)| (now >= context.expires_at).then_some(id.clone()))
            .collect::<Vec<_>>();
        for launch_id in &expired {
            if let Some(context) = self.contexts.remove(launch_id)
                && !context.matched
            {
                eventline::warn!(
                    "autostart cluster: no attributed window appeared for {:?}",
                    context.command
                );
            }
        }
        if expired.is_empty() {
            return;
        }
        self.client_launches
            .retain(|_, launch_id| !expired.contains(launch_id));
        self.surface_launches
            .retain(|(_, launch_id)| !expired.contains(launch_id));
    }
}

fn launch_id_from_process(mut pid: u32) -> Option<String> {
    for _ in 0..MAX_ANCESTORS {
        if pid == 0 {
            break;
        }
        if let Some(value) = process_environment_value(pid, LAUNCH_ID_ENV) {
            return Some(value);
        }
        let parent = process_parent(pid)?;
        if parent == pid {
            break;
        }
        pid = parent;
    }
    None
}

fn process_environment_value(pid: u32, name: &str) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{name}=");
    bytes.split(|byte| *byte == 0).find_map(|entry| {
        let entry = std::str::from_utf8(entry).ok()?;
        entry.strip_prefix(&prefix).map(str::to_string)
    })
}

fn process_parent(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("PPid:")
            .and_then(|value| value.trim().parse().ok())
    })
}

impl<D: super::SessionDriver> super::Session<D> {
    pub(crate) fn initialize_startup_clusters(
        &mut self,
        declarations: &[halley_config::StartupCluster],
        default_layout: halley_config::ClusterLayout,
        launch_members: bool,
    ) {
        use halley_core::cluster::layout::ClusterWorkspaceLayoutKind;
        use halley_core::field::Vec2;

        let primary = self.driver.primary_output().name();
        let available = self
            .wayland
            .space
            .outputs()
            .map(|output| output.name())
            .collect::<Vec<_>>();
        let resolved = declarations
            .iter()
            .filter_map(|declaration| {
                let output = declaration
                    .output
                    .clone()
                    .unwrap_or_else(|| primary.clone());
                if !available.contains(&output) {
                    eventline::warn!(
                        "autostart cluster {:?}: output {output:?} is not available",
                        declaration.name
                    );
                    return None;
                }
                Some((declaration, output))
            })
            .collect::<Vec<_>>();
        let mut totals = HashMap::<String, usize>::new();
        for (_, output) in &resolved {
            *totals.entry(output.clone()).or_default() += 1;
        }
        let mut indices = HashMap::<String, usize>::new();

        for (declaration, output_name) in resolved {
            let Some(output) = self
                .wayland
                .space
                .outputs()
                .find(|candidate| candidate.name() == output_name)
                .cloned()
            else {
                continue;
            };
            let Some(geometry) = self.wayland.space.output_geometry(&output) else {
                continue;
            };
            let Some(view) = self.cameras.view(&output_name) else {
                continue;
            };
            let total = totals.get(&output_name).copied().unwrap_or(1);
            let index = indices.entry(output_name.clone()).or_default();
            let screen_offset = (*index as f32 - (total.saturating_sub(1)) as f32 * 0.5) * 92.0;
            *index += 1;
            let scale = view.scale.max(0.05);
            let core_position = Vec2 {
                x: geometry.loc.x as f32 + view.center.x + screen_offset / scale,
                y: geometry.loc.y as f32 + view.center.y - geometry.size.h as f32 * 0.5 / scale
                    + 96.0 / scale,
            };
            let layout = match declaration.layout.unwrap_or(default_layout) {
                halley_config::ClusterLayout::Tiling => ClusterWorkspaceLayoutKind::Tiling,
                halley_config::ClusterLayout::Stacking => ClusterWorkspaceLayoutKind::Stacking,
            };
            match self.clusters.create_collapsed_cluster(
                &mut self.nodes.field,
                declaration.name.clone(),
                output_name,
                layout,
                Vec::new(),
                core_position,
            ) {
                Ok(cluster) => {
                    crate::nodes::resolve_new_cluster_core(self, cluster);
                    if launch_members {
                        for command in &declaration.members {
                            self.startup_clusters.queue(cluster, command.clone());
                        }
                    }
                }
                Err(message) => eventline::warn!(
                    "autostart cluster {:?} was not created: {message}",
                    declaration.name
                ),
            }
        }
        if !declarations.is_empty() {
            self.request_redraw();
        }
    }

    pub(crate) fn run_startup_cluster_commands(&mut self) {
        let Some(wayland_display) = self.wayland_display.clone() else {
            return;
        };
        let pending = self.startup_clusters.take_pending();
        let x11_display = self.xwayland.display_name();
        for launch in pending {
            let now = crate::frame_clock::monotonic_now();
            let launch_id =
                self.startup_clusters
                    .begin_launch(launch.cluster, launch.command.clone(), now);
            let token_data = smithay::wayland::xdg_activation::XdgActivationTokenData::default();
            token_data
                .user_data
                .insert_if_missing_threadsafe(|| ActivationAttribution {
                    launch_id: launch_id.clone(),
                });
            let token = {
                let (token, _) = self
                    .wayland
                    .xdg_activation_state
                    .create_external_token(token_data);
                token.to_string()
            };
            eventline::debug!(
                "autostart cluster: launching {:?} for cluster {}",
                launch.command,
                launch.cluster.as_u64()
            );
            super::spawn::spawn_detached_with_env(
                &launch.command,
                &wayland_display,
                x11_display.as_deref(),
                self.cursor.size(),
                &self.launch_environment,
                &[
                    ("XDG_ACTIVATION_TOKEN", token.as_str()),
                    (LAUNCH_ID_ENV, launch_id.as_str()),
                ],
            );
        }
    }

    pub(crate) fn startup_cluster_for_wayland_surface(
        &mut self,
        surface: &WlSurface,
    ) -> Option<ClusterId> {
        self.startup_clusters.cluster_for_wayland_surface(
            surface,
            &self.wayland.display_handle,
            crate::frame_clock::monotonic_now(),
        )
    }

    pub(crate) fn startup_cluster_for_x11_pid(&mut self, pid: u32) -> Option<ClusterId> {
        self.startup_clusters
            .cluster_for_pid(pid, crate::frame_clock::monotonic_now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_matches_only_explicit_launch_identifier() {
        let mut launches = StartupClusters::default();
        let now = Duration::from_secs(10);
        let cluster = ClusterId::new(7);
        let id = launches.begin_launch(cluster, "foot".into(), now);

        assert_eq!(launches.match_launch(&id), Some(cluster));
        assert_eq!(launches.match_launch("unrelated"), None);
    }

    #[test]
    fn context_expires_without_claiming_later_windows() {
        let mut launches = StartupClusters::default();
        let now = Duration::from_secs(10);
        let cluster = ClusterId::new(7);
        let id = launches.begin_launch(cluster, "foot".into(), now);

        launches.expire(now + LAUNCH_CONTEXT_LIFETIME);
        assert_eq!(launches.match_launch(&id), None);
    }

    #[test]
    fn current_process_environment_parser_is_safe_for_missing_marker() {
        assert_eq!(
            process_environment_value(std::process::id(), "HALLEY_TEST_MISSING_MARKER"),
            None
        );
    }
}
