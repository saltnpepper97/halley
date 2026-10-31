use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::utils::Transform;
use xcursor::CursorTheme;
use xcursor::parser::{Image, parse_xcursor};

const FALLBACK_CURSOR_RGBA: &[u8] = include_bytes!("../../assets/cursor/default.rgba");

pub struct CursorFrame {
    pub buffer: MemoryRenderBuffer,
    pub metadata_bgra: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub scale: i32,
}

pub struct PreparedCursor {
    frames: Vec<Rc<CursorFrame>>,
    delays_ms: Vec<u32>,
    duration_ms: u64,
}

impl PreparedCursor {
    fn new(images: Vec<Image>, scale: i32) -> Self {
        let delays_ms = images.iter().map(|image| image.delay).collect::<Vec<_>>();
        let duration_ms = delays_ms.iter().map(|delay| u64::from(*delay)).sum();
        let frames = images
            .into_iter()
            .map(|image| Rc::new(prepare_frame(image, scale)))
            .collect();
        Self {
            frames,
            delays_ms,
            duration_ms,
        }
    }

    pub fn frame(&self, time: Duration) -> Rc<CursorFrame> {
        let (index, _) = animation_frame(time.as_millis(), &self.delays_ms, self.duration_ms);
        self.frames[index].clone()
    }

    pub fn next_frame_in(&self, time: Duration) -> Option<Duration> {
        let (_, delay_ms) = animation_frame(time.as_millis(), &self.delays_ms, self.duration_ms);
        delay_ms.map(Duration::from_millis)
    }

    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1 && self.duration_ms > 0
    }
}

pub fn load_prepared_cursor(
    theme_name: &str,
    theme: &CursorTheme,
    default_theme: &CursorTheme,
    icon: CursorIcon,
    base_size: u8,
    output_scale: i32,
) -> PreparedCursor {
    let requested_size = u32::from(base_size) * output_scale as u32;
    let images = load_icon_with_aliases(theme, icon)
        .or_else(|| {
            (theme_name != "default")
                .then(|| load_icon_with_aliases(default_theme, icon))
                .flatten()
        })
        .or_else(|| {
            (icon != CursorIcon::Default)
                .then(|| load_icon_with_aliases(theme, CursorIcon::Default))
                .flatten()
        })
        .or_else(|| load_icon_with_aliases(default_theme, CursorIcon::Default))
        .map(|images| nearest_images(images, requested_size))
        .unwrap_or_else(|| {
            eventline::warn!(
                "cursor: theme {theme_name:?} has no usable {:?} cursor; using emergency fallback",
                icon
            );
            vec![fallback_image()]
        });

    PreparedCursor::new(images, output_scale)
}

fn load_icon_with_aliases(theme: &CursorTheme, icon: CursorIcon) -> Option<Vec<Image>> {
    std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .find_map(|name| load_icon(theme, name))
}

fn load_icon(theme: &CursorTheme, name: &str) -> Option<Vec<Image>> {
    let path = theme.load_icon(name)?;
    let data = fs::read(path).ok()?;
    parse_xcursor(&data).filter(|images| !images.is_empty())
}

fn nearest_images(images: Vec<Image>, requested_size: u32) -> Vec<Image> {
    let Some(nearest) = images
        .iter()
        .min_by_key(|image| requested_size.abs_diff(image.size))
    else {
        return Vec::new();
    };
    let nominal_size = nearest.size;
    images
        .into_iter()
        .filter(|image| image.size == nominal_size)
        .collect()
}

fn prepare_frame(image: Image, scale: i32) -> CursorFrame {
    let buffer = MemoryRenderBuffer::from_slice(
        &image.pixels_rgba,
        Fourcc::Abgr8888,
        (image.width as i32, image.height as i32),
        scale,
        Transform::Normal,
        None,
    );
    let mut metadata_bgra = image.pixels_rgba;
    for pixel in metadata_bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    CursorFrame {
        buffer,
        metadata_bgra: metadata_bgra.into(),
        width: image.width,
        height: image.height,
        hotspot_x: image.xhot as i32,
        hotspot_y: image.yhot as i32,
        scale,
    }
}

fn animation_frame(time_ms: u128, delays_ms: &[u32], duration_ms: u64) -> (usize, Option<u64>) {
    if delays_ms.len() <= 1 || duration_ms == 0 {
        return (0, None);
    }
    let mut remaining = (time_ms % u128::from(duration_ms)) as u64;
    for (index, delay) in delays_ms.iter().copied().enumerate() {
        let delay = u64::from(delay);
        if remaining < delay {
            return (index, Some(delay - remaining));
        }
        remaining = remaining.saturating_sub(delay);
    }
    (0, Some(duration_ms))
}

fn fallback_image() -> Image {
    Image {
        size: 32,
        width: 64,
        height: 64,
        xhot: 1,
        yhot: 1,
        delay: 0,
        pixels_rgba: FALLBACK_CURSOR_RGBA.to_vec(),
        pixels_argb: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: u32, width: u32, delay: u32) -> Image {
        Image {
            size,
            width,
            height: width,
            xhot: 1,
            yhot: 2,
            delay,
            pixels_rgba: vec![0; width as usize * width as usize * 4],
            pixels_argb: Vec::new(),
        }
    }

    #[test]
    fn nearest_nominal_size_keeps_frames_without_mixing_variants() {
        // The smaller nominal size may have either smaller or identical pixel dimensions.
        for dimensions in [24, 32] {
            let selected = nearest_images(
                vec![
                    image(24, dimensions, 10),
                    image(32, 32, 20),
                    image(32, 32, 30),
                ],
                30,
            );
            assert_eq!(selected.len(), 2, "dimensions={dimensions}");
            assert!(
                selected.iter().all(|image| image.size == 32),
                "dimensions={dimensions}"
            );
        }
    }

    #[test]
    fn animation_wraps_across_frame_delays() {
        let delays = [10, 20, 30];
        assert_eq!(animation_frame(0, &delays, 60), (0, Some(10)));
        assert_eq!(animation_frame(10, &delays, 60), (1, Some(20)));
        assert_eq!(animation_frame(29, &delays, 60), (1, Some(1)));
        assert_eq!(animation_frame(30, &delays, 60), (2, Some(30)));
        assert_eq!(animation_frame(60, &delays, 60), (0, Some(10)));
    }

    #[test]
    fn zero_delay_cursor_is_static() {
        assert_eq!(animation_frame(500, &[0, 0], 0), (0, None));
    }

    #[test]
    fn animation_time_does_not_wrap_after_u32_milliseconds() {
        let time = u128::from(u32::MAX) + 25;
        assert_eq!(animation_frame(time, &[10, 20], 30), (1, Some(20)));
    }

    #[test]
    fn emergency_fallback_has_valid_pixels_and_hotspot() {
        let image = fallback_image();
        assert_eq!(image.pixels_rgba.len(), 64 * 64 * 4);
        assert_eq!((image.xhot, image.yhot), (1, 1));
    }
}
