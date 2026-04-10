use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::{pick_overlay_color_mode, pick_string};

pub(crate) fn load_screenshot_section(cfg: &RuneConfig, out: &mut RuntimeTuning) {
    if let Some(directory) = pick_string(
        cfg,
        &[
            "screenshot.directory",
            "screenshots.directory",
            "screenshot.output-directory",
            "screenshot.output_directory",
            "screenshots.output-directory",
            "screenshots.output_directory",
        ],
    ) {
        let trimmed = directory.trim().trim_matches('"');
        if !trimmed.is_empty() {
            out.screenshot.directory = trimmed.to_string();
        }
    }

    out.screenshot.highlight_color = pick_overlay_color_mode(
        cfg,
        &[
            "screenshot.highlight-colour",
            "screenshot.highlight_colour",
            "screenshot.highlight-color",
            "screenshot.highlight_color",
            "screenshots.highlight-colour",
            "screenshots.highlight_colour",
            "screenshots.highlight-color",
            "screenshots.highlight_color",
        ],
        out.screenshot.highlight_color,
    );

    out.screenshot.background_color = pick_overlay_color_mode(
        cfg,
        &[
            "screenshot.background-colour",
            "screenshot.background_colour",
            "screenshot.background-color",
            "screenshot.background_color",
            "screenshots.background-colour",
            "screenshots.background_colour",
            "screenshots.background-color",
            "screenshots.background_color",
        ],
        out.screenshot.background_color,
    );
}

#[cfg(test)]
mod tests {
    use rune_cfg::RuneConfig;

    use crate::layout::{OverlayColorMode, RuntimeTuning};

    use super::load_screenshot_section;

    #[test]
    fn screenshot_section_parses_directory_and_colors() {
        let cfg = RuneConfig::from_str(
            r##"
screenshot:
  directory "$env.HOME/pictures/screenshots/"
  highlight-color "dark"
  background-color "#223344"
end
"##,
        )
        .expect("screenshot config should parse");

        let mut out = RuntimeTuning::default();
        load_screenshot_section(&cfg, &mut out);

        assert_eq!(
            out.screenshot.directory,
            format!(
                "{}/pictures/screenshots/",
                std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
            )
        );
        assert_eq!(out.screenshot.highlight_color, OverlayColorMode::Dark);
        assert_eq!(
            out.screenshot.background_color,
            OverlayColorMode::Fixed {
                r: 0x22 as f32 / 255.0,
                g: 0x33 as f32 / 255.0,
                b: 0x44 as f32 / 255.0,
            }
        );
    }

    #[test]
    fn screenshot_empty_color_values_keep_auto_defaults() {
        let cfg = RuneConfig::from_str(
            r##"
screenshot:
  highlight-color ""
  background-color ""
end
"##,
        )
        .expect("screenshot config should parse");

        let mut out = RuntimeTuning::default();
        out.screenshot.highlight_color = OverlayColorMode::Auto;
        out.screenshot.background_color = OverlayColorMode::Auto;
        load_screenshot_section(&cfg, &mut out);

        assert_eq!(out.screenshot.highlight_color, OverlayColorMode::Auto);
        assert_eq!(out.screenshot.background_color, OverlayColorMode::Auto);
    }
}
