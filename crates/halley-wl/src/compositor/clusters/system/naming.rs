use super::*;

use crate::compositor::clusters::state::{
    ClusterFinalizeAppLaunch, ClusterFinalizeDraftState, ClusterNameRecord,
    ClusterNamingPromptState, PendingLiftClusterBuildState,
};
use crate::compositor::interaction::state::{
    ClusterNamePromptRepeatAction, ClusterNamePromptRepeatState,
};
use halley_core::field::{NodeId, NodeKind, NodeState};

pub(super) fn cluster_mode_selection_banner(
    controller: &mut impl DerefMut<Target = Halley>,
    monitor: &str,
) {
    controller.ui.render_state.set_persistent_mode_banner(
        monitor,
        "Cluster mode",
        Some("Select windows"),
        &[
            OverlayActionHint {
                key: "Enter".to_string(),
                label: "name cluster".to_string(),
            },
            OverlayActionHint {
                key: "Esc".to_string(),
                label: "cancel".to_string(),
            },
        ],
    );
}

pub(super) fn cluster_name_prompt_banner(
    controller: &mut impl DerefMut<Target = Halley>,
    monitor: &str,
) {
    controller.ui.render_state.set_persistent_mode_banner(
        monitor,
        "Cluster mode",
        Some("Name new cluster"),
        &[
            OverlayActionHint {
                key: "Enter".to_string(),
                label: "confirm".to_string(),
            },
            OverlayActionHint {
                key: "Esc".to_string(),
                label: "back".to_string(),
            },
        ],
    );
}

fn prompt_char_len(text: &str) -> usize {
    text.chars().count()
}

fn prompt_char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn prompt_selection_range(prompt: &ClusterNamingPromptState) -> Option<(usize, usize)> {
    (prompt.selection_anchor_char != prompt.selection_focus_char).then(|| {
        (
            prompt
                .selection_anchor_char
                .min(prompt.selection_focus_char),
            prompt
                .selection_anchor_char
                .max(prompt.selection_focus_char),
        )
    })
}

fn prompt_replace_selection(prompt: &mut ClusterNamingPromptState, replacement: &str) {
    let (start, end) =
        prompt_selection_range(prompt).unwrap_or((prompt.caret_char, prompt.caret_char));
    let start_byte = prompt_char_to_byte(prompt.input.as_str(), start);
    let end_byte = prompt_char_to_byte(prompt.input.as_str(), end);
    prompt
        .input
        .replace_range(start_byte..end_byte, replacement);
    let inserted = prompt_char_len(replacement);
    prompt.caret_char = start + inserted;
    prompt.selection_anchor_char = prompt.caret_char;
    prompt.selection_focus_char = prompt.caret_char;
    prompt.scroll_char = prompt.scroll_char.min(prompt.caret_char);
}

fn prompt_delete_backspace(prompt: &mut ClusterNamingPromptState) {
    if prompt_selection_range(prompt).is_some() {
        prompt_replace_selection(prompt, "");
        return;
    }
    if prompt.caret_char == 0 {
        return;
    }
    let start = prompt.caret_char - 1;
    let start_byte = prompt_char_to_byte(prompt.input.as_str(), start);
    let end_byte = prompt_char_to_byte(prompt.input.as_str(), prompt.caret_char);
    prompt.input.replace_range(start_byte..end_byte, "");
    prompt.caret_char = start;
    prompt.selection_anchor_char = start;
    prompt.selection_focus_char = start;
    prompt.scroll_char = prompt.scroll_char.min(start);
}

fn prompt_delete_forward(prompt: &mut ClusterNamingPromptState) {
    if prompt_selection_range(prompt).is_some() {
        prompt_replace_selection(prompt, "");
        return;
    }
    let char_len = prompt_char_len(prompt.input.as_str());
    if prompt.caret_char >= char_len {
        return;
    }
    let start_byte = prompt_char_to_byte(prompt.input.as_str(), prompt.caret_char);
    let end_byte = prompt_char_to_byte(prompt.input.as_str(), prompt.caret_char + 1);
    prompt.input.replace_range(start_byte..end_byte, "");
    prompt.selection_anchor_char = prompt.caret_char;
    prompt.selection_focus_char = prompt.caret_char;
}

fn parse_reserved_generic_cluster_name(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let digits = lower.strip_prefix("cluster ")?;
    let slot = digits.parse::<u32>().ok()?;
    (slot > 0).then_some(slot)
}

impl<T: Deref<Target = Halley>> ClusterSystemController<T> {
    pub(crate) fn cluster_slot_order_for_monitor(&self, monitor: &str) -> Vec<ClusterId> {
        self.model
            .cluster_state
            .cluster_slot_order
            .get(monitor)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|&cid| {
                self.model.field.cluster(cid).is_some()
                    && self.preferred_monitor_for_cluster(cid, None).as_deref() == Some(monitor)
            })
            .collect()
    }

    pub(crate) fn cluster_slot_cluster_for_monitor(
        &self,
        monitor: &str,
        slot: u8,
    ) -> Option<ClusterId> {
        let slot_index = usize::from(slot.saturating_sub(1));
        self.cluster_slot_order_for_monitor(monitor)
            .get(slot_index)
            .copied()
    }

    pub(crate) fn cluster_name_prompt_active_for_monitor(&self, monitor: &str) -> bool {
        self.model
            .cluster_state
            .cluster_name_prompt
            .contains_key(monitor)
    }

    pub(crate) fn active_cluster_name_prompt_monitor(&self, preferred: &str) -> Option<String> {
        if self.cluster_name_prompt_active_for_monitor(preferred) {
            return Some(preferred.to_string());
        }
        (self.model.cluster_state.cluster_name_prompt.len() == 1)
            .then(|| {
                self.model
                    .cluster_state
                    .cluster_name_prompt
                    .keys()
                    .next()
                    .cloned()
            })
            .flatten()
    }

    pub(crate) fn cluster_name_record(&self, cid: ClusterId) -> Option<&ClusterNameRecord> {
        self.model.cluster_state.cluster_names.get(&cid)
    }

    pub(crate) fn cluster_display_name(&self, cid: ClusterId) -> Option<String> {
        match self.cluster_name_record(cid)? {
            ClusterNameRecord::Generic { slot } => Some(format!("Cluster {slot}")),
            ClusterNameRecord::Custom { name } => Some(name.clone()),
        }
    }

    pub(crate) fn next_generic_cluster_slot_for_monitor(
        &self,
        monitor: &str,
        exclude: Option<ClusterId>,
    ) -> u32 {
        let mut used = std::collections::HashSet::new();
        for (&cid, record) in &self.model.cluster_state.cluster_names {
            if Some(cid) == exclude {
                continue;
            }
            let ClusterNameRecord::Generic { slot } = record else {
                continue;
            };
            if self.preferred_monitor_for_cluster(cid, None).as_deref() == Some(monitor) {
                used.insert(*slot);
            }
        }
        let mut slot = 1;
        while used.contains(&slot) {
            slot += 1;
        }
        slot
    }

    fn resolve_unique_custom_cluster_name(
        &self,
        proposed: &str,
        exclude: Option<ClusterId>,
    ) -> String {
        let base = proposed.trim();
        if base.is_empty() {
            return "Cluster".to_string();
        }
        let mut candidate = base.to_string();
        let mut suffix = 1u32;
        while self
            .model
            .cluster_state
            .cluster_names
            .iter()
            .filter(|(cid, _)| Some(**cid) != exclude)
            .filter_map(|(_, record)| match record {
                ClusterNameRecord::Custom { name } => Some(name),
                ClusterNameRecord::Generic { .. } => None,
            })
            .any(|name| name.eq_ignore_ascii_case(candidate.as_str()))
        {
            candidate = format!("{base} ({suffix})");
            suffix += 1;
        }
        candidate
    }
}

impl<T: DerefMut<Target = Halley>> ClusterSystemController<T> {
    fn open_cluster_name_prompt_for_monitor(
        &mut self,
        monitor: &str,
        name_hint: Option<&str>,
        select_all: bool,
        show_banner: bool,
    ) -> bool {
        let generated_generic_name = format!(
            "Cluster {}",
            self.next_generic_cluster_slot_for_monitor(monitor, None)
        );
        let input = name_hint
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| generated_generic_name.clone());
        let char_len = prompt_char_len(input.as_str());
        let selection_anchor_char = if select_all { 0 } else { char_len };
        self.model.cluster_state.cluster_name_prompt.insert(
            monitor.to_string(),
            ClusterNamingPromptState {
                generated_generic_name,
                input,
                caret_char: char_len,
                selection_anchor_char,
                selection_focus_char: char_len,
                scroll_char: 0,
                confirm_hover_mix: 0.0,
            },
        );
        self.begin_modal_keyboard_capture();
        if show_banner {
            cluster_name_prompt_banner(self, monitor);
        } else {
            self.ui.render_state.remove_persistent_mode_banner(monitor);
        }
        true
    }

    pub(crate) fn open_lift_cluster_finalize_draft(
        &mut self,
        monitor: &str,
        name_hint: Option<String>,
        app_ids: Vec<String>,
        app_launches: Vec<ClusterFinalizeAppLaunch>,
        running_node_ids: Vec<NodeId>,
        now: Instant,
    ) -> bool {
        let app_launches = normalized_draft_app_launches(app_launches);
        let app_ids = normalized_draft_app_ids(app_ids);
        let mut selected = running_node_ids
            .into_iter()
            .filter(|node_id| {
                self.model.field.node(*node_id).is_some_and(|node| {
                    node.kind == NodeKind::Surface
                        && node.state != NodeState::Core
                        && self.model.field.is_visible(*node_id)
                })
            })
            .collect::<std::collections::HashSet<_>>();
        for (&node_id, node) in self.model.field.nodes() {
            if node.kind != NodeKind::Surface
                || node.state == NodeState::Core
                || !self.model.field.is_visible(node_id)
                || self
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&node_id)
                    .is_none_or(|node_monitor| node_monitor != monitor)
            {
                continue;
            }
            if let Some(app_id) = self.model.node_app_ids.get(&node_id)
                && draft_app_ids_match(&app_ids, app_id)
            {
                selected.insert(node_id);
            }
        }
        self.model
            .cluster_state
            .cluster_mode_selected_nodes
            .insert(monitor.to_string(), selected.clone());
        self.model.cluster_state.cluster_finalize_drafts.insert(
            monitor.to_string(),
            ClusterFinalizeDraftState {
                app_ids,
                app_launches,
                selected_node_ids: selected,
            },
        );
        let select_all = name_hint
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty);
        let opened = self.open_cluster_name_prompt_for_monitor(
            monitor,
            name_hint.as_deref(),
            select_all,
            false,
        );
        if opened {
            self.request_maintenance();
        } else {
            self.model
                .cluster_state
                .cluster_finalize_drafts
                .remove(monitor);
            self.model
                .cluster_state
                .cluster_mode_selected_nodes
                .remove(monitor);
        }
        let _ = now;
        opened
    }

    pub(crate) fn maybe_add_node_to_lift_cluster_finalize_draft(
        &mut self,
        monitor: &str,
        node_id: NodeId,
        app_id: &str,
    ) -> bool {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            return false;
        }
        if let Some(build) = self
            .model
            .cluster_state
            .pending_lift_cluster_builds
            .get_mut(monitor)
        {
            let app_ids = build
                .app_launches
                .iter()
                .map(|launch| launch.app_id.clone())
                .collect::<Vec<_>>();
            if !draft_app_ids_match(&app_ids, app_id) {
                return false;
            }
            if !build.selected_node_ids.insert(node_id) {
                return false;
            }
            self.model
                .spawn_state
                .pending_initial_reveal
                .remove(&node_id);
            let _ = self.model.field.set_detached(node_id, true);
            let completed = self.try_complete_pending_lift_cluster_build(monitor, Instant::now());
            if !completed {
                self.request_maintenance();
            }
            return true;
        }

        let Some(draft) = self
            .model
            .cluster_state
            .cluster_finalize_drafts
            .get_mut(monitor)
        else {
            return false;
        };
        if !draft_app_ids_match(&draft.app_ids, app_id) {
            return false;
        }
        let inserted = draft.selected_node_ids.insert(node_id);
        self.model
            .cluster_state
            .cluster_mode_selected_nodes
            .entry(monitor.to_string())
            .or_default()
            .insert(node_id);
        if inserted {
            self.request_maintenance();
        }
        inserted
    }

    fn record_cluster_slot_for_monitor(&mut self, cid: ClusterId, monitor: &str) {
        let target_monitor = monitor.to_string();
        let already_on_target = self
            .model
            .cluster_state
            .cluster_slot_order
            .get(target_monitor.as_str())
            .is_some_and(|order| order.contains(&cid));
        for (name, order) in &mut self.model.cluster_state.cluster_slot_order {
            if name != &target_monitor {
                order.retain(|existing| *existing != cid);
            }
        }
        if !already_on_target {
            self.model
                .cluster_state
                .cluster_slot_order
                .entry(target_monitor)
                .or_default()
                .push(cid);
        }
    }

    fn remove_cluster_slot_record(&mut self, cid: ClusterId) {
        self.model
            .cluster_state
            .cluster_slot_order
            .retain(|_, order| {
                order.retain(|existing| *existing != cid);
                !order.is_empty()
            });
        self.model
            .cluster_state
            .pending_cluster_slot_transition
            .retain(|_, pending| pending.cid != cid);
    }

    pub(crate) fn relabel_cluster_core(&mut self, cid: ClusterId) -> bool {
        let Some(label) = self.cluster_display_name(cid) else {
            return false;
        };
        let Some(core_id) = self
            .model
            .field
            .cluster(cid)
            .and_then(|cluster| cluster.core)
        else {
            return false;
        };
        if let Some(node) = self.model.field.node_mut(core_id) {
            node.label = label;
            return true;
        }
        false
    }

    pub(crate) fn ensure_cluster_name_record_for_monitor(
        &mut self,
        cid: ClusterId,
        monitor: &str,
    ) -> bool {
        if self.model.cluster_state.cluster_names.contains_key(&cid) {
            self.record_cluster_slot_for_monitor(cid, monitor);
            return self.relabel_cluster_core(cid);
        }
        let slot = self.next_generic_cluster_slot_for_monitor(monitor, Some(cid));
        self.model
            .cluster_state
            .cluster_names
            .insert(cid, ClusterNameRecord::Generic { slot });
        self.record_cluster_slot_for_monitor(cid, monitor);
        self.relabel_cluster_core(cid)
    }

    pub(crate) fn sync_cluster_name_for_monitor(&mut self, cid: ClusterId, monitor: &str) -> bool {
        let next_record = match self.model.cluster_state.cluster_names.get(&cid).cloned() {
            Some(ClusterNameRecord::Generic { .. }) => ClusterNameRecord::Generic {
                slot: self.next_generic_cluster_slot_for_monitor(monitor, Some(cid)),
            },
            Some(ClusterNameRecord::Custom { name }) => ClusterNameRecord::Custom { name },
            None => ClusterNameRecord::Generic {
                slot: self.next_generic_cluster_slot_for_monitor(monitor, Some(cid)),
            },
        };
        self.model
            .cluster_state
            .cluster_names
            .insert(cid, next_record);
        self.record_cluster_slot_for_monitor(cid, monitor);
        self.relabel_cluster_core(cid)
    }

    pub(crate) fn remove_cluster_name_record(&mut self, cid: ClusterId) {
        self.model.cluster_state.cluster_names.remove(&cid);
        self.remove_cluster_slot_record(cid);
    }

    pub(crate) fn sync_cluster_name_for_node_monitor(&mut self, node_id: NodeId, monitor: &str) {
        if let Some(cid) = self.model.field.cluster_id_for_core_public(node_id) {
            let _ = self.sync_cluster_name_for_monitor(cid, monitor);
        }
    }

    pub(crate) fn open_cluster_name_prompt(&mut self, now: Instant) -> bool {
        let monitor = self.model.monitor_state.current_monitor.clone();
        let Some(selected_nodes) = self
            .model
            .cluster_state
            .cluster_mode_selected_nodes
            .get(monitor.as_str())
        else {
            return false;
        };
        let now_ms = self.now_ms(now);
        if selected_nodes.is_empty() {
            self.ui.render_state.show_overlay_toast(
                monitor.as_str(),
                "Not enough selections\nSelect at least one window",
                3000,
                now_ms,
            );
            return false;
        }

        let _ = now;
        self.open_cluster_name_prompt_for_monitor(monitor.as_str(), None, true, true)
    }

    pub(crate) fn cancel_cluster_name_prompt_for_monitor(&mut self, monitor: &str) -> bool {
        let removed = self
            .model
            .cluster_state
            .cluster_name_prompt
            .remove(monitor)
            .is_some();
        if self
            .input
            .interaction_state
            .cluster_name_prompt_drag_monitor
            .as_deref()
            == Some(monitor)
        {
            self.input
                .interaction_state
                .cluster_name_prompt_drag_monitor = None;
        }
        if self
            .input
            .interaction_state
            .cluster_name_prompt_repeat
            .as_ref()
            .is_some_and(|repeat| repeat.monitor == monitor)
        {
            self.input.interaction_state.cluster_name_prompt_repeat = None;
        }
        if removed && self.cluster_mode_active_for_monitor(monitor) {
            cluster_mode_selection_banner(self, monitor);
        }
        if removed {
            self.model
                .cluster_state
                .cluster_finalize_drafts
                .remove(monitor);
            let focused_surface = self
                .last_input_surface_node_for_monitor(monitor)
                .or(self.last_input_surface_node());
            self.schedule_modal_focus_restore(focused_surface, Instant::now());
        }
        removed
    }

    pub(crate) fn insert_cluster_name_prompt_char_for_monitor(
        &mut self,
        monitor: &str,
        ch: char,
    ) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        prompt_replace_selection(prompt, ch.encode_utf8(&mut [0; 4]));
        true
    }

    pub(crate) fn cluster_name_prompt_backspace_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        prompt_delete_backspace(prompt);
        true
    }

    pub(crate) fn cluster_name_prompt_delete_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        prompt_delete_forward(prompt);
        true
    }

    pub(crate) fn cluster_name_prompt_move_left_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        if let Some((start, _)) = prompt_selection_range(prompt) {
            prompt.caret_char = start;
        } else if prompt.caret_char > 0 {
            prompt.caret_char -= 1;
        }
        prompt.selection_anchor_char = prompt.caret_char;
        prompt.selection_focus_char = prompt.caret_char;
        true
    }

    pub(crate) fn cluster_name_prompt_move_right_for_monitor(&mut self, monitor: &str) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        let char_len = prompt_char_len(prompt.input.as_str());
        if let Some((_, end)) = prompt_selection_range(prompt) {
            prompt.caret_char = end;
        } else if prompt.caret_char < char_len {
            prompt.caret_char += 1;
        }
        prompt.selection_anchor_char = prompt.caret_char;
        prompt.selection_focus_char = prompt.caret_char;
        true
    }

    pub(crate) fn begin_cluster_name_prompt_drag_for_monitor(
        &mut self,
        monitor: &str,
        caret_char: usize,
    ) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        let char_len = prompt_char_len(prompt.input.as_str());
        prompt.caret_char = caret_char.min(char_len);
        prompt.selection_anchor_char = prompt.caret_char;
        prompt.selection_focus_char = prompt.caret_char;
        self.input
            .interaction_state
            .cluster_name_prompt_drag_monitor = Some(monitor.to_string());
        true
    }

    pub(crate) fn drag_cluster_name_prompt_selection_for_monitor(
        &mut self,
        monitor: &str,
        caret_char: usize,
    ) -> bool {
        if self
            .input
            .interaction_state
            .cluster_name_prompt_drag_monitor
            .as_deref()
            != Some(monitor)
        {
            return false;
        }
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get_mut(monitor)
        else {
            return false;
        };
        let char_len = prompt_char_len(prompt.input.as_str());
        prompt.caret_char = caret_char.min(char_len);
        prompt.selection_focus_char = prompt.caret_char;
        true
    }

    pub(crate) fn end_cluster_name_prompt_drag_for_monitor(&mut self, monitor: &str) -> bool {
        if self
            .input
            .interaction_state
            .cluster_name_prompt_drag_monitor
            .as_deref()
            != Some(monitor)
        {
            return false;
        }
        self.input
            .interaction_state
            .cluster_name_prompt_drag_monitor = None;
        true
    }

    pub(crate) fn start_cluster_name_prompt_repeat_for_monitor(
        &mut self,
        monitor: &str,
        code: u32,
        action: ClusterNamePromptRepeatAction,
        now_ms: u64,
    ) {
        self.input.interaction_state.cluster_name_prompt_repeat =
            Some(ClusterNamePromptRepeatState {
                monitor: monitor.to_string(),
                code,
                action,
                next_repeat_ms: now_ms.saturating_add(360),
                interval_ms: 34,
            });
        self.request_maintenance();
    }

    pub(crate) fn stop_cluster_name_prompt_repeat_for_code(&mut self, code: u32) {
        if self
            .input
            .interaction_state
            .cluster_name_prompt_repeat
            .as_ref()
            .is_some_and(|repeat| repeat.code == code)
        {
            self.input.interaction_state.cluster_name_prompt_repeat = None;
        }
    }

    pub(crate) fn repeat_cluster_name_prompt_input_if_due(&mut self, now_ms: u64) -> bool {
        let Some(repeat) = self
            .input
            .interaction_state
            .cluster_name_prompt_repeat
            .clone()
        else {
            return false;
        };
        if now_ms < repeat.next_repeat_ms {
            return false;
        }
        let handled = match repeat.action {
            ClusterNamePromptRepeatAction::Insert(ch) => {
                self.insert_cluster_name_prompt_char_for_monitor(repeat.monitor.as_str(), ch)
            }
            ClusterNamePromptRepeatAction::Backspace => {
                self.cluster_name_prompt_backspace_for_monitor(repeat.monitor.as_str())
            }
            ClusterNamePromptRepeatAction::Delete => {
                self.cluster_name_prompt_delete_for_monitor(repeat.monitor.as_str())
            }
            ClusterNamePromptRepeatAction::MoveLeft => {
                self.cluster_name_prompt_move_left_for_monitor(repeat.monitor.as_str())
            }
            ClusterNamePromptRepeatAction::MoveRight => {
                self.cluster_name_prompt_move_right_for_monitor(repeat.monitor.as_str())
            }
        };
        if handled {
            if let Some(state) = self
                .input
                .interaction_state
                .cluster_name_prompt_repeat
                .as_mut()
            {
                state.next_repeat_ms = now_ms.saturating_add(state.interval_ms);
            }
            self.request_maintenance();
        }
        handled
    }

    fn prompt_name_record(
        &self,
        monitor: &str,
        prompt: &ClusterNamingPromptState,
    ) -> ClusterNameRecord {
        let submitted = prompt.input.trim();
        let reserved_generic = parse_reserved_generic_cluster_name(submitted).is_some();
        let exact_default = submitted.eq_ignore_ascii_case(prompt.generated_generic_name.as_str());
        if submitted.is_empty() || exact_default || reserved_generic {
            ClusterNameRecord::Generic {
                slot: self.next_generic_cluster_slot_for_monitor(monitor, None),
            }
        } else {
            ClusterNameRecord::Custom {
                name: self.resolve_unique_custom_cluster_name(submitted, None),
            }
        }
    }

    fn pending_lift_expected_members(
        &self,
        selected_nodes: &std::collections::HashSet<NodeId>,
        app_launches: &[ClusterFinalizeAppLaunch],
    ) -> usize {
        selected_nodes.len() + app_launches.len()
    }

    fn start_pending_lift_cluster_build(
        &mut self,
        monitor: &str,
        draft: ClusterFinalizeDraftState,
        name_record: ClusterNameRecord,
        now: Instant,
    ) -> bool {
        let expected_members = self
            .pending_lift_expected_members(&draft.selected_node_ids, draft.app_launches.as_slice());
        self.model.cluster_state.cluster_name_prompt.remove(monitor);
        self.model
            .cluster_state
            .cluster_finalize_drafts
            .remove(monitor);
        let _ = self
            .cluster_mutation_controller()
            .exit_cluster_mode(monitor);
        self.ui.render_state.clear_persistent_mode_banner(monitor);
        self.model.cluster_state.pending_lift_cluster_builds.insert(
            monitor.to_string(),
            PendingLiftClusterBuildState {
                selected_node_ids: draft.selected_node_ids,
                app_launches: draft.app_launches,
                name_record,
                expected_members,
                launched: false,
            },
        );
        if self.try_complete_pending_lift_cluster_build(monitor, now) {
            return true;
        }
        self.launch_pending_lift_cluster_apps(monitor, now);
        self.request_maintenance();
        true
    }

    fn finish_lift_finalized_cluster(
        &mut self,
        cid: ClusterId,
        monitor: &str,
        name_record: ClusterNameRecord,
        now: Instant,
    ) -> Option<NodeId> {
        self.model
            .cluster_state
            .cluster_names
            .insert(cid, name_record);
        let core_id = self.collapse_cluster(cid)?;
        let _ = self.sync_cluster_monitor(cid, Some(monitor));
        let target_pos = self.view_center_for_monitor(monitor);
        if let Some(core) = self.model.field.node_mut(core_id) {
            core.pos = target_pos;
        }
        let now_ms = self.now_ms(now);
        let _ = self.model.field.touch(core_id, now_ms);
        self.set_interaction_focus(Some(core_id), 30_000, now);
        Some(core_id)
    }

    fn launch_pending_lift_cluster_apps(&mut self, monitor: &str, now: Instant) {
        let launches = {
            let Some(build) = self
                .model
                .cluster_state
                .pending_lift_cluster_builds
                .get_mut(monitor)
            else {
                return;
            };
            if build.launched {
                return;
            }
            build.launched = true;
            build.app_launches.clone()
        };

        let wayland_display = self
            .runtime
            .wayland_display
            .clone()
            .or_else(|| std::env::var("WAYLAND_DISPLAY").ok())
            .unwrap_or_default();
        self.model.spawn_state.pending_spawn_monitor = Some(monitor.to_string());
        for launch in launches {
            if launch.command.trim().is_empty() {
                continue;
            }
            if let Some(child) = crate::input::keyboard::spawn::spawn_command(
                launch.command.as_str(),
                wayland_display.as_str(),
                &self.runtime.tuning.cursor,
                None,
                "lift cluster app",
            ) {
                self.runtime.spawned_children.push(child);
            }
        }
        let _ = now;
    }

    fn try_complete_pending_lift_cluster_build(&mut self, monitor: &str, now: Instant) -> bool {
        let Some(build) = self
            .model
            .cluster_state
            .pending_lift_cluster_builds
            .get(monitor)
            .cloned()
        else {
            return false;
        };
        if build.selected_node_ids.len() < build.expected_members
            || build.selected_node_ids.is_empty()
        {
            return false;
        }
        let members =
            self.order_cluster_creation_members(build.selected_node_ids.iter().copied().collect());
        for member in &members {
            let _ = self.model.field.set_detached(*member, false);
        }
        let created = self.create_cluster(members).ok().and_then(|cid| {
            self.finish_lift_finalized_cluster(cid, monitor, build.name_record.clone(), now)
        });
        if created.is_none() {
            return false;
        }
        self.model
            .cluster_state
            .pending_lift_cluster_builds
            .remove(monitor);
        true
    }

    pub(crate) fn confirm_cluster_name_prompt_for_monitor(
        &mut self,
        monitor: &str,
        now: Instant,
    ) -> bool {
        let Some(prompt) = self
            .model
            .cluster_state
            .cluster_name_prompt
            .get(monitor)
            .cloned()
        else {
            return false;
        };
        let Some(selected_nodes) = self
            .model
            .cluster_state
            .cluster_mode_selected_nodes
            .get(monitor)
            .cloned()
        else {
            return false;
        };
        let draft = self
            .model
            .cluster_state
            .cluster_finalize_drafts
            .get(monitor)
            .cloned();
        let name_record = self.prompt_name_record(monitor, &prompt);
        if let Some(draft) = draft.clone()
            && !draft.app_launches.is_empty()
        {
            return self.start_pending_lift_cluster_build(monitor, draft, name_record, now);
        }
        if selected_nodes.is_empty() {
            if self
                .model
                .cluster_state
                .cluster_finalize_drafts
                .contains_key(monitor)
            {
                let now_ms = self.now_ms(now);
                self.ui.render_state.show_overlay_toast(
                    monitor,
                    "Waiting for staged apps\nNeed at least one window",
                    3000,
                    now_ms,
                );
                return true;
            }
            return false;
        }
        let members = self.order_cluster_creation_members(selected_nodes.iter().copied().collect());
        let created = self.create_cluster(members).ok().and_then(|cid| {
            if draft.is_some() {
                self.finish_lift_finalized_cluster(cid, monitor, name_record.clone(), now)
            } else {
                let now_ms = self.now_ms(now);
                self.model
                    .cluster_state
                    .cluster_names
                    .insert(cid, name_record.clone());
                let core = self.collapse_cluster(cid);
                if let Some(core_id) = core {
                    self.assign_node_to_monitor(core_id, monitor);
                    let _ = self.sync_cluster_name_for_monitor(cid, monitor);
                    let _ = self.model.field.touch(core_id, now_ms);
                    self.set_interaction_focus(Some(core_id), 30_000, now);
                }
                core
            }
        });
        if created.is_some() {
            self.model.cluster_state.cluster_name_prompt.remove(monitor);
            self.model
                .cluster_state
                .cluster_finalize_drafts
                .remove(monitor);
            let _ = self
                .cluster_mutation_controller()
                .exit_cluster_mode(monitor);
            self.ui.render_state.clear_persistent_mode_banner(monitor);
            let focused_surface = self
                .last_input_surface_node_for_monitor(monitor)
                .or(self.last_input_surface_node());
            self.schedule_modal_focus_restore(focused_surface, Instant::now());
            return true;
        }
        false
    }
}

fn normalized_draft_app_ids(app_ids: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for app_id in app_ids {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            continue;
        }
        let folded = app_id.to_ascii_lowercase();
        if !out.iter().any(|existing: &String| existing == &folded) {
            out.push(folded);
        }
    }
    out
}

fn normalized_draft_app_launches(
    app_launches: Vec<ClusterFinalizeAppLaunch>,
) -> Vec<ClusterFinalizeAppLaunch> {
    let mut out = Vec::new();
    for launch in app_launches {
        let app_id = launch.app_id.trim().to_ascii_lowercase();
        let command = launch.command.trim().to_string();
        if app_id.is_empty() || command.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|existing: &ClusterFinalizeAppLaunch| existing.app_id == app_id)
        {
            continue;
        }
        out.push(ClusterFinalizeAppLaunch { app_id, command });
    }
    out
}

fn draft_app_ids_match(app_ids: &[String], app_id: &str) -> bool {
    let folded = app_id.to_ascii_lowercase();
    app_ids.iter().any(|candidate| {
        candidate == &folded
            || folded.ends_with(candidate)
            || candidate.ends_with(&folded)
            || compact_app_match_token(folded.as_str()) == compact_app_match_token(candidate)
    })
}

fn compact_app_match_token(value: &str) -> Option<&str> {
    value
        .trim_end_matches(".desktop")
        .rsplit(['.', '/'])
        .next()
        .filter(|token| !token.is_empty())
}
