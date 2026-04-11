use super::*;

#[derive(Clone, Copy)]
pub(super) struct StackTransitionPose {
    pub(super) center: Vec2,
    pub(super) size: Vec2,
    pub(super) alpha: f32,
    pub(super) draw_order: i32,
}

pub(super) struct StackTransitionExtraInstance {
    pub(super) node_id: NodeId,
    pub(super) pose: StackTransitionPose,
}

pub(super) struct StackTransitionPlan {
    pub(super) poses: HashMap<NodeId, StackTransitionPose>,
    pub(super) extra_instances: Vec<StackTransitionExtraInstance>,
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_vec2(a: Vec2, b: Vec2, t: f32) -> Vec2 {
    Vec2 {
        x: lerp_f32(a.x, b.x, t),
        y: lerp_f32(a.y, b.y, t),
    }
}

fn rect_center(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.x + rect.w * 0.5,
        y: rect.y + rect.h * 0.5,
    }
}

fn rect_size(rect: Rect) -> Vec2 {
    Vec2 {
        x: rect.w.max(1.0),
        y: rect.h.max(1.0),
    }
}

pub(super) fn stack_draw_order_map(front_to_back: &[NodeId]) -> HashMap<NodeId, i32> {
    let len = front_to_back.len() as i32;
    front_to_back
        .iter()
        .enumerate()
        .map(|(index, &node_id)| (node_id, len - index as i32 - 1))
        .collect()
}

pub(super) fn build_stack_transition_plan(
    st: &Halley,
    monitor: &str,
    transition: &crate::render::state::StackCycleTransitionSnapshot,
) -> Option<StackTransitionPlan> {
    let custom_source_rects = transition.source_rects.is_some();
    let old_rects = transition
        .source_rects
        .clone()
        .or_else(|| st.stack_layout_rects_for_members(monitor, &transition.old_visible))?;
    let new_rects = st.stack_layout_rects_for_members(monitor, &transition.new_visible)?;
    let t = ease_in_out_cubic(transition.progress);
    let draw_orders = stack_draw_order_map(&transition.new_visible);
    let topmost_order = transition.new_visible.len() as i32 + 1;
    let old_top = transition.old_visible.first().copied();
    let old_bottom = transition.old_visible.last().copied();
    let new_top = transition.new_visible.first().copied();
    let wrapped_same_set = !custom_source_rects
        && transition.old_visible.len() == transition.new_visible.len()
        && transition
            .old_visible
            .iter()
            .all(|id| transition.new_visible.contains(id));
    let wrapped_node = match transition.direction {
        ClusterCycleDirection::Next
            if wrapped_same_set && old_top == transition.new_visible.last().copied() =>
        {
            old_top
        }
        ClusterCycleDirection::Prev if wrapped_same_set && old_bottom == new_top => old_bottom,
        _ => None,
    };

    let mut ids = transition.old_visible.clone();
    for &node_id in &transition.new_visible {
        if !ids.contains(&node_id) {
            ids.push(node_id);
        }
    }

    let mut poses = HashMap::new();
    let mut extra_instances = Vec::new();
    for node_id in ids {
        if wrapped_node == Some(node_id)
            && let (Some(old_rect), Some(new_rect)) = (
                old_rects.get(&node_id).copied(),
                new_rects.get(&node_id).copied(),
            )
        {
            let canonical_pose = match transition.direction {
                ClusterCycleDirection::Next => StackTransitionPose {
                    center: rect_center(new_rect),
                    size: rect_size(new_rect),
                    alpha: t,
                    draw_order: draw_orders.get(&node_id).copied().unwrap_or_default(),
                },
                ClusterCycleDirection::Prev => {
                    let end_center = rect_center(new_rect);
                    let mut start_center = end_center;
                    start_center.x -= new_rect.w * 0.55;
                    StackTransitionPose {
                        center: lerp_vec2(start_center, end_center, t),
                        size: rect_size(new_rect),
                        alpha: t,
                        draw_order: topmost_order,
                    }
                }
            };
            poses.insert(node_id, canonical_pose);

            if matches!(transition.direction, ClusterCycleDirection::Next) {
                let mut end_center = rect_center(old_rect);
                end_center.x -= old_rect.w * 0.55;
                extra_instances.push(StackTransitionExtraInstance {
                    node_id,
                    pose: StackTransitionPose {
                        center: lerp_vec2(rect_center(old_rect), end_center, t),
                        size: rect_size(old_rect),
                        alpha: 1.0 - t,
                        draw_order: topmost_order,
                    },
                });
            }
            continue;
        }

        let pose = match (
            old_rects.get(&node_id).copied(),
            new_rects.get(&node_id).copied(),
        ) {
            (Some(old_rect), Some(new_rect)) => StackTransitionPose {
                center: lerp_vec2(rect_center(old_rect), rect_center(new_rect), t),
                size: lerp_vec2(rect_size(old_rect), rect_size(new_rect), t),
                alpha: 1.0,
                draw_order: draw_orders.get(&node_id).copied().unwrap_or_default(),
            },
            (Some(old_rect), None) => {
                let mut end_center = rect_center(old_rect);
                let draw_order = match transition.direction {
                    ClusterCycleDirection::Next if Some(node_id) == old_top => {
                        end_center.x -= old_rect.w * 0.55;
                        topmost_order
                    }
                    ClusterCycleDirection::Prev if Some(node_id) == old_bottom => {
                        end_center.x += old_rect.w * 0.08;
                        end_center.y += old_rect.h * 0.04;
                        0
                    }
                    _ => 0,
                };
                StackTransitionPose {
                    center: lerp_vec2(rect_center(old_rect), end_center, t),
                    size: rect_size(old_rect),
                    alpha: 1.0 - t,
                    draw_order,
                }
            }
            (None, Some(new_rect)) => {
                let end_center = rect_center(new_rect);
                let mut start_center = end_center;
                let draw_order = match transition.direction {
                    ClusterCycleDirection::Prev if Some(node_id) == new_top => {
                        start_center.x -= new_rect.w * 0.55;
                        topmost_order
                    }
                    _ => draw_orders.get(&node_id).copied().unwrap_or_default(),
                };
                StackTransitionPose {
                    center: lerp_vec2(start_center, end_center, t),
                    size: rect_size(new_rect),
                    alpha: t,
                    draw_order,
                }
            }
            (None, None) => continue,
        };
        poses.insert(node_id, pose);
    }

    Some(StackTransitionPlan {
        poses,
        extra_instances,
    })
}

fn transform_rect_about_center(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    from_center: (f32, f32),
    to_center: (f32, f32),
    scale_x: f32,
    scale_y: f32,
) -> (i32, i32, i32, i32) {
    let rect_center_x = x as f32 + w as f32 * 0.5;
    let rect_center_y = y as f32 + h as f32 * 0.5;
    let new_center_x = to_center.0 + (rect_center_x - from_center.0) * scale_x;
    let new_center_y = to_center.1 + (rect_center_y - from_center.1) * scale_y;
    let new_w = (w as f32 * scale_x).round().max(1.0) as i32;
    let new_h = (h as f32 * scale_y).round().max(1.0) as i32;
    (
        (new_center_x - new_w as f32 * 0.5).round() as i32,
        (new_center_y - new_h as f32 * 0.5).round() as i32,
        new_w,
        new_h,
    )
}

pub(super) fn clone_stack_window_unit_for_pose(
    st: &Halley,
    size: Size<i32, Physical>,
    unit: &StackWindowDrawUnit,
    from_pose: StackTransitionPose,
    to_pose: StackTransitionPose,
) -> Option<StackWindowDrawUnit> {
    let (from_cx, from_cy) =
        world_to_screen(st, size.w, size.h, from_pose.center.x, from_pose.center.y);
    let (to_cx, to_cy) = world_to_screen(st, size.w, size.h, to_pose.center.x, to_pose.center.y);
    let scale_x = (to_pose.size.x / from_pose.size.x.max(1.0)).max(0.01);
    let scale_y = (to_pose.size.y / from_pose.size.y.max(1.0)).max(0.01);

    let border_rect = unit.border_rect.as_ref().cloned().map(|mut rect| {
        let (x, y, w, h) = transform_rect_about_center(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            (from_cx as f32, from_cy as f32),
            (to_cx as f32, to_cy as f32),
            scale_x,
            scale_y,
        );
        rect.x = x;
        rect.y = y;
        rect.w = w;
        rect.h = h;
        rect.inner_w = w as f32;
        rect.inner_h = h as f32;
        rect.alpha *= to_pose.alpha.clamp(0.0, 1.0);
        rect
    });

    let offscreen_textures = unit
        .offscreen_textures
        .iter()
        .cloned()
        .map(|mut tex| {
            let (dst_x, dst_y, dst_w, dst_h) = transform_rect_about_center(
                tex.dst_x,
                tex.dst_y,
                tex.dst_w,
                tex.dst_h,
                (from_cx as f32, from_cy as f32),
                (to_cx as f32, to_cy as f32),
                scale_x,
                scale_y,
            );
            let (clip_x, clip_y, clip_w, clip_h) = transform_rect_about_center(
                tex.clip_x,
                tex.clip_y,
                tex.clip_w,
                tex.clip_h,
                (from_cx as f32, from_cy as f32),
                (to_cx as f32, to_cy as f32),
                scale_x,
                scale_y,
            );
            tex.dst_x = dst_x;
            tex.dst_y = dst_y;
            tex.dst_w = dst_w;
            tex.dst_h = dst_h;
            tex.clip_x = clip_x;
            tex.clip_y = clip_y;
            tex.clip_w = clip_w;
            tex.clip_h = clip_h;
            tex.geo_offset_x *= scale_x;
            tex.geo_offset_y *= scale_y;
            tex.geo_w *= scale_x;
            tex.geo_h *= scale_y;
            tex.alpha *= to_pose.alpha.clamp(0.0, 1.0);
            tex
        })
        .collect::<Vec<_>>();

    if border_rect.is_none() && offscreen_textures.is_empty() {
        return None;
    }

    Some(StackWindowDrawUnit {
        node_id: unit.node_id,
        draw_order: to_pose.draw_order,
        border_rect,
        active_elements: Vec::new(),
        offscreen_textures,
    })
}
