use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{
    pick_click_collapsed_outside_focus_mode, pick_click_collapsed_pan_mode, pick_f32,
    pick_node_background_color_mode, pick_node_border_color_mode, pick_node_display_policy,
    pick_shape_style, pick_u64,
};

pub(crate) fn load_nodes_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.primary_to_node_ms = pick_u64(
        cfg,
        &[
            "node.primary-to-node-ms",
            "node.primary_to_node_ms",
            "nodes.primary-to-node-ms",
            "nodes.primary_to_node_ms",
            "nodes.node-delay",
            "nodes.node_delay",
        ],
        out.primary_to_node_ms,
    );

    let legacy_preview = pick_u64(
        cfg,
        &[
            "node.primary-to-preview-ms",
            "node.primary_to_preview_ms",
            "nodes.primary-to-preview-ms",
            "nodes.primary_to_preview_ms",
            "nodes.preview-delay",
            "nodes.preview_delay",
        ],
        0,
    );
    let legacy_preview_to_node = pick_u64(
        cfg,
        &[
            "node.primary-preview-to-node-ms",
            "node.primary_preview_to_node_ms",
            "node.preview-to-node-ms",
            "node.preview_to_node_ms",
            "nodes.primary-preview-to-node-ms",
            "nodes.primary_preview_to_node_ms",
            "nodes.preview-to-node-ms",
            "nodes.preview_to_node_ms",
        ],
        0,
    );

    if legacy_preview > 0 || legacy_preview_to_node > 0 {
        let combined = legacy_preview.saturating_add(legacy_preview_to_node);
        if combined > 0 {
            out.primary_to_node_ms = combined;
        }
    }

    out.primary_hot_inner_frac = pick_f32(
        cfg,
        &[
            "node.primary-hot-inner-frac",
            "node.primary_hot_inner_frac",
            "node.hot-inner-frac",
            "node.hot_inner_frac",
            "nodes.primary-hot-inner-frac",
            "nodes.primary_hot_inner_frac",
            "nodes.hot-inner-frac",
            "nodes.hot_inner_frac",
        ],
        out.primary_hot_inner_frac,
    );

    out.node_show_labels = pick_node_display_policy(
        cfg,
        &[
            "node.show-labels",
            "node.show_labels",
            "nodes.show-labels",
            "nodes.show_labels",
        ],
        out.node_show_labels,
    );
    out.node_show_app_icons = pick_node_display_policy(
        cfg,
        &[
            "node.show-app-icons",
            "node.show_app_icons",
            "node.show-icons",
            "node.show_icons",
            "nodes.show-app-icons",
            "nodes.show_app_icons",
            "nodes.show-icons",
            "nodes.show_icons",
        ],
        out.node_show_app_icons,
    );
    out.node_shape = pick_shape_style(
        cfg,
        &[
            "node.node-shape",
            "node.node_shape",
            "node.shape",
            "nodes.node-shape",
            "nodes.node_shape",
            "nodes.shape",
        ],
        out.node_shape,
    );
    out.node_label_shape = pick_shape_style(
        cfg,
        &[
            "node.node-label-shape",
            "node.node_label_shape",
            "node.label-shape",
            "node.label_shape",
            "nodes.node-label-shape",
            "nodes.node_label_shape",
            "nodes.label-shape",
            "nodes.label_shape",
        ],
        out.node_label_shape,
    );
    out.node_icon_size = pick_f32(
        cfg,
        &[
            "node.icon-size",
            "node.icon_size",
            "nodes.icon-size",
            "nodes.icon_size",
        ],
        out.node_icon_size,
    );
    out.node_background_color = pick_node_background_color_mode(
        cfg,
        &[
            "node.background-colour",
            "node.background_colour",
            "node.background-color",
            "node.background_color",
            "nodes.background-colour",
            "nodes.background_colour",
            "nodes.background-color",
            "nodes.background_color",
        ],
        out.node_background_color,
    );

    out.node_border_color_hover = pick_node_border_color_mode(
        cfg,
        &[
            "node.border-colour-hover",
            "node.border_colour_hover",
            "node.border-color-hover",
            "node.border_color_hover",
            "nodes.border-colour-hover",
            "nodes.border_colour_hover",
            "nodes.border-color-hover",
            "nodes.border_color_hover",
        ],
        out.node_border_color_hover,
    );
    out.node_border_color_inactive = pick_node_border_color_mode(
        cfg,
        &[
            "node.border-colour-inactive",
            "node.border_colour_inactive",
            "node.border-color-inactive",
            "node.border_color_inactive",
            "nodes.border-colour-inactive",
            "nodes.border_colour_inactive",
            "nodes.border-color-inactive",
            "nodes.border_color_inactive",
        ],
        out.node_border_color_inactive,
    );
    out.click_collapsed_outside_focus = pick_click_collapsed_outside_focus_mode(
        cfg,
        &[
            "node.click-collapsed-outside-focus",
            "node.click_collapsed_outside_focus",
            "nodes.click-collapsed-outside-focus",
            "nodes.click_collapsed_outside_focus",
        ],
        out.click_collapsed_outside_focus,
    );
    out.click_collapsed_pan = pick_click_collapsed_pan_mode(
        cfg,
        &[
            "node.click-collapsed-pan",
            "node.click_collapsed_pan",
            "nodes.click-collapsed-pan",
            "nodes.click_collapsed_pan",
        ],
        out.click_collapsed_pan,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{NodeBorderColorMode, RuntimeTuning};

    use super::load_nodes_section;

    #[test]
    fn nodes_section_parses_secondary_border_color_modes() {
        let cfg = RuneConfig::from_str(
            r##"
node:
  border-colour-hover "use-window-secondary-active"
  border-colour-inactive "use-window-secondary-inactive"
end
"##,
        )
        .expect("node config should parse");

        let mut out = RuntimeTuning::default();
        load_nodes_section(&cfg, &mut out);

        assert_eq!(
            out.node_border_color_hover,
            NodeBorderColorMode::UseWindowSecondaryActive
        );
        assert_eq!(
            out.node_border_color_inactive,
            NodeBorderColorMode::UseWindowSecondaryInactive
        );
    }
}
