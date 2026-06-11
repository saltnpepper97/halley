use std::path::{Path, PathBuf};

use rune_cfg::RuneConfig;

#[derive(Clone, Debug)]
pub struct LensConfig {
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
    pub keyboard_interactivity: String,
    pub close_on_focus_loss: bool,
    pub close_on_click_away: bool,
    pub alt_number_jump: bool,
    pub ui: LensUiConfig,
    pub position: LensPositionConfig,
    pub rounding: LensRoundingConfig,
    pub colors: LensColorConfig,
    pub modes: LensModeConfig,
    pub providers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LensPositionConfig {
    pub anchor: String,
    pub offset_x: i32,
    pub offset_y: i32,
}

#[derive(Clone, Debug)]
pub struct LensRoundingConfig {
    pub panel: i32,
    pub dropdown: i32,
    pub search: i32,
    pub row: i32,
    pub badge: i32,
    pub draft: i32,
}

#[derive(Clone, Debug)]
pub struct LensColorConfig {
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
pub struct LensUiConfig {
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
pub struct LensModeConfig {
    pub apps: bool,
    pub clusters: bool,
    pub nodes: bool,
    pub actions: bool,
    pub config: bool,
}

impl Default for LensConfig {
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
            keyboard_interactivity: "exclusive".into(),
            close_on_focus_loss: false,
            close_on_click_away: false,
            alt_number_jump: true,
            ui: LensUiConfig::default(),
            position: LensPositionConfig::default(),
            rounding: LensRoundingConfig::default(),
            colors: LensColorConfig::default(),
            modes: LensModeConfig::default(),
            providers: vec![
                "apps".into(),
                "clusters".into(),
                "nodes".into(),
                "actions".into(),
            ],
        }
    }
}

impl Default for LensPositionConfig {
    fn default() -> Self {
        Self {
            anchor: "center".into(),
            offset_x: 0,
            offset_y: 0,
        }
    }
}

impl Default for LensRoundingConfig {
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

impl Default for LensColorConfig {
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

impl Default for LensUiConfig {
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

impl Default for LensModeConfig {
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

impl LensConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let cfg = RuneConfig::from_file(path)
            .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
        let mut out = Self::default();
        out.placeholder = cfg.get_or("lens.placeholder", out.placeholder.clone());
        out.width = cfg.get_or("lens.width", out.width).clamp(420, 1400);
        out.max_results = cfg
            .get_or::<u32>("lens.max-results", out.max_results as u32)
            .clamp(3, 40) as usize;
        out.visible_results = cfg
            .get_or::<u32>("lens.visible-results", out.visible_results as u32)
            .clamp(3, out.max_results as u32) as usize;
        out.fuzzy = cfg.get_or("lens.fuzzy", out.fuzzy);
        out.show_section_labels = cfg.get_or("lens.show-section-labels", out.show_section_labels);
        out.icons = cfg.get_or("lens.icons", out.icons);
        out.icon_size = cfg
            .get_or::<u32>("lens.icon-size", out.icon_size)
            .clamp(16, 64);
        out.icon_search_depth = cfg
            .get_or::<u32>("lens.icon-search-depth", out.icon_search_depth as u32)
            .clamp(1, 8) as usize;
        out.icon_theme = cfg.get_or("lens.icon-theme", out.icon_theme.clone());
        out.keyboard_interactivity = cfg.get_or(
            "lens.keyboard-interactivity",
            out.keyboard_interactivity.clone(),
        );
        out.close_on_focus_loss = cfg.get_or("lens.close-on-focus-loss", out.close_on_focus_loss);
        out.close_on_click_away = cfg.get_or("lens.close-on-click-away", out.close_on_click_away);
        out.alt_number_jump = cfg.get_or("lens.alt-number-jump", out.alt_number_jump);
        out.position.anchor = cfg.get_or("lens.position.anchor", out.position.anchor.clone());
        out.position.offset_x = cfg
            .get_or("lens.position.offset-x", out.position.offset_x)
            .clamp(-2000, 2000);
        out.position.offset_y = cfg
            .get_or("lens.position.offset-y", out.position.offset_y)
            .clamp(-2000, 2000);
        out.rounding.panel = cfg
            .get_or("lens.rounding.panel", out.rounding.panel)
            .clamp(0, 48);
        out.rounding.dropdown = cfg
            .get_or("lens.rounding.dropdown", out.rounding.dropdown)
            .clamp(0, 48);
        out.rounding.search = cfg
            .get_or("lens.rounding.search", out.rounding.search)
            .clamp(0, 48);
        out.rounding.row = cfg
            .get_or("lens.rounding.row", out.rounding.row)
            .clamp(0, 48);
        out.rounding.badge = cfg
            .get_or("lens.rounding.badge", out.rounding.badge)
            .clamp(0, 48);
        out.rounding.draft = cfg
            .get_or("lens.rounding.draft", out.rounding.draft)
            .clamp(0, 48);
        out.colors.panel = cfg.get_or("lens.colors.panel", out.colors.panel.clone());
        out.colors.panel_border =
            cfg.get_or("lens.colors.panel-border", out.colors.panel_border.clone());
        out.colors.dropdown = cfg.get_or("lens.colors.dropdown", out.colors.dropdown.clone());
        out.colors.dropdown_border = cfg.get_or(
            "lens.colors.dropdown-border",
            out.colors.dropdown_border.clone(),
        );
        out.colors.search = cfg.get_or("lens.colors.search", out.colors.search.clone());
        out.colors.row_selected =
            cfg.get_or("lens.colors.row-selected", out.colors.row_selected.clone());
        out.colors.divider = cfg.get_or("lens.colors.divider", out.colors.divider.clone());
        out.colors.text = cfg.get_or("lens.colors.text", out.colors.text.clone());
        out.colors.subtext = cfg.get_or("lens.colors.subtext", out.colors.subtext.clone());
        out.colors.hint = cfg.get_or("lens.colors.hint", out.colors.hint.clone());
        out.colors.accent = cfg.get_or("lens.colors.accent", out.colors.accent.clone());
        out.colors.badge = cfg.get_or("lens.colors.badge", out.colors.badge.clone());
        out.colors.danger = cfg.get_or("lens.colors.danger", out.colors.danger.clone());
        out.ui.top_margin = cfg
            .get_or("lens.ui.top-margin", out.ui.top_margin)
            .clamp(0, 480);
        out.ui.padding = cfg.get_or("lens.ui.padding", out.ui.padding).clamp(8, 48);
        out.ui.dropdown_gap = cfg
            .get_or("lens.ui.dropdown-gap", out.ui.dropdown_gap)
            .clamp(0, 32);
        out.ui.dropdown_padding = cfg
            .get_or("lens.ui.dropdown-padding", out.ui.dropdown_padding)
            .clamp(0, 32);
        out.ui.search_height = cfg
            .get_or("lens.ui.search-height", out.ui.search_height)
            .clamp(42, 96);
        out.ui.draft_height = cfg
            .get_or("lens.ui.draft-height", out.ui.draft_height)
            .clamp(28, 72);
        out.ui.row_height = cfg
            .get_or("lens.ui.row-height", out.ui.row_height)
            .clamp(46, 110);
        out.ui.row_gap = cfg.get_or("lens.ui.row-gap", out.ui.row_gap).clamp(0, 20);
        out.ui.section_height = cfg
            .get_or("lens.ui.section-height", out.ui.section_height)
            .clamp(0, 40);
        out.ui.footer_height = cfg
            .get_or("lens.ui.footer-height", out.ui.footer_height)
            .clamp(0, 70);
        out.ui.font = cfg.get_or("lens.ui.font", out.ui.font.clone());
        out.ui.search_font_size = cfg
            .get_or::<u32>("lens.ui.search-font-size", out.ui.search_font_size)
            .clamp(14, 42);
        out.ui.badge_font_size = cfg
            .get_or::<u32>("lens.ui.badge-font-size", out.ui.badge_font_size)
            .clamp(10, 28);
        out.ui.title_font_size = cfg
            .get_or::<u32>("lens.ui.title-font-size", out.ui.title_font_size)
            .clamp(12, 34);
        out.ui.subtitle_font_size = cfg
            .get_or::<u32>("lens.ui.subtitle-font-size", out.ui.subtitle_font_size)
            .clamp(10, 26);
        out.ui.hint_font_size = cfg
            .get_or::<u32>("lens.ui.hint-font-size", out.ui.hint_font_size)
            .clamp(10, 24);
        out.modes.apps = cfg.get_or("lens.modes.apps", out.modes.apps);
        out.modes.clusters = cfg.get_or("lens.modes.clusters", out.modes.clusters);
        out.modes.nodes = cfg.get_or("lens.modes.nodes", out.modes.nodes);
        out.modes.actions = cfg.get_or("lens.modes.actions", out.modes.actions);
        out.modes.config = cfg.get_or("lens.modes.config", out.modes.config);
        if let Ok(Some(providers)) = cfg.get_optional::<Vec<String>>("lens.providers") {
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

fn validate(config: &LensConfig) -> Result<(), String> {
    if config.width < 420 {
        return Err("lens.width must be at least 420".into());
    }
    if config.max_results == 0 {
        return Err("lens.max-results must be greater than zero".into());
    }
    if config.visible_results == 0 || config.visible_results > config.max_results {
        return Err("lens.visible-results must be between 1 and lens.max-results".into());
    }
    if !matches!(
        config.keyboard_interactivity.to_ascii_lowercase().as_str(),
        "exclusive" | "on-demand" | "ondemand" | "on_demand"
    ) {
        return Err("lens.keyboard-interactivity must be `exclusive` or `on-demand`".into());
    }
    if !matches!(
        config.position.anchor.to_ascii_lowercase().as_str(),
        "center" | "top" | "top-left" | "top-right" | "bottom" | "bottom-left" | "bottom-right"
    ) {
        return Err("lens.position.anchor must be center, top, top-left, top-right, bottom, bottom-left, or bottom-right".into());
    }
    Ok(())
}

pub fn default_config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Path::new(&xdg).join("halley/lens.rune");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Path::new(&home).join(".config/halley/lens.rune");
    }
    PathBuf::from("lens.rune")
}
