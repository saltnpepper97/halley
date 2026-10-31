use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::time::Duration;

use halley_core::camera::Camera;
use halley_core::cluster::ClusterId;
use halley_core::field::NodeId;
use smithay::utils::{Logical, Point, Rectangle};

use super::ClusterSystem;

pub const HOLD_DURATION: Duration = Duration::from_millis(1_700);
const TOKEN_ANIMATION_DURATION: Duration = Duration::from_millis(180);
const TOKEN_STAGGER: Duration = Duration::from_millis(52);
pub const TOKEN_RADIUS_PX: i32 = 24;
pub const PULL_SLOP_PX: f32 = 12.0;
pub const DETACH_HOLD_DURATION: Duration = Duration::from_millis(1_200);
const TETHER_MAX_PX: f32 = 60.0;
const TETHER_SOFTNESS_PX: f32 = 30.0;

#[derive(Clone, Copy, Debug)]
struct PendingHover {
    cluster_id: ClusterId,
    started_at: Duration,
}

#[derive(Clone, Copy, Debug)]
struct OpenBloom {
    cluster_id: ClusterId,
    opened_at: Duration,
    closing_at: Option<Duration>,
}

#[derive(Clone, Debug)]
struct PullPreview {
    cluster_id: ClusterId,
    member_id: NodeId,
    output: String,
    slot_center: Point<i32, Logical>,
    core_center: Point<i32, Logical>,
    raw_offset: halley_core::field::Vec2,
    tether_started: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenLayout {
    pub cluster_id: ClusterId,
    pub member_id: NodeId,
    pub center: Point<i32, Logical>,
    pub core_center: Point<i32, Logical>,
    pub radius: i32,
    pub alpha: f32,
    pub pull_progress: f32,
}

#[derive(Default)]
pub(super) struct BloomState {
    pending_hover: Option<PendingHover>,
    open: HashMap<String, OpenBloom>,
    pull: Option<PullPreview>,
}

impl BloomState {
    pub(super) fn set_hovered(
        &mut self,
        hovered: Option<ClusterId>,
        now: Duration,
        output_for: impl FnOnce(ClusterId) -> Option<String>,
    ) {
        let Some(cluster_id) = hovered else {
            self.pending_hover = None;
            return;
        };
        if self.open.values().any(|open| open.cluster_id == cluster_id) {
            self.pending_hover = None;
            return;
        }
        if self
            .pending_hover
            .is_some_and(|pending| pending.cluster_id == cluster_id)
        {
            return;
        }
        self.pending_hover = output_for(cluster_id).map(|_| PendingHover {
            cluster_id,
            started_at: now,
        });
    }

    pub(super) fn wakeup(
        &mut self,
        now: Duration,
        output_for: impl FnOnce(ClusterId) -> Option<String>,
        cluster_is_active: impl FnOnce(&str, ClusterId) -> bool,
    ) -> bool {
        let Some(pending) = self.pending_hover else {
            return false;
        };
        if now.saturating_sub(pending.started_at) < HOLD_DURATION {
            return false;
        }
        self.pending_hover = None;
        let Some(output) = output_for(pending.cluster_id) else {
            return false;
        };
        if cluster_is_active(&output, pending.cluster_id) {
            return false;
        }
        self.open.insert(
            output,
            OpenBloom {
                cluster_id: pending.cluster_id,
                opened_at: now,
                closing_at: None,
            },
        );
        true
    }

    pub(super) fn close(&mut self, output: &str, now: Duration) -> bool {
        if self
            .pull
            .as_ref()
            .is_some_and(|preview| preview.output == output)
        {
            self.pull = None;
        }
        let Some(bloom) = self.open.get_mut(output) else {
            return false;
        };
        if bloom.closing_at.is_some() {
            return false;
        }
        bloom.closing_at = Some(now);
        true
    }

    pub(super) fn force_close(&mut self, output: &str) -> bool {
        if self
            .pull
            .as_ref()
            .is_some_and(|preview| preview.output == output)
        {
            self.pull = None;
        }
        self.open.remove(output).is_some()
    }

    pub(super) fn remove_cluster(&mut self, cluster_id: ClusterId) {
        if self
            .pending_hover
            .is_some_and(|pending| pending.cluster_id == cluster_id)
        {
            self.pending_hover = None;
        }
        self.open.retain(|_, bloom| bloom.cluster_id != cluster_id);
        if self
            .pull
            .as_ref()
            .is_some_and(|preview| preview.cluster_id == cluster_id)
        {
            self.pull = None;
        }
    }

    fn snapshot(&self, output: &str) -> Option<OpenBloom> {
        self.open.get(output).copied()
    }

    pub(super) fn is_animating(&self, output: &str, now: Duration, member_count: usize) -> bool {
        let total = bloom_animation_duration(member_count);
        self.pull
            .as_ref()
            .is_some_and(|preview| preview.output == output)
            || self.open.get(output).is_some_and(|bloom| {
                let started_at = bloom.closing_at.unwrap_or(bloom.opened_at);
                now.saturating_sub(started_at) < total
            })
    }

    pub(super) fn finish_closing(
        &mut self,
        now: Duration,
        member_count: impl Fn(ClusterId) -> usize,
    ) -> bool {
        let before = self.open.len();
        self.open.retain(|_, bloom| {
            bloom.closing_at.is_none_or(|started_at| {
                now.saturating_sub(started_at)
                    < bloom_animation_duration(member_count(bloom.cluster_id))
            })
        });
        self.open.len() != before
    }

    pub(super) fn cluster_on_output(&self, output: &str) -> Option<ClusterId> {
        self.open.get(output).map(|bloom| bloom.cluster_id)
    }

    pub(super) fn join_target_on_output(&self, output: &str) -> Option<ClusterId> {
        self.open
            .get(output)
            .filter(|bloom| bloom.closing_at.is_none())
            .map(|bloom| bloom.cluster_id)
    }

    fn is_closing(&self, output: &str) -> bool {
        self.open
            .get(output)
            .is_some_and(|bloom| bloom.closing_at.is_some())
    }
}

impl ClusterSystem {
    pub fn bloom_wakeup(&mut self, now: Duration) -> bool {
        let registry = &self.registry;
        let closed = self.bloom.finish_closing(now, |id| {
            registry
                .cluster(id)
                .map(|cluster| cluster.members().len())
                .unwrap_or(0)
        });
        let metadata = &self.metadata;
        let active = &self.active;
        let opened = self.bloom.wakeup(
            now,
            |id| metadata.get(&id).map(|metadata| metadata.output.clone()),
            |output, id| active.get(output) == Some(&id),
        );
        if opened {
            self.hovered_core = None;
        }
        opened || closed
    }

    pub fn bloom_is_animating_on_output(&self, output: &str, now: Duration) -> bool {
        let count = self
            .bloom
            .cluster_on_output(output)
            .and_then(|id| self.registry.cluster(id))
            .map(|cluster| cluster.members().len())
            .unwrap_or(0);
        self.bloom.is_animating(output, now, count)
    }

    pub fn close_bloom(&mut self, output: &str, now: Duration) -> bool {
        let cluster = self.bloom.cluster_on_output(output);
        let members = cluster
            .map(|cluster| self.member_ids(cluster))
            .unwrap_or_default();
        let closed = self.bloom.close(output, now);
        if closed {
            if self
                .join_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.output == output)
            {
                self.join_candidate = None;
            }
            if let Some(cluster) = cluster {
                self.label_hover.borrow_mut().remove(&cluster);
            }
            if self
                .overlay_hovered
                .as_ref()
                .is_some_and(|(candidate, _)| candidate == output)
            {
                self.overlay_hovered = None;
            }
            let mut labels = self.overlay_label_hover.borrow_mut();
            for member in members {
                labels.remove(&member);
            }
        }
        closed
    }

    pub fn force_close_bloom(&mut self, output: &str) -> bool {
        let closed = self.bloom.force_close(output);
        if closed {
            if self
                .join_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.output == output)
            {
                self.join_candidate = None;
            }
            self.overlay_hovered = None;
        }
        closed
    }

    pub fn bloom_open_on_output(&self, output: &str) -> Option<ClusterId> {
        self.bloom.cluster_on_output(output)
    }

    pub fn bloom_edit_target_on_output(&self, output: &str) -> Option<ClusterId> {
        self.bloom.join_target_on_output(output)
    }

    pub fn begin_bloom_pull(&mut self, token: TokenLayout, output: String) -> bool {
        if self.bloom.cluster_on_output(&output) != Some(token.cluster_id)
            || self.bloom.is_closing(&output)
        {
            return false;
        }
        self.bloom.pull = Some(PullPreview {
            cluster_id: token.cluster_id,
            member_id: token.member_id,
            output,
            slot_center: token.center,
            core_center: token.core_center,
            raw_offset: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
            tether_started: None,
        });
        true
    }

    pub fn update_bloom_pull(&mut self, pointer: Point<f64, Logical>, now: Duration) -> bool {
        let Some(preview) = self.bloom.pull.as_mut() else {
            return false;
        };
        let raw = halley_core::field::Vec2 {
            x: pointer.x as f32 - preview.slot_center.x as f32,
            y: pointer.y as f32 - preview.slot_center.y as f32,
        };
        preview.raw_offset = raw;
        let outward_axis = halley_core::field::Vec2 {
            x: (preview.slot_center.x - preview.core_center.x) as f32,
            y: (preview.slot_center.y - preview.core_center.y) as f32,
        };
        let outward_len = outward_axis.x.hypot(outward_axis.y);
        let outward_pull = if outward_len > f32::EPSILON {
            (raw.x * outward_axis.x / outward_len + raw.y * outward_axis.y / outward_len).max(0.0)
        } else {
            raw.x.hypot(raw.y)
        };
        if outward_pull >= PULL_SLOP_PX {
            preview.tether_started.get_or_insert(now);
        } else {
            preview.tether_started = None;
        }
        true
    }

    pub fn bloom_pull(&self) -> Option<(ClusterId, NodeId, String, Option<Duration>)> {
        let preview = self.bloom.pull.as_ref()?;
        Some((
            preview.cluster_id,
            preview.member_id,
            preview.output.clone(),
            preview.tether_started,
        ))
    }

    pub fn clear_bloom_pull(&mut self) -> bool {
        self.bloom.pull.take().is_some()
    }

    pub fn bloom_layout(
        &self,
        output: &str,
        camera: &Camera,
        output_geometry: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Vec<TokenLayout> {
        let Some(bloom) = self.bloom.snapshot(output) else {
            return Vec::new();
        };
        let cluster_id = bloom.cluster_id;
        if self.active_on(output).is_some() {
            return Vec::new();
        }
        let Some(metadata) = self.metadata(cluster_id) else {
            return Vec::new();
        };
        let Some(cluster) = self.registry.cluster(cluster_id) else {
            return Vec::new();
        };
        let core_center =
            crate::nodes::screen_from_world(metadata.core_position, camera, output_geometry);
        let mut members = cluster.members().to_vec();
        members.sort_by_key(|id| id.as_u64());
        let count = members.len().max(1);
        let slots = count.max(10);
        let angle_step = TAU / slots as f32;
        let min_chord = TOKEN_RADIUS_PX as f32 * 2.0 + 18.0;
        let radius = (min_chord / (2.0 * (angle_step * 0.5).sin()).max(0.20)).max(84.0)
            + (count as f32 - 1.0).min(5.0) * 3.0;
        let direction = match self.config.bloom_direction {
            halley_config::ClusterBloomDirection::Clockwise => 1.0,
            halley_config::ClusterBloomDirection::CounterClockwise => -1.0,
        };
        let pull = self.bloom.pull.as_ref();
        members
            .into_iter()
            .enumerate()
            .map(|(index, member_id)| {
                let mix = bloom_token_mix(bloom, index, count, now);
                let angle = -PI * 0.5 + direction * angle_step * index as f32;
                let mut center = Point::from((
                    core_center.x + (angle.cos() * radius * mix).round() as i32,
                    core_center.y + (angle.sin() * radius * mix).round() as i32,
                ));
                let mut pull_progress = 0.0;
                if let Some(preview) = pull.filter(|preview| {
                    preview.cluster_id == cluster_id && preview.member_id == member_id
                }) {
                    let offset = constrained_pull_offset(preview.raw_offset);
                    center.x += offset.x.round() as i32;
                    center.y += offset.y.round() as i32;
                    pull_progress = preview
                        .tether_started
                        .map(|started| {
                            now.saturating_sub(started).as_secs_f32()
                                / DETACH_HOLD_DURATION.as_secs_f32()
                        })
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0);
                }
                TokenLayout {
                    cluster_id,
                    member_id,
                    center,
                    core_center,
                    radius: TOKEN_RADIUS_PX + (pull_progress * 12.0).round() as i32,
                    alpha: mix,
                    pull_progress,
                }
            })
            .collect()
    }

    pub fn bloom_hit_test(
        &self,
        output: &str,
        camera: &Camera,
        output_geometry: Rectangle<i32, Logical>,
        point: Point<f64, Logical>,
        now: Duration,
    ) -> Option<TokenLayout> {
        self.bloom_layout(output, camera, output_geometry, now)
            .into_iter()
            .find(|token| {
                let dx = point.x - f64::from(token.center.x);
                let dy = point.y - f64::from(token.center.y);
                dx * dx + dy * dy <= f64::from(token.radius * token.radius)
            })
    }
}

fn bloom_animation_duration(member_count: usize) -> Duration {
    TOKEN_ANIMATION_DURATION
        .saturating_add(TOKEN_STAGGER.saturating_mul(member_count.saturating_sub(1) as u32))
}

fn bloom_token_mix(bloom: OpenBloom, index: usize, count: usize, now: Duration) -> f32 {
    let Some(closing_at) = bloom.closing_at else {
        return token_eased_progress(bloom.opened_at, index, now);
    };
    let opening_mix = token_eased_progress(bloom.opened_at, index, closing_at);
    let order = count.saturating_sub(1).saturating_sub(index);
    opening_mix * (1.0 - token_eased_progress(closing_at, order, now))
}

fn token_eased_progress(started_at: Duration, order: usize, now: Duration) -> f32 {
    let delay = TOKEN_STAGGER.saturating_mul(order as u32);
    let elapsed = now.saturating_sub(started_at).saturating_sub(delay);
    let linear = if TOKEN_ANIMATION_DURATION.is_zero() {
        1.0
    } else {
        (elapsed.as_secs_f32() / TOKEN_ANIMATION_DURATION.as_secs_f32()).clamp(0.0, 1.0)
    };
    linear * linear * (3.0 - 2.0 * linear)
}

fn constrained_pull_offset(raw: halley_core::field::Vec2) -> halley_core::field::Vec2 {
    let length = raw.x.hypot(raw.y);
    if length <= f32::EPSILON {
        return raw;
    }
    let constrained = if length <= TETHER_MAX_PX {
        length
    } else {
        TETHER_MAX_PX
            + TETHER_SOFTNESS_PX * (1.0 - (-(length - TETHER_MAX_PX) / TETHER_SOFTNESS_PX).exp())
    };
    halley_core::field::Vec2 {
        x: raw.x / length * constrained,
        y: raw.y / length * constrained,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::{Field, Vec2};

    fn clustered_system() -> (ClusterSystem, ClusterId, Field) {
        let mut field = Field::new();
        let first = field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 120.0 },
        );
        let second = field.spawn_surface(
            "second",
            Vec2 { x: 220.0, y: 0.0 },
            Vec2 { x: 200.0, y: 120.0 },
        );
        let mut system = ClusterSystem::new(
            halley_config::Clusters::default(),
            halley_config::ClusterAnimation::default(),
        );
        assert!(system.begin_creation("DP-1".into()));
        assert!(system.toggle_creation_member(first, "DP-1"));
        assert!(system.toggle_creation_member(second, "DP-1"));
        assert!(system.begin_naming());
        let id = system.finish_creation(&mut field).unwrap();
        (system, id, field)
    }

    #[test]
    fn bloom_opens_only_after_the_old_hover_dwell() {
        let (mut system, id, _) = clustered_system();
        system.set_hovered_core(Some(id), Duration::from_millis(100));
        assert!(!system.bloom_wakeup(Duration::from_millis(1_799)));
        assert!(system.bloom_wakeup(Duration::from_millis(1_800)));
        assert!(system.bloom_is_animating_on_output("DP-1", Duration::from_millis(1_801)));
    }

    #[test]
    fn opening_bloom_hands_hover_ownership_off_the_core() {
        let (mut system, id, _) = clustered_system();
        assert!(system.set_hovered_core(Some(id), Duration::ZERO));
        assert!(system.bloom_wakeup(HOLD_DURATION));
        assert_eq!(system.hovered_core(), None);
        assert_eq!(system.bloom_open_on_output("DP-1"), Some(id));
    }

    #[test]
    fn open_bloom_temporarily_pins_its_core_until_closing_starts() {
        let (mut system, id, _) = clustered_system();
        let core = system.core_node(id).expect("core");
        assert!(!system.collapsed_core_landmarks()[0].4);

        system.set_hovered_core(Some(id), Duration::ZERO);
        assert!(system.bloom_wakeup(HOLD_DURATION));
        assert!(system.collapsed_core_landmarks()[0].4);
        assert_eq!(system.bloom_pinned_core_nodes(), vec![core]);

        assert!(system.close_bloom("DP-1", HOLD_DURATION));
        assert!(!system.collapsed_core_landmarks()[0].4);
        assert!(system.bloom_pinned_core_nodes().is_empty());
    }

    #[test]
    fn temporary_bloom_pin_does_not_clear_a_persistent_cluster_pin() {
        let mut field = Field::new();
        let first = field.spawn_surface(
            "first",
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: 200.0, y: 120.0 },
        );
        let second = field.spawn_surface(
            "second",
            Vec2 { x: 220.0, y: 0.0 },
            Vec2 { x: 200.0, y: 120.0 },
        );
        assert!(field.set_pinned(first, true));
        let mut system = ClusterSystem::new(
            halley_config::Clusters::default(),
            halley_config::ClusterAnimation::default(),
        );
        assert!(system.begin_creation("DP-1".into()));
        assert!(system.toggle_creation_member(first, "DP-1"));
        assert!(system.toggle_creation_member(second, "DP-1"));
        assert!(system.begin_naming());
        let id = system.finish_creation(&mut field).expect("cluster");
        assert!(system.collapsed_core_landmarks()[0].4);

        system.set_hovered_core(Some(id), Duration::ZERO);
        assert!(system.bloom_wakeup(HOLD_DURATION));
        assert!(system.close_bloom("DP-1", HOLD_DURATION));
        assert!(system.collapsed_core_landmarks()[0].4);
    }

    #[test]
    fn bloom_tokens_open_and_close_one_at_a_time_in_reverse_orders() {
        let (mut system, id, _) = clustered_system();
        system.set_hovered_core(Some(id), Duration::ZERO);
        assert!(system.bloom_wakeup(HOLD_DURATION));
        let camera = Camera::new(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1_920.0,
                y: 1_080.0,
            },
        );
        let geometry = Rectangle::new((0, 0).into(), (1_920, 1_080).into());
        let opening = system.bloom_layout(
            "DP-1",
            &camera,
            geometry,
            HOLD_DURATION + Duration::from_millis(40),
        );
        assert!(opening[0].alpha > 0.0);
        assert_eq!(opening[1].alpha, 0.0);

        let fully_open = HOLD_DURATION + bloom_animation_duration(2);
        assert!(system.close_bloom("DP-1", fully_open));
        let closing = system.bloom_layout(
            "DP-1",
            &camera,
            geometry,
            fully_open + Duration::from_millis(40),
        );
        assert_eq!(closing[0].alpha, 1.0);
        assert!(closing[1].alpha < 1.0);
        assert_eq!(system.bloom_open_on_output("DP-1"), Some(id));
        assert!(system.bloom_wakeup(fully_open + bloom_animation_duration(2)));
        assert_eq!(system.bloom_open_on_output("DP-1"), None);
    }

    #[test]
    fn closing_during_opening_reverses_without_a_visual_jump() {
        let (mut system, id, _) = clustered_system();
        system.set_hovered_core(Some(id), Duration::ZERO);
        assert!(system.bloom_wakeup(HOLD_DURATION));
        let camera = Camera::new(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1_920.0,
                y: 1_080.0,
            },
        );
        let geometry = Rectangle::new((0, 0).into(), (1_920, 1_080).into());
        let interrupted = HOLD_DURATION + Duration::from_millis(80);
        let before = system.bloom_layout("DP-1", &camera, geometry, interrupted);

        assert!(system.close_bloom("DP-1", interrupted));
        let after = system.bloom_layout("DP-1", &camera, geometry, interrupted);

        assert_eq!(before, after);
    }

    #[test]
    fn bloom_pull_arms_only_when_dragged_outward_and_tracks_hold_progress() {
        let (mut system, id, _) = clustered_system();
        system.set_hovered_core(Some(id), Duration::ZERO);
        assert!(system.bloom_wakeup(HOLD_DURATION));
        let camera = Camera::new(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1_920.0,
                y: 1_080.0,
            },
        );
        let geometry = Rectangle::new((0, 0).into(), (1_920, 1_080).into());
        let now = HOLD_DURATION + bloom_animation_duration(2);
        let token = system.bloom_layout("DP-1", &camera, geometry, now)[0];
        assert!(system.begin_bloom_pull(token, "DP-1".into()));

        let inward = Point::from((token.center.x, token.center.y + 30));
        assert!(system.update_bloom_pull(inward.to_f64(), now));
        assert!(system.bloom_pull().expect("pull").3.is_none());

        let outward = Point::from((token.center.x, token.center.y - 30));
        assert!(system.update_bloom_pull(outward.to_f64(), now));
        assert_eq!(system.bloom_pull().expect("pull").3, Some(now));
        let held = system.bloom_layout("DP-1", &camera, geometry, now + DETACH_HOLD_DURATION);
        assert_eq!(held[0].pull_progress, 1.0);
    }

    #[test]
    fn detaching_a_bloom_member_restores_it_to_the_field() {
        let (mut system, id, mut field) = clustered_system();
        let member = system.first_member(id).expect("member");
        let target = Vec2 { x: 500.0, y: 300.0 };
        assert!(system.detach_member(&mut field, id, member, target, Duration::from_millis(10),));
        assert!(!system.is_member(member));
        let node = field.node(member).expect("detached node");
        assert_eq!(node.pos, target);
        assert!(field.is_visible(member));
    }
}
