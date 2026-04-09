use rune_cfg::RuneConfig;

use crate::layout::RuntimeTuning;

use super::super::primitives::pick_string;

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
}
