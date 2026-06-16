use std::path::{Path, PathBuf};

use rune_cfg::RuneConfig;

#[derive(Clone, Debug)]
pub struct LiftConfig {
    pub placeholder: String,
    pub width: u32,
    pub max_results: usize,
    pub visible_results: usize,
    pub fuzzy: bool,
    pub show_section_labels: bool,
    pub icons: bool,
    pub icon_size: u32,
    pub icon_search_depth: usize,
    pub icon_theme: String,
    pub terminal: String,
    pub close_on_focus_loss: bool,
    pub alt_number_jump: bool,
    pub ui: LiftUiConfig,
    pub position: LiftPositionConfig,
    pub rounding: LiftRoundingConfig,
    pub colors: LiftColorConfig,
    pub cursor: LiftCursorConfig,
    pub modes: LiftModeConfig,
    pub providers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LiftPositionConfig {
    pub anchor: String,
    pub offset_x: i32,
    pub offset_y: i32,
}

#[derive(Clone, Debug)]
pub struct LiftRoundingConfig {
    pub panel: i32,
    pub dropdown: i32,
    pub search: i32,
    pub row: i32,
    pub badge: i32,
    pub draft: i32,
}

#[derive(Clone, Debug)]
pub struct LiftColorConfig {
    pub panel: String,
    pub panel_border: String,
    pub dropdown: String,
    pub dropdown_border: String,
    pub search: String,
    pub row_selected: String,
    pub divider: String,
    pub text: String,
    pub subtext: String,
    pub hint: String,
    pub accent: String,
    pub badge: String,
    pub danger: String,
}

#[derive(Clone, Debug)]
pub struct LiftCursorConfig {
    pub enabled: bool,
    pub width: i32,
    pub blink_ms: u64,
    pub stop_blink_after_ms: u64,
}

#[derive(Clone, Debug)]
pub struct LiftUiConfig {
    pub top_margin: i32,
    pub padding: i32,
    pub dropdown_gap: i32,
    pub dropdown_padding: i32,
    pub search_height: i32,
    pub draft_height: i32,
    pub row_height: i32,
    pub row_gap: i32,
    pub section_height: i32,
    pub footer_height: i32,
    pub font: String,
    pub search_font_size: u32,
    pub badge_font_size: u32,
    pub title_font_size: u32,
    pub subtitle_font_size: u32,
    pub hint_font_size: u32,
}

#[derive(Clone, Debug)]
pub struct LiftModeConfig {
    pub apps: bool,
    pub clusters: bool,
    pub nodes: bool,
    pub actions: bool,
    pub config: bool,
}

impl Default for LiftConfig {
    fn default() -> Self {
        Self {
            placeholder: "Search apps, nodes, clusters, actions...".into(),
            width: 760,
            max_results: 12,
            visible_results: 8,
            fuzzy: true,
            show_section_labels: true,
            icons: true,
            icon_size: 28,
            icon_search_depth: 5,
            icon_theme: "auto".into(),
            terminal: "x-terminal-emulator -e".into(),
            close_on_focus_loss: false,
            alt_number_jump: true,
            ui: LiftUiConfig::default(),
            position: LiftPositionConfig::default(),
            rounding: LiftRoundingConfig::default(),
            colors: LiftColorConfig::default(),
            cursor: LiftCursorConfig::default(),
            modes: LiftModeConfig::default(),
            providers: vec![
                "apps".into(),
                "clusters".into(),
                "nodes".into(),
                "actions".into(),
            ],
        }
    }
}

impl Default for LiftPositionConfig {
    fn default() -> Self {
        Self {
            anchor: "center".into(),
            offset_x: 0,
            offset_y: 0,
        }
    }
}

impl Default for LiftRoundingConfig {
    fn default() -> Self {
        Self {
            panel: 18,
            dropdown: 14,
            search: 12,
            row: 12,
            badge: 10,
            draft: 10,
        }
    }
}

impl Default for LiftColorConfig {
    fn default() -> Self {
        Self {
            panel: "#151720ee".into(),
            panel_border: "#2b3248cc".into(),
            dropdown: "#151720ee".into(),
            dropdown_border: "#2b3248cc".into(),
            search: "#090b12d8".into(),
            row_selected: "#2e4575ea".into(),
            divider: "#2b324899".into(),
            text: "#f2f5ff".into(),
            subtext: "#9ea7bf".into(),
            hint: "#858fa8".into(),
            accent: "#8fb5ff".into(),
            badge: "#334875f2".into(),
            danger: "#eb9a8f".into(),
        }
    }
}

impl Default for LiftCursorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 2,
            blink_ms: 500,
            stop_blink_after_ms: 5000,
        }
    }
}

impl Default for LiftUiConfig {
    fn default() -> Self {
        Self {
            top_margin: 96,
            padding: 20,
            dropdown_gap: 0,
            dropdown_padding: 10,
            search_height: 60,
            draft_height: 36,
            row_height: 64,
            row_gap: 6,
            section_height: 22,
            footer_height: 0,
            font: "sans-serif".into(),
            search_font_size: 22,
            badge_font_size: 15,
            title_font_size: 17,
            subtitle_font_size: 13,
            hint_font_size: 12,
        }
    }
}

impl Default for LiftModeConfig {
    fn default() -> Self {
        Self {
            apps: true,
            clusters: true,
            nodes: true,
            actions: true,
            config: true,
        }
    }
}

impl LiftConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let cfg = RuneConfig::from_file(path)
            .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
        let mut out = Self::default();
        out.placeholder = cfg.get_or("lift.placeholder", out.placeholder.clone());
        out.width = cfg.get_or("lift.width", out.width).clamp(420, 1400);
        out.max_results = cfg
            .get_or::<u32>("lift.max-results", out.max_results as u32)
            .clamp(3, 40) as usize;
        out.visible_results = cfg
            .get_or::<u32>("lift.visible-results", out.visible_results as u32)
            .clamp(3, out.max_results as u32) as usize;
        out.fuzzy = cfg.get_or("lift.fuzzy", out.fuzzy);
        out.show_section_labels = cfg.get_or("lift.show-section-labels", out.show_section_labels);
        out.icons = cfg.get_or("lift.icons", out.icons);
        out.icon_size = cfg
            .get_or::<u32>("lift.icon-size", out.icon_size)
            .clamp(16, 64);
        out.icon_search_depth = cfg
            .get_or::<u32>("lift.icon-search-depth", out.icon_search_depth as u32)
            .clamp(1, 8) as usize;
        out.icon_theme = cfg.get_or("lift.icon-theme", out.icon_theme.clone());
        out.terminal = cfg.get_or("lift.terminal", out.terminal.clone());
        out.close_on_focus_loss = cfg.get_or("lift.close-on-focus-loss", out.close_on_focus_loss);
        out.alt_number_jump = cfg.get_or("lift.alt-number-jump", out.alt_number_jump);
        out.position.anchor = cfg.get_or("lift.position.anchor", out.position.anchor.clone());
        out.position.offset_x = cfg
            .get_or("lift.position.offset-x", out.position.offset_x)
            .clamp(-2000, 2000);
        out.position.offset_y = cfg
            .get_or("lift.position.offset-y", out.position.offset_y)
            .clamp(-2000, 2000);
        out.rounding.panel = cfg
            .get_or("lift.rounding.panel", out.rounding.panel)
            .clamp(0, 48);
        out.rounding.dropdown = cfg
            .get_or("lift.rounding.dropdown", out.rounding.dropdown)
            .clamp(0, 48);
        out.rounding.search = cfg
            .get_or("lift.rounding.search", out.rounding.search)
            .clamp(0, 48);
        out.rounding.row = cfg
            .get_or("lift.rounding.row", out.rounding.row)
            .clamp(0, 48);
        out.rounding.badge = cfg
            .get_or("lift.rounding.badge", out.rounding.badge)
            .clamp(0, 48);
        out.rounding.draft = cfg
            .get_or("lift.rounding.draft", out.rounding.draft)
            .clamp(0, 48);
        out.colors.panel = cfg.get_or("lift.colors.panel", out.colors.panel.clone());
        out.colors.panel_border =
            cfg.get_or("lift.colors.panel-border", out.colors.panel_border.clone());
        out.colors.dropdown = cfg.get_or("lift.colors.dropdown", out.colors.dropdown.clone());
        out.colors.dropdown_border = cfg.get_or(
            "lift.colors.dropdown-border",
            out.colors.dropdown_border.clone(),
        );
        out.colors.search = cfg.get_or("lift.colors.search", out.colors.search.clone());
        out.colors.row_selected =
            cfg.get_or("lift.colors.row-selected", out.colors.row_selected.clone());
        out.colors.divider = cfg.get_or("lift.colors.divider", out.colors.divider.clone());
        out.colors.text = cfg.get_or("lift.colors.text", out.colors.text.clone());
        out.colors.subtext = cfg.get_or("lift.colors.subtext", out.colors.subtext.clone());
        out.colors.hint = cfg.get_or("lift.colors.hint", out.colors.hint.clone());
        out.colors.accent = cfg.get_or("lift.colors.accent", out.colors.accent.clone());
        out.colors.badge = cfg.get_or("lift.colors.badge", out.colors.badge.clone());
        out.colors.danger = cfg.get_or("lift.colors.danger", out.colors.danger.clone());
        out.cursor.enabled = cfg.get_or("lift.cursor.enabled", out.cursor.enabled);
        out.cursor.width = cfg
            .get_or("lift.cursor.width", out.cursor.width)
            .clamp(1, 8);
        out.cursor.blink_ms = cfg
            .get_or::<u64>("lift.cursor.blink-ms", out.cursor.blink_ms)
            .clamp(150, 2000);
        let stop_blink_after_ms = cfg.get_or::<u64>(
            "lift.cursor.stop-blink-after-ms",
            out.cursor.stop_blink_after_ms,
        );
        out.cursor.stop_blink_after_ms = if stop_blink_after_ms == 0 {
            0
        } else {
            stop_blink_after_ms.clamp(1000, 60_000)
        };
        out.ui.top_margin = cfg
            .get_or("lift.ui.top-margin", out.ui.top_margin)
            .clamp(0, 480);
        out.ui.padding = cfg.get_or("lift.ui.padding", out.ui.padding).clamp(8, 48);
        out.ui.dropdown_gap = cfg
            .get_or("lift.ui.dropdown-gap", out.ui.dropdown_gap)
            .clamp(0, 32);
        out.ui.dropdown_padding = cfg
            .get_or("lift.ui.dropdown-padding", out.ui.dropdown_padding)
            .clamp(0, 32);
        out.ui.search_height = cfg
            .get_or("lift.ui.search-height", out.ui.search_height)
            .clamp(42, 96);
        out.ui.draft_height = cfg
            .get_or("lift.ui.draft-height", out.ui.draft_height)
            .clamp(28, 72);
        out.ui.row_height = cfg
            .get_or("lift.ui.row-height", out.ui.row_height)
            .clamp(46, 110);
        out.ui.row_gap = cfg.get_or("lift.ui.row-gap", out.ui.row_gap).clamp(0, 20);
        out.ui.section_height = cfg
            .get_or("lift.ui.section-height", out.ui.section_height)
            .clamp(0, 40);
        out.ui.footer_height = cfg
            .get_or("lift.ui.footer-height", out.ui.footer_height)
            .clamp(0, 70);
        out.ui.font = cfg.get_or("lift.ui.font", out.ui.font.clone());
        out.ui.search_font_size = cfg
            .get_or::<u32>("lift.ui.search-font-size", out.ui.search_font_size)
            .clamp(14, 42);
        out.ui.badge_font_size = cfg
            .get_or::<u32>("lift.ui.badge-font-size", out.ui.badge_font_size)
            .clamp(10, 28);
        out.ui.title_font_size = cfg
            .get_or::<u32>("lift.ui.title-font-size", out.ui.title_font_size)
            .clamp(12, 34);
        out.ui.subtitle_font_size = cfg
            .get_or::<u32>("lift.ui.subtitle-font-size", out.ui.subtitle_font_size)
            .clamp(10, 26);
        out.ui.hint_font_size = cfg
            .get_or::<u32>("lift.ui.hint-font-size", out.ui.hint_font_size)
            .clamp(10, 24);
        out.modes.apps = cfg.get_or("lift.modes.apps", out.modes.apps);
        out.modes.clusters = cfg.get_or("lift.modes.clusters", out.modes.clusters);
        out.modes.nodes = cfg.get_or("lift.modes.nodes", out.modes.nodes);
        out.modes.actions = cfg.get_or("lift.modes.actions", out.modes.actions);
        out.modes.config = cfg.get_or("lift.modes.config", out.modes.config);
        if let Ok(Some(providers)) = cfg.get_optional::<Vec<String>>("lift.providers") {
            let providers = providers
                .into_iter()
                .filter(|p| known_provider(p))
                .collect::<Vec<_>>();
            if !providers.is_empty() {
                out.providers = providers;
            }
        }
        validate(&out)?;
        Ok(out)
    }
}

fn known_provider(value: &str) -> bool {
    matches!(value, "apps" | "clusters" | "nodes" | "actions" | "config")
}

fn validate(config: &LiftConfig) -> Result<(), String> {
    if config.width < 420 {
        return Err("lift.width must be at least 420".into());
    }
    if config.max_results == 0 {
        return Err("lift.max-results must be greater than zero".into());
    }
    if config.terminal.trim().is_empty() {
        return Err("lift.terminal must not be empty".into());
    }
    if config.visible_results == 0 || config.visible_results > config.max_results {
        return Err("lift.visible-results must be between 1 and lift.max-results".into());
    }
    if !matches!(
        config.position.anchor.to_ascii_lowercase().as_str(),
        "center" | "top" | "top-left" | "top-right" | "bottom" | "bottom-left" | "bottom-right"
    ) {
        return Err("lift.position.anchor must be center, top, top-left, top-right, bottom, bottom-left, or bottom-right".into());
    }
    Ok(())
}

pub fn default_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Path::new(&xdg).join("halley/lift.rune");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Path::new(&home).join(".config/halley/lift.rune");
    }
    PathBuf::from("lift.rune")
}
