use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use halley_core::field::NodeId;
use smithay::utils::{Logical, Point, Rectangle};

const SHOW_RATE_AT_60HZ: f32 = 0.18;
const HIDE_RETAIN_AT_60HZ: f32 = 0.72;
const SETTLE_EPSILON: f32 = 0.004;

#[derive(Clone, Copy, Debug)]
struct OutputAnimation {
    mix: f32,
    last_tick: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BearingTarget {
    Node(NodeId),
    ClusterCore {
        cluster: halley_core::cluster::ClusterId,
        core: NodeId,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct BearingHitbox {
    pub target: BearingTarget,
    pub rect: Rectangle<i32, Logical>,
}

pub struct BearingsState {
    pub config: halley_config::Bearings,
    visible: bool,
    held_show_keys: HashSet<u32>,
    animation: HashMap<String, OutputAnimation>,
    hitboxes: RefCell<HashMap<String, Vec<BearingHitbox>>>,
}

impl BearingsState {
    pub fn new(config: halley_config::Bearings) -> Self {
        Self {
            config,
            visible: false,
            held_show_keys: HashSet::new(),
            animation: HashMap::new(),
            hitboxes: RefCell::new(HashMap::new()),
        }
    }

    pub fn reload(&mut self, config: halley_config::Bearings) -> bool {
        if self.config == config {
            return false;
        }
        self.config = config;
        self.hitboxes.borrow_mut().clear();
        true
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) -> bool {
        if self.visible == visible {
            return false;
        }
        self.visible = visible;
        true
    }

    pub fn toggle(&mut self) -> bool {
        self.set_visible(!self.visible);
        self.visible
    }

    pub fn show_while_held(&mut self, keycode: u32) -> bool {
        self.held_show_keys.insert(keycode);
        self.set_visible(true)
    }

    pub fn release_show_key(&mut self, keycode: u32) -> bool {
        if !self.held_show_keys.remove(&keycode) {
            return false;
        }
        if self.held_show_keys.is_empty() {
            self.set_visible(false);
        }
        true
    }

    pub fn is_show_key_held(&self, keycode: u32) -> bool {
        self.held_show_keys.contains(&keycode)
    }

    pub fn tick(&mut self, output: &str, now: Duration, allowed: bool) -> bool {
        let target = if self.visible && allowed { 1.0 } else { 0.0 };
        let state = self
            .animation
            .entry(output.to_string())
            .or_insert(OutputAnimation {
                mix: target,
                last_tick: now,
            });
        let dt = now
            .saturating_sub(state.last_tick)
            .as_secs_f32()
            .clamp(0.0, 0.1);
        state.last_tick = now;
        if !allowed {
            let changed = state.mix > SETTLE_EPSILON;
            state.mix = 0.0;
            return changed;
        }
        state.mix = animate_mix(state.mix, target, dt);
        (state.mix - target).abs() > SETTLE_EPSILON
    }

    pub fn mix(&self, output: &str) -> f32 {
        self.animation
            .get(output)
            .map(|state| state.mix)
            .unwrap_or(if self.visible { 1.0 } else { 0.0 })
    }

    pub fn set_hitboxes(&self, output: &str, hitboxes: Vec<BearingHitbox>) {
        self.hitboxes
            .borrow_mut()
            .insert(output.to_string(), hitboxes);
    }

    /// The chips last drawn on `output`, for the nested test server.
    #[cfg(test)]
    pub fn hitboxes(&self, output: &str) -> Vec<BearingHitbox> {
        self.hitboxes
            .borrow()
            .get(output)
            .cloned()
            .unwrap_or_default()
    }

    pub fn hit_test(&self, output: &str, local: Point<f64, Logical>) -> Option<BearingTarget> {
        (self.mix(output) > 0.05)
            .then(|| self.hitboxes.borrow().get(output).cloned())
            .flatten()?
            .into_iter()
            .find(|hitbox| hitbox.rect.to_f64().contains(local))
            .map(|hitbox| hitbox.target)
    }
}

fn animate_mix(current: f32, target: f32, dt: f32) -> f32 {
    if (current - target).abs() <= SETTLE_EPSILON {
        return target;
    }
    let retain_at_60hz = if target > current {
        1.0 - SHOW_RATE_AT_60HZ
    } else {
        HIDE_RETAIN_AT_60HZ
    };
    let retain = retain_at_60hz.powf(dt.max(0.0) * 60.0);
    let next = target + (current - target) * retain;
    if (next - target).abs() <= SETTLE_EPSILON {
        target
    } else {
        next.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_is_refresh_rate_independent() {
        let at_60 = (0..60).fold(0.0, |mix, _| animate_mix(mix, 1.0, 1.0 / 60.0));
        let at_120 = (0..120).fold(0.0, |mix, _| animate_mix(mix, 1.0, 1.0 / 120.0));
        let at_144 = (0..144).fold(0.0, |mix, _| animate_mix(mix, 1.0, 1.0 / 144.0));
        assert!((at_60 - at_120).abs() < 0.002);
        assert!((at_60 - at_144).abs() < 0.002);
    }

    #[test]
    fn held_show_key_hides_even_after_modifier_release() {
        let mut bearings = BearingsState::new(halley_config::Bearings::default());
        assert!(bearings.show_while_held(44));
        assert!(bearings.visible());
        assert!(bearings.release_show_key(44));
        assert!(!bearings.visible());
    }

    #[test]
    fn immersive_output_hides_immediately_without_clearing_visible() {
        let mut bearings = BearingsState::new(halley_config::Bearings::default());
        assert!(bearings.set_visible(true));
        assert!(!bearings.tick("DP-1", Duration::from_millis(16), true));
        assert_eq!(bearings.mix("DP-1"), 1.0);

        assert!(bearings.tick("DP-1", Duration::from_millis(32), false));
        assert_eq!(bearings.mix("DP-1"), 0.0);
        assert!(bearings.visible());
        assert!(!bearings.tick("DP-1", Duration::from_millis(48), false));
    }
}
