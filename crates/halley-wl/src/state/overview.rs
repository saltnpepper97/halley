use super::*;

pub(super) struct OverviewAnim {
    pub(super) start_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) from_viewport: Viewport,
    pub(super) to_viewport: Viewport,
    pub(super) from_positions: HashMap<NodeId, Vec2>,
    pub(super) to_positions: HashMap<NodeId, Vec2>,
}

impl HalleyWlState {
    pub fn overview_mode_active(&self) -> bool {
        self.overview_mode
    }

    pub fn tick_overview_frame(&mut self, now: Instant) {
        self.tick_overview_animation(self.now_ms(now));
    }

    pub fn toggle_overview_mode(&mut self, now: Instant) {
        let now_ms = self.now_ms(now);
        if self.overview_mode {
            let from_viewport = self.viewport;
            let to_viewport = self.overview_saved_viewport.unwrap_or(self.viewport);
            let ids: Vec<NodeId> = self.field.nodes().keys().copied().collect();
            let mut from_positions = HashMap::new();
            let mut to_positions = HashMap::new();
            for id in ids {
                let Some(n) = self.field.node(id) else {
                    continue;
                };
                from_positions.insert(id, n.pos);
                to_positions.insert(
                    id,
                    self.overview_saved_positions
                        .get(&id)
                        .copied()
                        .unwrap_or(n.pos),
                );
                let restore_state =
                    self.overview_saved_states
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| {
                            if n.kind == halley_core::field::NodeKind::Surface {
                                halley_core::field::NodeState::Active
                            } else {
                                n.state.clone()
                            }
                        });
                let _ = self.field.set_state(id, restore_state);
            }
            self.overview_mode = false;
            self.overview_saved_viewport = None;
            self.overview_saved_positions.clear();
            self.overview_saved_states.clear();
            self.overview_anim = Some(OverviewAnim {
                start_ms: now_ms,
                duration_ms: 340,
                from_viewport,
                to_viewport,
                from_positions,
                to_positions,
            });
            return;
        }

        let from_viewport = self.viewport;
        self.overview_saved_viewport = Some(self.viewport);
        self.overview_saved_positions.clear();
        self.overview_saved_states.clear();

        let mut ids: Vec<NodeId> = self
            .field
            .nodes()
            .keys()
            .copied()
            .filter(|&id| self.field.is_visible(id))
            .filter(|&id| {
                self.field
                    .node(id)
                    .is_some_and(|n| n.kind == halley_core::field::NodeKind::Surface)
            })
            .collect();
        ids.sort_by_key(|id| id.as_u64());
        if ids.is_empty() {
            self.overview_mode = true;
            return;
        }

        let center = self.viewport.center;
        let cols = (ids.len() as f32).sqrt().ceil() as i32;
        let cols = cols.max(1);
        let spacing_x = 200.0f32;
        let spacing_y = 130.0f32;
        let mut to_positions = HashMap::new();
        let mut from_positions = HashMap::new();
        for (idx, id) in ids.iter().enumerate() {
            if let Some(n) = self.field.node(*id) {
                self.overview_saved_positions.insert(*id, n.pos);
                self.overview_saved_states.insert(*id, n.state.clone());
                from_positions.insert(*id, n.pos);
            }
            let i = idx as i32;
            let row = i / cols;
            let col = i % cols;
            let grid_w = (cols - 1) as f32 * spacing_x;
            let rows = ((ids.len() as i32 + cols - 1) / cols).max(1);
            let grid_h = (rows - 1) as f32 * spacing_y;
            let x = center.x - grid_w * 0.5 + col as f32 * spacing_x;
            let y = center.y + grid_h * 0.5 - row as f32 * spacing_y;
            to_positions.insert(*id, Vec2 { x, y });
            let _ = self
                .field
                .set_state(*id, halley_core::field::NodeState::Node);
        }

        let rows = ((ids.len() as i32 + cols - 1) / cols).max(1);
        let to_viewport = Viewport {
            center,
            size: Vec2 {
                x: (cols as f32 * spacing_x + 560.0).max(self.zoom_ref_size.x),
                y: (rows as f32 * spacing_y + 420.0).max(self.zoom_ref_size.y),
            },
            home: from_viewport.home,
        };

        self.overview_mode = true;
        self.overview_anim = Some(OverviewAnim {
            start_ms: now_ms,
            duration_ms: 360,
            from_viewport,
            to_viewport,
            from_positions,
            to_positions,
        });
    }

    pub(super) fn tick_overview_animation(&mut self, now_ms: u64) {
        let Some(anim) = &self.overview_anim else {
            return;
        };
        let dur = anim.duration_ms.max(1);
        let t = ((now_ms.saturating_sub(anim.start_ms)) as f32 / dur as f32).clamp(0.0, 1.0);
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powf(3.0) * 0.5
        };
        for (&id, from) in &anim.from_positions {
            let Some(to) = anim.to_positions.get(&id).copied() else {
                continue;
            };
            let p = Vec2 {
                x: from.x + (to.x - from.x) * e,
                y: from.y + (to.y - from.y) * e,
            };
            let _ = self.field.carry(id, p);
        }
        self.viewport = Viewport {
            center: Vec2 {
                x: anim.from_viewport.center.x
                    + (anim.to_viewport.center.x - anim.from_viewport.center.x) * e,
                y: anim.from_viewport.center.y
                    + (anim.to_viewport.center.y - anim.from_viewport.center.y) * e,
            },
            size: Vec2 {
                x: anim.from_viewport.size.x
                    + (anim.to_viewport.size.x - anim.from_viewport.size.x) * e,
                y: anim.from_viewport.size.y
                    + (anim.to_viewport.size.y - anim.from_viewport.size.y) * e,
            },
            home: anim.from_viewport.home,
        };
        self.tuning.viewport_center = self.viewport.center;
        self.tuning.viewport_size = self.viewport.size;
        if t >= 1.0 {
            self.overview_anim = None;
        }
    }
}
