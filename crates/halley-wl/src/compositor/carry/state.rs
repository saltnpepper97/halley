use halley_core::decay::DecayLevel;
use halley_core::field::NodeId;
use halley_core::viewport::FocusZone;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub(crate) struct CarryState {
    pub(crate) carry_zone_hint: HashMap<NodeId, FocusZone>,
    pub(crate) carry_zone_last_change_ms: HashMap<NodeId, u64>,
    pub(crate) carry_zone_pending: HashMap<NodeId, FocusZone>,
    pub(crate) carry_zone_pending_since_ms: HashMap<NodeId, u64>,
    pub(crate) carry_activation_anim_armed: HashSet<NodeId>,
    pub(crate) carry_direct_nodes: HashSet<NodeId>,
    pub(crate) carry_state_hold: HashMap<NodeId, halley_core::field::NodeState>,
}

pub(crate) fn enforce_carry_zone_states(st: &mut crate::compositor::root::Halley) {
    let tracked: Vec<(NodeId, FocusZone)> = st
        .model
        .carry_state
        .carry_zone_hint
        .iter()
        .map(|(&id, &z)| (id, z))
        .collect();

    for (id, zone) in tracked {
        if !st.model.field.is_visible(id) {
            continue;
        }
        let Some(n) = st.model.field.node(id) else {
            continue;
        };
        if n.kind != halley_core::field::NodeKind::Surface {
            continue;
        }
        if st.preserve_collapsed_surface(id) {
            continue;
        }

        let held_state = st.model.carry_state.carry_state_hold.get(&id);
        let target = match zone {
            _ if matches!(held_state, Some(halley_core::field::NodeState::Active)) => {
                DecayLevel::Hot
            }
            _ if matches!(
                held_state,
                Some(halley_core::field::NodeState::Node | halley_core::field::NodeState::Core)
            ) =>
            {
                DecayLevel::Cold
            }
            FocusZone::Inside if n.state == halley_core::field::NodeState::Active => {
                DecayLevel::Hot
            }
            FocusZone::Inside => DecayLevel::Cold,
            FocusZone::Outside => DecayLevel::Cold,
        };
        if matches!(target, DecayLevel::Cold) {
            crate::compositor::workspace::state::start_active_to_node_close_animation(
                st,
                id,
                Instant::now(),
            );
        }
        let _ = st.model.field.set_decay_level(id, target);
    }
}
