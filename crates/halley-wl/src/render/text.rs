use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use halley_config::FontConfig;
use smithay::{
    backend::renderer::{Color32F, Frame},
    utils::{Physical, Rectangle},
};

use crate::compositor::root::Halley;

use super::state::RenderState;
use super::utils::draw_rect;

const UI_TEXT_CACHE_TTL_SECS: u64 = 30;

#[derive(Clone, Copy, Debug)]
struct UiTextCommand {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    coverage: u8,
}

#[derive(Clone, Debug)]
struct UiTextCacheEntry {
    width: i32,
    height: i32,
    commands: Arc<[UiTextCommand]>,
    last_used_at: Instant,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct UiTextCacheKey {
    text: String,
    family: String,
    size_px: u32,
    weight: u16,
}

#[derive(Default)]
pub(crate) struct UiTextRenderer {
    cache: HashMap<UiTextCacheKey, UiTextCacheEntry>,
    font_system: Option<FontSystem>,
    swash_cache: Option<SwashCache>,
}

#[derive(Clone)]
pub(crate) struct PreparedUiText {
    commands: Arc<[UiTextCommand]>,
}

impl UiTextRenderer {
    pub(crate) fn size(&mut self, font: &FontConfig, text: &str, scale: i32) -> (i32, i32) {
        let entry = self.entry(font, text, scale);
        (entry.width, entry.height)
    }

    pub(crate) fn prepared(&mut self, font: &FontConfig, text: &str, scale: i32) -> PreparedUiText {
        let entry = self.entry(font, text, scale);
        PreparedUiText {
            commands: entry.commands,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }

    pub(crate) fn prune(&mut self, now: Instant) {
        self.cache.retain(|_, entry| {
            now.saturating_duration_since(entry.last_used_at).as_secs() < UI_TEXT_CACHE_TTL_SECS
        });
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.len()
    }

    fn entry(&mut self, font: &FontConfig, text: &str, scale: i32) -> UiTextCacheEntry {
        let key = UiTextCacheKey {
            text: text.to_string(),
            family: normalized_family(font),
            size_px: font_size_for_scale(font, scale),
            weight: font.weight,
        };
        let now = Instant::now();
        if let Some(entry) = self.cache.get_mut(&key) {
            entry.last_used_at = now;
            return entry.clone();
        }

        let entry = self.build_entry(&key, now);
        self.cache.insert(key, entry.clone());
        entry
    }

    fn build_entry(&mut self, key: &UiTextCacheKey, now: Instant) -> UiTextCacheEntry {
        if key.text.is_empty() {
            return UiTextCacheEntry {
                width: 0,
                height: 0,
                commands: Arc::from(Vec::<UiTextCommand>::new().into_boxed_slice()),
                last_used_at: now,
            };
        }

        let metrics = metrics_for_size(key.size_px);
        let font_system = self.font_system.get_or_insert_with(FontSystem::new);
        let swash_cache = self.swash_cache.get_or_insert_with(SwashCache::new);
        let resolved_family = resolve_named_family(font_system, key.family.as_str());
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_size(font_system, None, None);
        buffer.set_text(
            font_system,
            key.text.as_str(),
            &attrs_for_key(key, resolved_family.as_deref()),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(font_system, true);

        let mut width = 0i32;
        let mut height = 0i32;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w.ceil() as i32);
            height = height.max((run.line_top + run.line_height).ceil() as i32);
        }

        let mut commands = Vec::new();
        buffer.draw(
            font_system,
            swash_cache,
            Color::rgba(255, 255, 255, 255),
            |x, y, w, h, color| {
                if w == 0 || h == 0 || color.a() == 0 {
                    return;
                }
                let w = w as i32;
                let h = h as i32;
                width = width.max(x.saturating_add(w));
                height = height.max(y.saturating_add(h));
                commands.push(UiTextCommand {
                    x,
                    y,
                    w,
                    h,
                    coverage: color.a(),
                });
            },
        );

        UiTextCacheEntry {
            width: width.max(0),
            height: height.max(metrics.line_height.ceil() as i32).max(0),
            commands: Arc::from(commands.into_boxed_slice()),
            last_used_at: now,
        }
    }
}

pub(crate) fn ui_text_size(st: &Halley, text: &str, scale: i32) -> (i32, i32) {
    ui_text_size_in(&st.ui.render_state, &st.runtime.tuning.font, text, scale)
}

pub(crate) fn ui_text_size_in(
    render_state: &RenderState,
    font: &FontConfig,
    text: &str,
    scale: i32,
) -> (i32, i32) {
    render_state.ui_text.borrow_mut().size(font, text, scale)
}

pub(crate) fn draw_ui_text<F: Frame>(
    frame: &mut F,
    st: &Halley,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    draw_ui_text_in(
        frame,
        &st.ui.render_state,
        &st.runtime.tuning.font,
        x,
        y,
        text,
        scale,
        color,
        damage,
    )
}

pub(crate) fn draw_ui_text_in<F: Frame>(
    frame: &mut F,
    render_state: &RenderState,
    font: &FontConfig,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    let prepared = render_state
        .ui_text
        .borrow_mut()
        .prepared(font, text, scale);
    for cmd in prepared.commands.iter() {
        let alpha = color.a() * (cmd.coverage as f32 / 255.0);
        if alpha <= 0.001 {
            continue;
        }
        draw_rect(
            frame,
            x + cmd.x,
            y + cmd.y,
            cmd.w,
            cmd.h,
            Color32F::new(color.r(), color.g(), color.b(), alpha),
            damage,
        )?;
    }
    Ok(())
}

fn attrs_for_key<'a>(key: &'a UiTextCacheKey, resolved_family: Option<&'a str>) -> Attrs<'a> {
    Attrs::new()
        .family(resolve_family(
            resolved_family.unwrap_or(key.family.as_str()),
        ))
        .weight(Weight(key.weight))
}

fn metrics_for_size(size_px: u32) -> Metrics {
    let font_size = size_px.max(1) as f32;
    Metrics::new(font_size, (font_size * 1.25).ceil())
}

fn normalized_family(font: &FontConfig) -> String {
    let family = font.family.trim();
    if family.is_empty() {
        FontConfig::default().family
    } else {
        family.to_string()
    }
}

fn resolve_family(family: &str) -> Family<'_> {
    match family.trim().to_ascii_lowercase().as_str() {
        "serif" => Family::Serif,
        "sans-serif" | "sans_serif" | "sansserif" | "sans" => Family::SansSerif,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        "monospace" => Family::Monospace,
        _ => Family::Name(family),
    }
}

fn resolve_named_family(font_system: &FontSystem, requested_family: &str) -> Option<String> {
    let requested = requested_family.trim();
    if requested.is_empty() {
        return None;
    }
    if matches!(
        resolve_family(requested),
        Family::Serif | Family::SansSerif | Family::Cursive | Family::Fantasy | Family::Monospace
    ) {
        return None;
    }

    let requested_folded = fold_font_name(requested);
    font_system.db().faces().find_map(|face| {
        face.families
            .iter()
            .map(|(family, _language)| family.as_str())
            .find(|family| fold_font_name(family) == requested_folded)
            .map(str::to_string)
            .or_else(|| {
                (fold_font_name(face.post_script_name.as_str()) == requested_folded)
                    .then(|| face.families.first().map(|(family, _)| family.clone()))
                    .flatten()
            })
    })
}

fn fold_font_name(name: &str) -> String {
    name.chars()
        .filter(|ch| !matches!(ch, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn font_size_for_scale(font: &FontConfig, scale: i32) -> u32 {
    match scale.max(1) {
        1 => font.size.saturating_sub(2).max(8),
        2 => font.size,
        3 => font.size.saturating_add(4),
        n => font.size.saturating_add(((n - 2) as u32).saturating_mul(4)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_scale_maps_font_sizes_from_runtime_config() {
        let font = FontConfig {
            family: "monospace".to_string(),
            size: 11,
            weight: 500,
        };

        assert_eq!(font_size_for_scale(&font, 1), 9);
        assert_eq!(font_size_for_scale(&font, 2), 11);
        assert_eq!(font_size_for_scale(&font, 3), 15);
    }

    #[test]
    fn cache_can_be_invalidated() {
        let mut renderer = UiTextRenderer::default();
        let font = FontConfig::default();

        let _ = renderer.size(&font, "HALLEY", 2);
        assert!(renderer.cache_len() >= 1);
        renderer.clear();
        assert_eq!(renderer.cache_len(), 0);
    }

    #[test]
    fn named_family_resolution_matches_case_insensitively() {
        let font_system = FontSystem::new();
        let family = font_system
            .db()
            .faces()
            .find_map(|face| face.families.first().map(|(family, _)| family.clone()))
            .expect("system font family");
        let folded = family.to_ascii_lowercase();

        assert_eq!(
            resolve_named_family(&font_system, folded.as_str()),
            Some(family)
        );
    }
}
