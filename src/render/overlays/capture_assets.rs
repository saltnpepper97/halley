use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::sync::{Mutex, OnceLock};

use resvg::{tiny_skia, usvg};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;

const RASTER_SIZE: u32 = 48;
pub const DISPLAY_SIZE: i32 = 42;
const REGION_SVG: &[u8] = include_bytes!("../../../assets/screenshot/region.svg");
const MONITOR_SVG: &[u8] = include_bytes!("../../../assets/screenshot/screen.svg");
const WINDOW_SVG: &[u8] = include_bytes!("../../../assets/screenshot/window.svg");

type IconSet = Result<[MemoryRenderBuffer; 3], String>;
type IconCache = Mutex<HashMap<[u8; 3], IconSet>>;

static ICONS: OnceLock<IconCache> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureIcon {
    Region,
    Monitor,
    Window,
}

impl CaptureIcon {
    fn index(self) -> usize {
        match self {
            Self::Region => 0,
            Self::Monitor => 1,
            Self::Window => 2,
        }
    }
}

pub fn buffer(rgb: [u8; 3], icon: CaptureIcon) -> Result<MemoryRenderBuffer, Box<dyn Error>> {
    let cache = ICONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("capture icon cache poisoned");
    let result = cache.entry(rgb).or_insert_with(|| {
        let region = rasterize_svg(REGION_SVG, rgb)
            .ok_or_else(|| "could not rasterize Region capture icon".to_string())?;
        let monitor = rasterize_svg(MONITOR_SVG, rgb)
            .ok_or_else(|| "could not rasterize Monitor capture icon".to_string())?;
        let window = rasterize_svg(WINDOW_SVG, rgb)
            .ok_or_else(|| "could not rasterize Window capture icon".to_string())?;
        Ok([
            memory_buffer(&region),
            memory_buffer(&monitor),
            memory_buffer(&window),
        ])
    });
    match result {
        Ok(icons) => Ok(icons[icon.index()].clone()),
        Err(message) => Err(io::Error::other(message.clone()).into()),
    }
}

fn memory_buffer(pixels: &[u8]) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        pixels,
        Fourcc::Abgr8888,
        (RASTER_SIZE as i32, RASTER_SIZE as i32),
        1,
        Transform::Normal,
        None,
    )
}

fn rasterize_svg(svg: &[u8], rgb: [u8; 3]) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let svg_size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(RASTER_SIZE, RASTER_SIZE)?;
    let scale_x = RASTER_SIZE as f32 / svg_size.width() as f32;
    let scale_y = RASTER_SIZE as f32 / svg_size.height() as f32;
    let scale = scale_x.min(scale_y);
    let dx = (RASTER_SIZE as f32 - svg_size.width() as f32 * scale) * 0.5;
    let dy = (RASTER_SIZE as f32 - svg_size.height() as f32 * scale) * 0.5;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        pixel[0] = ((u16::from(rgb[0]) * u16::from(alpha)) / 255) as u8;
        pixel[1] = ((u16::from(rgb[1]) * u16::from(alpha)) / 255) as u8;
        pixel[2] = ((u16::from(rgb[2]) * u16::from(alpha)) / 255) as u8;
    }
    Some(pixels)
}
