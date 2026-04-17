use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Hinting, Metrics, Shaping, Style, SwashCache, Weight,
};
use halley_config::FontConfig;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ImportMem, Texture,
            gles::{GlesFrame, GlesRenderer},
        },
    },
    utils::{Buffer as BufferCoords, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::render::draw_primitives::draw_rect;
use crate::render::state::RenderState;

const UI_TEXT_CACHE_TTL_SECS: u64 = 30;
const UI_TEXT_HINTING_MAX_SIZE_PX: u32 = 16;

#[derive(Clone, Copy, Debug)]
struct UiTextCommand {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    coverage: u8,
}

struct UiTextCacheEntry {
    width: i32,
    height: i32,
    commands: Arc<[UiTextCommand]>,
    pixels_rgba: Option<Vec<u8>>,
    texture: Option<smithay::backend::renderer::gles::GlesTexture>,
    last_used_at: Instant,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct UiTextCacheKey {
    text: String,
    family: String,
    size_px: u32,
    color_rgb: [u8; 3],
}

#[derive(Default)]
pub(crate) struct UiTextRenderer {
    cache: HashMap<UiTextCacheKey, UiTextCacheEntry>,
    font_system: Option<FontSystem>,
    swash_cache: Option<SwashCache>,
}

#[derive(Clone)]
pub(crate) struct PreparedUiText {
    width: i32,
    height: i32,
    commands: Arc<[UiTextCommand]>,
    texture: Option<smithay::backend::renderer::gles::GlesTexture>,
}

impl UiTextRenderer {
    pub(crate) fn size(&mut self, font: &FontConfig, text: &str, scale: i32) -> (i32, i32) {
        let key = cache_key(font, text, scale, Color32F::new(1.0, 1.0, 1.0, 1.0));
        self.ensure_entry(&key);
        let entry = self.cache.get_mut(&key).expect("ui text cache entry");
        entry.last_used_at = Instant::now();
        (entry.width, entry.height)
    }

    pub(crate) fn prepared(
        &mut self,
        font: &FontConfig,
        text: &str,
        scale: i32,
        color: Color32F,
    ) -> PreparedUiText {
        let key = cache_key(font, text, scale, color);
        self.ensure_entry(&key);
        let entry = self.cache.get_mut(&key).expect("ui text cache entry");
        entry.last_used_at = Instant::now();
        PreparedUiText {
            width: entry.width,
            height: entry.height,
            commands: Arc::clone(&entry.commands),
            texture: entry.texture.clone(),
        }
    }

    pub(crate) fn ensure_textures(
        &mut self,
        renderer: &mut GlesRenderer,
    ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
        for entry in self.cache.values_mut() {
            if entry.texture.is_some() || entry.width <= 0 || entry.height <= 0 {
                continue;
            }
            let Some(pixels) = entry.pixels_rgba.as_ref() else {
                continue;
            };
            let texture = renderer.import_memory(
                pixels,
                Fourcc::Abgr8888,
                (entry.width, entry.height).into(),
                false,
            )?;
            entry.texture = Some(texture);
            entry.pixels_rgba = None;
        }
        Ok(())
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

    fn ensure_entry(&mut self, key: &UiTextCacheKey) {
        if self.cache.contains_key(key) {
            return;
        }

        let entry = self.build_entry(key, Instant::now());
        self.cache.insert(key.clone(), entry);
    }

    fn build_entry(&mut self, key: &UiTextCacheKey, now: Instant) -> UiTextCacheEntry {
        if key.text.is_empty() {
            return UiTextCacheEntry {
                width: 0,
                height: 0,
                commands: Arc::from(Vec::<UiTextCommand>::new().into_boxed_slice()),
                pixels_rgba: None,
                texture: None,
                last_used_at: now,
            };
        }

        let metrics = metrics_for_size(key.size_px);
        let font_system = self.font_system.get_or_insert_with(FontSystem::new);
        let swash_cache = self.swash_cache.get_or_insert_with(SwashCache::new);
        let resolved_family = resolve_named_family(font_system, key.family.as_str());
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_hinting(
            font_system,
            if key.size_px <= UI_TEXT_HINTING_MAX_SIZE_PX {
                Hinting::Enabled
            } else {
                Hinting::Disabled
            },
        );
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

        width = width.max(0);
        height = height.max(metrics.line_height.ceil() as i32).max(0);
        let pixels_rgba = rasterized_pixels(
            &buffer,
            font_system,
            swash_cache,
            width,
            height,
            Color::rgba(key.color_rgb[0], key.color_rgb[1], key.color_rgb[2], 255),
        );

        UiTextCacheEntry {
            width,
            height,
            commands: Arc::from(commands.into_boxed_slice()),
            pixels_rgba,
            texture: None,
            last_used_at: now,
        }
    }
}

pub(crate) fn ensure_ui_text_resources(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
) -> Result<(), Box<dyn Error>> {
    st.ui
        .render_state
        .cache
        .ui_text
        .borrow_mut()
        .ensure_textures(renderer)?;
    Ok(())
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
    render_state
        .cache
        .ui_text
        .borrow_mut()
        .size(font, text, scale)
}

pub(crate) fn ui_text_size_px_in(
    render_state: &RenderState,
    family: &str,
    size_px: u32,
    text: &str,
) -> (i32, i32) {
    let font = FontConfig {
        family: family.to_string(),
        size: size_px.max(1),
    };
    ui_text_size_in(render_state, &font, text, 2)
}

pub(crate) fn draw_ui_text(
    frame: &mut GlesFrame<'_, '_>,
    st: &Halley,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
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

pub(crate) fn prime_ui_text(st: &Halley, text: &str, scale: i32, color: Color32F) {
    prime_ui_text_in(
        &st.ui.render_state,
        &st.runtime.tuning.font,
        text,
        scale,
        color,
    )
}

pub(crate) fn prime_ui_text_in(
    render_state: &RenderState,
    font: &FontConfig,
    text: &str,
    scale: i32,
    color: Color32F,
) {
    if color.a() <= 0.001 || text.is_empty() {
        return;
    }

    let opaque_color = Color32F::new(color.r(), color.g(), color.b(), 1.0);
    let _ = render_state
        .cache
        .ui_text
        .borrow_mut()
        .prepared(font, text, scale, opaque_color);
}

pub(crate) fn draw_ui_text_in(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    font: &FontConfig,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    if color.a() <= 0.001 {
        return Ok(());
    }

    let prepared = render_state
        .cache
        .ui_text
        .borrow_mut()
        .prepared(font, text, scale, color);
    if let Some(texture) = prepared.texture.as_ref()
        && prepared.width > 0
        && prepared.height > 0
    {
        let tex_size: smithay::utils::Size<i32, BufferCoords> = texture.size();
        let src = Rectangle::<f64, BufferCoords>::new(
            (0.0, 0.0).into(),
            (tex_size.w as f64, tex_size.h as f64).into(),
        );
        let dst = Rectangle::<i32, Physical>::new(
            (x, y).into(),
            (prepared.width.max(1), prepared.height.max(1)).into(),
        );
        return frame.render_texture_from_to(
            texture,
            src,
            dst,
            &[damage],
            &[],
            Transform::Normal,
            color.a().clamp(0.0, 1.0),
            None,
            &[],
        );
    }

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
    let request = parse_font_request(key.family.as_str());
    Attrs::new()
        .family(resolve_family(resolved_family.unwrap_or(request.family)))
        .style(request.style)
        .weight(request.weight)
}

fn rasterized_pixels(
    buffer: &Buffer,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    width: i32,
    height: i32,
    base_color: Color,
) -> Option<Vec<u8>> {
    if width <= 0 || height <= 0 {
        return None;
    }

    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, run.line_y), 1.0);
            swash_cache.with_pixels(
                font_system,
                physical.cache_key,
                base_color,
                |gx, gy, color| {
                    blend_rgba_pixel(
                        &mut pixels,
                        width,
                        height,
                        physical.x + gx,
                        physical.y + gy,
                        [color.r(), color.g(), color.b(), color.a()],
                    );
                },
            );
        }
    }

    Some(pixels)
}

fn blend_rgba_pixel(pixels: &mut [u8], width: i32, height: i32, x: i32, y: i32, src_rgba: [u8; 4]) {
    if src_rgba[3] == 0 || x < 0 || y < 0 || x >= width || y >= height {
        return;
    }

    let idx = ((y as usize * width as usize) + x as usize) * 4;
    let src_a = src_rgba[3] as f32 / 255.0;
    let dst_r = pixels[idx] as f32 / 255.0;
    let dst_g = pixels[idx + 1] as f32 / 255.0;
    let dst_b = pixels[idx + 2] as f32 / 255.0;
    let dst_a = pixels[idx + 3] as f32 / 255.0;

    // Store cached UI text as premultiplied RGBA. Straight-alpha white glyph edges
    // bloom badly on dark backgrounds in a premultiplied compositor pipeline.
    let src_r = (src_rgba[0] as f32 / 255.0) * src_a;
    let src_g = (src_rgba[1] as f32 / 255.0) * src_a;
    let src_b = (src_rgba[2] as f32 / 255.0) * src_a;

    let out_a = src_a + dst_a * (1.0 - src_a);
    let out_r = src_r + dst_r * (1.0 - src_a);
    let out_g = src_g + dst_g * (1.0 - src_a);
    let out_b = src_b + dst_b * (1.0 - src_a);

    pixels[idx] = (out_r.clamp(0.0, 1.0) * 255.0).round() as u8;
    pixels[idx + 1] = (out_g.clamp(0.0, 1.0) * 255.0).round() as u8;
    pixels[idx + 2] = (out_b.clamp(0.0, 1.0) * 255.0).round() as u8;
    pixels[idx + 3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

fn cache_key(font: &FontConfig, text: &str, scale: i32, color: Color32F) -> UiTextCacheKey {
    UiTextCacheKey {
        text: text.to_string(),
        family: normalized_family(font),
        size_px: font_size_for_scale(font, scale),
        color_rgb: [
            (color.r().clamp(0.0, 1.0) * 255.0).round() as u8,
            (color.g().clamp(0.0, 1.0) * 255.0).round() as u8,
            (color.b().clamp(0.0, 1.0) * 255.0).round() as u8,
        ],
    }
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
    let request = parse_font_request(requested_family);
    let requested = request.family.trim();
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParsedFontRequest<'a> {
    family: &'a str,
    style: Style,
    weight: Weight,
}

fn parse_font_request(requested: &str) -> ParsedFontRequest<'_> {
    let trimmed = requested.trim();
    let mut family = trimmed;
    let mut style = Style::Normal;
    let mut weight = Weight::NORMAL;

    loop {
        if matches!(style, Style::Normal) {
            if let Some(stripped) = strip_font_suffix(family, &[" italic"]) {
                family = stripped;
                style = Style::Italic;
                continue;
            }
            if let Some(stripped) = strip_font_suffix(family, &[" oblique"]) {
                family = stripped;
                style = Style::Oblique;
                continue;
            }
        }

        if matches!(weight, Weight::NORMAL) {
            let weight_suffixes = [
                (
                    &[
                        " extra bold",
                        " extra-bold",
                        " extrabold",
                        " ultra bold",
                        " ultra-bold",
                        " ultrabold",
                    ][..],
                    Weight::EXTRA_BOLD,
                ),
                (
                    &[
                        " semi bold",
                        " semi-bold",
                        " semibold",
                        " demi bold",
                        " demi-bold",
                        " demibold",
                    ][..],
                    Weight::SEMIBOLD,
                ),
                (
                    &[
                        " extra light",
                        " extra-light",
                        " extralight",
                        " ultra light",
                        " ultra-light",
                        " ultralight",
                    ][..],
                    Weight::EXTRA_LIGHT,
                ),
                (&[" bold"][..], Weight::BOLD),
                (&[" medium"][..], Weight::MEDIUM),
                (&[" light"][..], Weight::LIGHT),
                (&[" thin"][..], Weight::THIN),
                (&[" black", " heavy"][..], Weight::BLACK),
                (
                    &[" regular", " normal", " book", " roman"][..],
                    Weight::NORMAL,
                ),
            ];
            if let Some((stripped, parsed_weight)) =
                weight_suffixes
                    .iter()
                    .find_map(|(suffixes, parsed_weight)| {
                        strip_font_suffix(family, suffixes)
                            .map(|stripped| (stripped, *parsed_weight))
                    })
            {
                family = stripped;
                weight = parsed_weight;
                continue;
            }
        }

        break;
    }

    ParsedFontRequest {
        family: if family.trim().is_empty() {
            trimmed
        } else {
            family.trim()
        },
        style,
        weight,
    }
}

fn strip_font_suffix<'a>(value: &'a str, suffixes: &[&str]) -> Option<&'a str> {
    let folded = value.to_ascii_lowercase();
    suffixes.iter().find_map(|suffix| {
        folded
            .ends_with(suffix)
            .then(|| value[..value.len().saturating_sub(suffix.len())].trim_end())
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

    #[test]
    fn parses_weight_and_style_suffixes_from_font_request() {
        let parsed = parse_font_request("CommitMono Nerd Font Bold Italic");

        assert_eq!(parsed.family, "CommitMono Nerd Font");
        assert_eq!(parsed.weight, Weight::BOLD);
        assert_eq!(parsed.style, Style::Italic);
    }

    #[test]
    fn leaves_plain_family_unchanged() {
        let parsed = parse_font_request("CommitMono Nerd Font");

        assert_eq!(parsed.family, "CommitMono Nerd Font");
        assert_eq!(parsed.weight, Weight::NORMAL);
        assert_eq!(parsed.style, Style::Normal);
    }
}
