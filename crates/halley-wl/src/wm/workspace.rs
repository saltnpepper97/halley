use super::*;

impl HalleyWlState {
    pub fn toggle_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        if let Some(cid) = self.active_cluster_workspace
            && self.field.cluster_id_for_core_public(core_id) == Some(cid)
        {
            return self.exit_cluster_workspace(now);
        }
        self.enter_cluster_workspace_by_core(core_id, now)
    }

    pub fn has_active_cluster_workspace(&self) -> bool {
        self.active_cluster_workspace.is_some()
    }

    pub fn exit_cluster_workspace_if_member(&mut self, member: NodeId, now: Instant) -> bool {
        let Some(cid) = self.active_cluster_workspace else {
            return false;
        };
        let Some(c) = self.field.cluster(cid) else {
            return false;
        };
        if !c.members.contains(&member) {
            return false;
        }
        self.exit_cluster_workspace(now)
    }

    fn enter_cluster_workspace_by_core(&mut self, core_id: NodeId, now: Instant) -> bool {
        let Some(cid) = self.field.cluster_id_for_core_public(core_id) else {
            return false;
        };
        if self.active_cluster_workspace == Some(cid) {
            return true;
        }
        if self.active_cluster_workspace.is_some() {
            let _ = self.exit_cluster_workspace(now);
        }

        let Some(cluster) = self.field.cluster(cid) else {
            return false;
        };
        let members = cluster.members.clone();
        if members.is_empty() {
            return false;
        }

        self.workspace_prev_viewport = Some(self.viewport);
        self.zoom_ref_size = self.viewport.size;
        if let Some(core) = self.field.node(core_id) {
            self.viewport.center = core.pos;
        }
        self.snap_camera_targets_to_live();
        self.tuning.viewport_center = self.viewport.center;
        self.tuning.viewport_size = self.viewport.size;

        let mut hidden = Vec::new();
        let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
        for id in ids {
            let is_member = members.contains(&id);
            if is_member || id == core_id {
                continue;
            }
            let already_detached = self
                .field
                .node(id)
                .is_some_and(|n| n.visibility.has(Visibility::DETACHED));
            if !already_detached {
                let _ = self.field.set_detached(id, true);
                hidden.push(id);
            }
        }
        let _ = self.field.set_detached(core_id, true);
        let _ = self.field.expand_cluster(cid);
        if let Some(c) = self.field.cluster_mut(cid) {
            c.enter_active(ActiveLayoutMode::TiledWeighted);
        }

        self.workspace_hidden_nodes = hidden;
        self.active_cluster_workspace = Some(cid);
        self.set_interaction_focus(None, 0, now);
        self.layout_active_cluster_workspace(self.now_ms(now));
        true
    }

    fn exit_cluster_workspace(&mut self, now: Instant) -> bool {
        let Some(cid) = self.active_cluster_workspace else {
            return false;
        };

        for id in self.workspace_hidden_nodes.drain(..) {
            let _ = self.field.set_detached(id, false);
        }

        let core_before = self.field.cluster(cid).and_then(|c| c.core);
        if let Some(c) = self.field.cluster_mut(cid) {
            c.set_collapsed(false);
        }
        let core = self.field.collapse_cluster(cid).or(core_before);
        if let Some(core_id) = core {
            let _ = self.field.set_detached(core_id, false);
            let _ = self.field.touch(core_id, self.now_ms(now));
        }

        if let Some(vp) = self.workspace_prev_viewport.take() {
            self.viewport = vp;
            self.zoom_ref_size = self.viewport.size;
            self.snap_camera_targets_to_live();
            self.tuning.viewport_center = self.viewport.center;
            self.tuning.viewport_size = self.viewport.size;
        }
        self.active_cluster_workspace = None;
        true
    }

    pub(crate) fn layout_active_cluster_workspace(&mut self, now_ms: u64) {
        let Some(cid) = self.active_cluster_workspace else {
            return;
        };
        // Workspace mode stays fixed to the logical fullscreen view while active.
        self.zoom_ref_size = self.viewport.size;
        self.camera_target_view_size = self.zoom_ref_size;
        self.tuning.viewport_size = self.viewport.size;
        let Some(cluster) = self.field.cluster(cid) else {
            self.active_cluster_workspace = None;
            return;
        };
        let members = cluster.members.clone();
        if members.is_empty() {
            return;
        }
        let mut wm = halley_core::tiling::WeightModel::new();
        if let Some(a) = &cluster.active {
            for (&id, &w) in &a.weights {
                wm.weights.insert(id.as_u64(), w);
            }
        }

        let world_rect = halley_core::tiling::Rect {
            x: self.viewport.center.x - self.viewport.size.x * 0.5,
            y: self.viewport.center.y - self.viewport.size.y * 0.5,
            w: self.viewport.size.x,
            h: self.viewport.size.y,
        };
        let ids_u64: Vec<u64> = members.iter().map(|id| id.as_u64()).collect();
        let mut recency = ids_u64.clone();
        recency.sort_unstable_by(|a, b| b.cmp(a));
        let out = halley_core::tiling::layout_weighted_tiling(
            world_rect,
            &ids_u64,
            None,
            &wm,
            &recency,
            halley_core::tiling::TilingParams::default(),
            0,
        );

        for t in out.majors {
            let nid = NodeId::new(t.id);
            let _ = self.field.set_detached(nid, false);
            let _ = self
                .field
                .set_state(nid, halley_core::field::NodeState::Active);
            if let Some(n) = self.field.node_mut(nid) {
                n.intrinsic_size.x = t.rect.w.max(64.0);
                n.intrinsic_size.y = t.rect.h.max(64.0);
            }
            let _ = self.field.carry(
                nid,
                Vec2 {
                    x: t.rect.x + t.rect.w * 0.5,
                    y: t.rect.y + t.rect.h * 0.5,
                },
            );
            let _ = self.field.touch(nid, now_ms);
            self.request_toplevel_resize(nid, t.rect.w.round() as i32, t.rect.h.round() as i32);
        }
        for t in out.bay {
            let nid = NodeId::new(t.id);
            let _ = self.field.set_detached(nid, false);
            let _ = self
                .field
                .set_state(nid, halley_core::field::NodeState::Node);
            if let Some(n) = self.field.node_mut(nid) {
                n.intrinsic_size.x = t.rect.w.max(64.0);
                n.intrinsic_size.y = t.rect.h.max(64.0);
            }
            let _ = self.field.carry(
                nid,
                Vec2 {
                    x: t.rect.x + t.rect.w * 0.5,
                    y: t.rect.y + t.rect.h * 0.5,
                },
            );
            let _ = self.field.touch(nid, now_ms);
            self.request_toplevel_resize(nid, t.rect.w.round() as i32, t.rect.h.round() as i32);
        }
    }
}
