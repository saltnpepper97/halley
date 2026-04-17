use std::collections::HashMap;
use std::time::Instant;

use halley_core::field::NodeId;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;

use crate::compositor::overlap::physics::{
    CONTACT_SKIN, MAX_PHYSICS_SPEED, PHYSICS_REST_EPSILON, POSITION_SOLVER_ITERS,
    resolve_contact_pair,
};
use crate::compositor::root::Halley;

use super::{
    carry_overlap_node_direct, clamp_speed, collision_extents_for_node,
    layout_collision_extents_for_node, node_participates_in_overlap, nodes_share_overlap_group,
    non_overlap_gap_world, physics_damping_per_sec, physics_inv_mass, required_sep_x,
    required_sep_y,
};

fn resolve_static_surface_overlap(st: &mut Halley, ids: &[NodeId]) {
    let drag_authority = st.input.interaction_state.drag_authority_node;

    for _ in 0..24 {
        let mut changed = false;

        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let a = ids[i];
                let b = ids[j];

                let Some(na) = st.model.field.node(a) else {
                    continue;
                };
                let Some(nb) = st.model.field.node(b) else {
                    continue;
                };
                if !nodes_share_overlap_group(st, a, b) {
                    continue;
                }

                let a_locked = na.pinned
                    || st.input.interaction_state.resize_active == Some(a)
                    || st.input.interaction_state.resize_static_node == Some(a);
                let b_locked = nb.pinned
                    || st.input.interaction_state.resize_active == Some(b)
                    || st.input.interaction_state.resize_static_node == Some(b);
                if a_locked && b_locked {
                    continue;
                }

                let a_pos = na.pos;
                let b_pos = nb.pos;
                let ea = layout_collision_extents_for_node(st, na);
                let eb = layout_collision_extents_for_node(st, nb);
                let gap = non_overlap_gap_world(st);
                let dx = b_pos.x - a_pos.x;
                let dy = b_pos.y - a_pos.y;
                let req_x = required_sep_x(st, a_pos.x, ea, b_pos.x, eb, gap);
                let req_y = required_sep_y(st, a_pos.y, ea, b_pos.y, eb, gap);
                let ox = req_x - dx.abs();
                let oy = req_y - dy.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }

                let sep_on_x = ox < oy;
                let sep = if sep_on_x { ox + 0.3 } else { oy + 0.3 };
                let dir_x = if dx.abs() > f32::EPSILON {
                    dx.signum()
                } else if a.as_u64() < b.as_u64() {
                    -1.0
                } else {
                    1.0
                };
                let dir_y = if dy.abs() > f32::EPSILON {
                    dy.signum()
                } else {
                    1.0
                };

                if drag_authority == Some(a) && !b_locked {
                    let target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: b_pos.x + dir_x * sep,
                            y: b_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: b_pos.x,
                            y: b_pos.y + dir_y * sep,
                        }
                    };
                    if carry_overlap_node_direct(st, b, target) {
                        changed = true;
                    }
                } else if drag_authority == Some(b) && !a_locked {
                    let target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: a_pos.x - dir_x * sep,
                            y: a_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: a_pos.x,
                            y: a_pos.y - dir_y * sep,
                        }
                    };
                    if carry_overlap_node_direct(st, a, target) {
                        changed = true;
                    }
                } else if !a_locked && !b_locked {
                    let half = sep * 0.5;
                    let a_target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: a_pos.x - dir_x * half,
                            y: a_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: a_pos.x,
                            y: a_pos.y - dir_y * half,
                        }
                    };
                    let b_target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: b_pos.x + dir_x * half,
                            y: b_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: b_pos.x,
                            y: b_pos.y + dir_y * half,
                        }
                    };
                    let moved_a = carry_overlap_node_direct(st, a, a_target);
                    let moved_b = carry_overlap_node_direct(st, b, b_target);
                    if moved_a || moved_b {
                        changed = true;
                    }
                } else if !a_locked {
                    let target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: a_pos.x - dir_x * sep,
                            y: a_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: a_pos.x,
                            y: a_pos.y - dir_y * sep,
                        }
                    };
                    if carry_overlap_node_direct(st, a, target) {
                        changed = true;
                    }
                } else if !b_locked {
                    let target = if sep_on_x {
                        halley_core::field::Vec2 {
                            x: b_pos.x + dir_x * sep,
                            y: b_pos.y,
                        }
                    } else {
                        halley_core::field::Vec2 {
                            x: b_pos.x,
                            y: b_pos.y + dir_y * sep,
                        }
                    };
                    if carry_overlap_node_direct(st, b, target) {
                        changed = true;
                    }
                }
            }
        }

        if !changed {
            break;
        }
    }
}

pub(crate) fn resolve_surface_overlap(st: &mut Halley) {
    if st.input.interaction_state.suspend_overlap_resolve {
        return;
    }

    let mut ids: Vec<NodeId> = st
        .model
        .field
        .nodes()
        .keys()
        .copied()
        .filter(|&id| node_participates_in_overlap(st, id))
        .collect();

    if ids.is_empty() {
        return;
    }

    ids.sort_by_key(|id| id.as_u64());

    if !st.runtime.tuning.physics_enabled {
        resolve_static_surface_overlap(st, &ids);
        return;
    }

    let now = Instant::now();
    let dt = now
        .saturating_duration_since(st.input.interaction_state.physics_last_tick)
        .as_secs_f32()
        .min(1.0 / 30.0);
    st.input.interaction_state.physics_last_tick = now;
    if dt <= f32::EPSILON {
        return;
    }

    let gap = non_overlap_gap_world(st);
    let damping_per_sec = physics_damping_per_sec(st);
    let damping = (-damping_per_sec * dt).exp();
    let mut positions: HashMap<NodeId, halley_core::field::Vec2> = HashMap::new();
    let mut velocities: HashMap<NodeId, halley_core::field::Vec2> = HashMap::new();

    for &id in &ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        positions.insert(id, node.pos);
        let vel = if st.input.interaction_state.drag_authority_node == Some(id) {
            st.input.interaction_state.drag_authority_velocity
        } else {
            st.input
                .interaction_state
                .physics_velocity
                .get(&id)
                .copied()
                .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 })
        };
        velocities.insert(id, clamp_speed(vel, MAX_PHYSICS_SPEED));
    }

    for &id in &ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        let pinned = node.pinned || st.input.interaction_state.resize_static_node == Some(id);
        if physics_inv_mass(st, id, pinned) <= 0.0 {
            continue;
        }
        if let (Some(pos), Some(vel)) = (positions.get_mut(&id), velocities.get_mut(&id)) {
            pos.x += vel.x * dt;
            pos.y += vel.y * dt;
            vel.x *= damping;
            vel.y *= damping;
        }
    }

    for _ in 0..POSITION_SOLVER_ITERS {
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let a = ids[i];
                let b = ids[j];

                let Some(na) = st.model.field.node(a) else {
                    continue;
                };
                let Some(nb) = st.model.field.node(b) else {
                    continue;
                };
                if !nodes_share_overlap_group(st, a, b) {
                    continue;
                }

                let a_pinned =
                    na.pinned || st.input.interaction_state.resize_static_node == Some(a);
                let b_pinned =
                    nb.pinned || st.input.interaction_state.resize_static_node == Some(b);
                let inv_mass_a = physics_inv_mass(st, a, a_pinned);
                let inv_mass_b = physics_inv_mass(st, b, b_pinned);
                if inv_mass_a <= 0.0 && inv_mass_b <= 0.0 {
                    continue;
                }

                let Some(a_pos) = positions.get(&a).copied() else {
                    continue;
                };
                let Some(b_pos) = positions.get(&b).copied() else {
                    continue;
                };

                let ea = layout_collision_extents_for_node(st, na);
                let eb = layout_collision_extents_for_node(st, nb);
                let dx = b_pos.x - a_pos.x;
                let dy = b_pos.y - a_pos.y;
                let req_x = required_sep_x(st, a_pos.x, ea, b_pos.x, eb, gap);
                let req_y = required_sep_y(st, a_pos.y, ea, b_pos.y, eb, gap);
                let gap_x = dx.abs() - req_x;
                let gap_y = dy.abs() - req_y;
                if gap_x > CONTACT_SKIN || gap_y > CONTACT_SKIN {
                    continue;
                }

                resolve_contact_pair(
                    &mut positions,
                    &mut velocities,
                    a,
                    b,
                    dx,
                    dy,
                    gap_x,
                    gap_y,
                    inv_mass_a,
                    inv_mass_b,
                );
            }
        }
    }

    for id in ids {
        let Some(node) = st.model.field.node(id) else {
            continue;
        };
        let pinned = node.pinned || st.input.interaction_state.resize_static_node == Some(id);
        if st.input.interaction_state.drag_authority_node != Some(id)
            && let Some(pos) = positions.get(&id).copied()
        {
            let _ = if node.kind == halley_core::field::NodeKind::Core {
                st.model.field.carry_cluster_by_core(id, pos)
            } else {
                st.model.field.carry(id, pos)
            };
        }
        if physics_inv_mass(st, id, pinned) <= 0.0 {
            continue;
        }
        let vel = clamp_speed(
            velocities
                .get(&id)
                .copied()
                .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 }),
            MAX_PHYSICS_SPEED,
        );
        if vel.x.abs() < PHYSICS_REST_EPSILON && vel.y.abs() < PHYSICS_REST_EPSILON {
            st.input.interaction_state.physics_velocity.remove(&id);
        } else {
            st.input.interaction_state.physics_velocity.insert(id, vel);
        }
    }
}

pub(crate) fn request_toplevel_resize(st: &mut Halley, node_id: NodeId, width: i32, height: i32) {
    let (min_w, min_h) = crate::compositor::surface::toplevel_min_size_for_node(st, node_id);
    let width = width.max(min_w).max(96);
    let height = height.max(min_h).max(72);
    let focused_node = st.last_input_surface_node();

    for top in st.platform.xdg_shell_state.toplevel_surfaces() {
        let wl = top.wl_surface();
        let key = wl.id();

        if st.model.surface_to_node.get(&key).copied() != Some(node_id) {
            continue;
        }

        let monitor = st
            .model
            .monitor_state
            .node_monitor
            .get(&node_id)
            .cloned()
            .unwrap_or_else(|| st.focused_monitor().to_string());
        let view = st.usable_viewport_for_monitor(&monitor);
        let bounds_w = view.size.x as i32;
        let bounds_h = view.size.y as i32;

        top.with_pending_state(|s| {
            s.size = Some((width, height).into());
            s.bounds = Some((bounds_w, bounds_h).into());
            if focused_node == Some(node_id) {
                s.states.set(xdg_toplevel::State::Activated);
            } else {
                s.states.unset(xdg_toplevel::State::Activated);
            }
            st.apply_toplevel_tiled_hint(s);
        });
        top.send_configure();
        break;
    }
}
