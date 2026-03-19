use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use image::{RgbaImage, imageops};
use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::ImportMem;
use smithay::backend::renderer::gles::GlesRenderer;

use crate::state::{HalleyWlState, NodeAppIconCacheEntry, NodeAppIconTexture};

use super::node_render::NodeSnapshot;

const NODE_ICON_RASTER_PX: u32 = 64;
const ICON_WALK_MAX_DEPTH: usize = 6;

struct AppIconRaster {
    width: i32,
    height: i32,
    pixels_rgba: Vec<u8>,
}

pub(crate) fn ensure_node_app_icon_resources(
    renderer: &mut GlesRenderer,
    st: &mut HalleyWlState,
    render_nodes: &[NodeSnapshot],
) -> Result<(), Box<dyn std::error::Error>> {
    for node in render_nodes {
        if !matches!(
            node.state,
            halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
        ) {
            continue;
        }

        let Some(app_id) = st.node_app_ids.get(&node.id).cloned() else {
            continue;
        };
        if st.node_app_icon_cache.contains_key(&app_id) {
            continue;
        }

        let Some(icon_path) = resolve_app_icon_path(&app_id) else {
            st.node_app_icon_cache
                .insert(app_id, NodeAppIconCacheEntry::Missing);
            continue;
        };

        let Some(raster) = load_icon_raster(&icon_path) else {
            st.node_app_icon_cache
                .insert(app_id, NodeAppIconCacheEntry::Missing);
            continue;
        };

        let texture = renderer.import_memory(
            &raster.pixels_rgba,
            Fourcc::Abgr8888,
            (raster.width, raster.height).into(),
            false,
        );

        let entry = match texture {
            Ok(texture) => NodeAppIconCacheEntry::Ready(NodeAppIconTexture {
                texture,
                width: raster.width,
                height: raster.height,
            }),
            Err(_) => NodeAppIconCacheEntry::Missing,
        };
        st.node_app_icon_cache.insert(app_id, entry);
    }

    Ok(())
}

fn resolve_app_icon_path(app_id: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    push_unique(&mut candidates, app_id.to_string());
    if let Some(tail) = app_id.rsplit(['.', '/']).next()
        && !tail.is_empty()
    {
        push_unique(&mut candidates, tail.to_string());
    }
    if let Some(icon_name) = desktop_entry_icon_name(app_id) {
        push_unique(&mut candidates, icon_name);
    }

    for candidate in candidates {
        if let Some(path) = find_best_icon_path(&candidate) {
            return Some(path);
        }
    }

    None
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn desktop_entry_icon_name(app_id: &str) -> Option<String> {
    for dir in desktop_entry_dirs() {
        let exact = dir.join(format!("{app_id}.desktop"));
        if exact.is_file()
            && let Some(icon_name) = parse_desktop_icon_name(&exact)
        {
            return Some(icon_name);
        }
    }

    let mut best_match = None;
    let app_id_lower = app_id.to_ascii_lowercase();
    for dir in desktop_entry_dirs() {
        walk_files(&dir, 2, &mut |path| {
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                return;
            }
            if best_match.is_some() {
                return;
            }

            let Some(entry) = parse_desktop_entry(path) else {
                return;
            };
            let stem_matches = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(app_id));
            let wm_class_matches = entry
                .startup_wm_class
                .as_deref()
                .is_some_and(|wm_class| wm_class.eq_ignore_ascii_case(&app_id_lower));

            if (stem_matches || wm_class_matches) && entry.icon.is_some() {
                best_match = entry.icon;
            }
        });
        if best_match.is_some() {
            break;
        }
    }
    best_match
}

#[derive(Default)]
struct DesktopEntry {
    icon: Option<String>,
    startup_wm_class: Option<String>,
}

fn parse_desktop_icon_name(path: &Path) -> Option<String> {
    parse_desktop_entry(path).and_then(|entry| entry.icon)
}

fn parse_desktop_entry(path: &Path) -> Option<DesktopEntry> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_desktop_entry = false;
    let mut entry = DesktopEntry::default();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_desktop_entry = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_desktop_entry {
            continue;
        }

        if let Some(value) = line.strip_prefix("Icon=") {
            entry.icon = Some(unescape_desktop_value(value));
        } else if let Some(value) = line.strip_prefix("StartupWMClass=") {
            entry.startup_wm_class = Some(unescape_desktop_value(value));
        }
    }

    Some(entry)
}

fn unescape_desktop_value(value: &str) -> String {
    value
        .replace("\\s", " ")
        .replace("\\n", " ")
        .replace("\\t", " ")
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | ' '))
        .to_string()
}

fn find_best_icon_path(icon_name: &str) -> Option<PathBuf> {
    let direct_path = PathBuf::from(icon_name);
    if direct_path.is_file() {
        return Some(direct_path);
    }

    let mut best: Option<(i32, PathBuf)> = None;
    for root in icon_search_roots() {
        walk_files(&root, ICON_WALK_MAX_DEPTH, &mut |path| {
            let Some(score) = icon_candidate_score(path, icon_name) else {
                return;
            };
            let replace = best.as_ref().is_none_or(|(best_score, best_path)| {
                score < *best_score || (score == *best_score && path < best_path.as_path())
            });
            if replace {
                best = Some((score, path.to_path_buf()));
            }
        });
    }

    best.map(|(_, path)| path)
}

fn icon_candidate_score(path: &Path, icon_name: &str) -> Option<i32> {
    let stem = path.file_stem()?.to_str()?;
    if stem != icon_name {
        return None;
    }

    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let format_score = match ext.as_str() {
        "svg" => 0,
        "png" => 40,
        "jpg" | "jpeg" => 60,
        _ => return None,
    };

    let size_score = icon_size_hint(path)
        .map(|size| (size as i32 - NODE_ICON_RASTER_PX as i32).abs())
        .unwrap_or(24);
    let theme_score = if path.to_string_lossy().contains("/hicolor/") {
        12
    } else {
        0
    };

    Some(format_score + size_score + theme_score)
}

fn icon_size_hint(path: &Path) -> Option<u32> {
    for component in path.components() {
        let part = component.as_os_str().to_str()?;
        if part.eq_ignore_ascii_case("scalable") {
            return Some(NODE_ICON_RASTER_PX);
        }
        if let Some((w, h)) = part.split_once('x')
            && let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>())
        {
            return Some(w.min(h));
        }
    }
    None
}

fn desktop_entry_dirs() -> Vec<PathBuf> {
    data_roots()
        .into_iter()
        .map(|root| root.join("applications"))
        .filter(|path| path.is_dir())
        .collect()
}

fn icon_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in data_roots() {
        let icons = root.join("icons");
        if icons.is_dir() {
            roots.push(icons);
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        let legacy = home.join(".icons");
        if legacy.is_dir() {
            roots.push(legacy);
        }
    }
    let pixmaps = PathBuf::from("/usr/share/pixmaps");
    if pixmaps.is_dir() {
        roots.push(pixmaps);
    }
    roots
}

fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = env::var_os("XDG_DATA_HOME") {
        roots.push(PathBuf::from(home));
    } else if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share"));
    }

    let data_dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        if dir.trim().is_empty() {
            continue;
        }
        roots.push(PathBuf::from(dir));
    }

    roots
}

fn walk_files(root: &Path, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
    fn recurse(path: &Path, depth: usize, max_depth: usize, visit: &mut dyn FnMut(&Path)) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < max_depth {
                    recurse(&path, depth + 1, max_depth, visit);
                }
            } else {
                visit(&path);
            }
        }
    }

    if root.is_dir() {
        recurse(root, 0, max_depth, visit);
    }
}

fn load_icon_raster(path: &Path) -> Option<AppIconRaster> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())?;
    match ext.as_str() {
        "svg" => load_svg_icon(path),
        "png" | "jpg" | "jpeg" => load_raster_icon(path),
        _ => None,
    }
}

fn load_raster_icon(path: &Path) -> Option<AppIconRaster> {
    let image = image::open(path).ok()?.to_rgba8();
    let normalized = normalize_icon_canvas(image);
    Some(AppIconRaster {
        width: normalized.width() as i32,
        height: normalized.height() as i32,
        pixels_rgba: normalized.into_vec(),
    })
}

fn normalize_icon_canvas(source: RgbaImage) -> RgbaImage {
    let (src_w, src_h) = source.dimensions();
    if src_w == 0 || src_h == 0 {
        return RgbaImage::new(NODE_ICON_RASTER_PX, NODE_ICON_RASTER_PX);
    }

    let resized = imageops::thumbnail(&source, NODE_ICON_RASTER_PX, NODE_ICON_RASTER_PX);
    let mut canvas = RgbaImage::new(NODE_ICON_RASTER_PX, NODE_ICON_RASTER_PX);
    let dx = ((NODE_ICON_RASTER_PX - resized.width()) / 2) as i64;
    let dy = ((NODE_ICON_RASTER_PX - resized.height()) / 2) as i64;
    imageops::overlay(&mut canvas, &resized, dx, dy);
    canvas
}

fn load_svg_icon(path: &Path) -> Option<AppIconRaster> {
    let mut options = usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    options.fontdb_mut().load_system_fonts();

    let data = fs::read(path).ok()?;
    let tree = usvg::Tree::from_data(&data, &options).ok()?;
    let svg_size = tree.size().to_int_size();
    if svg_size.width() == 0 || svg_size.height() == 0 {
        return None;
    }

    let mut pixmap = tiny_skia::Pixmap::new(NODE_ICON_RASTER_PX, NODE_ICON_RASTER_PX)?;
    let scale_x = NODE_ICON_RASTER_PX as f32 / svg_size.width() as f32;
    let scale_y = NODE_ICON_RASTER_PX as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (NODE_ICON_RASTER_PX as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (NODE_ICON_RASTER_PX as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut pixels = pixmap.data().to_vec();
    unpremultiply_rgba(&mut pixels);
    Some(AppIconRaster {
        width: NODE_ICON_RASTER_PX as i32,
        height: NODE_ICON_RASTER_PX as i32,
        pixels_rgba: pixels,
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
