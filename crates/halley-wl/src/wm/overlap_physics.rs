use std::collections::HashMap;

use halley_core::field::{NodeId, Vec2};

pub(crate) const CONTACT_SLOP: f32 = 0.5;
pub(crate) const CONTACT_SKIN: f32 = 1.5;
pub(crate) const MAX_PHYSICS_SPEED: f32 = 1600.0;
pub(crate) const CONTACT_RESTITUTION: f32 = 0.02;
pub(crate) const CONTACT_FRICTION: f32 = 0.22;
pub(crate) const MAX_CONTACT_IMPULSE: f32 = 380.0;
pub(crate) const MAX_POSITION_CORRECTION: f32 = 48.0;
pub(crate) const POSITION_SOLVER_ITERS: usize = 6;
pub(crate) const PHYSICS_REST_EPSILON: f32 = 4.0;

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_contact_pair(
    positions: &mut HashMap<NodeId, Vec2>,
    velocities: &mut HashMap<NodeId, Vec2>,
    a: NodeId,
    b: NodeId,
    dx: f32,
    dy: f32,
    gap_x: f32,
    gap_y: f32,
    inv_mass_a: f32,
    inv_mass_b: f32,
) {
    let solve_x = gap_x >= gap_y;
    let normal = if solve_x {
        Vec2 {
            x: if dx.abs() > f32::EPSILON {
                dx.signum()
            } else if a.as_u64() < b.as_u64() {
                -1.0
            } else {
                1.0
            },
            y: 0.0,
        }
    } else {
        Vec2 {
            x: 0.0,
            y: if dy.abs() > f32::EPSILON {
                dy.signum()
            } else {
                1.0
            },
        }
    };

    let penetration = if solve_x {
        (-gap_x).max(0.0)
    } else {
        (-gap_y).max(0.0)
    };
    if penetration > 0.0 {
        let correction = (penetration + CONTACT_SLOP).min(MAX_POSITION_CORRECTION);
        let total_inv = inv_mass_a + inv_mass_b;
        if total_inv > 0.0 {
            let move_a = correction * (inv_mass_a / total_inv);
            let move_b = correction * (inv_mass_b / total_inv);
            if let Some(pos) = positions.get_mut(&a) {
                pos.x -= normal.x * move_a;
                pos.y -= normal.y * move_a;
            }
            if let Some(pos) = positions.get_mut(&b) {
                pos.x += normal.x * move_b;
                pos.y += normal.y * move_b;
            }
        }
    }

    let Some(va) = velocities.get(&a).copied() else {
        return;
    };
    let Some(vb) = velocities.get(&b).copied() else {
        return;
    };
    let rel_x = vb.x - va.x;
    let rel_y = vb.y - va.y;
    let rel_normal = rel_x * normal.x + rel_y * normal.y;
    if rel_normal >= 0.0 {
        return;
    }

    let total_inv = inv_mass_a + inv_mass_b;
    if total_inv <= 0.0 {
        return;
    }

    let normal_impulse = (-(1.0 + CONTACT_RESTITUTION) * rel_normal / total_inv)
        .min(MAX_CONTACT_IMPULSE)
        .max(0.0);
    let impulse_x = normal.x * normal_impulse;
    let impulse_y = normal.y * normal_impulse;

    if let Some(vel) = velocities.get_mut(&a) {
        vel.x -= impulse_x * inv_mass_a;
        vel.y -= impulse_y * inv_mass_a;
    }
    if let Some(vel) = velocities.get_mut(&b) {
        vel.x += impulse_x * inv_mass_b;
        vel.y += impulse_y * inv_mass_b;
    }

    let tangent_x = rel_x - normal.x * rel_normal;
    let tangent_y = rel_y - normal.y * rel_normal;
    let tangent_len = (tangent_x * tangent_x + tangent_y * tangent_y).sqrt();
    if tangent_len <= f32::EPSILON {
        return;
    }
    let tx = tangent_x / tangent_len;
    let ty = tangent_y / tangent_len;
    let rel_tangent = rel_x * tx + rel_y * ty;
    let friction_impulse = (-rel_tangent / total_inv).clamp(
        -CONTACT_FRICTION * normal_impulse,
        CONTACT_FRICTION * normal_impulse,
    );
    let friction_x = tx * friction_impulse;
    let friction_y = ty * friction_impulse;

    if let Some(vel) = velocities.get_mut(&a) {
        vel.x -= friction_x * inv_mass_a;
        vel.y -= friction_y * inv_mass_a;
    }
    if let Some(vel) = velocities.get_mut(&b) {
        vel.x += friction_x * inv_mass_b;
        vel.y += friction_y * inv_mass_b;
    }
}
