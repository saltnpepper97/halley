use super::*;
use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen};
use smithay::utils::Size;

use crate::render::effects::backdrop_blur::{BackdropBlurRenderer, BlurIdentity, BlurPatch};
use crate::render::effects::shadow::ShadowRenderer;
use crate::render::scene::SceneElement;

// A shader that finishes at the unmodified texture is a direct assertion of
// the public endpoint contract, independent of any particular wave formula.
const IDENTITY_OPEN: &str = r#"
vec4 open_color(vec3 coords_geo, vec3 size_geo) {
    if (coords_geo.x < 0.0 || coords_geo.y < 0.0 || coords_geo.x >= 1.0 || coords_geo.y >= 1.0)
        return vec4(0.0);
    return texture2D(tex, coords_geo.xy * halley_tex_scale + halley_tex_offset)
        * halley_clamped_progress;
}
"#;

fn solid(rect: Rectangle<i32, Physical>, color: Color32F, tick: usize) -> SceneElement {
    SceneElement::Border(SolidColorRenderElement::new(
        Id::new(),
        rect,
        tick,
        color,
        Kind::Unspecified,
    ))
}

fn window_scene(
    renderer: &mut GlesRenderer,
    effects: &mut BackdropBlurRenderer,
    shadows: &mut ShadowRenderer,
    size: Size<i32, Physical>,
    tick: usize,
    transform: Transform,
    noise: f32,
) -> Vec<SceneElement> {
    effects.begin_scene("opening-test");
    let location = if tick == 3 {
        (-12, 34)
    } else {
        (63 + tick as i32 * 3, 57)
    };
    let rect = Rectangle::new(location.into(), (108 + tick as i32 * 2, 79).into());
    let title = Rectangle::new(
        (rect.loc.x, rect.loc.y - 23).into(),
        (rect.size.w, 23).into(),
    );
    let mut decorations = crate::render::window_decoration::WindowDecorationRenderer::default();
    let mut scene = vec![
        solid(
            Rectangle::new((rect.loc.x + 11, rect.loc.y + 12).into(), (49, 6).into()),
            Color32F::new(0.75, 0.9, 1.0, 1.0),
            tick,
        ),
        // Rule opacity applies to client pixels, but not the titlebar/shadow.
        {
            // A viewport-cropped, scaled, alpha-bearing client texture catches
            // orientation and source mapping errors that solid-color fixtures
            // cannot reveal. Alternate imported texture Y orientations too.
            let mut data = Vec::new();
            for y in 0..29_u8 {
                for x in 0..37_u8 {
                    data.extend_from_slice(&[20 + x * 2, 15 + y * 3, 45 + x, 180 + y]);
                }
            }
            let texture = renderer
                .import_memory(
                    &data,
                    Fourcc::Abgr8888,
                    (37, 29).into(),
                    !tick.is_multiple_of(2),
                )
                .unwrap();
            let base = TextureRenderElement::from_static_texture(
                Id::new(),
                renderer.context_id(),
                rect.loc.to_f64(),
                texture.clone(),
                1,
                Transform::Normal,
                Some(0.55),
                Some(Rectangle::new((4.0, 3.0).into(), (28.0, 19.0).into())),
                Some(rect.size.to_logical(1)),
                None,
                Kind::Unspecified,
            );
            SceneElement::RoundedTexture(
                decorations
                    .texture_element_with_radii(
                        renderer,
                        base,
                        texture,
                        rect,
                        crate::render::window_decoration::CornerRadii::all(9.0),
                        (1.0, 1.0, 1.0, 1.0),
                    )
                    .unwrap(),
            )
        },
        SceneElement::RoundedTexture(
            decorations
                .tint_element_with_radii(
                    renderer,
                    Id::new(),
                    title,
                    crate::render::window_decoration::CornerRadii {
                        top: 9.0,
                        bottom: 0.0,
                    },
                    Color32F::new(0.07, 0.14, 0.17 + tick as f32 * 0.03, 1.0),
                    1.0,
                )
                .unwrap(),
        ),
    ];
    if tick != 4 {
        let blur = effects
            .blur_element(
                renderer,
                "opening-test",
                BlurIdentity::Overlay("window"),
                size.to_logical(1),
                vec![BlurPatch {
                    rect,
                    radius: 0.0,
                    alpha: 0.85,
                    clip: Some((
                        rect,
                        crate::render::window_decoration::CornerRadii::all(9.0),
                    )),
                }],
                halley_config::Blur {
                    noise,
                    ..Default::default()
                },
                tick as u64,
                transform,
            )
            .unwrap()
            .unwrap();
        scene.push(SceneElement::BackdropBlur(blur));
    }
    let config = halley_config::ShadowLayer {
        enabled: tick != 2,
        color: halley_config::ShadowColor {
            r: 0.07,
            g: 0.14,
            b: 0.17,
            a: 0.35,
        },
        blur_radius: 8.0,
        offset_y: 5.0,
        ..halley_config::Shadows::default().window
    };
    if let Some(shadow) = shadows
        .element(
            renderer,
            "opening-test",
            rect.merge(title),
            9.0,
            1.0,
            config,
        )
        .unwrap()
    {
        scene.push(SceneElement::Shadow(shadow));
    }
    scene
}

fn background(size: Size<i32, Physical>, tick: usize) -> Vec<SceneElement> {
    (0..24)
        .map(|i| {
            let x = i % 6 * size.w / 6;
            let y = i / 6 * size.h / 4;
            solid(
                Rectangle::new(
                    (x, y).into(),
                    (
                        ((i % 6 + 1) * size.w / 6) - x,
                        ((i / 6 + 1) * size.h / 4) - y,
                    )
                        .into(),
                ),
                Color32F::new(
                    (i % 3) as f32 * 0.3,
                    (i % 4) as f32 * 0.2,
                    ((i + tick as i32) % 5) as f32 * 0.15,
                    1.0,
                ),
                tick,
            )
        })
        .collect()
}

fn pixels(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    mode: Size<i32, Physical>,
    transform: Transform,
    scene: &[SceneElement],
) -> Vec<u8> {
    let mut fbo = renderer.bind(target).unwrap();
    {
        let mut frame = renderer.render(&mut fbo, mode, transform).unwrap();
        let visible = Rectangle::from_size(transform.transform_size(mode));
        frame.clear(Color32F::BLACK, &[visible]).unwrap();
        let cache = UserDataMap::new();
        for element in scene.iter().rev() {
            let geometry = element.geometry(1.0.into());
            let Some(visible) = geometry.intersection(visible) else {
                continue;
            };
            if element.is_framebuffer_effect() {
                element
                    .capture_framebuffer(&mut frame, element.src(), geometry, &cache)
                    .unwrap();
            }
            element
                .draw(
                    &mut frame,
                    element.src(),
                    geometry,
                    &[Rectangle::new(visible.loc - geometry.loc, visible.size)],
                    &[],
                    Some(&cache),
                )
                .unwrap();
        }
        let _ = frame.finish().unwrap();
    }
    let mapping = renderer
        .copy_framebuffer(
            &fbo,
            Rectangle::from_size(mode.to_logical(1).to_buffer(1, Transform::Normal)),
            Fourcc::Abgr8888,
        )
        .unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn titlebar_and_content_join_has_no_backdrop_seam() {
    use crate::render::window_decoration::{CornerRadii, WindowDecorationRenderer};

    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size: Size<i32, Physical> = (64, 64).into();
    let mut target = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        &mut renderer,
        Fourcc::Abgr8888,
        size.to_logical(1).to_buffer(1, Transform::Normal),
    )
    .unwrap();
    let title = Rectangle::new((8, 8).into(), (48, 12).into());
    let content = Rectangle::new((8, 20).into(), (48, 36).into());
    let mut decorations = WindowDecorationRenderer::default();
    let mut effects = BackdropBlurRenderer::default();

    for blurred in [false, true] {
        effects.begin_scene("titlebar-join");
        let mut scene = vec![
            SceneElement::RoundedTexture(
                decorations
                    .tint_element_with_radii(
                        &mut renderer,
                        Id::new(),
                        title,
                        CornerRadii::top(8.0),
                        Color32F::new(0.0, 0.8, 0.0, 1.0),
                        1.0,
                    )
                    .unwrap(),
            ),
            SceneElement::RoundedTexture(
                decorations
                    .tint_element_with_radii(
                        &mut renderer,
                        Id::new(),
                        content,
                        CornerRadii::bottom(8.0),
                        Color32F::new(0.0, 0.0, 0.8, 1.0),
                        0.9,
                    )
                    .unwrap(),
            ),
        ];
        if blurred {
            scene.push(SceneElement::BackdropBlur(
                effects
                    .blur_element(
                        &mut renderer,
                        "titlebar-join",
                        BlurIdentity::Overlay("join"),
                        size.to_logical(1),
                        vec![BlurPatch {
                            rect: content,
                            radius: 0.0,
                            alpha: 0.9,
                            clip: Some((content, CornerRadii::bottom(8.0))),
                        }],
                        halley_config::Blur {
                            noise: 0.0,
                            ..Default::default()
                        },
                        0,
                        Transform::Normal,
                    )
                    .unwrap()
                    .unwrap(),
            ));
        }
        scene.push(solid(
            Rectangle::from_size(size),
            Color32F::new(0.8, 0.1, 0.1, 1.0),
            0,
        ));
        let image = pixels(&mut renderer, &mut target, size, Transform::Normal, &scene);
        let at = |x: usize, y: usize| &image[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(32, 19), at(32, 18), "titlebar edge, blur={blurred}");
        assert_eq!(at(32, 20), at(32, 21), "content edge, blur={blurred}");
        assert_ne!(at(8, 8), at(32, 8), "top corner must remain rounded");
        assert_ne!(at(8, 55), at(32, 55), "bottom corner must remain rounded");
    }
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn opening_endpoint_matches_live_blur_shadow_chrome_and_opacity() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut effects = BackdropBlurRenderer::default();
    let mut shadows = ShadowRenderer::default();
    let program = compile_program(&mut renderer, ShaderKind::Open, IDENTITY_OPEN).unwrap();
    let mut shaders = WindowAnimationShaders {
        open: Some(CompiledProgram {
            key: "test".into(),
            context: renderer.context_id(),
            program,
        }),
        ..Default::default()
    };
    let id = Id::new();
    let logical_mode: Size<i32, Physical> = (257, 193).into();
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mode = logical_mode.to_f64().upscale(scale).to_i32_round();
        let mut target = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            &mut renderer,
            Fourcc::Abgr8888,
            mode.to_logical(1).to_buffer(1, Transform::Normal),
        )
        .unwrap();
        for transform in [
            Transform::Normal,
            Transform::Flipped180,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped270,
        ] {
            let size = transform.transform_size(logical_mode);
            effects.set_display_scale("opening-test", scale, transform.transform_size(mode));
            for noise in [0.0_f32, 0.012] {
                for tick in 0..5 {
                    let mut native = window_scene(
                        &mut renderer,
                        &mut effects,
                        &mut shadows,
                        size,
                        tick,
                        transform,
                        noise,
                    );
                    native.extend(background(size, tick));
                    let native = native
                        .into_iter()
                        .map(|element| crate::render::display_scale::element(element, scale, false))
                        .collect::<Vec<_>>();
                    let reference = pixels(&mut renderer, &mut target, mode, transform, &native);
                    let mut opening = window_scene(
                        &mut renderer,
                        &mut effects,
                        &mut shadows,
                        size,
                        tick,
                        transform,
                        noise,
                    );
                    let shader = shaders
                        .open_scene_element(&renderer, id.clone(), &mut opening, 1.0, 1.0, 0.37)
                        .unwrap();
                    assert!(opening.is_empty());
                    opening.push(SceneElement::WindowOpen(shader));
                    opening.extend(background(size, tick));
                    let opening = opening
                        .into_iter()
                        .map(|element| crate::render::display_scale::element(element, scale, false))
                        .collect::<Vec<_>>();
                    let actual = pixels(&mut renderer, &mut target, mode, transform, &opening);
                    let maximum = actual
                        .iter()
                        .zip(&reference)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        maximum <= (noise * 510.0).ceil() as u8 + 2,
                        "endpoint mismatch: scale={scale}, {transform:?}, tick {tick}, noise {noise}, max {maximum}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn opening_zero_progress_hides_blur_and_shadow_together_with_client() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size: Size<i32, Physical> = (257, 193).into();
    let mut target = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        &mut renderer,
        Fourcc::Abgr8888,
        size.to_logical(1).to_buffer(1, Transform::Normal),
    )
    .unwrap();
    let mut effects = BackdropBlurRenderer::default();
    let mut shadows = ShadowRenderer::default();
    let mut native = window_scene(
        &mut renderer,
        &mut effects,
        &mut shadows,
        size,
        0,
        Transform::Normal,
        0.012,
    );
    let program = compile_program(&mut renderer, ShaderKind::Open, IDENTITY_OPEN).unwrap();
    let mut shaders = WindowAnimationShaders {
        open: Some(CompiledProgram {
            key: "test".into(),
            context: renderer.context_id(),
            program,
        }),
        ..Default::default()
    };
    let shader = shaders
        .open_scene_element(&renderer, Id::new(), &mut native, 0.0, 0.0, 0.37)
        .unwrap();
    let mut opening = vec![SceneElement::WindowOpen(shader)];
    opening.extend(background(size, 0));
    let actual = pixels(
        &mut renderer,
        &mut target,
        size,
        Transform::Normal,
        &opening,
    );
    let reference = pixels(
        &mut renderer,
        &mut target,
        size,
        Transform::Normal,
        &background(size, 0),
    );
    assert_eq!(
        actual, reference,
        "blur or shadow leaked outside the opening silhouette"
    );
}
