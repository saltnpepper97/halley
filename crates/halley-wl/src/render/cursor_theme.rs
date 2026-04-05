use std::fs;
use std::sync::{Arc, Mutex};

use halley_config::CursorConfig;
use once_cell::sync::Lazy;
use smithay::input::pointer::CursorIcon;
use xcursor::{CursorTheme, parser::Image};

// ---------------------------------------------------------------------------
// Sprite data
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct SoftwareCursorSprite {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) pixels_bgra: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedCursorSprite {
    theme: String,
    size: u32,
    icon_key: String,
    sprite: Option<Arc<SoftwareCursorSprite>>,
}

static CURSOR_SPRITE_CACHE: Lazy<Mutex<Option<CachedCursorSprite>>> =
    Lazy::new(|| Mutex::new(None));

// ---------------------------------------------------------------------------
// Theme loading
// ---------------------------------------------------------------------------

fn pick_best_cursor_image(images: &[Image], requested_size: u32) -> Option<&Image> {
    images.iter().min_by_key(|img| {
        let nominal_delta = img.size.abs_diff(requested_size);
        let width_delta = img.width.abs_diff(requested_size);
        (nominal_delta, width_delta, img.delay)
    })
}

fn load_cursor_from_theme(
    theme_name: &str,
    requested_size: u32,
    icon: CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    let theme = CursorTheme::load(theme_name);
    for icon_name in std::iter::once(icon.name()).chain(icon.alt_names().iter().copied()) {
        let Some(icon_path) = theme.load_icon(icon_name) else {
            continue;
        };
        let Some(bytes) = fs::read(icon_path).ok() else {
            continue;
        };
        let Some(images) = xcursor::parser::parse_xcursor(&bytes) else {
            continue;
        };
        let Some(image) = pick_best_cursor_image(&images, requested_size) else {
            continue;
        };
        let width = usize::try_from(image.width).ok()?;
        let height = usize::try_from(image.height).ok()?;
        let max_hotspot_x = i32::try_from(width.saturating_sub(1)).ok().unwrap_or(0);
        let max_hotspot_y = i32::try_from(height.saturating_sub(1)).ok().unwrap_or(0);
        return Some(Arc::new(SoftwareCursorSprite {
            width,
            height,
            hotspot_x: (image.xhot as i32).clamp(0, max_hotspot_x),
            hotspot_y: (image.yhot as i32).clamp(0, max_hotspot_y),
            pixels_bgra: image.pixels_rgba.clone(),
        }));
    }
    None
}

// ---------------------------------------------------------------------------
// Public sprite resolution with fallback chain
// ---------------------------------------------------------------------------

fn themed_cursor_sprite(
    cursor: &CursorConfig,
    icon: CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    let theme = cursor.theme.trim();
    let theme = if theme.is_empty() { "Adwaita" } else { theme };
    let size = cursor.size.clamp(8, 128);
    let icon_key = icon.name().to_string();

    let mut cache = CURSOR_SPRITE_CACHE.lock().ok()?;
    if let Some(cached) = cache.as_ref()
        && cached.theme == theme
        && cached.size == size
        && cached.icon_key == icon_key
    {
        return cached.sprite.clone();
    }

    let sprite = load_cursor_from_theme(theme, size, icon).or_else(|| {
        if theme == "Adwaita" {
            None
        } else {
            load_cursor_from_theme("Adwaita", size, icon)
        }
    });

    *cache = Some(CachedCursorSprite {
        theme: theme.to_string(),
        size,
        icon_key,
        sprite: sprite.clone(),
    });
    sprite
}

pub(crate) fn themed_cursor_sprite_with_fallback(
    cursor: &CursorConfig,
    icon: CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    themed_cursor_sprite(cursor, icon).or_else(|| {
        if icon == CursorIcon::Default {
            None
        } else {
            themed_cursor_sprite(cursor, CursorIcon::Default)
        }
    })
}
