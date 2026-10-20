//! Exercise the real Smithay shader variants and framebuffer blending.
use super::*;
use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen};

fn client_pixels(bgr: bool, alpha: u8, premultiply: bool) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..48 {
        for x in 0..64 {
            // Opaque page, dark text-like marks, and a coloured control track.
            let rgb: [u8; 3] = if (8..56).contains(&x) && (9..14).contains(&y) {
                [17, 35, 51]
            } else if (8..44).contains(&x) && (28..33).contains(&y) {
                [40, 105, 205]
            } else {
                [238, 242, 247]
            };
            let [r, g, b] = rgb.map(|c| {
                if premultiply {
                    (u16::from(c) * u16::from(alpha) / 255) as u8
                } else {
                    c
                }
            });
            let pixel = if bgr {
                [b, g, r, alpha]
            } else {
                [r, g, b, alpha]
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    pixels
}

#[allow(clippy::too_many_arguments)]
fn render(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    decorations: &mut WindowDecorationRenderer,
    texture: &GlesTexture,
    background: Color32F,
    opacity: f32,
    radius: f32,
    transform: Transform,
) -> Vec<u8> {
    let size = (96, 80).into();
    let rect = Rectangle::new((16, 16).into(), (64, 48).into());
    let base = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        rect.loc.to_f64(),
        texture.clone(),
        1,
        transform,
        Some(opacity),
        None,
        Some(rect.size.to_logical(1)),
        None,
        Kind::Unspecified,
    );
    let element = decorations
        .texture_element(renderer, base, texture.clone(), rect, radius)
        .unwrap();
    let mut fbo = renderer.bind(target).unwrap();
    {
        let mut frame = renderer.render(&mut fbo, size, Transform::Normal).unwrap();
        frame
            .clear(background, &[Rectangle::from_size(size)])
            .unwrap();
        element
            .draw(
                &mut frame,
                element.src(),
                rect,
                &[Rectangle::from_size(rect.size)],
                &[],
                None,
            )
            .unwrap();
        let _ = frame.finish().unwrap();
    }
    let mapping = renderer
        .copy_framebuffer(
            &fbo,
            Rectangle::from_size(size.to_logical(1).to_buffer(1, Transform::Normal)),
            Fourcc::Abgr8888,
        )
        .unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn rounded_xrgb_ignores_padding_and_matches_opaque_argb_over_backdrops() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut decorations = WindowDecorationRenderer::default();
    let mut target = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        &mut renderer,
        Fourcc::Abgr8888,
        (96, 80).into(),
    )
    .unwrap();
    for (xrgb, argb, bgr) in [
        (Fourcc::Xrgb8888, Fourcc::Argb8888, true),
        (Fourcc::Xbgr8888, Fourcc::Abgr8888, false),
    ] {
        let reference = renderer
            .import_memory(
                &client_pixels(bgr, 255, false),
                argb,
                (64, 48).into(),
                false,
            )
            .unwrap();
        for padding in [0, 73, 255] {
            let client = renderer
                .import_memory(
                    &client_pixels(bgr, padding, false),
                    xrgb,
                    (64, 48).into(),
                    false,
                )
                .unwrap();
            for background in [
                Color32F::new(0.7, 0.1, 0.35, 1.0),
                Color32F::new(0.1, 0.6, 0.2, 1.0),
            ] {
                for opacity in [1.0, 0.55] {
                    for radius in [0.0, 8.0] {
                        for transform in [Transform::Normal, Transform::_90, Transform::Flipped] {
                            let expected = render(
                                &mut renderer,
                                &mut target,
                                &mut decorations,
                                &reference,
                                background,
                                opacity,
                                radius,
                                transform,
                            );
                            let actual = render(
                                &mut renderer,
                                &mut target,
                                &mut decorations,
                                &client,
                                background,
                                opacity,
                                radius,
                                transform,
                            );
                            let error = actual
                                .iter()
                                .zip(&expected)
                                .map(|(a, b)| a.abs_diff(*b))
                                .max()
                                .unwrap();
                            assert!(
                                error <= 1,
                                "{xrgb:?}, padding={padding}, opacity={opacity}, radius={radius}, {transform:?}: max error {error}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn rounded_argb_keeps_real_transparency_and_antialiased_corners() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut decorations = WindowDecorationRenderer::default();
    let mut target = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        &mut renderer,
        Fourcc::Abgr8888,
        (96, 80).into(),
    )
    .unwrap();
    let transparent = renderer
        .import_memory(
            &client_pixels(false, 128, true),
            Fourcc::Abgr8888,
            (64, 48).into(),
            false,
        )
        .unwrap();
    let opaque = renderer
        .import_memory(
            &client_pixels(false, 255, false),
            Fourcc::Abgr8888,
            (64, 48).into(),
            false,
        )
        .unwrap();
    let background = Color32F::new(0.1, 0.6, 0.2, 1.0);
    let actual = render(
        &mut renderer,
        &mut target,
        &mut decorations,
        &transparent,
        background,
        1.0,
        8.0,
        Transform::Normal,
    );
    let solid = render(
        &mut renderer,
        &mut target,
        &mut decorations,
        &opaque,
        background,
        1.0,
        8.0,
        Transform::Normal,
    );
    let pixel = |image: &[u8], x: usize, y: usize| -> [u8; 4] {
        image[(y * 96 + x) * 4..(y * 96 + x) * 4 + 4]
            .try_into()
            .unwrap()
    };
    assert_ne!(pixel(&actual, 20, 40), pixel(&solid, 20, 40));
    assert_eq!(pixel(&actual, 16, 16), pixel(&actual, 0, 0));
    let center = pixel(&actual, 20, 40);
    // Known source-over of the premultiplied light page onto the green backdrop.
    for (channel, expected) in center.iter().zip([131_u8, 197, 149, 255]) {
        assert!(
            channel.abs_diff(expected) <= 2,
            "unexpected source-over {center:?}"
        );
    }
}
