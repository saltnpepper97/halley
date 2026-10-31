use std::collections::HashMap;
use std::time::Duration;

use halley_core::cluster::ClusterId;
use halley_core::cluster::layout::{ClusterCycleDirection, ClusterWorkspaceLayoutKind};
use halley_core::field::NodeId;
use smithay::utils::{Logical, Point, Rectangle};

use super::ClusterSystem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransitionKind {
    Opening,
    Closing,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WorkspaceTransition {
    pub(super) cluster_id: ClusterId,
    pub(super) kind: TransitionKind,
    pub(super) layout: ClusterWorkspaceLayoutKind,
    pub(super) started_at: Duration,
    pub(super) duration: Duration,
    pub(super) stagger: Duration,
    pub(super) visible_members: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TransitionVisual {
    pub(super) rect: Rectangle<i32, Logical>,
    pub(super) alpha: f32,
}

#[derive(Clone, Debug)]
pub(super) struct ReflowTransition {
    cluster_id: ClusterId,
    started_at: Duration,
    duration: Duration,
    from: HashMap<NodeId, ReflowPlacement>,
    kind: ReflowKind,
    wrapped: Option<NodeId>,
}

#[derive(Clone, Copy, Debug)]
struct ReflowPlacement {
    rect: Rectangle<i32, Logical>,
    depth: usize,
}

#[derive(Clone, Copy, Debug)]
enum ReflowKind {
    Standard,
    LayoutChange { preserve_source_depth: bool },
    StackCycle(ClusterCycleDirection),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ReflowVisual {
    pub(super) rect: Rectangle<i32, Logical>,
    pub(super) depth: usize,
    pub(super) alpha: f32,
}

impl ClusterSystem {
    pub(super) fn remove_cluster_transitions(&mut self, cluster_id: ClusterId) {
        self.transitions
            .retain(|_, transition| transition.cluster_id != cluster_id);
        self.reflows
            .retain(|_, reflow| reflow.cluster_id != cluster_id);
    }

    pub(super) fn begin_transition(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        kind: TransitionKind,
        now: Duration,
    ) {
        let Some(layout) = self.metadata(cluster_id).map(|metadata| metadata.layout) else {
            return;
        };
        if !self.animations.enabled {
            self.transitions.remove(output);
            return;
        }
        let duration_ms = match (layout, kind) {
            (ClusterWorkspaceLayoutKind::Tiling, TransitionKind::Opening) => {
                self.animations.tiling.open_duration_ms
            }
            (ClusterWorkspaceLayoutKind::Tiling, TransitionKind::Closing) => {
                self.animations.tiling.close_duration_ms
            }
            (ClusterWorkspaceLayoutKind::Stacking, TransitionKind::Opening) => {
                self.animations.stacking.open_duration_ms
            }
            (ClusterWorkspaceLayoutKind::Stacking, TransitionKind::Closing) => {
                self.animations.stacking.close_duration_ms
            }
        };
        let visible_members = self
            .registry
            .cluster(cluster_id)
            .map_or(0, |cluster| match layout {
                ClusterWorkspaceLayoutKind::Tiling if self.config.tiling.max_stack > 0 => cluster
                    .members()
                    .len()
                    .min(self.config.tiling.max_stack.saturating_add(1)),
                _ => cluster.members().len(),
            });
        let stagger =
            if kind == TransitionKind::Opening && layout == ClusterWorkspaceLayoutKind::Tiling {
                Duration::from_millis(u64::from(self.animations.tiling.stagger_ms))
            } else {
                Duration::ZERO
            };
        self.transitions.insert(
            output.to_string(),
            WorkspaceTransition {
                cluster_id,
                kind,
                layout,
                started_at: now,
                duration: Duration::from_millis(u64::from(duration_ms.max(1))),
                stagger,
                visible_members,
            },
        );
    }

    pub fn transition_cluster_on(&self, output: &str, now: Duration) -> Option<ClusterId> {
        let transition = self.transitions.get(output)?;
        self.transition_is_live(transition, now)
            .then_some(transition.cluster_id)
    }

    pub fn is_animating_on_output(&self, output: &str, now: Duration) -> bool {
        self.transitions
            .get(output)
            .is_some_and(|transition| self.transition_is_live(transition, now))
            || self
                .reflows
                .get(output)
                .is_some_and(|reflow| now.saturating_sub(reflow.started_at) < reflow.duration)
    }

    fn transition_is_live(&self, transition: &WorkspaceTransition, now: Duration) -> bool {
        let stagger_tail = transition
            .stagger
            .saturating_mul(transition.visible_members.saturating_sub(1) as u32);
        now.saturating_sub(transition.started_at) < transition.duration + stagger_tail
    }

    pub(super) fn transition_visual(
        &self,
        output: &str,
        cluster_id: ClusterId,
        member: NodeId,
        target: Rectangle<i32, Logical>,
        core: Option<Point<i32, Logical>>,
        now: Duration,
    ) -> Option<TransitionVisual> {
        let transition = self.transitions.get(output)?;
        if transition.cluster_id != cluster_id || !self.transition_is_live(transition, now) {
            return None;
        }
        let member_delay = if transition.kind == TransitionKind::Opening
            && transition.layout == ClusterWorkspaceLayoutKind::Tiling
        {
            self.registry
                .cluster(cluster_id)
                .and_then(|cluster| {
                    cluster
                        .members()
                        .iter()
                        .position(|candidate| *candidate == member)
                })
                .map_or(Duration::ZERO, |index| {
                    transition.stagger.saturating_mul(index as u32)
                })
        } else {
            Duration::ZERO
        };
        let elapsed = now
            .saturating_sub(transition.started_at)
            .saturating_sub(member_delay);
        let linear = (elapsed.as_secs_f32() / transition.duration.as_secs_f32()).clamp(0.0, 1.0);
        let eased = linear * linear * (3.0 - 2.0 * linear);
        let progress = match transition.kind {
            TransitionKind::Opening => eased,
            TransitionKind::Closing => 1.0 - eased,
        };
        let center = core.unwrap_or_else(|| {
            Point::from((
                target.loc.x + target.size.w / 2,
                target.loc.y + target.size.h / 2,
            ))
        });
        let origin = Rectangle::new(center, (1, 1).into());
        Some(TransitionVisual {
            rect: lerp_rect(origin, target, progress),
            alpha: progress,
        })
    }

    pub(super) fn begin_reflow(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        now: Duration,
        duration_ms: u32,
    ) {
        self.begin_reflow_kind(
            output,
            cluster_id,
            before,
            now,
            duration_ms,
            ReflowKind::Standard,
        );
    }

    pub(super) fn begin_layout_reflow(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        target_layout: ClusterWorkspaceLayoutKind,
        now: Duration,
        duration_ms: u32,
    ) {
        self.begin_reflow_kind(
            output,
            cluster_id,
            before,
            now,
            duration_ms,
            ReflowKind::LayoutChange {
                preserve_source_depth: target_layout == ClusterWorkspaceLayoutKind::Tiling,
            },
        );
    }

    fn begin_reflow_kind(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        now: Duration,
        duration_ms: u32,
        kind: ReflowKind,
    ) {
        if !self.animations.enabled {
            self.reflows.remove(output);
            return;
        }
        let from = self.current_reflow_origins(output, cluster_id, before, now);
        self.reflows.insert(
            output.to_string(),
            ReflowTransition {
                cluster_id,
                started_at: now,
                duration: Duration::from_millis(u64::from(duration_ms.max(1))),
                from,
                kind,
                wrapped: None,
            },
        );
    }

    pub(super) fn begin_reflow_with_origin(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        member: NodeId,
        origin: Rectangle<i32, Logical>,
        now: Duration,
    ) {
        if !self.animations.enabled {
            self.reflows.remove(output);
            return;
        }
        let mut from = self.current_reflow_origins(output, cluster_id, before, now);
        from.insert(
            member,
            ReflowPlacement {
                rect: origin,
                depth: usize::MAX,
            },
        );
        let duration_ms = self
            .metadata(cluster_id)
            .map(|metadata| match metadata.layout {
                ClusterWorkspaceLayoutKind::Tiling => self.animations.tiling.reflow_duration_ms,
                ClusterWorkspaceLayoutKind::Stacking => self.animations.stacking.cycle_duration_ms,
            })
            .unwrap_or(self.animations.tiling.reflow_duration_ms);
        self.reflows.insert(
            output.to_string(),
            ReflowTransition {
                cluster_id,
                started_at: now,
                duration: Duration::from_millis(u64::from(duration_ms.max(1))),
                from,
                kind: ReflowKind::Standard,
                wrapped: None,
            },
        );
    }

    pub(super) fn begin_stack_cycle_reflow(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        after: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        direction: ClusterCycleDirection,
        now: Duration,
    ) {
        if !self.animations.enabled {
            self.reflows.remove(output);
            return;
        }
        let from = placement_rects(before);
        let to = placement_rects(after);
        let same_visible_set =
            from.len() == to.len() && from.keys().all(|member| to.contains_key(member));
        let old_top = from
            .iter()
            .max_by_key(|(_, placement)| placement.depth)
            .map(|(member, _)| *member);
        let old_bottom = from
            .iter()
            .min_by_key(|(_, placement)| placement.depth)
            .map(|(member, _)| *member);
        let new_top = to
            .iter()
            .max_by_key(|(_, placement)| placement.depth)
            .map(|(member, _)| *member);
        let new_bottom = to
            .iter()
            .min_by_key(|(_, placement)| placement.depth)
            .map(|(member, _)| *member);
        let wrapped = match direction {
            ClusterCycleDirection::Next
                if same_visible_set && old_top.is_some() && old_top == new_bottom =>
            {
                old_top
            }
            ClusterCycleDirection::Prev
                if same_visible_set && old_bottom.is_some() && old_bottom == new_top =>
            {
                old_bottom
            }
            _ => None,
        };
        self.reflows.insert(
            output.to_string(),
            ReflowTransition {
                cluster_id,
                started_at: now,
                duration: Duration::from_millis(u64::from(
                    self.animations.stacking.cycle_duration_ms.max(1),
                )),
                from,
                kind: ReflowKind::StackCycle(direction),
                wrapped,
            },
        );
    }

    fn current_reflow_origins(
        &self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        now: Duration,
    ) -> HashMap<NodeId, ReflowPlacement> {
        placement_rects(before)
            .into_iter()
            .map(|(member, target)| {
                let current = self
                    .reflow_visual(
                        output,
                        cluster_id,
                        member,
                        Some((target.rect, target.depth)),
                        now,
                    )
                    .map_or(target, |visual| ReflowPlacement {
                        rect: visual.rect,
                        depth: visual.depth,
                    });
                (member, current)
            })
            .collect()
    }

    pub(super) fn begin_stack_insert_reflow(
        &mut self,
        output: &str,
        cluster_id: ClusterId,
        before: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
        inserted: NodeId,
        work_area: Rectangle<i32, Logical>,
        now: Duration,
    ) {
        if !self.animations.enabled {
            self.reflows.remove(output);
            return;
        }
        let mut from = placement_rects(before);
        if let Some(target) = self
            .workspace_layout(cluster_id, work_area)
            .and_then(|layout| {
                layout
                    .placements
                    .into_iter()
                    .find(|placement| placement.node_id == inserted)
            })
        {
            let depth = target.depth;
            let target = placement_rect(target);
            from.insert(
                inserted,
                ReflowPlacement {
                    rect: Rectangle::new(
                        (
                            target.loc.x - (target.size.w as f32 * 0.55).round() as i32,
                            target.loc.y,
                        )
                            .into(),
                        target.size,
                    ),
                    depth,
                },
            );
        }
        self.reflows.insert(
            output.to_string(),
            ReflowTransition {
                cluster_id,
                started_at: now,
                duration: Duration::from_millis(u64::from(
                    self.animations.stacking.cycle_duration_ms.max(1),
                )),
                from,
                kind: ReflowKind::Standard,
                wrapped: None,
            },
        );
    }

    pub(super) fn reflow_visual(
        &self,
        output: &str,
        cluster_id: ClusterId,
        member: NodeId,
        target: Option<(Rectangle<i32, Logical>, usize)>,
        now: Duration,
    ) -> Option<ReflowVisual> {
        let reflow = self.reflows.get(output)?;
        if reflow.cluster_id != cluster_id {
            return None;
        }
        let elapsed = now.saturating_sub(reflow.started_at);
        if elapsed >= reflow.duration {
            return None;
        }
        let linear = (elapsed.as_secs_f32() / reflow.duration.as_secs_f32()).clamp(0.0, 1.0);
        let eased = match reflow.kind {
            ReflowKind::StackCycle(_) => ease_in_out_cubic(linear),
            ReflowKind::Standard | ReflowKind::LayoutChange { .. } => {
                linear * linear * (3.0 - 2.0 * linear)
            }
        };
        let from = reflow.from.get(&member).copied();
        match reflow.kind {
            ReflowKind::Standard => {
                let from = from?;
                let (target, depth) = target?;
                Some(ReflowVisual {
                    rect: lerp_rect(from.rect, target, eased),
                    depth,
                    alpha: 1.0,
                })
            }
            ReflowKind::LayoutChange {
                preserve_source_depth,
            } => {
                let from = from?;
                let (target, target_depth) = target?;
                Some(ReflowVisual {
                    rect: lerp_rect(from.rect, target, eased),
                    // Cards leaving a stack retain its established overlap
                    // while separating. Cards entering a stack adopt their
                    // destination overlap before their paths begin crossing.
                    depth: if preserve_source_depth {
                        from.depth
                    } else {
                        target_depth
                    },
                    alpha: 1.0,
                })
            }
            ReflowKind::StackCycle(direction) => stack_cycle_visual(
                from,
                target,
                direction,
                reflow.wrapped == Some(member),
                eased,
            ),
        }
    }

    pub(super) fn extra_reflow_visual(
        &self,
        output: &str,
        cluster_id: ClusterId,
        member: NodeId,
        now: Duration,
    ) -> Option<ReflowVisual> {
        let reflow = self.reflows.get(output)?;
        if reflow.cluster_id != cluster_id || reflow.wrapped != Some(member) {
            return None;
        }
        let ReflowKind::StackCycle(ClusterCycleDirection::Next) = reflow.kind else {
            return None;
        };
        let elapsed = now.saturating_sub(reflow.started_at);
        if elapsed >= reflow.duration {
            return None;
        }
        let linear = (elapsed.as_secs_f32() / reflow.duration.as_secs_f32()).clamp(0.0, 1.0);
        let progress = ease_in_out_cubic(linear);
        let from = reflow.from.get(&member).copied()?;
        stack_cycle_extra_visual(
            from,
            ClusterCycleDirection::Next,
            reflow.wrapped == Some(member),
            progress,
        )
    }
}

fn ease_in_out_cubic(progress: f32) -> f32 {
    if progress < 0.5 {
        4.0 * progress * progress * progress
    } else {
        1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0
    }
}

fn placement_rects(
    layout: halley_core::cluster::layout::ClusterWorkspaceLayoutResult,
) -> HashMap<NodeId, ReflowPlacement> {
    layout
        .placements
        .into_iter()
        .map(|placement| {
            (
                placement.node_id,
                ReflowPlacement {
                    rect: placement_rect(placement),
                    depth: placement.depth,
                },
            )
        })
        .collect()
}

fn stack_cycle_visual(
    from: Option<ReflowPlacement>,
    target: Option<(Rectangle<i32, Logical>, usize)>,
    direction: ClusterCycleDirection,
    wrapped: bool,
    progress: f32,
) -> Option<ReflowVisual> {
    if wrapped && let Some((target, depth)) = target {
        return Some(match direction {
            ClusterCycleDirection::Next => ReflowVisual {
                rect: target,
                depth,
                alpha: progress,
            },
            ClusterCycleDirection::Prev => {
                let mut start = target;
                start.loc.x -= (start.size.w as f32 * 0.55).round() as i32;
                ReflowVisual {
                    rect: lerp_rect(start, target, progress),
                    depth: usize::MAX,
                    alpha: progress,
                }
            }
        });
    }
    match (from, target) {
        (Some(from), Some((target, depth))) => Some(ReflowVisual {
            rect: lerp_rect(from.rect, target, progress),
            depth,
            alpha: 1.0,
        }),
        (Some(from), None) => {
            let mut end = from.rect;
            match direction {
                ClusterCycleDirection::Next => {
                    end.loc.x -= (end.size.w as f32 * 0.55).round() as i32;
                }
                ClusterCycleDirection::Prev => {
                    end.loc.x += (end.size.w as f32 * 0.08).round() as i32;
                    end.loc.y += (end.size.h as f32 * 0.04).round() as i32;
                }
            }
            Some(ReflowVisual {
                rect: lerp_rect(from.rect, end, progress),
                depth: match direction {
                    ClusterCycleDirection::Next => usize::MAX,
                    ClusterCycleDirection::Prev => from.depth,
                },
                alpha: 1.0 - progress,
            })
        }
        (None, Some((target, depth))) => {
            let mut start = target;
            if direction == ClusterCycleDirection::Prev {
                start.loc.x -= (start.size.w as f32 * 0.55).round() as i32;
            }
            Some(ReflowVisual {
                rect: lerp_rect(start, target, progress),
                depth,
                alpha: progress,
            })
        }
        (None, None) => None,
    }
}

fn stack_cycle_extra_visual(
    from: ReflowPlacement,
    direction: ClusterCycleDirection,
    wrapped: bool,
    progress: f32,
) -> Option<ReflowVisual> {
    if !wrapped || direction != ClusterCycleDirection::Next {
        return None;
    }
    let mut end = from.rect;
    end.loc.x -= (end.size.w as f32 * 0.55).round() as i32;
    Some(ReflowVisual {
        rect: lerp_rect(from.rect, end, progress),
        depth: usize::MAX,
        alpha: 1.0 - progress,
    })
}

fn placement_rect(
    placement: halley_core::cluster::layout::ClusterWorkspacePlacement,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (
            placement.rect.x.round() as i32,
            placement.rect.y.round() as i32,
        )
            .into(),
        (
            placement.rect.w.round().max(1.0) as i32,
            placement.rect.h.round().max(1.0) as i32,
        )
            .into(),
    )
}

fn lerp_rect(
    from: Rectangle<i32, Logical>,
    to: Rectangle<i32, Logical>,
    progress: f32,
) -> Rectangle<i32, Logical> {
    let lerp = |from: i32, to: i32| (from as f32 + (to - from) as f32 * progress).round() as i32;
    Rectangle::new(
        (lerp(from.loc.x, to.loc.x), lerp(from.loc.y, to.loc.y)).into(),
        (
            lerp(from.size.w, to.size.w).max(1),
            lerp(from.size.h, to.size.h).max(1),
        )
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_stack_cycle_flies_an_outgoing_card_left() {
        let rect = Rectangle::new((100, 80).into(), (800, 600).into());
        let visual = stack_cycle_visual(
            Some(ReflowPlacement { rect, depth: 3 }),
            None,
            ClusterCycleDirection::Next,
            false,
            0.5,
        )
        .expect("outgoing visual");
        assert!(visual.rect.loc.x < rect.loc.x);
        assert_eq!(visual.alpha, 0.5);
        assert_eq!(visual.depth, usize::MAX);
    }

    #[test]
    fn backward_stack_cycle_flies_an_incoming_card_from_the_left() {
        let target = Rectangle::new((100, 80).into(), (800, 600).into());
        let visual = stack_cycle_visual(
            None,
            Some((target, 3)),
            ClusterCycleDirection::Prev,
            false,
            0.5,
        )
        .expect("incoming visual");
        assert!(visual.rect.loc.x < target.loc.x);
        assert_eq!(visual.alpha, 0.5);
        assert_eq!(visual.depth, 3);
    }

    #[test]
    fn forward_wrapped_card_is_already_canonical_at_the_back() {
        let old = Rectangle::new((100, 80).into(), (800, 600).into());
        let target = Rectangle::new((220, 100).into(), (700, 520).into());
        let visual = stack_cycle_visual(
            Some(ReflowPlacement {
                rect: old,
                depth: 3,
            }),
            Some((target, 0)),
            ClusterCycleDirection::Next,
            true,
            0.25,
        )
        .expect("canonical wrapped visual");
        assert_eq!(visual.rect, target);
        assert_eq!(visual.depth, 0);
        assert_eq!(visual.alpha, 0.25);
    }

    #[test]
    fn forward_wrap_keeps_a_second_topmost_outgoing_copy() {
        let rect = Rectangle::new((100, 80).into(), (800, 600).into());
        let visual = stack_cycle_extra_visual(
            ReflowPlacement { rect, depth: 3 },
            ClusterCycleDirection::Next,
            true,
            0.5,
        )
        .expect("render-only outgoing copy");
        assert!(visual.rect.loc.x < rect.loc.x);
        assert_eq!(visual.depth, usize::MAX);
        assert_eq!(visual.alpha, 0.5);
        assert!(
            stack_cycle_extra_visual(
                ReflowPlacement { rect, depth: 3 },
                ClusterCycleDirection::Prev,
                true,
                0.5,
            )
            .is_none()
        );
    }

    #[test]
    fn backward_wrapped_card_rises_from_the_left_above_the_stack() {
        let old = Rectangle::new((220, 100).into(), (700, 520).into());
        let target = Rectangle::new((100, 80).into(), (800, 600).into());
        let visual = stack_cycle_visual(
            Some(ReflowPlacement {
                rect: old,
                depth: 0,
            }),
            Some((target, 3)),
            ClusterCycleDirection::Prev,
            true,
            0.5,
        )
        .expect("canonical wrapped visual");
        assert!(visual.rect.loc.x < target.loc.x);
        assert_eq!(visual.depth, usize::MAX);
        assert_eq!(visual.alpha, 0.5);
    }
}
