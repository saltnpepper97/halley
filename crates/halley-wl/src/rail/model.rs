use halley_config::{RailConfig, RailObstructionBehavior, RailPlacement, RailSizingMode};
use halley_core::cluster_layout::ClusterWorkspaceLayoutKind;
use halley_core::field::{NodeId, NodeKind, NodeState};
use halley_core::tiling::Rect;

use crate::compositor::root::Halley;

const SEPARATOR_THICKNESS: i32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RailOrientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RailItem {
    pub(crate) node_id: NodeId,
    pub(crate) title: String,
    pub(crate) app_id: Option<String>,
    pub(crate) pinned: bool,
    pub(crate) active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RailItemLayout {
    pub(crate) node_id: NodeId,
    pub(crate) rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RailSnapshot {
    pub(crate) monitor: String,
    pub(crate) orientation: RailOrientation,
    pub(crate) bounds: Rect,
    pub(crate) items: Vec<RailItem>,
    pub(crate) item_layouts: Vec<RailItemLayout>,
    pub(crate) pinned_count: usize,
    pub(crate) separator: Option<Rect>,
}

pub(crate) fn rail_snapshot_for_monitor(
    st: &Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    now: std::time::Instant,
) -> Option<RailSnapshot> {
    let config = st.runtime.tuning.rail;
    if !config.enabled
        || st
            .model
            .fullscreen_state
            .fullscreen_active_node
            .contains_key(monitor)
    {
        return None;
    }
    if st.active_cluster_workspace_for_monitor(monitor).is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Tiling
        )
    {
        return None;
    }

    let mut items = collect_rail_items(st, monitor);
    if items.is_empty() {
        return None;
    }
    sort_pinned_first(&mut items);
    let pinned_count = items.iter().take_while(|item| item.pinned).count();
    let snapshot = layout_rail_snapshot(monitor, &config, screen_w, screen_h, items, pinned_count)?;
    if matches!(config.obstruction, RailObstructionBehavior::AutoHide) {
        let blockers = active_window_rects_for_monitor(st, monitor, screen_w, screen_h, now);
        if rail_obstructed(snapshot.bounds, &blockers) {
            return None;
        }
    }
    Some(snapshot)
}

fn collect_rail_items(st: &Halley, monitor: &str) -> Vec<RailItem> {
    let focused = st
        .focused_node_for_monitor(monitor)
        .or(st.model.focus_state.primary_interaction_focus);
    let ids = if st.active_cluster_workspace_for_monitor(monitor).is_some()
        && matches!(
            st.runtime.tuning.cluster_layout_kind(),
            ClusterWorkspaceLayoutKind::Stacking
        ) {
        active_cluster_member_ids(st, monitor)
    } else {
        st.model.field.node_ids_all()
    };

    ids.into_iter()
        .filter_map(|node_id| {
            let node = st.model.field.node(node_id)?;
            if node.kind != NodeKind::Surface || !st.model.field.is_visible(node_id) {
                return None;
            }
            if st
                .model
                .monitor_state
                .node_monitor
                .get(&node_id)
                .map(String::as_str)
                .unwrap_or(st.model.monitor_state.current_monitor.as_str())
                != monitor
            {
                return None;
            }
            Some(RailItem {
                node_id,
                title: rail_item_title(st, node_id, node.label.as_str()),
                app_id: st.model.node_app_ids.get(&node_id).cloned(),
                pinned: st.node_user_pinned(node_id),
                active: focused == Some(node_id),
            })
        })
        .collect()
}

fn active_cluster_member_ids(st: &Halley, monitor: &str) -> Vec<NodeId> {
    st.active_cluster_workspace_for_monitor(monitor)
        .and_then(|cid| st.model.field.cluster(cid))
        .map(|cluster| cluster.members().to_vec())
        .unwrap_or_default()
}

fn rail_item_title(st: &Halley, node_id: NodeId, label: &str) -> String {
    let label = label.trim();
    if !label.is_empty() {
        return label.to_string();
    }
    st.model
        .node_app_ids
        .get(&node_id)
        .cloned()
        .unwrap_or_else(|| format!("Window {node_id}"))
}

fn sort_pinned_first(items: &mut [RailItem]) {
    items.sort_by_key(|item| (!item.pinned, item.node_id.as_u64()));
}

pub(crate) fn layout_rail_snapshot(
    monitor: &str,
    config: &RailConfig,
    screen_w: i32,
    screen_h: i32,
    items: Vec<RailItem>,
    pinned_count: usize,
) -> Option<RailSnapshot> {
    if items.is_empty() || screen_w <= 0 || screen_h <= 0 {
        return None;
    }
    let orientation = orientation_for_placement(config.placement);
    let separator_len = separator_len(config, items.len(), pinned_count);
    let item_count = items.len() as i32;
    let gap_count = (item_count - 1).max(0);
    let main_content =
        config.padding * 2 + item_count * config.icon_size + gap_count * config.gap + separator_len;
    let cross_content = config.padding * 2 + config.icon_size;

    let (mut width, mut height) = match (orientation, config.sizing) {
        (RailOrientation::Horizontal, RailSizingMode::GrowToContent) => (
            capped_or_content(main_content, config.width),
            positive_or(config.height, cross_content),
        ),
        (RailOrientation::Vertical, RailSizingMode::GrowToContent) => (
            positive_or(config.width, cross_content),
            capped_or_content(main_content, config.height),
        ),
        (RailOrientation::Horizontal, RailSizingMode::Fixed) => (
            positive_or(config.width, screen_w),
            positive_or(config.height, cross_content),
        ),
        (RailOrientation::Vertical, RailSizingMode::Fixed) => (
            positive_or(config.width, cross_content),
            positive_or(config.height, screen_h),
        ),
    };
    width = width.clamp(1, screen_w.max(1));
    height = height.clamp(1, screen_h.max(1));

    let (x, y) = match config.placement {
        RailPlacement::Up => ((screen_w - width) / 2 + config.offset_x, config.offset_y),
        RailPlacement::Down => (
            (screen_w - width) / 2 + config.offset_x,
            screen_h - height - config.offset_y,
        ),
        RailPlacement::Left => (config.offset_x, (screen_h - height) / 2 + config.offset_y),
        RailPlacement::Right => (
            screen_w - width - config.offset_x,
            (screen_h - height) / 2 + config.offset_y,
        ),
    };
    let bounds = Rect {
        x: x.clamp(0, (screen_w - width).max(0)) as f32,
        y: y.clamp(0, (screen_h - height).max(0)) as f32,
        w: width as f32,
        h: height as f32,
    };
    let (item_layouts, separator) = layout_items(config, orientation, bounds, &items, pinned_count);
    Some(RailSnapshot {
        monitor: monitor.to_string(),
        orientation,
        bounds,
        items,
        item_layouts,
        pinned_count,
        separator,
    })
}

fn orientation_for_placement(placement: RailPlacement) -> RailOrientation {
    match placement {
        RailPlacement::Up | RailPlacement::Down => RailOrientation::Horizontal,
        RailPlacement::Left | RailPlacement::Right => RailOrientation::Vertical,
    }
}

fn positive_or(value: i32, default: i32) -> i32 {
    if value > 0 { value } else { default.max(1) }
}

fn capped_or_content(content: i32, cap: i32) -> i32 {
    if cap > 0 {
        content.min(cap).max(1)
    } else {
        content.max(1)
    }
}

fn separator_len(config: &RailConfig, total: usize, pinned_count: usize) -> i32 {
    if config.pinned_separator && pinned_count > 0 && pinned_count < total {
        SEPARATOR_THICKNESS + config.gap * 2
    } else {
        0
    }
}

fn layout_items(
    config: &RailConfig,
    orientation: RailOrientation,
    bounds: Rect,
    items: &[RailItem],
    pinned_count: usize,
) -> (Vec<RailItemLayout>, Option<Rect>) {
    let mut cursor = match orientation {
        RailOrientation::Horizontal => bounds.x as i32 + config.padding,
        RailOrientation::Vertical => bounds.y as i32 + config.padding,
    };
    let cross = match orientation {
        RailOrientation::Horizontal => bounds.y as i32 + ((bounds.h as i32 - config.icon_size) / 2),
        RailOrientation::Vertical => bounds.x as i32 + ((bounds.w as i32 - config.icon_size) / 2),
    };
    let mut layouts = Vec::with_capacity(items.len());
    let mut separator = None;

    for (index, item) in items.iter().enumerate() {
        if index == pinned_count
            && pinned_count > 0
            && pinned_count < items.len()
            && config.pinned_separator
        {
            cursor += config.gap;
            let sep = match orientation {
                RailOrientation::Horizontal => Rect {
                    x: cursor as f32,
                    y: bounds.y + config.padding as f32,
                    w: SEPARATOR_THICKNESS as f32,
                    h: (bounds.h - (config.padding * 2) as f32).max(1.0),
                },
                RailOrientation::Vertical => Rect {
                    x: bounds.x + config.padding as f32,
                    y: cursor as f32,
                    w: (bounds.w - (config.padding * 2) as f32).max(1.0),
                    h: SEPARATOR_THICKNESS as f32,
                },
            };
            separator = Some(sep);
            cursor += SEPARATOR_THICKNESS + config.gap;
        }
        let rect = match orientation {
            RailOrientation::Horizontal => Rect {
                x: cursor as f32,
                y: cross as f32,
                w: config.icon_size as f32,
                h: config.icon_size as f32,
            },
            RailOrientation::Vertical => Rect {
                x: cross as f32,
                y: cursor as f32,
                w: config.icon_size as f32,
                h: config.icon_size as f32,
            },
        };
        layouts.push(RailItemLayout {
            node_id: item.node_id,
            rect,
        });
        cursor += config.icon_size + config.gap;
    }
    (layouts, separator)
}

pub(crate) fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

pub(crate) fn rail_obstructed(bounds: Rect, windows: &[Rect]) -> bool {
    windows
        .iter()
        .copied()
        .any(|window| rects_intersect(bounds, window))
}

fn active_window_rects_for_monitor(
    st: &Halley,
    monitor: &str,
    screen_w: i32,
    screen_h: i32,
    now: std::time::Instant,
) -> Vec<Rect> {
    st.model
        .field
        .nodes()
        .iter()
        .filter_map(|(&node_id, node)| {
            if node.kind != NodeKind::Surface
                || !matches!(node.state, NodeState::Active | NodeState::Drifting)
                || !st.model.field.is_visible(node_id)
                || st
                    .model
                    .monitor_state
                    .node_monitor
                    .get(&node_id)
                    .map(String::as_str)
                    .unwrap_or(st.model.monitor_state.current_monitor.as_str())
                    != monitor
                || crate::compositor::surface::is_active_cluster_workspace_member(st, node_id)
            {
                return None;
            }
            node_screen_rect_for_monitor(st, monitor, node_id, screen_w, screen_h, now)
        })
        .collect()
}

fn node_screen_rect_for_monitor(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
    screen_w: i32,
    screen_h: i32,
    now: std::time::Instant,
) -> Option<Rect> {
    if monitor == st.model.monitor_state.current_monitor
        && let Some((left, top, right, bottom)) =
            crate::input::active_node_screen_rect(st, screen_w, screen_h, node_id, now, None)
    {
        return Some(Rect {
            x: left.min(right),
            y: top.min(bottom),
            w: (right - left).abs(),
            h: (bottom - top).abs(),
        });
    }
    let space = st.model.monitor_state.monitors.get(monitor)?;
    let node = st.model.field.node(node_id)?;
    let nx = ((node.pos.x - space.viewport.center.x) / space.viewport.size.x.max(1.0)) + 0.5;
    let ny = ((node.pos.y - space.viewport.center.y) / space.viewport.size.y.max(1.0)) + 0.5;
    let cx = nx * screen_w as f32;
    let cy = ny * screen_h as f32;
    let w = node.intrinsic_size.x.max(1.0);
    let h = node.intrinsic_size.y.max(1.0);
    Some(Rect {
        x: cx - w * 0.5,
        y: cy - h * 0.5,
        w,
        h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use halley_core::field::Vec2;
    use smithay::reexports::wayland_server::Display;
    use std::time::Instant;

    fn item(id: u64, pinned: bool) -> RailItem {
        RailItem {
            node_id: NodeId::new(id),
            title: format!("item-{id}"),
            app_id: None,
            pinned,
            active: false,
        }
    }

    #[test]
    fn pinned_items_sort_first() {
        let mut items = vec![item(3, false), item(2, true), item(1, false)];
        sort_pinned_first(&mut items);
        assert_eq!(
            items
                .iter()
                .map(|item| item.node_id.as_u64())
                .collect::<Vec<_>>(),
            vec![2, 1, 3]
        );
    }

    #[test]
    fn empty_items_hide_rail() {
        assert!(
            layout_rail_snapshot("a", &RailConfig::default(), 800, 600, Vec::new(), 0).is_none()
        );
    }

    #[test]
    fn grow_to_content_horizontal_sizing_includes_separator() {
        let cfg = RailConfig {
            placement: RailPlacement::Down,
            icon_size: 20,
            gap: 5,
            padding: 10,
            width: 0,
            height: 40,
            sizing: RailSizingMode::GrowToContent,
            ..RailConfig::default()
        };
        let snapshot =
            layout_rail_snapshot("a", &cfg, 800, 600, vec![item(1, true), item(2, false)], 1)
                .unwrap();
        assert_eq!(snapshot.bounds.w, 76.0);
        assert!(snapshot.separator.is_some());
    }

    #[test]
    fn fixed_vertical_sizing_uses_configured_height() {
        let cfg = RailConfig {
            placement: RailPlacement::Left,
            width: 56,
            height: 300,
            sizing: RailSizingMode::Fixed,
            ..RailConfig::default()
        };
        let snapshot = layout_rail_snapshot("a", &cfg, 800, 600, vec![item(1, false)], 0).unwrap();
        assert_eq!(snapshot.bounds.w, 56.0);
        assert_eq!(snapshot.bounds.h, 300.0);
    }

    #[test]
    fn obstruction_detects_overlap() {
        let rail = Rect {
            x: 0.0,
            y: 100.0,
            w: 56.0,
            h: 200.0,
        };
        let blockers = [Rect {
            x: 40.0,
            y: 120.0,
            w: 100.0,
            h: 100.0,
        }];
        assert!(rail_obstructed(rail, &blockers));
        let blockers = [Rect {
            x: 80.0,
            y: 120.0,
            w: 100.0,
            h: 100.0,
        }];
        assert!(!rail_obstructed(rail, &blockers));
    }

    fn two_monitor_tuning() -> halley_config::RuntimeTuning {
        let mut tuning = halley_config::RuntimeTuning::default();
        tuning.tty_viewports = vec![
            halley_config::ViewportOutputConfig {
                connector: "left".to_string(),
                enabled: true,
                offset_x: 0,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
            halley_config::ViewportOutputConfig {
                connector: "right".to_string(),
                enabled: true,
                offset_x: 800,
                offset_y: 0,
                width: 800,
                height: 600,
                refresh_rate: None,
                transform_degrees: 0,
                vrr: halley_config::ViewportVrrMode::Off,
                focus_ring: None,
            },
        ];
        tuning.rail.obstruction = halley_config::RailObstructionBehavior::StayOnTop;
        tuning
    }

    fn spawn_surface(st: &mut Halley, label: &str, monitor: &str) -> NodeId {
        let id = st.model.field.spawn_surface(
            label,
            Vec2 { x: 300.0, y: 220.0 },
            Vec2 { x: 160.0, y: 120.0 },
        );
        st.assign_node_to_monitor(id, monitor);
        id
    }

    #[test]
    fn per_monitor_filtering_only_collects_owner_monitor() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, two_monitor_tuning());
        let left = spawn_surface(&mut st, "left-window", "left");
        let _right = spawn_surface(&mut st, "right-window", "right");

        let snapshot = rail_snapshot_for_monitor(&st, "left", 800, 600, Instant::now()).unwrap();

        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].node_id, left);
    }

    #[test]
    fn empty_monitor_hides_rail() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let st = Halley::new_for_test(&dh, two_monitor_tuning());

        assert!(rail_snapshot_for_monitor(&st, "left", 800, 600, Instant::now()).is_none());
    }

    #[test]
    fn fullscreen_monitor_hides_rail() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut st = Halley::new_for_test(&dh, two_monitor_tuning());
        let id = spawn_surface(&mut st, "full", "left");
        st.model
            .fullscreen_state
            .fullscreen_active_node
            .insert("left".to_string(), id);

        assert!(rail_snapshot_for_monitor(&st, "left", 800, 600, Instant::now()).is_none());
    }

    #[test]
    fn tiled_cluster_workspace_hides_rail() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = two_monitor_tuning();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Tiling;
        let mut st = Halley::new_for_test(&dh, tuning);
        let a = spawn_surface(&mut st, "a", "left");
        let b = spawn_surface(&mut st, "b", "left");
        let cid = st.create_cluster(vec![a, b]).expect("cluster");
        st.model
            .cluster_state
            .active_cluster_workspaces
            .insert("left".to_string(), cid);

        assert!(rail_snapshot_for_monitor(&st, "left", 800, 600, Instant::now()).is_none());
    }

    #[test]
    fn stacked_cluster_workspace_uses_cluster_local_items() {
        let dh = Display::<Halley>::new().expect("display").handle();
        let mut tuning = two_monitor_tuning();
        tuning.cluster_default_layout = halley_config::ClusterDefaultLayout::Stacking;
        let mut st = Halley::new_for_test(&dh, tuning);
        let a = spawn_surface(&mut st, "a", "left");
        let b = spawn_surface(&mut st, "b", "left");
        let outside = spawn_surface(&mut st, "outside", "left");
        let cid = st.create_cluster(vec![a, b]).expect("cluster");
        st.model
            .cluster_state
            .active_cluster_workspaces
            .insert("left".to_string(), cid);

        let snapshot = rail_snapshot_for_monitor(&st, "left", 800, 600, Instant::now()).unwrap();
        let ids = snapshot
            .items
            .iter()
            .map(|item| item.node_id)
            .collect::<Vec<_>>();

        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
        assert!(!ids.contains(&outside));
    }
}
