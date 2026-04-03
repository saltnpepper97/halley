use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_cluster_bloom_direction, pick_cluster_default_layout, pick_f32, pick_u64};

pub(crate) fn load_clusters_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.cluster_distance_px = pick_f32(
        cfg,
        &["clusters.distance-px", "clusters.distance_px"],
        out.cluster_distance_px,
    );
    out.cluster_dwell_ms = pick_u64(
        cfg,
        &["clusters.dwell-ms", "clusters.dwell_ms"],
        out.cluster_dwell_ms,
    );
    out.cluster_show_icons = pick_bool(
        cfg,
        &["clusters.show-icons", "clusters.show_icons"],
        out.cluster_show_icons,
    );
    out.cluster_bloom_direction = pick_cluster_bloom_direction(
        cfg,
        &["clusters.bloom-direction", "clusters.bloom_direction"],
        out.cluster_bloom_direction,
    );
    out.cluster_default_layout = pick_cluster_default_layout(
        cfg,
        &["clusters.default-layout", "clusters.default_layout"],
        out.cluster_default_layout,
    );
}

