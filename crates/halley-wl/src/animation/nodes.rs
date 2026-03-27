use std::collections::HashMap;
use std::time::{Duration, Instant};

use halley_core::field::{Field, NodeId, NodeState};

use super::curves::ease_out_back;

#[derive(Clone, Copy, Debug)]
pub struct AnimSpec {
    pub state_change_ms: u64,
    pub bounce: f32,
}

impl Default for AnimSpec {
    fn default() -> Self {
        Self {
            state_change_ms: 280,
            bounce: 1.45,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AnimStyle {
    pub scale: f32,
    pub alpha: f32,
}

impl Default for AnimStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            alpha: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
struct Track {
    last_state: NodeState,
    from: AnimStyle,
    to: AnimStyle,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct Pulse {
    started_at: Instant,
    duration: Duration,
    amplitude: f32,
}

#[derive(Default)]
pub struct Animator {
    spec: AnimSpec,
    tracks: HashMap<NodeId, Track>,
    pulses: HashMap<NodeId, Pulse>,
}

impl Animator {
    pub fn new(_now: Instant) -> Self {
        Self {
            spec: AnimSpec::default(),
            tracks: HashMap::new(),
            pulses: HashMap::new(),
        }
    }

    pub fn set_spec(&mut self, spec: AnimSpec) {
        self.spec = spec;
    }

    pub fn observe_field(&mut self, field: &Field, now: Instant) {
        for id in field.node_ids_all() {
            let Some(n) = field.node(id) else {
                continue;
            };
            let target = base_style(n.state.clone());
            match self.tracks.get_mut(&id) {
                Some(track) => {
                    if track.last_state != n.state {
                        let current = style_for_track(self.spec, track, now);
                        track.from = current;
                        track.to = target;
                        track.started_at = now;
                        track.last_state = n.state.clone();
                    }
                }
                None => {
                    self.tracks.insert(
                        id,
                        Track {
                            last_state: n.state.clone(),
                            from: target,
                            to: target,
                            started_at: now,
                        },
                    );
                }
            }
        }

        self.tracks.retain(|id, _| field.node(*id).is_some());
        self.pulses.retain(|id, p| {
            field.node(*id).is_some() && now.saturating_duration_since(p.started_at) <= p.duration
        });
    }

    pub fn pulse(&mut self, id: NodeId, now: Instant, amplitude: f32, duration_ms: u64) {
        self.pulses.insert(
            id,
            Pulse {
                started_at: now,
                duration: Duration::from_millis(duration_ms.max(1)),
                amplitude: amplitude.max(0.0),
            },
        );
    }

    pub fn style_for(&self, id: NodeId, state: NodeState, now: Instant) -> AnimStyle {
        let mut out = if let Some(track) = self.tracks.get(&id) {
            style_for_track(self.spec, track, now)
        } else {
            base_style(state)
        };
        if let Some(pulse) = self.pulses.get(&id) {
            let elapsed = now.saturating_duration_since(pulse.started_at);
            if elapsed <= pulse.duration {
                let t = (elapsed.as_secs_f32() / pulse.duration.as_secs_f32()).clamp(0.0, 1.0);
                let wobble = elastic_pulse(t) * pulse.amplitude;
                out.scale *= (1.0 + wobble).clamp(0.6, 1.7);
            }
        }
        out
    }

    pub fn track_elapsed_for(
        &self,
        id: NodeId,
        state: NodeState,
        now: Instant,
    ) -> Option<Duration> {
        let track = self.tracks.get(&id)?;
        if track.last_state != state {
            return None;
        }
        Some(now.saturating_duration_since(track.started_at))
    }
}

fn base_style(state: NodeState) -> AnimStyle {
    match state {
        NodeState::Active => AnimStyle {
            scale: 1.0,
            alpha: 1.0,
        },
        NodeState::Node => AnimStyle {
            scale: 0.30,
            alpha: 1.0,
        },
        NodeState::Core => AnimStyle {
            scale: 0.30,
            alpha: 1.0,
        },
        _ => AnimStyle::default(),
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn elastic_pulse(t: f32) -> f32 {
    ((1.0 - t) * 8.5).sin() * (-3.4 * t).exp()
}

fn style_for_track(spec: AnimSpec, track: &Track, now: Instant) -> AnimStyle {
    let dur = Duration::from_millis(spec.state_change_ms.max(1));
    let elapsed = now.saturating_duration_since(track.started_at);
    if elapsed >= dur {
        return track.to;
    }
    let t = (elapsed.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0);
    // Node/Core transitions should be monotonic at tail to avoid proxy flicker residue.
    let e = if track.to.scale <= 0.35 {
        ease_out_cubic(t)
    } else {
        ease_out_back(t, spec.bounce.max(0.0))
    };
    AnimStyle {
        scale: mix(track.from.scale, track.to.scale, e),
        alpha: mix(track.from.alpha, track.to.alpha, e.clamp(0.0, 1.0)),
    }
}
