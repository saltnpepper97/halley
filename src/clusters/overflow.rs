use std::collections::HashMap;
use std::time::Duration;

use halley_core::cluster::ClusterId;
use halley_core::field::NodeId;
use smithay::utils::{Logical, Point, Rectangle};

use super::ClusterSystem;

const STRIP_WIDTH: i32 = 68;
const STRIP_PADDING: i32 = 10;
const ITEM_SIZE: i32 = 44;
const ITEM_GAP: i32 = 8;
pub const REVEAL_EDGE_PX: f64 = 28.0;
const REVEAL_DURATION: Duration = Duration::from_millis(2_200);
const REVEAL_ANIMATION: Duration = Duration::from_millis(220);
const REVEAL_SLIDE_PX: i32 = 28;

#[derive(Clone, Copy, Debug)]
struct VisibilityState {
    revealed_at: Duration,
    visible_until: Duration,
}

#[derive(Clone, Copy, Debug)]
pub struct OverflowDrag {
    pub cluster_id: ClusterId,
    pub member_id: NodeId,
    pub output_local: Point<f64, Logical>,
    pub press_local: Point<f64, Logical>,
}

#[derive(Default)]
pub(super) struct OverflowState {
    visible: HashMap<String, VisibilityState>,
    scroll_offsets: HashMap<String, usize>,
    scroll_residue: HashMap<String, f64>,
    drag: Option<(String, OverflowDrag)>,
}

impl OverflowState {
    pub(super) fn reveal(&mut self, output: &str, now: Duration) -> bool {
        let was_visible = self
            .visible
            .get(output)
            .is_some_and(|state| now < state.visible_until);
        let state = self
            .visible
            .entry(output.to_string())
            .or_insert(VisibilityState {
                revealed_at: now,
                visible_until: now,
            });
        if !was_visible {
            state.revealed_at = now;
        }
        state.visible_until = now.saturating_add(REVEAL_DURATION);
        !was_visible
    }

    pub(super) fn hide(&mut self, output: &str) -> bool {
        let removed = self.visible.remove(output).is_some();
        self.scroll_offsets.remove(output);
        self.scroll_residue.remove(output);
        if self
            .drag
            .as_ref()
            .is_some_and(|(candidate, _)| candidate == output)
        {
            self.drag = None;
        }
        removed
    }

    pub(super) fn remove_cluster(&mut self, cluster_id: ClusterId) {
        if self
            .drag
            .as_ref()
            .is_some_and(|(_, drag)| drag.cluster_id == cluster_id)
        {
            self.drag = None;
        }
    }

    fn visibility_mix(&self, output: &str, now: Duration) -> Option<f32> {
        let state = self.visible.get(output)?;
        if now >= state.visible_until {
            return None;
        }
        let intro = if REVEAL_ANIMATION.is_zero() {
            1.0
        } else {
            now.saturating_sub(state.revealed_at).as_secs_f32() / REVEAL_ANIMATION.as_secs_f32()
        }
        .clamp(0.0, 1.0);
        let outro = if REVEAL_ANIMATION.is_zero() {
            1.0
        } else {
            state.visible_until.saturating_sub(now).as_secs_f32() / REVEAL_ANIMATION.as_secs_f32()
        }
        .clamp(0.0, 1.0);
        let smooth = |value: f32| value * value * (3.0 - 2.0 * value);
        Some(smooth(intro).min(smooth(outro)))
    }

    pub(super) fn wakeup(&mut self, now: Duration) -> bool {
        let before = self.visible.len();
        self.visible.retain(|_, state| now < state.visible_until);
        let expired = self.visible.len() != before;
        let animating = self.visible.values().any(|state| {
            now.saturating_sub(state.revealed_at) < REVEAL_ANIMATION
                || state.visible_until.saturating_sub(now) <= REVEAL_ANIMATION
        });
        expired || animating
    }

    pub(super) fn is_revealed(&self, output: &str) -> bool {
        self.visible.contains_key(output)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverflowItem {
    pub node_id: NodeId,
    pub rect: Rectangle<i32, Logical>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverflowScrollbar {
    pub track: Rectangle<i32, Logical>,
    pub thumb: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverflowLayout {
    pub strip: Rectangle<i32, Logical>,
    pub items: Vec<OverflowItem>,
    pub scroll_offset: usize,
    pub total_items: usize,
    pub scrollbar: Option<OverflowScrollbar>,
    pub visibility: f32,
}

impl ClusterSystem {
    pub(super) fn overflow_geometry(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
    ) -> Option<OverflowLayout> {
        let active = self.active_on(output)?;
        let layout = self.workspace_layout(active, work_area)?;
        if layout.queue_members.is_empty() {
            return None;
        }

        let available_height = (work_area.size.h - STRIP_PADDING * 2).max(ITEM_SIZE);
        let visible_count = ((available_height + ITEM_GAP) / (ITEM_SIZE + ITEM_GAP))
            .max(1)
            .min(layout.queue_members.len() as i32) as usize;
        let max_offset = layout.queue_members.len().saturating_sub(visible_count);
        let scroll_offset = self
            .overflow
            .scroll_offsets
            .get(output)
            .copied()
            .unwrap_or(0)
            .min(max_offset);
        let content_height =
            visible_count as i32 * ITEM_SIZE + visible_count.saturating_sub(1) as i32 * ITEM_GAP;
        let strip_height = content_height + STRIP_PADDING * 2;
        let strip = Rectangle::new(
            (
                work_area.loc.x + work_area.size.w - STRIP_WIDTH,
                work_area.loc.y + (work_area.size.h - strip_height) / 2,
            )
                .into(),
            (STRIP_WIDTH, strip_height).into(),
        );
        let item_x = strip.loc.x + (strip.size.w - ITEM_SIZE) / 2;
        let items = layout
            .queue_members
            .into_iter()
            .skip(scroll_offset)
            .take(visible_count)
            .enumerate()
            .map(|(index, node_id)| OverflowItem {
                node_id,
                rect: Rectangle::new(
                    (
                        item_x,
                        strip.loc.y + STRIP_PADDING + index as i32 * (ITEM_SIZE + ITEM_GAP),
                    )
                        .into(),
                    (ITEM_SIZE, ITEM_SIZE).into(),
                ),
            })
            .collect();
        let total_items = visible_count + max_offset;
        let scrollbar = (max_offset > 0).then(|| {
            let track = Rectangle::new(
                (strip.loc.x + strip.size.w - 6, strip.loc.y + STRIP_PADDING).into(),
                (3, content_height).into(),
            );
            let thumb_height = ((track.size.h as f64 * visible_count as f64 / total_items as f64)
                .round() as i32)
                .clamp(12, track.size.h);
            let thumb_travel = track.size.h - thumb_height;
            let thumb_y = track.loc.y
                + ((thumb_travel as f64 * scroll_offset as f64 / max_offset as f64).round() as i32);
            OverflowScrollbar {
                track,
                thumb: Rectangle::new(
                    (track.loc.x, thumb_y).into(),
                    (track.size.w, thumb_height).into(),
                ),
            }
        });
        Some(OverflowLayout {
            strip,
            items,
            scroll_offset,
            total_items,
            scrollbar,
            visibility: 1.0,
        })
    }

    pub fn has_overflow(&self, output: &str) -> bool {
        let Some(active) = self.active_on(output) else {
            return false;
        };
        self.metadata(active).is_some_and(|metadata| {
            metadata.layout == halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling
        }) && self.registry.cluster(active).is_some_and(|cluster| {
            cluster
                .members()
                .iter()
                .filter(|member| !self.member_floats.is_floating(**member))
                .count()
                > self.config.tiling.max_stack.saturating_add(1)
        })
    }

    pub fn reveal_overflow(&mut self, output: &str, now: Duration) -> bool {
        if !self.has_overflow(output) {
            return self.overflow.hide(output);
        }
        self.overflow.reveal(output, now)
    }

    pub fn hide_overflow(&mut self, output: &str) -> bool {
        let hidden = self.overflow.hide(output);
        if hidden {
            if self
                .overlay_hovered
                .as_ref()
                .is_some_and(|(candidate, _)| candidate == output)
            {
                self.overlay_hovered = None;
            }
            let members = self
                .active_on(output)
                .map(|cluster| self.member_ids(cluster))
                .unwrap_or_default();
            let mut labels = self.overlay_label_hover.borrow_mut();
            for member in members {
                labels.remove(&member);
            }
        }
        hidden
    }

    pub fn overflow_wakeup(&mut self, now: Duration) -> bool {
        let held_open_changed = self
            .overlay_hovered
            .clone()
            .filter(|(output, _)| self.has_overflow(output))
            .is_some_and(|(output, _)| self.overflow.reveal(&output, now));
        self.overflow.wakeup(now) || held_open_changed
    }

    /// Returns the output-local queue geometry shared by rendering and input.
    ///
    /// Keeping this layout here prevents the compositor overlay from owning
    /// cluster policy or reconstructing hit boxes independently.
    pub fn overflow_layout(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<OverflowLayout> {
        let visibility = self.overflow.visibility_mix(output, now)?;
        let mut layout = self.overflow_geometry(output, work_area)?;
        let slide = ((1.0 - visibility) * REVEAL_SLIDE_PX as f32).round() as i32;
        layout.strip.loc.x += slide;
        for item in &mut layout.items {
            item.rect.loc.x += slide;
        }
        if let Some(scrollbar) = layout.scrollbar.as_mut() {
            scrollbar.track.loc.x += slide;
            scrollbar.thumb.loc.x += slide;
        }
        layout.visibility = visibility;
        Some(layout)
    }

    pub fn overflow_hit_test(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> Option<NodeId> {
        self.overflow_layout(output, work_area, now)?
            .items
            .into_iter()
            .find_map(|item| {
                item.rect
                    .to_f64()
                    .contains(output_local)
                    .then_some(item.node_id)
            })
    }

    pub fn overflow_strip_contains(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> bool {
        self.overflow_layout(output, work_area, now)
            .is_some_and(|layout| layout.strip.to_f64().contains(output_local))
    }

    pub fn overflow_strip_slot(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> Option<usize> {
        let layout = self.overflow_layout(output, work_area, now)?;
        if !layout.strip.to_f64().contains(output_local) || layout.total_items == 0 {
            return None;
        }
        let relative_y =
            (output_local.y.round() as i32 - layout.strip.loc.y - STRIP_PADDING).max(0);
        let visible_slot = (relative_y / (ITEM_SIZE + ITEM_GAP)) as usize;
        Some((layout.scroll_offset + visible_slot).min(layout.total_items.saturating_sub(1)))
    }

    /// Consumes vertical wheel/touchpad steps while the overflow strip is
    /// under the pointer. Fractional touchpad movement is accumulated so one
    /// item advances per logical scroll step.
    pub fn scroll_overflow(
        &mut self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        delta_steps: f64,
        stopped: bool,
        now: Duration,
    ) -> bool {
        let Some(layout) = self.overflow_geometry(output, work_area) else {
            return false;
        };
        let visible_count = layout.items.len();
        let max_offset = layout.total_items.saturating_sub(visible_count);
        if max_offset == 0 {
            self.overflow.scroll_offsets.remove(output);
            self.overflow.scroll_residue.remove(output);
            return false;
        }

        self.overflow.reveal(output, now);
        if stopped {
            self.overflow.scroll_residue.remove(output);
            return true;
        }
        let residue = self
            .overflow
            .scroll_residue
            .entry(output.to_string())
            .or_default();
        if *residue != 0.0 && delta_steps != 0.0 && residue.signum() != delta_steps.signum() {
            *residue = 0.0;
        }
        *residue += delta_steps;
        let ticks = residue.trunc() as i32;
        *residue -= f64::from(ticks);
        if ticks == 0 {
            return true;
        }

        let current = self
            .overflow
            .scroll_offsets
            .get(output)
            .copied()
            .unwrap_or(0);
        let next = (current as i32 + ticks).clamp(0, max_offset as i32) as usize;
        if next != current {
            self.overflow
                .scroll_offsets
                .insert(output.to_string(), next);
        }
        true
    }

    pub fn begin_overflow_drag(
        &mut self,
        output: &str,
        member: NodeId,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> bool {
        let Some(cluster_id) = self.active_on(output) else {
            return false;
        };
        self.overflow.reveal(output, now);
        self.overflow.drag = Some((
            output.to_string(),
            OverflowDrag {
                cluster_id,
                member_id: member,
                output_local,
                press_local: output_local,
            },
        ));
        true
    }

    pub fn update_overflow_drag(
        &mut self,
        output: &str,
        output_local: Point<f64, Logical>,
        now: Duration,
    ) -> bool {
        let Some((drag_output, drag)) = self.overflow.drag.as_mut() else {
            return false;
        };
        if drag_output != output {
            return true;
        }
        drag.output_local = output_local;
        self.overflow.reveal(output, now);
        true
    }

    pub fn overflow_drag(&self) -> Option<(String, OverflowDrag)> {
        self.overflow.drag.clone()
    }

    /// Drop an overflow-strip member released at `output_local`. A drag that
    /// never `moved` past the click threshold promotes the member into the
    /// layout; otherwise it lands on the strip slot under the pointer, or
    /// swaps with the visible tile there, or is promoted when it is over
    /// neither. The strip is revealed again either way. Returns whether the
    /// workspace changed.
    pub fn drop_overflow_member(
        &mut self,
        output: &str,
        member: NodeId,
        output_local: Point<f64, Logical>,
        moved: bool,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let strip_slot = moved
            .then(|| self.overflow_strip_slot(output, work_area, output_local, now))
            .flatten();
        let target = (moved && strip_slot.is_none())
            .then(|| self.visible_tile_hit_test(output, work_area, output_local))
            .flatten();
        let changed = if let Some(target_slot) = strip_slot {
            self.reorder_overflow_member(output, member, target_slot, now)
        } else if let Some(target) = target {
            self.swap_overflow_member(output, member, target, work_area, now)
        } else {
            self.promote_overflow_member(output, member, work_area, now)
        };
        self.reveal_overflow(output, now);
        changed
    }

    pub fn take_overflow_drag(&mut self) -> Option<(String, OverflowDrag)> {
        self.overflow.drag.take()
    }

    pub fn visible_tile_hit_test(
        &self,
        output: &str,
        work_area: Rectangle<i32, Logical>,
        output_local: Point<f64, Logical>,
    ) -> Option<NodeId> {
        let active = self.active_on(output)?;
        let layout = self.workspace_layout(active, work_area)?;
        super::member_at_point(&layout, output_local)
    }

    /// Promotes a queued tile into the last visible stack position. The
    /// master remains stable, which avoids an unrelated workspace reshuffle.
    pub fn promote_overflow_member(
        &mut self,
        output: &str,
        member: NodeId,
        work_area: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> bool {
        let Some(active) = self.active_on(output) else {
            return false;
        };
        let Some(before) = self.workspace_layout(active, work_area) else {
            return false;
        };
        let origin = self
            .overflow_geometry(output, work_area)
            .and_then(|layout| {
                layout
                    .items
                    .into_iter()
                    .find(|item| item.node_id == member)
                    .map(|item| item.rect)
            });
        if self.config.tiling.max_stack == 0 || !before.queue_members.contains(&member) {
            return false;
        }
        let Some(visible_member) = before.placements.last().map(|placement| placement.node_id)
        else {
            return false;
        };
        let changed = self.swap_nonfloating_members(active, member, visible_member);
        if changed {
            self.overflow.reveal(output, now);
            if let Some(origin) = origin {
                self.begin_reflow_with_origin(output, active, before, member, origin, now);
            } else {
                self.begin_reflow(
                    output,
                    active,
                    before,
                    now,
                    self.animations.tiling.reflow_duration_ms,
                );
            }
        }
        changed
    }

    pub fn swap_overflow_member(
        &mut self,
        output: &str,
        overflow_member: NodeId,
        visible_member: NodeId,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let Some(active) = self.active_on(output) else {
            return false;
        };
        let Some(before) = self.workspace_layout(active, work_area) else {
            return false;
        };
        if !before.queue_members.contains(&overflow_member)
            || !before
                .placements
                .iter()
                .any(|placement| placement.node_id == visible_member)
        {
            return false;
        }
        let origin = self
            .overflow_geometry(output, work_area)
            .and_then(|layout| {
                layout
                    .items
                    .into_iter()
                    .find(|item| item.node_id == overflow_member)
                    .map(|item| item.rect)
            });
        let changed = self.swap_nonfloating_members(active, overflow_member, visible_member);
        if changed {
            self.overflow.reveal(output, now);
            if let Some(origin) = origin {
                self.begin_reflow_with_origin(output, active, before, overflow_member, origin, now);
            } else {
                self.begin_reflow(
                    output,
                    active,
                    before,
                    now,
                    self.animations.tiling.reflow_duration_ms,
                );
            }
        }
        changed
    }

    pub fn reorder_overflow_member(
        &mut self,
        output: &str,
        overflow_member: NodeId,
        target_overflow_index: usize,
        now: Duration,
    ) -> bool {
        let Some(active) = self.active_on(output) else {
            return false;
        };
        let Some(cluster) = self.registry.cluster(active) else {
            return false;
        };
        if self.config.tiling.max_stack == 0 {
            return false;
        }
        let mut layout_members = cluster
            .members()
            .iter()
            .copied()
            .filter(|member| !self.member_floats.is_floating(*member))
            .collect::<Vec<_>>();
        let visible_limit = self
            .config
            .tiling
            .max_stack
            .saturating_add(1)
            .min(layout_members.len());
        let Some(from) = layout_members
            .iter()
            .position(|member| *member == overflow_member)
            .filter(|from| *from >= visible_limit)
        else {
            return false;
        };
        let queue_len = layout_members.len().saturating_sub(visible_limit);
        if target_overflow_index >= queue_len {
            return false;
        }
        let member = layout_members.remove(from);
        let target = visible_limit + target_overflow_index;
        layout_members.insert(target.min(layout_members.len()), member);
        let changed = self.reorder_nonfloating_members(active, layout_members);
        if changed {
            self.overflow.reveal(output, now);
        }
        changed
    }

    fn swap_nonfloating_members(
        &mut self,
        cluster: halley_core::cluster::ClusterId,
        first: NodeId,
        second: NodeId,
    ) -> bool {
        let Some(mut members) = self
            .registry
            .cluster(cluster)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let Some(first_index) = members.iter().position(|member| *member == first) else {
            return false;
        };
        let Some(second_index) = members.iter().position(|member| *member == second) else {
            return false;
        };
        if self.member_floats.is_floating(first) || self.member_floats.is_floating(second) {
            return false;
        }
        members.swap(first_index, second_index);
        self.registry
            .reorder_cluster_members(cluster, members)
            .is_ok()
    }

    fn reorder_nonfloating_members(
        &mut self,
        cluster: halley_core::cluster::ClusterId,
        ordered: Vec<NodeId>,
    ) -> bool {
        let Some(mut members) = self
            .registry
            .cluster(cluster)
            .map(|cluster| cluster.members().to_vec())
        else {
            return false;
        };
        let indices = members
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                (!self.member_floats.is_floating(*member)).then_some(index)
            })
            .collect::<Vec<_>>();
        if indices.len() != ordered.len() {
            return false;
        }
        for (index, member) in indices.into_iter().zip(ordered) {
            members[index] = member;
        }
        self.registry
            .reorder_cluster_members(cluster, members)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use halley_core::field::{Field, Vec2};

    use super::*;

    fn surface(field: &mut Field, label: &str) -> NodeId {
        field.spawn_surface(
            label,
            Vec2 { x: 20.0, y: 20.0 },
            Vec2 { x: 320.0, y: 200.0 },
        )
    }

    #[test]
    fn queue_geometry_and_promotion_share_one_member_order() {
        let mut field = Field::new();
        let members = (0..7)
            .map(|index| surface(&mut field, &format!("window-{index}")))
            .collect::<Vec<_>>();
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 2,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        for member in &members {
            assert!(system.toggle_creation_member(*member, "DP-1"));
        }
        assert!(system.begin_naming());
        let cluster = system.finish_creation(&mut field).unwrap();
        assert!(system.activate("DP-1", cluster, Duration::ZERO));

        let work_area = Rectangle::new((0, 0).into(), (1280, 720).into());
        system.reveal_overflow("DP-1", Duration::ZERO);
        let queue = system
            .overflow_layout("DP-1", work_area, Duration::ZERO)
            .unwrap();
        assert_eq!(
            queue
                .items
                .iter()
                .map(|item| item.node_id)
                .collect::<Vec<_>>(),
            members[3..].to_vec()
        );
        let promoted = queue.items[1].node_id;
        let promotion_origin = system
            .overflow_geometry("DP-1", work_area)
            .expect("overflow geometry")
            .items[1]
            .rect;
        let settled = Duration::from_secs(5);
        assert!(system.promote_overflow_member("DP-1", promoted, work_area, settled,));
        assert_eq!(
            system.registry().cluster(cluster).unwrap().members()[2],
            promoted
        );
        assert_eq!(
            system.window_presentation(promoted, "DP-1", work_area, None, settled),
            super::super::WindowPresentation::Workspace {
                rect: promotion_origin,
                depth: 2,
                alpha: 1.0,
            }
        );
    }

    #[test]
    fn overflow_order_ignores_cluster_floating_members() {
        let mut field = Field::new();
        let members = (0..4)
            .map(|index| surface(&mut field, &format!("window-{index}")))
            .collect::<Vec<_>>();
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 1,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        for member in &members {
            system.toggle_creation_member(*member, "DP-1");
        }
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).unwrap();
        system.activate("DP-1", cluster, Duration::ZERO);
        let work_area = Rectangle::new((0, 0).into(), (1280, 720).into());
        let floating = Rectangle::new((80, 60).into(), (500, 380).into());

        assert_eq!(
            system.toggle_member_floating("DP-1", members[0], work_area, floating, Duration::ZERO,),
            Some(true)
        );
        assert!(system.has_overflow("DP-1"));
        assert_eq!(
            system
                .workspace_layout(cluster, work_area)
                .unwrap()
                .queue_members,
            vec![members[3]]
        );
        assert!(system.promote_overflow_member(
            "DP-1",
            members[3],
            work_area,
            Duration::from_secs(2),
        ));
        assert_eq!(
            system.registry().cluster(cluster).unwrap().members(),
            &[members[0], members[1], members[3], members[2]]
        );
        assert!(system.is_member_floating(members[0]));

        assert_eq!(
            system.toggle_member_floating(
                "DP-1",
                members[1],
                work_area,
                floating,
                Duration::from_secs(4),
            ),
            Some(true)
        );
        assert!(!system.has_overflow("DP-1"));
    }

    #[test]
    fn overflow_auto_hides_and_hover_keeps_it_revealed() {
        let mut field = Field::new();
        let members = (0..5)
            .map(|index| surface(&mut field, &format!("window-{index}")))
            .collect::<Vec<_>>();
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 1,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        for member in &members {
            system.toggle_creation_member(*member, "DP-1");
        }
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).unwrap();
        system.activate("DP-1", cluster, Duration::ZERO);
        let work_area = Rectangle::new((0, 0).into(), (1280, 720).into());

        assert!(
            system
                .overflow_layout("DP-1", work_area, Duration::from_millis(100))
                .is_some()
        );
        assert!(system.overflow_wakeup(Duration::from_millis(2_201)));
        assert!(
            system
                .overflow_layout("DP-1", work_area, Duration::from_millis(2_201))
                .is_none()
        );

        system.reveal_overflow("DP-1", Duration::from_millis(3_000));
        system.set_overlay_hovered(Some(("DP-1".into(), members[2])));
        system.overflow_wakeup(Duration::from_millis(5_500));
        assert!(
            system
                .overflow_layout("DP-1", work_area, Duration::from_millis(5_500))
                .is_some()
        );
    }

    #[test]
    fn each_new_overflow_window_extends_the_reveal_deadline() {
        let mut field = Field::new();
        let first = surface(&mut field, "first");
        let second = surface(&mut field, "second");
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 1,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        system.toggle_creation_member(first, "DP-1");
        system.toggle_creation_member(second, "DP-1");
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).unwrap();
        system.activate("DP-1", cluster, Duration::ZERO);
        let work_area = Rectangle::new((0, 0).into(), (1280, 720).into());

        let third = surface(&mut field, "third");
        assert!(system.admit_mapped_window(
            &mut field,
            "DP-1",
            third,
            halley_config::WindowClusterParticipation::Layout,
            work_area,
            Duration::from_millis(2_000),
        ));
        let fourth = surface(&mut field, "fourth");
        assert!(system.admit_mapped_window(
            &mut field,
            "DP-1",
            fourth,
            halley_config::WindowClusterParticipation::Layout,
            work_area,
            Duration::from_millis(4_000),
        ));

        assert!(
            system
                .overflow_layout("DP-1", work_area, Duration::from_millis(5_000))
                .is_some()
        );
    }

    #[test]
    fn destroyed_visible_tile_promotes_the_queue_from_its_strip_position() {
        let mut field = Field::new();
        let members = (0..5)
            .map(|index| surface(&mut field, &format!("window-{index}")))
            .collect::<Vec<_>>();
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 1,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        for member in &members {
            system.toggle_creation_member(*member, "DP-1");
        }
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).unwrap();
        system.activate("DP-1", cluster, Duration::ZERO);
        let work_area = Rectangle::new((0, 0).into(), (1_280, 720).into());
        let origin = system
            .overflow_geometry("DP-1", work_area)
            .expect("overflow")
            .items[0]
            .rect;
        let settled = Duration::from_secs(5);

        assert!(
            system.forget_destroyed_member_animated(&mut field, members[1], work_area, settled,)
        );
        assert_eq!(
            system.window_presentation(members[2], "DP-1", work_area, None, settled),
            super::super::WindowPresentation::Workspace {
                rect: origin,
                depth: 1,
                alpha: 1.0,
            }
        );
    }

    #[test]
    fn long_overflow_scrolls_and_reorders_with_visible_slots() {
        let mut field = Field::new();
        let members = (0..8)
            .map(|index| surface(&mut field, &format!("window-{index}")))
            .collect::<Vec<_>>();
        let config = halley_config::Clusters {
            default_layout: halley_config::ClusterLayout::Tiling,
            tiling: halley_config::ClusterTiling {
                max_stack: 2,
                ..halley_config::ClusterTiling::default()
            },
            ..halley_config::Clusters::default()
        };
        let mut system = ClusterSystem::new(config, halley_config::ClusterAnimation::default());
        system.begin_creation("DP-1".into());
        for member in &members {
            system.toggle_creation_member(*member, "DP-1");
        }
        system.begin_naming();
        let cluster = system.finish_creation(&mut field).unwrap();
        system.activate("DP-1", cluster, Duration::ZERO);

        let work_area = Rectangle::new((0, 0).into(), (800, 180).into());
        system.reveal_overflow("DP-1", Duration::ZERO);
        let first = system
            .overflow_layout("DP-1", work_area, Duration::ZERO)
            .unwrap();
        assert_eq!(first.scroll_offset, 0);
        assert_eq!(first.total_items, 5);
        assert!(first.scrollbar.is_some());
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.node_id)
                .collect::<Vec<_>>(),
            members[3..6].to_vec()
        );

        assert!(system.scroll_overflow("DP-1", work_area, 1.0, false, Duration::from_millis(10),));
        let scrolled = system
            .overflow_layout("DP-1", work_area, Duration::from_millis(10))
            .unwrap();
        assert_eq!(scrolled.scroll_offset, 1);
        assert_eq!(
            scrolled
                .items
                .iter()
                .map(|item| item.node_id)
                .collect::<Vec<_>>(),
            members[4..7].to_vec()
        );

        assert!(system.reorder_overflow_member("DP-1", members[3], 3, Duration::from_millis(20),));
        assert_eq!(
            system.registry().cluster(cluster).unwrap().members(),
            &[
                members[0], members[1], members[2], members[4], members[5], members[6], members[3],
                members[7],
            ]
        );
    }
}
