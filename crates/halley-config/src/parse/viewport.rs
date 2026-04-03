use rune_cfg::RuneConfig;

use crate::layout::{ViewportOutputConfig, ViewportVrrMode};

use super::primitives::{parse_viewport_focus_ring, pick_bool, pick_f32, pick_i32, pick_string, pick_u32};

pub(crate) fn parse_viewport_outputs(cfg: &RuneConfig, root: &str) -> Vec<ViewportOutputConfig> {
    let mut out = Vec::new();

    let Ok(keys) = cfg.get_keys(root) else {
        return out;
    };

    for key in keys {
        let enabled = pick_bool(
            cfg,
            &[
                format!("{root}.{key}.enabled").as_str(),
                format!("{root}.{key}.active").as_str(),
            ],
            true,
        );

        let width = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.width").as_str(),
                format!("{root}.{key}.size-w").as_str(),
                format!("{root}.{key}.size_w").as_str(),
            ],
            0,
        );

        let height = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.height").as_str(),
                format!("{root}.{key}.size-h").as_str(),
                format!("{root}.{key}.size_h").as_str(),
            ],
            0,
        );

        if width == 0 || height == 0 {
            continue;
        }

        let offset_x = pick_i32(
            cfg,
            &[
                format!("{root}.{key}.offset-x").as_str(),
                format!("{root}.{key}.offset_x").as_str(),
            ],
            0,
        );

        let offset_y = pick_i32(
            cfg,
            &[
                format!("{root}.{key}.offset-y").as_str(),
                format!("{root}.{key}.offset_y").as_str(),
            ],
            0,
        );

        let refresh_rate = {
            let v = pick_f32(
                cfg,
                &[
                    format!("{root}.{key}.refresh-rate").as_str(),
                    format!("{root}.{key}.refresh_rate").as_str(),
                    format!("{root}.{key}.rate").as_str(),
                ],
                0.0,
            );
            if v > 0.0 { Some(v as f64) } else { None }
        };

        let transform_degrees = pick_u32(
            cfg,
            &[
                format!("{root}.{key}.transform").as_str(),
                format!("{root}.{key}.rotation").as_str(),
            ],
            0,
        );
        let transform_degrees = match transform_degrees {
            0 | 90 | 180 | 270 => transform_degrees as u16,
            1 => 90,
            2 => 180,
            3 => 270,
            _ => 0,
        };

        let vrr = pick_viewport_vrr_mode(
            cfg,
            &[
                format!("{root}.{key}.vrr").as_str(),
                format!("{root}.{key}.variable-refresh-rate").as_str(),
                format!("{root}.{key}.variable_refresh_rate").as_str(),
            ],
            ViewportVrrMode::Off,
        );
        let focus_ring = parse_viewport_focus_ring(cfg, root, &key);

        out.push(ViewportOutputConfig {
            connector: key,
            enabled,
            offset_x,
            offset_y,
            width,
            height,
            refresh_rate,
            transform_degrees,
            vrr,
            focus_ring,
        });
    }

    out
}

fn pick_viewport_vrr_mode(
    cfg: &RuneConfig,
    paths: &[&str],
    default: ViewportVrrMode,
) -> ViewportVrrMode {
    let Some(raw) = pick_string(cfg, paths) else {
        return default;
    };
    match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "off" | "false" => ViewportVrrMode::Off,
        "on" | "true" => ViewportVrrMode::On,
        "on-demand" | "ondemand" | "adaptive" => ViewportVrrMode::OnDemand,
        _ => default,
    }
}

