use std::env;
use std::fs;
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
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
    pub(crate) pixels_rgba: Vec<u8>,
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
// Environment
// ---------------------------------------------------------------------------

pub(crate) fn env_cursor_theme_and_size() -> (String, u32) {
    let theme = env::var("XCURSOR_THEME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Adwaita".to_string());
    let size = env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|v| v.clamp(8, 128))
        .unwrap_or(24);
    (theme, size)
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
    icon_names: &[&str],
) -> Option<Arc<SoftwareCursorSprite>> {
    let theme = CursorTheme::load(theme_name);
    for icon_name in icon_names {
        let icon_path = theme.load_icon(icon_name)?;
        let bytes = fs::read(icon_path).ok()?;
        let images = xcursor::parser::parse_xcursor(&bytes)?;
        let image = pick_best_cursor_image(&images, requested_size)?;
        let width = usize::try_from(image.width).ok()?;
        let height = usize::try_from(image.height).ok()?;
        let max_hotspot_x = i32::try_from(width.saturating_sub(1)).ok().unwrap_or(0);
        let max_hotspot_y = i32::try_from(height.saturating_sub(1)).ok().unwrap_or(0);
        return Some(Arc::new(SoftwareCursorSprite {
            width,
            height,
            hotspot_x: (image.xhot as i32).clamp(0, max_hotspot_x),
            hotspot_y: (image.yhot as i32).clamp(0, max_hotspot_y),
            pixels_rgba: image.pixels_rgba.clone(),
        }));
    }
    None
}

// ---------------------------------------------------------------------------
// Icon → xcursor name candidates (Open/Closed for extension)
// ---------------------------------------------------------------------------

pub(crate) fn cursor_icon_candidates(
    icon: smithay::input::pointer::CursorIcon,
) -> &'static [&'static str] {
    use smithay::input::pointer::CursorIcon;
    match icon {
        CursorIcon::Default => &["left_ptr", "default", "arrow", "top_left_arrow"],
        CursorIcon::Text => &["text", "xterm", "ibeam", "left_ptr"],
        CursorIcon::VerticalText => &["vertical-text", "text", "xterm", "left_ptr"],
        CursorIcon::Pointer => &["pointer", "hand2", "hand1", "left_ptr"],
        CursorIcon::Grab => &["grab", "openhand", "fleur", "left_ptr"],
        CursorIcon::Grabbing => &["grabbing", "closedhand", "fleur", "left_ptr"],
        CursorIcon::Move | CursorIcon::AllScroll => &["move", "all-scroll", "fleur", "left_ptr"],
        CursorIcon::Wait => &["wait", "watch", "left_ptr_watch", "left_ptr"],
        CursorIcon::Progress => &["progress", "left_ptr_watch", "watch", "left_ptr"],
        CursorIcon::Crosshair => &["crosshair", "cross", "tcross", "left_ptr"],
        CursorIcon::EwResize | CursorIcon::ColResize => &[
            "ew-resize",
            "h_double_arrow",
            "sb_h_double_arrow",
            "left_ptr",
        ],
        CursorIcon::NsResize | CursorIcon::RowResize => &[
            "ns-resize",
            "v_double_arrow",
            "sb_v_double_arrow",
            "left_ptr",
        ],
        CursorIcon::NeResize => &["ne-resize", "top_right_corner", "right_side", "left_ptr"],
        CursorIcon::NwResize => &["nw-resize", "top_left_corner", "left_side", "left_ptr"],
        CursorIcon::SeResize => &["se-resize", "bottom_right_corner", "right_side", "left_ptr"],
        CursorIcon::SwResize => &["sw-resize", "bottom_left_corner", "left_side", "left_ptr"],
        CursorIcon::NResize => &["n-resize", "top_side", "ns-resize", "left_ptr"],
        CursorIcon::SResize => &["s-resize", "bottom_side", "ns-resize", "left_ptr"],
        CursorIcon::EResize => &["e-resize", "right_side", "ew-resize", "left_ptr"],
        CursorIcon::WResize => &["w-resize", "left_side", "ew-resize", "left_ptr"],
        CursorIcon::NeswResize => &[
            "nesw-resize",
            "fd_double_arrow",
            "bottom_left_corner",
            "left_ptr",
        ],
        CursorIcon::NwseResize => &[
            "nwse-resize",
            "bd_double_arrow",
            "bottom_right_corner",
            "left_ptr",
        ],
        CursorIcon::NoDrop | CursorIcon::NotAllowed => {
            &["not-allowed", "crossed_circle", "no-drop", "left_ptr"]
        }
        CursorIcon::Copy => &["copy", "dnd-copy", "left_ptr"],
        CursorIcon::Alias => &["alias", "dnd-link", "left_ptr"],
        CursorIcon::Help => &["help", "question_arrow", "left_ptr"],
        CursorIcon::ContextMenu => &["context-menu", "left_ptr"],
        CursorIcon::Cell => &["cell", "crosshair", "left_ptr"],
        CursorIcon::ZoomIn => &["zoom-in", "left_ptr"],
        CursorIcon::ZoomOut => &["zoom-out", "left_ptr"],
        _ => &["left_ptr", "default", "arrow", "top_left_arrow"],
    }
}

fn cursor_icon_fallback_chain(
    icon: smithay::input::pointer::CursorIcon,
) -> &'static [smithay::input::pointer::CursorIcon] {
    use smithay::input::pointer::CursorIcon;
    match icon {
        CursorIcon::Text | CursorIcon::VerticalText => &[CursorIcon::Text, CursorIcon::Default],
        CursorIcon::EwResize | CursorIcon::ColResize => {
            &[CursorIcon::EwResize, CursorIcon::Move, CursorIcon::Default]
        }
        CursorIcon::NsResize | CursorIcon::RowResize => {
            &[CursorIcon::NsResize, CursorIcon::Move, CursorIcon::Default]
        }
        CursorIcon::NeResize
        | CursorIcon::NwResize
        | CursorIcon::SeResize
        | CursorIcon::SwResize
        | CursorIcon::NeswResize
        | CursorIcon::NwseResize
        | CursorIcon::NResize
        | CursorIcon::SResize
        | CursorIcon::EResize
        | CursorIcon::WResize => &[
            CursorIcon::NwseResize,
            CursorIcon::Move,
            CursorIcon::Default,
        ],
        CursorIcon::Pointer => &[CursorIcon::Pointer, CursorIcon::Default],
        CursorIcon::NoDrop | CursorIcon::NotAllowed => {
            &[CursorIcon::NotAllowed, CursorIcon::Default]
        }
        CursorIcon::Copy => &[CursorIcon::Copy, CursorIcon::Pointer, CursorIcon::Default],
        CursorIcon::Alias => &[CursorIcon::Alias, CursorIcon::Pointer, CursorIcon::Default],
        CursorIcon::Wait | CursorIcon::Progress => {
            &[CursorIcon::Progress, CursorIcon::Wait, CursorIcon::Default]
        }
        CursorIcon::Grab | CursorIcon::Grabbing => {
            &[CursorIcon::Grab, CursorIcon::Move, CursorIcon::Default]
        }
        CursorIcon::Move | CursorIcon::AllScroll => &[CursorIcon::Move, CursorIcon::Default],
        CursorIcon::Crosshair | CursorIcon::Cell => &[CursorIcon::Crosshair, CursorIcon::Default],
        _ => &[CursorIcon::Default],
    }
}

// ---------------------------------------------------------------------------
// Public sprite resolution with fallback chain
// ---------------------------------------------------------------------------

fn themed_cursor_sprite(
    icon: smithay::input::pointer::CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    let (theme, size) = env_cursor_theme_and_size();
    let icon_key = format!("{:?}", icon);
    let names = cursor_icon_candidates(icon);

    let mut cache = CURSOR_SPRITE_CACHE.lock().ok()?;
    if let Some(cached) = cache.as_ref() {
        if cached.theme == theme && cached.size == size && cached.icon_key == icon_key {
            return cached.sprite.clone();
        }
    }

    let sprite = load_cursor_from_theme(theme.as_str(), size, names).or_else(|| {
        if theme == "Adwaita" {
            None
        } else {
            load_cursor_from_theme("Adwaita", size, names)
        }
    });

    *cache = Some(CachedCursorSprite {
        theme,
        size,
        icon_key,
        sprite: sprite.clone(),
    });
    sprite
}

pub(crate) fn themed_cursor_sprite_with_fallback(
    icon: smithay::input::pointer::CursorIcon,
) -> Option<Arc<SoftwareCursorSprite>> {
    themed_cursor_sprite(icon).or_else(|| {
        for fallback in cursor_icon_fallback_chain(icon) {
            if *fallback == icon {
                continue;
            }
            if let Some(sprite) = themed_cursor_sprite(*fallback) {
                return Some(sprite);
            }
        }
        None
    })
}
