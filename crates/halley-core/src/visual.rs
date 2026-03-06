use crate::field::{Field, NodeId, NodeKind, NodeState, Rect, Vec2};
use crate::viewport::Viewport;

/// Render-facing snapshot of a node.
/// All coordinates/sizes are in Field-space; renderer applies Viewport transform.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeVisual {
    pub id: NodeId,

    // Geometry
    pub pos: Vec2,
    pub size: Vec2,

    // Semantics
    pub kind: NodeKind,
    pub state: NodeState,

    /// Back-compat field name: this is "pinned in place" (movement constraint).
    /// (Routing anchor marker is `Node.anchor` in Field.)
    pub anchored: bool,

    // Label rendering (hover above node; becomes more prominent when zoomed out)
    pub label: String,
    pub label_scale: f32,

    // Cluster marker/badge support
    pub is_cluster_core: bool,
    pub cluster_member_count: Option<usize>,

    // Z ordering hint (higher draws on top)
    pub z: i32,

    // Optional fade hint (renderer can ignore)
    pub alpha: f32,
}

/// Parameters controlling how visuals are derived (not stored in Field).
#[derive(Clone, Copy, Debug)]
pub struct VisualParams {
    /// 1.0 = normal; smaller means zoomed OUT (you see more).
    pub zoom: f32,

    /// Optional focused node id (draw on top).
    pub focused: Option<NodeId>,

    /// Clamp range for label growth.
    pub min_label_scale: f32,
    pub max_label_scale: f32,
}

impl Default for VisualParams {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            focused: None,
            min_label_scale: 1.0,
            max_label_scale: 4.0,
        }
    }
}

fn make_visual(field: &Field, id: NodeId, params: VisualParams) -> NodeVisual {
    let n = field
        .node(id)
        .expect("make_visual called with missing node");

    let zoom = params.zoom.max(0.0001);

    // As you zoom OUT (zoom < 1), labels should grow.
    // If zoom=1 => scale=1
    // If zoom=0.5 => scale=2
    let label_scale = (1.0 / zoom).clamp(params.min_label_scale, params.max_label_scale);

    let is_cluster_core = n.kind == NodeKind::Core;

    // Optional badge count: find the cluster that owns this core, if any.
    let cluster_member_count = if is_cluster_core {
        field
            .cluster_id_for_core_public(id)
            .and_then(|cid| field.cluster(cid))
            .map(|c| c.members.len())
    } else {
        None
    };

    // Z ordering:
    // - focused highest
    // - cores above normal nodes
    // - active above node
    let mut z = 0;
    if params.focused == Some(id) {
        z += 10_000;
    }
    if is_cluster_core {
        z += 1_000;
    }
    z += match n.state {
        NodeState::Active => 300,
        NodeState::Preview => 100,
        NodeState::Drifting => 150,
        NodeState::Node => 100,
        NodeState::Core => 400,
    };

    // Alpha hint (optional): make “Node” representation a bit lighter.
    let alpha = match n.state {
        NodeState::Node => 0.85,
        _ => 1.0,
    };

    NodeVisual {
        id,
        pos: n.pos,
        size: n.footprint,
        kind: n.kind.clone(),
        state: n.state.clone(),

        // IMPORTANT: old name, new meaning
        anchored: n.pinned,

        label: n.label.clone(),
        label_scale,

        is_cluster_core,
        cluster_member_count,

        z,
        alpha,
    }
}

/// Build a render-friendly list of visuals from the current Field + Viewport.
/// - Skips nodes that are not experience-visible.
/// - Emits label scaling that grows as you zoom out.
/// - Marks Core nodes as cluster cores (badge is optional; uses cluster lookup if available).
pub fn build_visuals(field: &Field, _vp: &Viewport, params: VisualParams) -> Vec<NodeVisual> {
    let mut out = Vec::new();

    for (&id, _) in field.nodes().iter() {
        if !field.is_visible(id) {
            continue;
        }
        out.push(make_visual(field, id, params));
    }

    // Stable draw order: sort by z then id
    out.sort_by(|a, b| {
        a.z.cmp(&b.z)
            .then_with(|| a.id.as_u64().cmp(&b.id.as_u64()))
    });
    out
}

/// Like `build_visuals`, but only returns visuals whose bounds intersect `view` (in Field-space).
pub fn build_visuals_in_view(
    field: &Field,
    _vp: &Viewport,
    view: Rect,
    params: VisualParams,
) -> Vec<NodeVisual> {
    let mut out = Vec::new();

    for (&id, _) in field.nodes().iter() {
        if !field.is_visible(id) {
            continue;
        }
        if field.bounds(id).is_some_and(|b| b.intersects(view)) {
            out.push(make_visual(field, id, params));
        }
    }

    out.sort_by(|a, b| {
        a.z.cmp(&b.z)
            .then_with(|| a.id.as_u64().cmp(&b.id.as_u64()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Field;

    #[test]
    fn visuals_skip_hidden_nodes() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        assert!(f.set_hidden(b, true));

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });
        let visuals = build_visuals(&f, &vp, VisualParams::default());

        assert_eq!(visuals.len(), 1);
        assert_eq!(visuals[0].id, a);
        assert_eq!(visuals[0].label, "A");
    }

    #[test]
    fn label_scale_grows_when_zoomed_out() {
        let mut f = Field::new();
        let _a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let v1 = build_visuals(
            &f,
            &vp,
            VisualParams {
                zoom: 1.0,
                ..Default::default()
            },
        );
        let v2 = build_visuals(
            &f,
            &vp,
            VisualParams {
                zoom: 0.5,
                ..Default::default()
            },
        );

        assert!(v2[0].label_scale > v1[0].label_scale);
        assert_eq!(v1[0].label_scale, 1.0);
        assert_eq!(v2[0].label_scale, 2.0);
    }

    #[test]
    fn focused_node_draws_on_top() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let b = f.spawn_surface("B", Vec2 { x: 20.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let visuals = build_visuals(
            &f,
            &vp,
            VisualParams {
                focused: Some(b),
                ..Default::default()
            },
        );

        let za = visuals.iter().find(|v| v.id == a).unwrap().z;
        let zb = visuals.iter().find(|v| v.id == b).unwrap().z;
        assert!(zb > za);
    }

    #[test]
    fn in_view_filters_nodes() {
        let mut f = Field::new();
        let a = f.spawn_surface("A", Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 10.0, y: 10.0 });
        let _b = f.spawn_surface("B", Vec2 { x: 100.0, y: 100.0 }, Vec2 { x: 10.0, y: 10.0 });

        let vp = Viewport::new(Vec2 { x: 0.0, y: 0.0 }, Vec2 { x: 100.0, y: 100.0 });

        let view = Rect {
            min: Vec2 { x: -20.0, y: -20.0 },
            max: Vec2 { x: 20.0, y: 20.0 },
        };

        let visuals = build_visuals_in_view(&f, &vp, view, VisualParams::default());
        assert_eq!(visuals.len(), 1);
        assert_eq!(visuals[0].id, a);
    }
}
