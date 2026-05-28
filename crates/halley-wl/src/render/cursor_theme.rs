use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};

use halley_config::CursorConfig;
use once_cell::sync::Lazy;
use smithay::input::pointer::{CursorIcon, CursorImageStatus};
use smithay::utils::IsAlive;
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

type CursorSpriteCache = HashMap<(String, u32, String), Option<Arc<SoftwareCursorSprite>>>;

#[derive(Default)]
struct CursorSpriteManager {
    cache: CursorSpriteCache,
}

impl CursorSpriteManager {
    fn sprite_with_fallback(
        &mut self,
        cursor: &CursorConfig,
        icon: CursorIcon,
    ) -> Option<Arc<SoftwareCursorSprite>> {
        self.sprite(cursor, icon).or_else(|| {
            if icon == CursorIcon::Default {
                None
            } else {
                self.sprite(cursor, CursorIcon::Default)
            }
        })
    }

    fn sprite(
        &mut self,
        cursor: &CursorConfig,
        icon: CursorIcon,
    ) -> Option<Arc<SoftwareCursorSprite>> {
        let theme = cursor.theme.trim();
        let theme = if theme.is_empty() { "Adwaita" } else { theme };
        let size = cursor.size.clamp(8, 128);
        let icon_key = icon.name().to_string();
        let cache_key = (theme.to_string(), size, icon_key);

        self.cache
            .entry(cache_key)
            .or_insert_with(|| {
                load_cursor_from_theme(theme, size, icon).or_else(|| {
                    if theme == "Adwaita" {
                        None
                    } else {
                        load_cursor_from_theme("Adwaita", size, icon)
                    }
                })
            })
            .clone()
    }
}

static CURSOR_SPRITES: Lazy<Mutex<CursorSpriteManager>> =
    Lazy::new(|| Mutex::new(CursorSpriteManager::default()));

pub(crate) struct CursorManager {
    current_cursor: CursorImageStatus,
    sprites: CursorSpriteManager,
}

impl Default for CursorManager {
    fn default() -> Self {
        Self {
            current_cursor: CursorImageStatus::default_named(),
            sprites: CursorSpriteManager::default(),
        }
    }
}

impl CursorManager {
    pub(crate) fn cursor_image(&self) -> &CursorImageStatus {
        &self.current_cursor
    }

    pub(crate) fn set_cursor_image(&mut self, cursor: CursorImageStatus) {
        self.current_cursor = cursor;
    }

    pub(crate) fn check_cursor_image_surface_alive(&mut self) {
        if let CursorImageStatus::Surface(surface) = &self.current_cursor
            && !surface.alive()
        {
            self.current_cursor = CursorImageStatus::default_named();
        }
    }

    pub(crate) fn sprite_with_fallback(
        &mut self,
        cursor: &CursorConfig,
        icon: CursorIcon,
    ) -> Option<Arc<SoftwareCursorSprite>> {
        self.sprites.sprite_with_fallback(cursor, icon)
    }
}

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

pub(crate) fn themed_cursor_sprite_with_fallback(
    cursor: &CursorConfig,
    icon: CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    CURSOR_SPRITES
        .lock()
        .ok()?
        .sprite_with_fallback(cursor, icon)
}
