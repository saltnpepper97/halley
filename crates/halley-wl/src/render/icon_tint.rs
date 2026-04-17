use image::RgbaImage;

pub(super) fn tint_alpha_mask_image(image: &mut RgbaImage, rgba: [u8; 4]) {
    for pixel in image.pixels_mut() {
        let alpha = pixel[3] as u16;
        if alpha == 0 {
            continue;
        }

        // tiny-skia rasterizes SVGs into premultiplied RGBA, and Smithay composites imported
        // textures with premultiplied-alpha blending. Keep the recolored raster premultiplied.
        let tinted_alpha = ((alpha * rgba[3] as u16) / 255) as u8;
        pixel[0] = ((rgba[0] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[1] = ((rgba[1] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[2] = ((rgba[2] as u16 * tinted_alpha as u16) / 255) as u8;
        pixel[3] = tinted_alpha;
    }
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::tint_alpha_mask_image;

    #[test]
    fn tint_keeps_semi_transparent_pixels_premultiplied() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([240, 240, 240, 128]));

        tint_alpha_mask_image(&mut image, [200, 100, 50, 255]);

        assert_eq!(image.get_pixel(0, 0).0, [100, 50, 25, 128]);
    }

    #[test]
    fn tint_scales_alpha_before_premultiplying_rgb() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([12, 34, 56, 128]));

        tint_alpha_mask_image(&mut image, [200, 100, 50, 128]);

        assert_eq!(image.get_pixel(0, 0).0, [50, 25, 12, 64]);
    }

    #[test]
    fn tint_skips_fully_transparent_pixels() {
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([9, 8, 7, 0]));

        tint_alpha_mask_image(&mut image, [200, 100, 50, 255]);

        assert_eq!(image.get_pixel(0, 0).0, [9, 8, 7, 0]);
    }
}
