use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::UNIX_EPOCH;

use fontdb::{Database, Family, Query, Stretch, Style, Weight};
use image::{RgbaImage, imageops};
use resvg::{tiny_skia, usvg};
use rusttype::{Font, PositionedGlyph, Scale, point};

use crate::config::LiftConfig;
use crate::mode::{LiftMode, ModeInputState};
use crate::model::{ClusterDraft, LiftResult, LiftResultKind};

#[derive(Clone, Copy)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy)]
pub struct View<'a> {
    pub config: &'a LiftConfig,
    pub input: &'a ModeInputState,
    pub mode: LiftMode,
    pub results: &'a [LiftResult],
    pub selected: usize,
    pub scroll_offset: usize,
    pub draft: &'a ClusterDraft,
    pub status: Option<&'a str>,
    pub cursor_visible: bool,
}

pub fn panel_height(config: &LiftConfig) -> i32 {
    config.ui.search_height.max(1)
}

pub fn surface_height(view: View<'_>) -> i32 {
    let ui = &view.config.ui;
    let mut height = ui.search_height;
    let dropdown = dropdown_visible(view);
    if !dropdown {
        return height.max(1);
    }

    height += ui.dropdown_gap + ui.dropdown_padding * 2;
    if view.mode == LiftMode::Clusters && view.draft.count() > 0 {
        height += ui.draft_height + ui.row_gap;
    }

    let section_budget = if view.config.show_section_labels {
        visible_section_count(view) as i32 * ui.section_height
    } else {
        0
    };
    let mut rows = view
        .results
        .iter()
        .skip(view.scroll_offset)
        .take(view.config.visible_results)
        .count() as i32;
    if rows == 0 {
        rows = 1;
    }
    if rows > 0 {
        height += section_budget + rows * ui.row_height + (rows - 1).max(0) * ui.row_gap;
    }
    height += list_bottom_padding(view.config);
    if ui.footer_height > 0 {
        height += ui.row_gap + ui.footer_height;
    }
    height.clamp(ui.search_height.max(1), 980)
}

fn dropdown_visible(view: View<'_>) -> bool {
    !view.input.query.trim().is_empty()
        || view.input.mode != LiftMode::General
        || (view.mode == LiftMode::Clusters && view.draft.count() > 0)
}

fn list_bottom_padding(config: &LiftConfig) -> i32 {
    config.ui.row_gap.max(8)
}

fn visible_section_count(view: View<'_>) -> usize {
    let mut count = 0;
    let mut last_section = "";
    for result in view
        .results
        .iter()
        .skip(view.scroll_offset)
        .take(view.config.visible_results)
    {
        if result.section != last_section {
            count += 1;
            last_section = result.section.as_str();
        }
    }
    count
}

pub fn panel_rect(config: &LiftConfig, width: u32, height: u32) -> Rect {
    let _ = config;
    Rect {
        x: 0,
        y: 0,
        w: width.max(1) as i32,
        h: height.max(1) as i32,
    }
}

pub fn contains(rect: Rect, sx: f64, sy: f64) -> bool {
    sx >= rect.x as f64
        && sx < (rect.x + rect.w) as f64
        && sy >= rect.y as f64
        && sy < (rect.y + rect.h) as f64
}

pub fn result_index_at(view: View<'_>, width: u32, height: u32, sx: f64, sy: f64) -> Option<usize> {
    let panel = panel_rect(view.config, width, height);
    if !contains(panel, sx, sy) {
        return None;
    }
    let ui = &view.config.ui;
    if !dropdown_visible(view) {
        return None;
    }
    let mut y = panel.y + ui.search_height + ui.dropdown_gap + ui.dropdown_padding;
    if view.mode == LiftMode::Clusters && view.draft.count() > 0 {
        y += ui.draft_height + ui.row_gap;
    }
    let mut last_section = "";
    for (visible_index, result) in view
        .results
        .iter()
        .skip(view.scroll_offset)
        .take(view.config.visible_results)
        .enumerate()
    {
        if view.config.show_section_labels && result.section != last_section {
            y += ui.section_height;
            last_section = result.section.as_str();
        }
        if sy >= y as f64 && sy < (y + ui.row_height) as f64 {
            return Some(view.scroll_offset + visible_index);
        }
        y += ui.row_height + ui.row_gap;
    }
    None
}

pub struct FontRenderer {
    font: Font<'static>,
}

impl FontRenderer {
    pub fn new(family: &str) -> Result<Self, String> {
        let mut db = Database::new();
        db.load_system_fonts();
        let requested = if family.trim().is_empty() {
            "sans-serif"
        } else {
            family.trim()
        };
        let families = if requested.eq_ignore_ascii_case("monospace") {
            vec![Family::Monospace, Family::SansSerif]
        } else if requested.eq_ignore_ascii_case("serif") {
            vec![Family::Serif, Family::SansSerif]
        } else {
            vec![
                Family::Name(requested),
                Family::SansSerif,
                Family::Monospace,
            ]
        };
        let id = db
            .query(&Query {
                families: families.as_slice(),
                weight: Weight::NORMAL,
                stretch: Stretch::Normal,
                style: Style::Normal,
            })
            .ok_or_else(|| format!("unable to resolve font `{family}`"))?;
        let bytes = db
            .with_face_data(id, |data, _| data.to_vec())
            .ok_or_else(|| format!("unable to read font `{family}`"))?;
        let font = Font::try_from_vec(bytes).ok_or_else(|| format!("invalid font `{family}`"))?;
        Ok(Self { font })
    }

    fn measure(&self, text: &str, px: u32) -> (i32, i32) {
        if text.is_empty() {
            return (0, px as i32);
        }
        let scale = Scale::uniform(px as f32);
        let v = self.font.v_metrics(scale);
        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(0.0, v.ascent))
            .collect();
        let Some(bounds) = union_bounds(&glyphs) else {
            return (px as i32, px as i32);
        };
        ((bounds.2 - bounds.0).max(1), (bounds.3 - bounds.1).max(1))
    }

    fn advance_width(&self, text: &str, px: u32) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let scale = Scale::uniform(px as f32);
        let v = self.font.v_metrics(scale);
        let mut width = 0.0f32;
        for glyph in self.font.layout(text, scale, point(0.0, v.ascent)) {
            width = glyph.position().x + glyph.unpositioned().h_metrics().advance_width;
        }
        width.ceil().max(1.0) as i32
    }

    fn fit_text(&self, text: &str, px: u32, max_width: i32) -> String {
        if max_width <= 0 || text.is_empty() {
            return String::new();
        }
        if self.advance_width(text, px) <= max_width {
            return text.to_string();
        }
        let ellipsis = "...";
        let ellipsis_width = self.advance_width(ellipsis, px);
        if ellipsis_width > max_width {
            return String::new();
        }

        let mut fitted = String::new();
        for ch in text.chars() {
            fitted.push(ch);
            let candidate = format!("{}{}", fitted.trim_end(), ellipsis);
            if self.advance_width(candidate.as_str(), px) > max_width {
                fitted.pop();
                break;
            }
        }

        if fitted.is_empty() {
            ellipsis.to_string()
        } else {
            format!("{}{}", fitted.trim_end(), ellipsis)
        }
    }

    fn fit_text_tail(&self, text: &str, px: u32, max_width: i32) -> String {
        if max_width <= 0 || text.is_empty() {
            return String::new();
        }
        if self.advance_width(text, px) <= max_width {
            return text.to_string();
        }
        let ellipsis = "...";
        let ellipsis_width = self.advance_width(ellipsis, px);
        if ellipsis_width > max_width {
            return String::new();
        }

        let mut fitted = String::new();
        for ch in text.chars().rev() {
            fitted.insert(0, ch);
            let candidate = format!("{ellipsis}{fitted}");
            if self.advance_width(candidate.as_str(), px) > max_width {
                fitted.remove(0);
                break;
            }
        }

        if fitted.is_empty() {
            ellipsis.to_string()
        } else {
            format!("{ellipsis}{fitted}")
        }
    }

    fn draw(
        &self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        text: &str,
        px: u32,
        color: Color,
    ) {
        let scale = Scale::uniform(px as f32);
        let v = self.font.v_metrics(scale);
        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(0.0, v.ascent))
            .collect();
        let Some(bounds) = union_bounds(&glyphs) else {
            return;
        };
        for glyph in glyphs {
            let Some(bb) = glyph.pixel_bounding_box() else {
                continue;
            };
            glyph.draw(|gx, gy, coverage| {
                let px = x + bb.min.x - bounds.0 + gx as i32;
                let py = y + bb.min.y - bounds.1 + gy as i32;
                if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                    return;
                }
                let offset = ((py as u32 * width + px as u32) * 4) as usize;
                blend(
                    canvas,
                    offset,
                    Color(color.0, color.1, color.2, color.3 * coverage),
                );
            });
        }
    }

    /// Constant line height (ascent + |descent|) for the font at `px`, independent of the
    /// glyphs in any particular string. Use for vertical centering so text does not jump as
    /// ascenders/descenders (e.g. `y`, `g`) come and go.
    fn v_line_height(&self, px: u32) -> i32 {
        let scale = Scale::uniform(px as f32);
        let v = self.font.v_metrics(scale);
        (v.ascent - v.descent).ceil().max(1.0) as i32
    }

    /// Draw `text` with a stable baseline: `top_y` is the top of the line box (the ascent line),
    /// so vertical glyph positions depend only on the font metrics, never on which characters are
    /// present. The leftmost ink is aligned to `x`. Use for live-edited single-line text.
    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &self,
        canvas: &mut [u8],
        width: u32,
        height: u32,
        x: i32,
        top_y: i32,
        text: &str,
        px: u32,
        color: Color,
    ) {
        let scale = Scale::uniform(px as f32);
        let v = self.font.v_metrics(scale);
        let glyphs: Vec<_> = self
            .font
            .layout(text, scale, point(0.0, v.ascent))
            .collect();
        let x_min = glyphs
            .iter()
            .filter_map(|g| g.pixel_bounding_box())
            .map(|bb| bb.min.x)
            .min()
            .unwrap_or(0);
        for glyph in glyphs {
            let Some(bb) = glyph.pixel_bounding_box() else {
                continue;
            };
            glyph.draw(|gx, gy, coverage| {
                let px = x + bb.min.x - x_min + gx as i32;
                let py = top_y + bb.min.y + gy as i32;
                if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                    return;
                }
                let offset = ((py as u32 * width + px as u32) * 4) as usize;
                blend(
                    canvas,
                    offset,
                    Color(color.0, color.1, color.2, color.3 * coverage),
                );
            });
        }
    }
}

#[derive(Default)]
pub struct IconCache {
    entries: HashMap<String, IconSlot>,
    index: HashMap<String, IconIndexEntry>,
    roots: Vec<PathBuf>,
    target_size: u32,
    search_depth: usize,
    indexed: bool,
    loaded_disk_index: bool,
    index_refresh_requested: bool,
    index_rx: Option<Receiver<HashMap<String, IconIndexEntry>>>,
    decode_tx: Option<Sender<(String, PathBuf)>>,
    decode_rx: Option<Receiver<(String, Option<IconRaster>)>>,
    pending_decodes: usize,
    cache_path: Option<PathBuf>,
    cache_fingerprint: String,
    /// Lazily rasterized search-bar glyphs, keyed by the size they were rendered at.
    search_icon: Option<(u32, IconRaster)>,
    app_search_icon: Option<(u32, IconRaster)>,
    cluster_search_icon: Option<(u32, IconRaster)>,
    action_search_icon: Option<(u32, IconRaster)>,
    action_icon: Option<(u32, IconRaster)>,
    term_icon: Option<(u32, IconRaster)>,
}

/// Search-bar glyphs. Authored as square SVGs and rendered to alpha masks that are
/// tinted at draw time.
const SEARCH_ICON_SVG: &[u8] = include_bytes!("../assets/loupe.svg");
const APP_SEARCH_ICON_SVG: &[u8] = include_bytes!("../assets/apps.svg");
const ACTION_ICON_SVG: &[u8] = include_bytes!("../assets/spark.svg");
const TERM_ICON_SVG: &[u8] = include_bytes!("../assets/term.svg");
const CLUSTER_SEARCH_ICON_SVG: &[u8] =
    include_bytes!("../../halley-wl/src/compositor/clusters/assets/clusters.svg");

/// State of a single icon in the in-memory cache. Decoding happens on a worker thread,
/// so a freshly requested icon is `Pending` until its raster arrives.
enum IconSlot {
    Pending,
    Ready(IconRaster),
    Missing,
}

struct IconRaster {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Clone)]
struct IconIndexEntry {
    score: i32,
    path: PathBuf,
}

impl IconCache {
    pub fn new(config: &LiftConfig) -> Self {
        let roots = if config.icons {
            icon_roots(config)
        } else {
            Vec::new()
        };
        let target_size = config.icon_size.max(1);
        let search_depth = config.icon_search_depth;
        let cache_path = icon_index_cache_path();
        let cache_fingerprint = index_fingerprint(
            roots.as_slice(),
            config.icon_theme.as_str(),
            target_size,
            search_depth,
        );
        // A valid on-disk index lets us skip the directory walk entirely and treat the
        // cache as ready before the first draw — no cold-start window.
        let cached_index = config
            .icons
            .then(|| {
                cache_path
                    .as_ref()
                    .and_then(|path| load_index_cache(path, cache_fingerprint.as_str()))
            })
            .flatten();
        let loaded_disk_index = cached_index.is_some();
        let indexed = !config.icons || loaded_disk_index;
        Self {
            entries: HashMap::new(),
            index: cached_index.unwrap_or_default(),
            roots,
            target_size,
            search_depth,
            indexed,
            loaded_disk_index,
            index_refresh_requested: false,
            index_rx: None,
            decode_tx: None,
            decode_rx: None,
            pending_decodes: 0,
            cache_path,
            cache_fingerprint,
            search_icon: None,
            app_search_icon: None,
            cluster_search_icon: None,
            action_search_icon: None,
            action_icon: None,
            term_icon: None,
        }
    }

    /// Returns the search-bar glyph rasterized at `size`, rendering (and caching) it on
    /// first use or whenever the requested size changes.
    fn search_glyph(&mut self, size: u32, mode: LiftMode) -> Option<&IconRaster> {
        let size = size.max(1);
        let (slot, svg) = match mode {
            LiftMode::Apps => (&mut self.app_search_icon, APP_SEARCH_ICON_SVG),
            LiftMode::Clusters => (&mut self.cluster_search_icon, CLUSTER_SEARCH_ICON_SVG),
            LiftMode::Actions => (&mut self.action_search_icon, ACTION_ICON_SVG),
            LiftMode::Term => (&mut self.term_icon, TERM_ICON_SVG),
            _ => (&mut self.search_icon, SEARCH_ICON_SVG),
        };
        if slot.as_ref().map(|(s, _)| *s) != Some(size) {
            let raster = render_svg_data(svg, None, size)?;
            *slot = Some((size, raster));
        }
        slot.as_ref().map(|(_, raster)| raster)
    }

    fn term_glyph(&mut self, size: u32) -> Option<&IconRaster> {
        let size = size.max(1);
        if self.term_icon.as_ref().map(|(s, _)| *s) != Some(size) {
            let raster = render_svg_data(TERM_ICON_SVG, None, size)?;
            self.term_icon = Some((size, raster));
        }
        self.term_icon.as_ref().map(|(_, raster)| raster)
    }

    fn action_glyph(&mut self, size: u32) -> Option<&IconRaster> {
        let size = size.max(1);
        if self.action_icon.as_ref().map(|(s, _)| *s) != Some(size) {
            let raster = render_svg_data(ACTION_ICON_SVG, None, size)?;
            self.action_icon = Some((size, raster));
        }
        self.action_icon.as_ref().map(|(_, raster)| raster)
    }

    pub fn needs_index(&self) -> bool {
        (!self.indexed || self.index_refresh_requested) && self.index_rx.is_none()
    }

    pub fn has_pending_index(&self) -> bool {
        self.index_rx.is_some()
    }

    pub fn has_pending_decodes(&self) -> bool {
        self.pending_decodes > 0
    }

    pub fn start_index(&mut self) {
        if !self.needs_index() {
            return;
        }
        self.index_refresh_requested = false;
        let roots = self.roots.clone();
        let search_depth = self.search_depth;
        let target_size = self.target_size;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(build_icon_index(
                roots.as_slice(),
                search_depth,
                target_size,
            ));
        });
        self.index_rx = Some(rx);
    }

    pub fn finish_index_if_ready(&mut self) -> Option<usize> {
        let rx = self.index_rx.as_ref()?;
        let Ok(index) = rx.try_recv() else {
            return None;
        };
        self.index = index;
        self.indexed = true;
        self.loaded_disk_index = false;
        self.index_refresh_requested = false;
        self.index_rx = None;
        // Drop entries that resolved to nothing while the index was still building so
        // they get another chance now that path resolution can succeed.
        self.entries
            .retain(|_, slot| matches!(slot, IconSlot::Ready(_)));
        if let Some(path) = self.cache_path.as_ref() {
            write_index_cache(path, self.cache_fingerprint.as_str(), &self.index);
        }
        Some(self.index.len())
    }

    /// Drain any icons the worker thread finished decoding into the cache. Returns true
    /// if a redraw is warranted because a previously pending icon became available.
    pub fn poll_decodes(&mut self) -> bool {
        let Some(rx) = self.decode_rx.as_ref() else {
            return false;
        };
        let mut changed = false;
        let mut refresh_needed = false;
        while let Ok((key, raster)) = rx.try_recv() {
            let missing = raster.is_none();
            let slot = raster.map_or(IconSlot::Missing, IconSlot::Ready);
            self.entries.insert(key, slot);
            self.pending_decodes = self.pending_decodes.saturating_sub(1);
            if missing {
                refresh_needed = true;
            }
            changed = true;
        }
        if refresh_needed {
            self.request_index_refresh();
        }
        changed
    }

    fn load(&mut self, name: &str) -> Option<&IconRaster> {
        let key = name.trim().to_string();
        if key.is_empty() {
            return None;
        }
        if !self.entries.contains_key(&key) {
            match self.resolve_icon_path(&key) {
                Some(path) => {
                    self.entries.insert(key.clone(), IconSlot::Pending);
                    self.ensure_decode_worker();
                    if let Some(tx) = self.decode_tx.as_ref()
                        && tx.send((key.clone(), path)).is_ok()
                    {
                        self.pending_decodes += 1;
                    }
                }
                // Before the index is ready, resolution always fails; leave the entry
                // unrecorded so it retries once the index lands. Once indexed, an
                // unresolved name is genuinely missing and is cached as such.
                None if self.indexed => {
                    self.entries.insert(key.clone(), IconSlot::Missing);
                    self.request_index_refresh();
                }
                None => {}
            }
        }
        match self.entries.get(&key) {
            Some(IconSlot::Ready(raster)) => Some(raster),
            _ => None,
        }
    }

    /// Spawn the decode worker on first use. It outlives individual requests and exits
    /// when the cache (and thus the job sender) is dropped.
    fn ensure_decode_worker(&mut self) {
        if self.decode_tx.is_some() {
            return;
        }
        let (job_tx, job_rx) = mpsc::channel::<(String, PathBuf)>();
        let (res_tx, res_rx) = mpsc::channel::<(String, Option<IconRaster>)>();
        let target_size = self.target_size;
        thread::spawn(move || {
            while let Ok((key, path)) = job_rx.recv() {
                let raster = load_icon(path.as_path(), target_size);
                if res_tx.send((key, raster)).is_err() {
                    break;
                }
            }
        });
        self.decode_tx = Some(job_tx);
        self.decode_rx = Some(res_rx);
    }

    fn request_index_refresh(&mut self) {
        if self.loaded_disk_index && self.index_rx.is_none() {
            self.index_refresh_requested = true;
        }
    }

    fn resolve_icon_path(&self, name: &str) -> Option<PathBuf> {
        let path = Path::new(name);
        if path.is_absolute() && path.is_file() {
            return Some(path.to_path_buf());
        }
        // Resolution relies entirely on the pre-built index. Until it is ready we return
        // None rather than walking the icon tree per-request, which would stall the UI.
        if !self.indexed {
            return None;
        }
        let mut candidates = Vec::new();
        push_icon_candidate(&mut candidates, name.to_string());
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            push_icon_candidate(&mut candidates, stem.to_string());
        }
        candidates.iter().find_map(|candidate| {
            self.index
                .get(&candidate.to_ascii_lowercase())
                .map(|entry| entry.path.clone())
        })
    }
}

fn union_bounds(glyphs: &[PositionedGlyph<'_>]) -> Option<(i32, i32, i32, i32)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for glyph in glyphs {
        let Some(bb) = glyph.pixel_bounding_box() else {
            continue;
        };
        min_x = min_x.min(bb.min.x);
        min_y = min_y.min(bb.min.y);
        max_x = max_x.max(bb.max.x);
        max_y = max_y.max(bb.max.y);
    }
    (min_x != i32::MAX).then_some((min_x, min_y, max_x, max_y))
}

pub fn draw_palette(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    icon_cache: &mut IconCache,
    view: View<'_>,
) {
    canvas.fill(0);
    let ui = &view.config.ui;
    let panel = panel_rect(view.config, width, height);
    let colors = Palette::from_config(view.config);
    let expanded = dropdown_visible(view);

    draw_chrome(canvas, width, height, view.config, &colors, panel, expanded);

    let search_icon = if view.config.search_icon.enabled {
        icon_cache.search_glyph(view.config.search_icon.size.max(1) as u32, view.mode)
    } else {
        None
    };
    draw_search_box(
        canvas,
        width,
        height,
        font,
        view.input,
        view.config,
        panel,
        panel.y,
        view.cursor_visible,
        search_icon,
    );

    if !expanded {
        return;
    }

    let dropdown_y = panel.y + ui.search_height + ui.dropdown_gap;
    let mut y = dropdown_y + ui.dropdown_padding;

    if view.mode == LiftMode::Clusters && view.draft.count() > 0 {
        draw_draft_summary(canvas, width, height, font, view, panel, y);
        y += ui.draft_height + ui.row_gap;
    }

    draw_results(canvas, width, height, font, icon_cache, view, panel, y);
    draw_footer(canvas, width, height, font, view, panel);
}

struct Palette {
    dropdown: Color,
    dropdown_border: Color,
    panel_border: Color,
    search: Color,
    row_selected: Color,
    divider: Color,
    text: Color,
    subtext: Color,
    hint: Color,
    accent: Color,
    danger: Color,
}

impl Palette {
    fn from_config(config: &LiftConfig) -> Self {
        Self {
            dropdown: parse_color(&config.colors.dropdown, Color(0.08, 0.09, 0.13, 0.94)),
            dropdown_border: parse_color(
                &config.colors.dropdown_border,
                Color(0.17, 0.20, 0.28, 0.80),
            ),
            panel_border: parse_color(&config.colors.panel_border, Color(0.17, 0.20, 0.28, 0.80)),
            search: parse_color(&config.colors.search, Color(0.02, 0.025, 0.04, 0.80)),
            row_selected: parse_color(&config.colors.row_selected, Color(0.18, 0.27, 0.46, 0.92)),
            divider: parse_color(&config.colors.divider, Color(0.17, 0.20, 0.28, 0.60)),
            text: parse_color(&config.colors.text, Color(0.94, 0.96, 1.0, 1.0)),
            subtext: parse_color(&config.colors.subtext, Color(0.62, 0.66, 0.76, 0.95)),
            hint: parse_color(&config.colors.hint, Color(0.52, 0.58, 0.70, 0.9)),
            accent: parse_color(&config.colors.accent, Color(0.62, 0.74, 1.0, 1.0)),
            danger: parse_color(&config.colors.danger, Color(0.92, 0.62, 0.56, 0.95)),
        }
    }
}

/// Draws the launcher backgrounds and border. The search-bar text/caret and the
/// results content are drawn separately on top of this.
fn draw_chrome(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    config: &LiftConfig,
    colors: &Palette,
    panel: Rect,
    expanded: bool,
) {
    let ui = &config.ui;
    let search_h = ui.search_height;
    let dropdown_y = panel.y + search_h + ui.dropdown_gap;
    let dropdown_h = panel.h - search_h - ui.dropdown_gap;
    let divider_y = panel.y + search_h;

    let bw = if config.border.enabled {
        config.border.width.max(0)
    } else {
        0
    };
    // "outline" wraps the whole app; any other style keeps the legacy
    // dropdown-only ("inset") border.
    let outline = config.border.enabled && config.border.style != "inset";

    if outline {
        // One border ring around the whole app, interior inset by the border width.
        // The top corners always use the search-bar radius and the bottom corners the
        // panel radius, so the top is continuous whether or not results are showing.
        let top_r = config.rounding.search;
        let bottom_r = if expanded {
            config.rounding.panel
        } else {
            config.rounding.search
        };
        let app_h = if expanded { panel.h } else { search_h };
        fill_rounded_rect_radii(
            canvas,
            width,
            height,
            panel.x,
            panel.y,
            panel.w,
            app_h,
            (top_r, top_r, bottom_r, bottom_r),
            colors.panel_border,
            false,
        );
        let ix = panel.x + bw;
        let iw = (panel.w - 2 * bw).max(0);
        let top_in = (top_r - bw).max(0);
        let bottom_in = (bottom_r - bw).max(0);
        if expanded {
            // Search fill: top corners rounded, bottom flush to the divider.
            fill_rounded_rect_radii(
                canvas,
                width,
                height,
                ix,
                panel.y + bw,
                iw,
                (search_h - bw).max(0),
                (top_in, top_in, 0, 0),
                colors.search,
                true,
            );
            // Dropdown fill: bottom corners rounded.
            fill_rounded_rect_radii(
                canvas,
                width,
                height,
                ix,
                dropdown_y,
                iw,
                (dropdown_h - bw).max(0),
                (0, 0, bottom_in, bottom_in),
                colors.dropdown,
                true,
            );
            fill_rect(canvas, width, height, ix, divider_y, iw, 1, colors.divider);
        } else {
            fill_rounded_rect_radii(
                canvas,
                width,
                height,
                ix,
                panel.y + bw,
                iw,
                (search_h - 2 * bw).max(0),
                (top_in, top_in, bottom_in, bottom_in),
                colors.search,
                true,
            );
        }
        return;
    }

    // Legacy / "inset" look: search bar has no border; the dropdown is bordered.
    let search_corners = if expanded {
        (true, true, false, false)
    } else {
        (true, true, true, true)
    };
    fill_rounded_rect_corners(
        canvas,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        search_h,
        config.rounding.search,
        search_corners,
        colors.search,
    );
    if !expanded {
        return;
    }
    fill_rect(
        canvas,
        width,
        height,
        panel.x,
        divider_y,
        panel.w,
        1,
        colors.divider,
    );
    if bw > 0 {
        fill_rounded_rect_corners(
            canvas,
            width,
            height,
            panel.x,
            dropdown_y,
            panel.w,
            dropdown_h,
            config.rounding.dropdown,
            (false, false, true, true),
            colors.dropdown_border,
        );
    }
    fill_rounded_rect_corners(
        canvas,
        width,
        height,
        panel.x + bw,
        dropdown_y + bw,
        (panel.w - 2 * bw).max(0),
        (dropdown_h - 2 * bw).max(0),
        (config.rounding.dropdown - bw).max(0),
        (false, false, true, true),
        colors.dropdown,
    );
}

fn draw_search_box(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    input: &ModeInputState,
    config: &LiftConfig,
    panel: Rect,
    y: i32,
    cursor_visible: bool,
    search_icon: Option<&IconRaster>,
) {
    let ui = &config.ui;
    let colors = Palette::from_config(config);
    let pad = ui.padding;
    let mut text_x = panel.x + pad;
    let mut text_right = panel.x + panel.w - pad;
    // Magnifier glyph, tinted and placed on the configured side, with the text column
    // shrunk to make room for it.
    if let Some(raster) = search_icon {
        let isize = config.search_icon.size.clamp(1, ui.search_height);
        let gap = 8;
        let iy = y + (ui.search_height - isize) / 2;
        let tint = parse_color(&config.colors.search_icon, colors.hint);
        if config.search_icon.side.eq_ignore_ascii_case("right") {
            let ix = panel.x + panel.w - pad - isize;
            draw_raster_tinted(canvas, width, height, raster, ix, iy, isize, isize, tint);
            text_right = ix - gap;
        } else {
            let ix = panel.x + pad;
            draw_raster_tinted(canvas, width, height, raster, ix, iy, isize, isize, tint);
            text_x = ix + isize + gap;
        }
    }
    let _ = input.mode;
    let query_empty = input.query.is_empty();
    let caret_w = config.cursor.width.max(1);
    let placeholder_caret_gap = 4;
    let text_caret_gap = 0;
    let display_x = if query_empty && config.cursor.enabled {
        text_x + caret_w + placeholder_caret_gap
    } else {
        text_x
    };
    let text_width = (text_right - display_x).max(0);
    let display_text = if query_empty {
        font.fit_text(config.placeholder.as_str(), ui.search_font_size, text_width)
    } else {
        font.fit_text_tail(input.query.as_str(), ui.search_font_size, text_width)
    };
    let color = if query_empty {
        colors.hint
    } else {
        colors.text
    };
    let line_h = font.v_line_height(ui.search_font_size);
    let top_y = y + (ui.search_height - line_h) / 2;
    font.draw_line(
        canvas,
        width,
        height,
        display_x,
        top_y,
        display_text.as_str(),
        ui.search_font_size,
        color,
    );
    if config.cursor.enabled && cursor_visible {
        let text_w = if query_empty {
            0
        } else {
            font.advance_width(display_text.as_str(), ui.search_font_size)
        };
        let max_caret_h = (ui.search_height - 8).max(12);
        let caret_h = (line_h + 4).min(max_caret_h).max(12);
        let caret_x = if query_empty {
            text_x
        } else {
            (text_x + text_w + text_caret_gap)
                .min(text_right - caret_w)
                .max(text_x)
        };
        let caret_y = y + (ui.search_height - caret_h) / 2;
        fill_rect(
            canvas,
            width,
            height,
            caret_x,
            caret_y,
            caret_w,
            caret_h,
            colors.accent,
        );
    }
}

fn draw_draft_summary(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    view: View<'_>,
    panel: Rect,
    y: i32,
) {
    let ui = &view.config.ui;
    let colors = Palette::from_config(view.config);
    let pad = ui.padding;
    let name = view.input.query.trim();
    let name = if name.is_empty() { "untitled" } else { name };
    let summary = format!("Cluster Draft: {name} · {} selected", view.draft.count());
    fill_rounded_rect(
        canvas,
        width,
        height,
        panel.x + pad,
        y,
        panel.w - pad * 2,
        ui.draft_height,
        view.config.rounding.draft,
        colors.search,
    );
    let (_, th) = font.measure(&summary, ui.subtitle_font_size);
    font.draw(
        canvas,
        width,
        height,
        panel.x + pad + 14,
        y + (ui.draft_height - th) / 2,
        &summary,
        ui.subtitle_font_size,
        colors.text,
    );
}

fn draw_results(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    icon_cache: &mut IconCache,
    view: View<'_>,
    panel: Rect,
    mut y: i32,
) {
    let ui = &view.config.ui;
    let colors = Palette::from_config(view.config);
    let mut last_section = "";
    for (visible_index, result) in view
        .results
        .iter()
        .skip(view.scroll_offset)
        .take(view.config.visible_results)
        .enumerate()
    {
        if view.config.show_section_labels && result.section != last_section {
            if ui.section_height > 0 {
                font.draw(
                    canvas,
                    width,
                    height,
                    panel.x + ui.padding + 6,
                    y + 3,
                    result.section.as_str(),
                    ui.hint_font_size,
                    colors.hint,
                );
                y += ui.section_height;
            }
            last_section = result.section.as_str();
        }
        let index = view.scroll_offset + visible_index;
        draw_result_row(
            canvas, width, height, font, icon_cache, view, panel, result, index, y,
        );
        y += ui.row_height + ui.row_gap;
        if y > panel.y + panel.h - ui.footer_height - ui.padding {
            break;
        }
    }

    if view.results.is_empty() {
        font.draw(
            canvas,
            width,
            height,
            panel.x + ui.padding + 10,
            y + 14,
            "No results",
            ui.title_font_size,
            colors.subtext,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_result_row(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    icon_cache: &mut IconCache,
    view: View<'_>,
    panel: Rect,
    result: &LiftResult,
    index: usize,
    y: i32,
) {
    let ui = &view.config.ui;
    let colors = Palette::from_config(view.config);
    let pad = ui.padding;
    let selected = index == view.selected;
    if selected {
        fill_rounded_rect(
            canvas,
            width,
            height,
            panel.x + pad,
            y,
            panel.w - pad * 2,
            ui.row_height,
            view.config.rounding.row,
            colors.row_selected,
        );
    }

    if view.mode == LiftMode::Clusters && view.draft.contains_result(result) {
        let (_, check_h) = font.measure("✓", ui.hint_font_size + 3);
        font.draw(
            canvas,
            width,
            height,
            panel.x + pad + 8,
            y + (ui.row_height - check_h) / 2,
            "✓",
            ui.hint_font_size + 3,
            colors.accent,
        );
    }

    let icon_size = view.config.icon_size as i32;
    let icon_x = panel.x + pad + 28;
    let icon_y = y + (ui.row_height - icon_size) / 2;
    if view.config.icons {
        draw_result_icon(
            canvas,
            width,
            height,
            icon_cache,
            view.config,
            result,
            icon_x,
            icon_y,
        );
    }

    let visible_index = index.saturating_sub(view.scroll_offset);
    let alt_hint = (view.config.alt_number_jump && visible_index < 10).then(|| {
        if visible_index == 9 {
            "Alt+0".to_string()
        } else {
            format!("Alt+{}", visible_index + 1)
        }
    });
    let hint = alt_hint.as_deref().or(result.shortcut_hint.as_deref());
    let hint_metrics = hint.map(|hint| (hint, font.measure(hint, ui.hint_font_size)));
    let hint_x = hint_metrics
        .map(|(_, (hint_w, _))| panel.x + panel.w - pad - 14 - hint_w)
        .unwrap_or(panel.x + panel.w - pad - 14);

    let text_x = if view.config.icons {
        icon_x + icon_size + 16
    } else {
        icon_x
    };
    let text_right = hint_metrics
        .map(|_| hint_x - 18)
        .unwrap_or(panel.x + panel.w - pad - 14);
    let text_width = (text_right - text_x).max(0);
    let title = font.fit_text(result.title.as_str(), ui.title_font_size, text_width);
    let subtitle = result
        .subtitle
        .as_ref()
        .map(|subtitle| font.fit_text(subtitle.as_str(), ui.subtitle_font_size, text_width));
    let (_, title_h) = font.measure(title.as_str(), ui.title_font_size);
    let subtitle_h = subtitle
        .as_deref()
        .filter(|subtitle| !subtitle.is_empty())
        .map(|subtitle| font.measure(subtitle, ui.subtitle_font_size).1)
        .unwrap_or(0);
    let text_block_h = if subtitle_h > 0 {
        title_h + 7 + subtitle_h
    } else {
        title_h
    };
    let title_y = y + (ui.row_height - text_block_h) / 2;
    if !title.is_empty() {
        font.draw(
            canvas,
            width,
            height,
            text_x,
            title_y,
            title.as_str(),
            ui.title_font_size,
            colors.text,
        );
    }
    if let Some(subtitle) = subtitle.as_deref().filter(|subtitle| !subtitle.is_empty()) {
        font.draw(
            canvas,
            width,
            height,
            text_x,
            title_y + title_h + 7,
            subtitle,
            ui.subtitle_font_size,
            colors.subtext,
        );
    }
    if let Some((hint, (_, hint_h))) = hint_metrics {
        font.draw(
            canvas,
            width,
            height,
            hint_x,
            y + (ui.row_height - hint_h) / 2,
            hint,
            ui.hint_font_size,
            colors.hint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_result_icon(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    icon_cache: &mut IconCache,
    config: &LiftConfig,
    result: &LiftResult,
    x: i32,
    y: i32,
) {
    if !config.icons {
        return;
    }
    let size = config.icon_size as i32;
    // Prefer a resolvable raster icon (apps); fall back to a drawn shape for kinds that
    // never carry one so the slot isn't blank.
    if let Some(name) = result.icon_name.as_deref()
        && let Some(raster) = icon_cache.load(name)
    {
        draw_raster(canvas, width, height, raster, x, y, size, size);
        return;
    }
    let colors = Palette::from_config(config);
    let radius = size / 3;
    match result.kind {
        LiftResultKind::Node => {
            fill_rounded_rect(
                canvas,
                width,
                height,
                x,
                y,
                size,
                size,
                radius,
                colors.accent,
            );
        }
        LiftResultKind::App => {
            if let Some(raster) = icon_cache.search_glyph(config.icon_size, LiftMode::Apps) {
                draw_raster_tinted(
                    canvas,
                    width,
                    height,
                    raster,
                    x,
                    y,
                    size,
                    size,
                    colors.accent,
                );
            } else {
                fill_rounded_rect(
                    canvas,
                    width,
                    height,
                    x,
                    y,
                    size,
                    size,
                    radius,
                    colors.accent,
                );
            }
        }
        LiftResultKind::Term => {
            if let Some(raster) = icon_cache.term_glyph(config.icon_size) {
                draw_raster_tinted(
                    canvas,
                    width,
                    height,
                    raster,
                    x,
                    y,
                    size,
                    size,
                    colors.accent,
                );
            }
        }
        LiftResultKind::Action => {
            if let Some(raster) = icon_cache.action_glyph(config.icon_size) {
                draw_raster_tinted(
                    canvas,
                    width,
                    height,
                    raster,
                    x,
                    y,
                    size,
                    size,
                    colors.accent,
                );
            }
        }
        _ => {}
    }
}

fn draw_footer(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    font: &FontRenderer,
    view: View<'_>,
    panel: Rect,
) {
    let ui = &view.config.ui;
    if ui.footer_height <= 0 {
        return;
    }
    let colors = Palette::from_config(view.config);
    let y = panel.y + panel.h - ui.padding - ui.footer_height;
    if let Some(status) = view.status {
        font.draw(
            canvas,
            width,
            height,
            panel.x + ui.padding + 4,
            y - ui.hint_font_size as i32 - 6,
            status,
            ui.hint_font_size,
            colors.danger,
        );
    }
    let create_hint = if view.draft.count() == 0 {
        "Ctrl+Enter Create"
    } else {
        "Ctrl+Enter Finalize draft"
    };
    let footer = format!("Enter Open    Space Select    Tab Actions    {create_hint}    Esc Close");
    let (_, footer_h) = font.measure(&footer, ui.hint_font_size);
    font.draw(
        canvas,
        width,
        height,
        panel.x + ui.padding + 4,
        y + (ui.footer_height - footer_h) / 2,
        footer.as_str(),
        ui.hint_font_size,
        colors.hint,
    );
}

fn fill_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color,
) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x + w).clamp(0, width as i32) as u32;
    let y1 = (y + h).clamp(0, height as i32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            blend(canvas, ((py * width + px) * 4) as usize, color);
        }
    }
}

fn fill_rect_replace(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color,
) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x + w).clamp(0, width as i32) as u32;
    let y1 = (y + h).clamp(0, height as i32) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            set_pixel(canvas, ((py * width + px) * 4) as usize, color);
        }
    }
}

fn fill_rounded_rect(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: Color,
) {
    fill_rounded_rect_corners(
        canvas,
        width,
        height,
        x,
        y,
        w,
        h,
        radius,
        (true, true, true, true),
        color,
    );
}

/// Rounded-rect fill with an independent radius per corner: `(top_left, top_right,
/// bottom_right, bottom_left)`. Lets the outline keep the search-bar radius on top while
/// using the panel radius on the bottom, so collapsed→expanded transitions are seamless.
#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_radii(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radii: (i32, i32, i32, i32),
    color: Color,
    replace_covered: bool,
) {
    let cap = (w / 2).min(h / 2).max(0);
    let (tl, tr, br, bl) = radii;
    let (tl, tr, br, bl) = (
        tl.clamp(0, cap),
        tr.clamp(0, cap),
        br.clamp(0, cap),
        bl.clamp(0, cap),
    );
    if tl == 0 && tr == 0 && br == 0 && bl == 0 {
        if replace_covered {
            fill_rect_replace(canvas, width, height, x, y, w, h, color);
        } else {
            fill_rect(canvas, width, height, x, y, w, h, color);
        }
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(width as i32);
    let y1 = (y + h).min(height as i32);
    let midx = x as f32 + w as f32 / 2.0;
    let midy = y as f32 + h as f32 / 2.0;
    for py in y0..y1 {
        for px in x0..x1 {
            let cx = px as f32 + 0.5;
            let cy = py as f32 + 0.5;
            let left = cx < midx;
            let top = cy < midy;
            // Pick the nearest corner's radius and arc centre.
            let (r, ax, ay, in_corner) = if left && top {
                (
                    tl,
                    x + tl,
                    y + tl,
                    cx < (x + tl) as f32 && cy < (y + tl) as f32,
                )
            } else if !left && top {
                (
                    tr,
                    x + w - tr,
                    y + tr,
                    cx > (x + w - tr) as f32 && cy < (y + tr) as f32,
                )
            } else if !left && !top {
                (
                    br,
                    x + w - br,
                    y + h - br,
                    cx > (x + w - br) as f32 && cy > (y + h - br) as f32,
                )
            } else {
                (
                    bl,
                    x + bl,
                    y + h - bl,
                    cx < (x + bl) as f32 && cy > (y + h - bl) as f32,
                )
            };
            let coverage = if r <= 0 || !in_corner {
                1.0
            } else {
                let dist = ((cx - ax as f32).powi(2) + (cy - ay as f32).powi(2)).sqrt();
                (r as f32 + 0.5 - dist).clamp(0.0, 1.0)
            };
            if coverage <= 0.0 {
                continue;
            }
            let offset = ((py as u32 * width + px as u32) * 4) as usize;
            if replace_covered && coverage >= 1.0 {
                set_pixel(canvas, offset, color);
            } else {
                blend_coverage(canvas, offset, color, coverage);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect_corners(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    corners: (bool, bool, bool, bool),
    color: Color,
) {
    if radius <= 0 {
        fill_rect(canvas, width, height, x, y, w, h, color);
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(width as i32);
    let y1 = (y + h).min(height as i32);
    let r = radius.min(w / 2).min(h / 2).max(0) as f32;
    // Arc centres for each rounded corner (in pixel space).
    let left_c = x as f32 + r;
    let right_c = (x + w) as f32 - r;
    let top_c = y as f32 + r;
    let bottom_c = (y + h) as f32 - r;
    let (top_left, top_right, bottom_right, bottom_left) = corners;
    for py in y0..y1 {
        for px in x0..x1 {
            // Sample at the pixel centre for sub-pixel coverage.
            let cx = px as f32 + 0.5;
            let cy = py as f32 + 0.5;
            let left = cx < left_c;
            let right = cx > right_c;
            let top = cy < top_c;
            let bottom = cy > bottom_c;
            let (arc, rounded) = if left && top {
                ((left_c, top_c), top_left)
            } else if right && top {
                ((right_c, top_c), top_right)
            } else if right && bottom {
                ((right_c, bottom_c), bottom_right)
            } else if left && bottom {
                ((left_c, bottom_c), bottom_left)
            } else {
                ((0.0, 0.0), false)
            };
            // Straight interior, or a square (non-rounded) corner: full coverage.
            let coverage = if !rounded {
                1.0
            } else {
                let dist = ((cx - arc.0).powi(2) + (cy - arc.1).powi(2)).sqrt();
                (r + 0.5 - dist).clamp(0.0, 1.0)
            };
            if coverage <= 0.0 {
                continue;
            }
            blend_coverage(
                canvas,
                ((py as u32 * width + px as u32) * 4) as usize,
                color,
                coverage,
            );
        }
    }
}

fn draw_raster(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    raster: &IconRaster,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    for dy in 0..h.max(1) {
        for dx in 0..w.max(1) {
            let px = x + dx;
            let py = y + dy;
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                continue;
            }
            let sx = (dx as u32 * raster.width / w as u32).min(raster.width.saturating_sub(1));
            let sy = (dy as u32 * raster.height / h as u32).min(raster.height.saturating_sub(1));
            let src = ((sy * raster.width + sx) * 4) as usize;
            let offset = ((py as u32 * width + px as u32) * 4) as usize;
            blend(
                canvas,
                offset,
                Color(
                    raster.rgba[src] as f32 / 255.0,
                    raster.rgba[src + 1] as f32 / 255.0,
                    raster.rgba[src + 2] as f32 / 255.0,
                    raster.rgba[src + 3] as f32 / 255.0,
                ),
            );
        }
    }
}

/// Blits `raster` using only its alpha channel as coverage, painting every covered pixel
/// with `tint`. Lets a monochrome glyph take a themed color.
fn draw_raster_tinted(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    raster: &IconRaster,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    tint: Color,
) {
    for dy in 0..h.max(1) {
        for dx in 0..w.max(1) {
            let px = x + dx;
            let py = y + dy;
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                continue;
            }
            let sx = (dx as u32 * raster.width / w as u32).min(raster.width.saturating_sub(1));
            let sy = (dy as u32 * raster.height / h as u32).min(raster.height.saturating_sub(1));
            let src = ((sy * raster.width + sx) * 4) as usize;
            let coverage = raster.rgba[src + 3] as f32 / 255.0;
            if coverage <= 0.0 {
                continue;
            }
            let offset = ((py as u32 * width + px as u32) * 4) as usize;
            blend_coverage(canvas, offset, tint, coverage);
        }
    }
}

/// Source-over blend with the source alpha scaled by `coverage` (0..1) for anti-aliased edges.
fn blend_coverage(canvas: &mut [u8], offset: usize, color: Color, coverage: f32) {
    let Color(r, g, b, a) = color;
    blend(canvas, offset, Color(r, g, b, a * coverage.clamp(0.0, 1.0)));
}

fn set_pixel(canvas: &mut [u8], offset: usize, color: Color) {
    let Color(r, g, b, a) = color;
    if a <= 0.0 || offset + 3 >= canvas.len() {
        return;
    }
    let a = a.clamp(0.0, 1.0);
    canvas[offset] = (b.clamp(0.0, 1.0) * a * 255.0).round() as u8;
    canvas[offset + 1] = (g.clamp(0.0, 1.0) * a * 255.0).round() as u8;
    canvas[offset + 2] = (r.clamp(0.0, 1.0) * a * 255.0).round() as u8;
    canvas[offset + 3] = (a * 255.0).round() as u8;
}

fn blend(canvas: &mut [u8], offset: usize, color: Color) {
    let Color(r, g, b, a) = color;
    if a <= 0.0 || offset + 3 >= canvas.len() {
        return;
    }
    let dst_b = canvas[offset] as f32 / 255.0;
    let dst_g = canvas[offset + 1] as f32 / 255.0;
    let dst_r = canvas[offset + 2] as f32 / 255.0;
    let dst_a = canvas[offset + 3] as f32 / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    let out_r = r * a + dst_r * (1.0 - a);
    let out_g = g * a + dst_g * (1.0 - a);
    let out_b = b * a + dst_b * (1.0 - a);
    canvas[offset] = (out_b.clamp(0.0, 1.0) * 255.0).round() as u8;
    canvas[offset + 1] = (out_g.clamp(0.0, 1.0) * 255.0).round() as u8;
    canvas[offset + 2] = (out_r.clamp(0.0, 1.0) * 255.0).round() as u8;
    canvas[offset + 3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

fn parse_color(raw: &str, fallback: Color) -> Color {
    let value = raw.trim().trim_start_matches('#');
    let parse = |range: std::ops::Range<usize>| u8::from_str_radix(&value[range], 16).ok();
    match value.len() {
        6 => match (parse(0..2), parse(2..4), parse(4..6)) {
            (Some(r), Some(g), Some(b)) => {
                Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
            }
            _ => fallback,
        },
        8 => match (parse(0..2), parse(2..4), parse(4..6), parse(6..8)) {
            (Some(r), Some(g), Some(b), Some(a)) => Color(
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ),
            _ => fallback,
        },
        _ => fallback,
    }
}

fn push_icon_candidate(candidates: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !candidates.iter().any(|existing| existing == &value) {
        candidates.push(value);
    }
}

fn icon_roots(config: &LiftConfig) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        if let Some(root) = themed_icon_root(Path::new(&home).join(".local/share/icons"), config) {
            roots.push(root);
        }
        roots.push(Path::new(&home).join(".local/share/icons"));
        roots.push(Path::new(&home).join(".icons"));
    }
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::env::split_paths(&data_dirs) {
        if let Some(root) = themed_icon_root(dir.join("icons"), config) {
            roots.push(root);
        }
        roots.push(dir.join("icons"));
        roots.push(dir.join("pixmaps"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn themed_icon_root(root: PathBuf, config: &LiftConfig) -> Option<PathBuf> {
    let theme = config.icon_theme.trim();
    if theme.is_empty() || theme.eq_ignore_ascii_case("auto") {
        return None;
    }
    let themed = root.join(theme);
    themed.is_dir().then_some(themed)
}

fn build_icon_index(
    roots: &[PathBuf],
    search_depth: usize,
    target_size: u32,
) -> HashMap<String, IconIndexEntry> {
    let mut index = HashMap::<String, IconIndexEntry>::new();
    for root in roots {
        walk_icon_files(root.as_path(), search_depth, &mut |path| {
            let Some((key, score)) = icon_index_candidate(path, target_size) else {
                return;
            };
            let entry = IconIndexEntry {
                score,
                path: path.to_path_buf(),
            };
            let replace = index.get(&key).is_none_or(|existing| {
                entry.score < existing.score
                    || (entry.score == existing.score && entry.path < existing.path)
            });
            if replace {
                index.insert(key, entry);
            }
        });
    }
    index
}

/// Location of the persisted icon path index, under `$XDG_CACHE_HOME/halley` (or
/// `~/.cache/halley`). Returns `None` if neither is available.
fn icon_index_cache_path() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CACHE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("halley").join("lift-icons"))
}

/// Canonical description of the inputs that determine the index contents. A cache file
/// is only reused when this matches, so changing the theme, size, search depth, or the
/// set/mtime of the icon roots transparently invalidates it.
fn index_fingerprint(
    roots: &[PathBuf],
    theme: &str,
    target_size: u32,
    search_depth: usize,
) -> String {
    let mut out = format!("v1\nsize={target_size}\ndepth={search_depth}\ntheme={theme}\n");
    for root in roots {
        let mtime = fs::metadata(root)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |dur| dur.as_secs());
        out.push_str(&format!("root={}|{mtime}\n", root.display()));
    }
    out
}

const ICON_CACHE_SEPARATOR: &str = "\n==INDEX==\n";

fn load_index_cache(path: &Path, fingerprint: &str) -> Option<HashMap<String, IconIndexEntry>> {
    let contents = fs::read_to_string(path).ok()?;
    let (header, body) = contents.split_once(ICON_CACHE_SEPARATOR)?;
    if header != fingerprint {
        return None;
    }
    let mut index = HashMap::new();
    for line in body.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(name), Some(score), Some(icon_path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(score) = score.parse::<i32>() else {
            continue;
        };
        index.insert(
            name.to_string(),
            IconIndexEntry {
                score,
                path: PathBuf::from(icon_path),
            },
        );
    }
    Some(index)
}

fn write_index_cache(path: &Path, fingerprint: &str, index: &HashMap<String, IconIndexEntry>) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut out = String::with_capacity(fingerprint.len() + index.len() * 48);
    out.push_str(fingerprint);
    out.push_str(ICON_CACHE_SEPARATOR);
    for (name, entry) in index {
        out.push_str(&format!(
            "{name}\t{}\t{}\n",
            entry.score,
            entry.path.display()
        ));
    }
    // Write through a temp file so a concurrent launch never reads a half-written cache.
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, out.as_bytes()).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn icon_index_candidate(path: &Path, target_size: u32) -> Option<(String, i32)> {
    let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "svg" | "png" | "jpg" | "jpeg") {
        return None;
    }
    Some((stem, icon_path_score(path, target_size)))
}

fn walk_icon_files(dir: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_icon_files(path.as_path(), depth - 1, f);
        } else {
            f(path.as_path());
        }
    }
}

fn icon_path_score(path: &Path, target_size: u32) -> i32 {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let format_score = match ext.as_str() {
        "svg" => 0,
        "png" => 20,
        "jpg" | "jpeg" => 40,
        _ => 80,
    };
    let size_score = icon_size_hint(path)
        .map(|size| (size as i32 - target_size as i32).abs())
        .unwrap_or(24);
    let theme_score = if path.to_string_lossy().contains("/hicolor/") {
        10
    } else {
        0
    };
    format_score + size_score + theme_score
}

fn icon_size_hint(path: &Path) -> Option<u32> {
    for component in path.components() {
        let part = component.as_os_str().to_str()?;
        if part.eq_ignore_ascii_case("scalable") {
            return Some(64);
        }
        if let Some((w, h)) = part.split_once('x')
            && let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>())
        {
            return Some(w.min(h));
        }
    }
    None
}

fn load_icon(path: &Path, target_size: u32) -> Option<IconRaster> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "svg" => load_svg_icon(path, target_size),
        "png" | "jpg" | "jpeg" => load_raster_icon(path, target_size),
        _ => None,
    }
}

fn load_raster_icon(path: &Path, target_size: u32) -> Option<IconRaster> {
    let image = image::open(path).ok()?.to_rgba8();
    let image = normalize_icon_canvas(image, target_size);
    Some(IconRaster {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

fn normalize_icon_canvas(source: RgbaImage, target_size: u32) -> RgbaImage {
    let target_size = target_size.max(1);
    let (src_w, src_h) = source.dimensions();
    if src_w == 0 || src_h == 0 {
        return RgbaImage::new(target_size, target_size);
    }
    let resized = imageops::thumbnail(&source, target_size, target_size);
    let mut canvas = RgbaImage::new(target_size, target_size);
    let dx = ((target_size - resized.width()) / 2) as i64;
    let dy = ((target_size - resized.height()) / 2) as i64;
    imageops::overlay(&mut canvas, &resized, dx, dy);
    canvas
}

fn load_svg_icon(path: &Path, target_size: u32) -> Option<IconRaster> {
    let data = fs::read(path).ok()?;
    render_svg_data(&data, path.parent().map(Path::to_path_buf), target_size)
}

fn render_svg_data(
    data: &[u8],
    resources_dir: Option<PathBuf>,
    target_size: u32,
) -> Option<IconRaster> {
    let target_size = target_size.max(1);
    let options = usvg::Options {
        resources_dir,
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(target_size, target_size)?;
    let scale_x = target_size as f32 / svg_size.width() as f32;
    let scale_y = target_size as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (target_size as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (target_size as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = pixmap.data().to_vec();
    unpremultiply_rgba(&mut rgba);
    Some(IconRaster {
        width: target_size,
        height: target_size,
        rgba,
    })
}

fn unpremultiply_rgba(pixels: &mut [u8]) {
    for chunk in pixels.chunks_exact_mut(4) {
        let alpha = chunk[3] as u32;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        chunk[0] = ((chunk[0] as u32 * 255) / alpha).min(255) as u8;
        chunk[1] = ((chunk[1] as u32 * 255) / alpha).min(255) as u8;
        chunk[2] = ((chunk[2] as u32 * 255) / alpha).min(255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LiftAction;

    #[test]
    fn outline_search_fill_preserves_configured_alpha() {
        let mut config = LiftConfig::default();
        config.border.enabled = true;
        config.border.width = 4;
        config.border.style = "outline".into();
        config.colors.panel_border = "#ffffffff".into();
        config.colors.search = "#ffffff80".into();
        config.ui.search_height = 60;
        config.rounding.search = 12;

        let width = 120;
        let height = 60;
        let panel = Rect {
            x: 0,
            y: 0,
            w: width as i32,
            h: height as i32,
        };
        let colors = Palette::from_config(&config);
        let mut canvas = vec![0; (width * height * 4) as usize];

        draw_chrome(&mut canvas, width, height, &config, &colors, panel, false);

        let center = (((height / 2) * width + (width / 2)) * 4) as usize;
        assert_eq!(canvas[center + 3], 0x80);
    }

    #[test]
    fn cluster_mode_uses_cluster_search_glyph() {
        let mut config = LiftConfig::default();
        config.icons = false;
        let mut cache = IconCache::new(&config);

        let loupe = cache
            .search_glyph(24, LiftMode::General)
            .expect("loupe glyph")
            .rgba
            .clone();
        let cluster = cache
            .search_glyph(24, LiftMode::Clusters)
            .expect("cluster glyph")
            .rgba
            .clone();

        assert_ne!(loupe, cluster);
    }

    #[test]
    fn apps_mode_uses_apps_search_glyph() {
        let mut config = LiftConfig::default();
        config.icons = false;
        let mut cache = IconCache::new(&config);

        let loupe = cache
            .search_glyph(24, LiftMode::General)
            .expect("loupe glyph")
            .rgba
            .clone();
        let apps = cache
            .search_glyph(24, LiftMode::Apps)
            .expect("apps glyph")
            .rgba
            .clone();

        assert_ne!(loupe, apps);
    }

    #[test]
    fn term_mode_uses_terminal_search_glyph() {
        let mut config = LiftConfig::default();
        config.icons = false;
        let mut cache = IconCache::new(&config);

        let loupe = cache
            .search_glyph(24, LiftMode::General)
            .expect("loupe glyph")
            .rgba
            .clone();
        let term = cache
            .search_glyph(24, LiftMode::Term)
            .expect("term glyph")
            .rgba
            .clone();

        assert_ne!(loupe, term);
    }

    #[test]
    fn actions_mode_uses_action_search_glyph() {
        let mut config = LiftConfig::default();
        config.icons = false;
        let mut cache = IconCache::new(&config);

        let loupe = cache
            .search_glyph(24, LiftMode::General)
            .expect("loupe glyph")
            .rgba
            .clone();
        let action = cache
            .search_glyph(24, LiftMode::Actions)
            .expect("action glyph")
            .rgba
            .clone();

        assert_ne!(loupe, action);
    }

    #[test]
    fn disk_index_miss_requests_background_refresh() {
        let mut cache = IconCache {
            entries: HashMap::new(),
            index: HashMap::new(),
            roots: Vec::new(),
            target_size: 28,
            search_depth: 5,
            indexed: true,
            loaded_disk_index: true,
            index_refresh_requested: false,
            index_rx: None,
            decode_tx: None,
            decode_rx: None,
            pending_decodes: 0,
            cache_path: None,
            cache_fingerprint: String::new(),
            search_icon: None,
            app_search_icon: None,
            cluster_search_icon: None,
            action_search_icon: None,
            action_icon: None,
            term_icon: None,
        };

        assert!(cache.load("new-app-icon").is_none());

        assert!(cache.needs_index());
    }

    #[test]
    fn app_result_draws_fallback_when_raster_icon_is_missing() {
        let config = LiftConfig::default();
        let mut cache = IconCache::new(&config);
        cache.indexed = true;
        let result = LiftResult {
            section: "Applications".into(),
            title: "Missing Icon App".into(),
            subtitle: None,
            icon_name: Some("definitely-missing-icon".into()),
            kind: LiftResultKind::App,
            score: 1.0,
            is_field_pinned: false,
            shortcut_hint: None,
            action: LiftAction::ReloadConfig,
        };
        let width = 48;
        let height = 48;
        let mut canvas = vec![0; (width * height * 4) as usize];

        draw_result_icon(
            &mut canvas,
            width,
            height,
            &mut cache,
            &config,
            &result,
            8,
            8,
        );

        assert!(canvas.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
}
