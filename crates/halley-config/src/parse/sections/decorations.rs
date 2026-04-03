use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_bool, pick_decoration_border_color, pick_i32, pick_optional_bool};

pub(crate) fn load_decorations_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    out.border_size_px = pick_i32(
        cfg,
        &[
            "decoration.border-size",
            "decoration.border_size",
            "decorations.border-size",
            "decorations.border_size",
        ],
        out.border_size_px,
    );
    out.border_radius_px = pick_i32(
        cfg,
        &[
            "decoration.border-radius",
            "decoration.border_radius",
            "decorations.border-radius",
            "decorations.border_radius",
        ],
        out.border_radius_px,
    );
    out.border_color_focused = pick_decoration_border_color(
        cfg,
        &[
            "decoration.border-colour-focused",
            "decoration.border_colour_focused",
            "decoration.border-color-focused",
            "decoration.border_color_focused",
            "decorations.border-colour-focused",
            "decorations.border_colour_focused",
            "decorations.border-color-focused",
            "decorations.border_color_focused",
        ],
        out.border_color_focused,
    );
    out.border_color_unfocused = pick_decoration_border_color(
        cfg,
        &[
            "decoration.border-colour-unfocused",
            "decoration.border_colour_unfocused",
            "decoration.border-color-unfocused",
            "decoration.border_color_unfocused",
            "decorations.border-colour-unfocused",
            "decorations.border_colour_unfocused",
            "decorations.border-color-unfocused",
            "decorations.border_color_unfocused",
        ],
        out.border_color_unfocused,
    );
    out.resize_using_border = pick_bool(
        cfg,
        &[
            "decoration.resize-using-border",
            "decoration.resize_using_border",
            "decorations.resize-using-border",
            "decorations.resize_using_border",
        ],
        out.resize_using_border,
    );
    if let Some(no_csd) = pick_optional_bool(
        cfg,
        &[
            "decoration.no-csd",
            "decoration.no_csd",
            "decorations.no-csd",
            "decorations.no_csd",
        ],
    ) {
        out.no_csd = no_csd;
    }
}

