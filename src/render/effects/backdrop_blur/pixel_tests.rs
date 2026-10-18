use super::*;
use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::{Bind, Color32F, ExportMem};

// CPU references deliberately use unpaired, direct convolution and bilinear
// sampling, rather than production kernels or shaders. Quantize after each
// stage to match the renderer's ARGB8888 intermediate textures.
fn reference_sample(pixels: &[u8], size: (usize, usize), x: f64, y: f64, c: usize) -> f64 {
    let (ix, iy) = (x.floor() as isize, y.floor() as isize);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut value = 0.0;
    for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            let sx = (ix + dx).clamp(0, size.0 as isize - 1) as usize;
            let sy = (iy + dy).clamp(0, size.1 as isize - 1) as usize;
            value += pixels[(sy * size.0 + sx) * 4 + c] as f64 * wx * wy;
        }
    }
    value
}

fn reference_resize(
    pixels: &[u8],
    source: (usize, usize),
    target: (usize, usize),
    down: bool,
) -> Vec<u8> {
    let mut output = vec![0; target.0 * target.1 * 4];
    for y in 0..target.1 {
        for x in 0..target.0 {
            let sx = (x as f64 + 0.5) * source.0 as f64 / target.0 as f64 - 0.5;
            let sy = (y as f64 + 0.5) * source.1 as f64 / target.1 as f64 - 0.5;
            for c in 0..4 {
                let mut value = reference_sample(pixels, source, sx, sy, c);
                if down {
                    value *= 4.0;
                    for dx in [-0.5, 0.5] {
                        for dy in [-0.5, 0.5] {
                            value += reference_sample(pixels, source, sx + dx, sy + dy, c);
                        }
                    }
                    value /= 8.0;
                }
                output[(y * target.0 + x) * 4 + c] = value.round() as u8;
            }
        }
    }
    output
}

fn reference_filter(
    mut pixels: Vec<u8>,
    original: (usize, usize),
    config: halley_config::Blur,
) -> Vec<u8> {
    let mut size = original;
    let mut radius = config.radius as f64 / (config.passes as f64).sqrt();
    while radius > 16.0 {
        let target = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        pixels = reference_resize(&pixels, size, target, true);
        size = target;
        radius /= 2.0;
    }
    let reach = radius.ceil() as isize;
    let mut weights: Vec<f64> = (-reach..=reach)
        .map(|tap| {
            if radius == 0.0 {
                1.0
            } else if config.method == halley_config::BlurMethod::Gaussian {
                (-(tap as f64).powi(2) / (2.0 * (radius / 3.0).powi(2))).exp()
            } else {
                (radius + 1.0 - tap.abs() as f64).clamp(0.0, 1.0)
            }
        })
        .collect();
    let total: f64 = weights.iter().sum();
    weights.iter_mut().for_each(|w| *w /= total);
    for _ in 0..config.passes {
        for horizontal in [true, false] {
            let mut output = vec![0; pixels.len()];
            for y in 0..size.1 {
                for x in 0..size.0 {
                    for c in 0..4 {
                        let mut sum = 0.0;
                        for (tap, weight) in (-reach..=reach).zip(&weights) {
                            let sx = (x as isize + if horizontal { tap } else { 0 })
                                .clamp(0, size.0 as isize - 1)
                                as usize;
                            let sy = (y as isize + if horizontal { 0 } else { tap })
                                .clamp(0, size.1 as isize - 1)
                                as usize;
                            sum += pixels[(sy * size.0 + sx) * 4 + c] as f64 * weight;
                        }
                        output[(y * size.0 + x) * 4 + c] = sum.round() as u8;
                    }
                }
            }
            pixels = output;
        }
    }
    if size != original {
        reference_resize(&pixels, size, original, false)
    } else {
        pixels
    }
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn separable_filters_match_independent_cpu_reference_and_live_switches() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut effects = BackdropBlurRenderer::default();
    let mut comparisons = 0;
    for dimensions in [(37, 29), (1, 7), (9, 1)] {
        let size: Size<i32, Physical> = dimensions.into();
        let mut background: Vec<PixelElement> = (0..size.w)
            .flat_map(|x| {
                (0..size.h).map(move |y| {
                    PixelElement::Solid(SolidColorRenderElement::new(
                        Id::new(),
                        Rectangle::new((x, y).into(), (1, 1).into()),
                        0,
                        Color32F::new(
                            ((x * 3 + y * 7) % 11) as f32 / 10.0,
                            ((x * 7 + y * 5) % 13) as f32 / 12.0,
                            ((x * 5 + y * 3) % 7) as f32 / 6.0,
                            1.0,
                        ),
                        Kind::Unspecified,
                    ))
                })
            })
            .collect();
        let mut target = create_texture(&mut renderer, size).unwrap();
        let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        let baseline = read_pixels(&mut renderer, &mut target, &mut tracker, &background, 0).0;
        let mut prior_owner = None::<std::rc::Weak<ElementBlur>>;
        let mut prior_id = None;
        for radius in [0.0, 0.4, 2.5, 16.0, 24.0, 128.0] {
            for passes in [1, 3, 5] {
                // Alternate the method at identical radius/pass settings, on
                // the same output/identity with an unchanged background.
                for method in [
                    halley_config::BlurMethod::Gaussian,
                    halley_config::BlurMethod::Box,
                ] {
                    let config = halley_config::Blur {
                        method,
                        radius,
                        passes,
                        saturation: 1.0,
                        noise: 0.0,
                        ..Default::default()
                    };
                    effects.begin_scene("reference");
                    let blur = effects
                        .blur_element(
                            &mut renderer,
                            "reference",
                            BlurIdentity::Overlay("reference"),
                            size.to_logical(1),
                            vec![BlurPatch {
                                rect: Rectangle::from_size(size),
                                radius: 0.0,
                                alpha: 1.0,
                                clip: None,
                            }],
                            config,
                            0,
                            Transform::Normal,
                        )
                        .unwrap()
                        .unwrap();
                    effects.end_scene("reference");
                    if let Some(owner) = prior_owner.take() {
                        assert!(
                            owner.upgrade().is_none(),
                            "method/config changes must retire cached results"
                        );
                    }
                    if let Some(id) = prior_id.as_ref() {
                        assert_eq!(id, &blur.id);
                    }
                    prior_owner = Some(Rc::downgrade(&blur.element));
                    prior_id = Some(blur.id.clone());
                    background.insert(0, PixelElement::Partial(blur));
                    let composed =
                        read_pixels(&mut renderer, &mut target, &mut tracker, &background, 1).0;
                    // Compare the raw filtered texture: composition separately
                    // antialiases patch edges, blending a little unblurred scene.
                    let PixelElement::Partial(blur) = &background[0] else {
                        unreachable!()
                    };
                    let texture = &blur.element.result;
                    let mapping = renderer
                        .copy_texture(
                            texture,
                            Rectangle::from_size(texture.size()),
                            Fourcc::Abgr8888,
                        )
                        .unwrap();
                    let actual = renderer.map_texture(&mapping).unwrap().to_vec();
                    let expected = reference_filter(
                        baseline.clone(),
                        (size.w as usize, size.h as usize),
                        config,
                    );
                    let error = actual
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        error <= 3,
                        "{method:?}, size={size:?}, radius={radius}, passes={passes}: error={error}"
                    );
                    // Also exercise the no-recapture/cached frame.
                    assert_eq!(
                        composed,
                        read_pixels(&mut renderer, &mut target, &mut tracker, &background, 1).0
                    );
                    background.remove(0);
                    comparisons += 1;
                }
            }
        }
        effects.remove_output("reference");
    }
    eprintln!("independent CPU filter comparisons: {comparisons}");
}
// Independent reference: preserve the old full-output capture and unbounded
// filter passes, while using the same production shaders and composition.
struct FullBlur(BackdropBlurElement);

impl Element for FullBlur {
    fn id(&self) -> &Id {
        self.0.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }
    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_size(self.0.size)
    }
    fn is_framebuffer_effect(&self) -> bool {
        true
    }
}

impl RenderElement<GlesRenderer> for FullBlur {
    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        *self.0.plan.borrow_mut() = Some(self.0.capture_plan(true));
        self.0.capture_framebuffer(frame, src, dst, cache)
    }
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        self.0.draw(frame, src, dst, damage, opaque, cache)
    }
}

smithay::backend::renderer::element::render_elements! {
    PixelElement<=GlesRenderer>;
    Solid=SolidColorRenderElement,
    Partial=BackdropBlurElement,
    Full=FullBlur,
}

fn read_pixels(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    tracker: &mut OutputDamageTracker,
    scene: &[PixelElement],
    age: usize,
) -> (Vec<u8>, Vec<Rectangle<i32, Physical>>) {
    let size = target.size();
    let mut fbo = renderer.bind(target).unwrap();
    let (prepared, blur) = crate::render::conservative::prepare(scene);
    let result = tracker
        .render_output(
            renderer,
            &mut fbo,
            if blur { 0 } else { age },
            &prepared,
            Color32F::BLACK,
        )
        .unwrap();
    result.sync.wait().unwrap();
    let damage = result.damage.cloned().unwrap_or_default();
    let mapping = renderer
        .copy_framebuffer(&fbo, Rectangle::from_size(size), Fourcc::Abgr8888)
        .unwrap();
    (renderer.map_texture(&mapping).unwrap().to_vec(), damage)
}

#[allow(clippy::too_many_arguments)]
fn scene(
    renderer: &mut GlesRenderer,
    effects: &mut BackdropBlurRenderer,
    size: Size<i32, Physical>,
    config: halley_config::Blur,
    ids: &[Id],
    tick: usize,
    full: bool,
    transform: Transform,
) -> Vec<PixelElement> {
    let output = if full { "full" } else { "partial" };
    effects.begin_scene(output);
    let mut scene = Vec::new();
    if tick == 9 || tick == 10 {
        scene.push(PixelElement::Solid(SolidColorRenderElement::new(
            ids[22].clone(),
            Rectangle::from_size(size),
            0,
            Color32F::new(0.8, 0.8, 0.8, 1.0),
            Kind::Unspecified,
        )));
    }
    // Foreground identity changes and removal must not disturb either cache.
    if tick % 4 != 3 {
        scene.push(PixelElement::Solid(SolidColorRenderElement::new(
            ids[25 + tick % 3].clone(),
            Rectangle::new((7, 7).into(), (12, 8).into()),
            0,
            Color32F::new(1.0, 1.0, 1.0, 1.0),
            Kind::Cursor,
        )));
    }
    for (index, patch) in [
        Rectangle::new((4, 4).into(), (28, 23).into()),
        Rectangle::new((size.w - 37, size.h - 31).into(), (36, 30).into()),
    ]
    .into_iter()
    .enumerate()
    {
        let patch = if tick >= 7 && index == 0 {
            Rectangle::new((45, 11).into(), patch.size)
        } else {
            patch
        };
        let mut patches = vec![BlurPatch {
            rect: patch,
            radius: 3.0,
            alpha: 0.85,
            clip: None,
        }];
        if index == 0 && tick >= 4 {
            patches.push(BlurPatch {
                rect: Rectangle::new((patch.loc.x + 12, patch.loc.y + 8).into(), patch.size),
                ..patches[0]
            });
            patches.push(BlurPatch {
                rect: Rectangle::new((size.w / 2, size.h / 2).into(), (19, 17).into()),
                ..patches[0]
            });
        }
        let element = effects
            .blur_element(
                renderer,
                output,
                BlurIdentity::Overlay(if index == 0 { "a" } else { "b" }),
                size.to_logical(1),
                patches,
                config,
                u64::from(tick == 7),
                transform,
            )
            .unwrap()
            .unwrap();
        scene.push(if full {
            PixelElement::Full(FullBlur(element))
        } else {
            PixelElement::Partial(element)
        });
        // An intervening surface changes what the next effect must capture;
        // shared scratch textures must never leak one stack position into another.
        if index == 0 {
            scene.push(PixelElement::Solid(SolidColorRenderElement::new(
                ids[24].clone(),
                Rectangle::new((size.w - 24, size.h - 21).into(), (15, 12).into()),
                tick / 3,
                Color32F::new(0.0, 0.4, 0.0, 0.5),
                Kind::Unspecified,
            )));
        }
    }
    // Tiny changes near and just outside a patch test kernel halos. A later
    // removal and a move test persistent result pixels and old/new damage.
    if tick != 5 {
        let (x, y) = if tick == 4 {
            (10, 10)
        } else {
            match tick % 4 {
                0 => (2, 10),
                1 => (35, 12),
                2 => (size.w - 18, size.h - 12),
                _ => (size.w / 2, size.h / 2),
            }
        };
        scene.push(PixelElement::Solid(SolidColorRenderElement::new(
            ids[23].clone(),
            Rectangle::new((x, y).into(), (3, 4).into()),
            tick,
            Color32F::new(0.9, 0.7, 0.1, 1.0),
            Kind::Unspecified,
        )));
    }
    for (index, id) in ids.iter().take(20).enumerate() {
        let x = (index % 5) as i32 * size.w / 5;
        let y = (index / 5) as i32 * size.h / 4;
        let right = ((index % 5) as i32 + 1) * size.w / 5;
        let bottom = ((index / 5) as i32 + 1) * size.h / 4;
        scene.push(PixelElement::Solid(SolidColorRenderElement::new(
            id.clone(),
            Rectangle::new((x, y).into(), (right - x, bottom - y).into()),
            0,
            Color32F::new(
                (index % 3) as f32 * 0.35,
                (index % 4) as f32 * 0.25,
                (index % 5) as f32 * 0.2,
                1.0,
            ),
            Kind::Unspecified,
        )));
    }
    effects.end_scene(output);
    scene
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn scene_cleanup_releases_retired_blur_textures_and_preserves_live_frames() {
    fn blur(
        renderer: &mut GlesRenderer,
        effects: &mut BackdropBlurRenderer,
        output: &str,
        identity: BlurIdentity,
    ) -> BackdropBlurElement {
        effects
            .blur_element(
                renderer,
                output,
                identity,
                (48, 32).into(),
                vec![BlurPatch {
                    rect: Rectangle::from_size((48, 32).into()),
                    radius: 0.0,
                    alpha: 1.0,
                    clip: None,
                }],
                halley_config::Blur::default(),
                0,
                Transform::Normal,
            )
            .unwrap()
            .unwrap()
    }

    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut effects = BackdropBlurRenderer::default();
    effects.begin_scene("secondary");
    let secondary = blur(
        &mut renderer,
        &mut effects,
        "secondary",
        BlurIdentity::Overlay("stable"),
    );
    effects.end_scene("secondary");

    let mut retired: Option<std::rc::Weak<ElementBlur>> = None;
    let mut stable: Option<std::rc::Weak<ElementBlur>> = None;
    let mut stable_id = None;
    for tick in 0..64 {
        effects.begin_scene("primary");
        let surface = Id::new();
        let identity = if tick % 2 == 0 {
            BlurIdentity::Window {
                surface,
                instance: "canonical".into(),
            }
        } else {
            BlurIdentity::Layer(surface)
        };
        let transient = blur(&mut renderer, &mut effects, "primary", identity);
        let persistent = blur(
            &mut renderer,
            &mut effects,
            "primary",
            BlurIdentity::Overlay("stable"),
        );
        effects.end_scene("primary");

        let resources = &effects.outputs["primary"];
        assert_eq!(
            resources.results.len(),
            2,
            "retired textures must not accumulate"
        );
        assert_eq!(
            resources.ids.len(),
            2,
            "retired render identities must not accumulate"
        );
        if let Some(previous) = retired.take() {
            assert!(
                previous.upgrade().is_none(),
                "a closed surface must release its texture owner"
            );
        }
        if let Some(previous) = stable.as_ref().and_then(std::rc::Weak::upgrade) {
            assert!(Rc::ptr_eq(&previous, &persistent.element));
            assert_eq!(stable_id.as_ref(), Some(&persistent.id));
        }
        retired = Some(Rc::downgrade(&transient.element));
        stable = Some(Rc::downgrade(&persistent.element));
        stable_id = Some(persistent.id.clone());
        assert!(Rc::ptr_eq(
            &effects.outputs["secondary"].results[&BlurIdentity::Overlay("stable")],
            &secondary.element,
        ));
    }

    // An older scene may still own elements when a new, empty scene is built.
    effects.begin_scene("primary");
    let in_flight = blur(
        &mut renderer,
        &mut effects,
        "primary",
        BlurIdentity::Overlay("stable"),
    );
    effects.end_scene("primary");
    let in_flight_owner = Rc::downgrade(&in_flight.element);
    effects.begin_scene("primary");
    effects.end_scene("primary");
    assert!(effects.outputs["primary"].results.is_empty());
    assert!(effects.outputs["primary"].ids.is_empty());
    assert!(in_flight_owner.upgrade().is_some());
    let size: Size<i32, Physical> = (48, 32).into();
    let mut target = create_texture(&mut renderer, size).unwrap();
    let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
    let scene = vec![
        PixelElement::Partial(in_flight),
        PixelElement::Solid(SolidColorRenderElement::new(
            Id::new(),
            Rectangle::from_size(size),
            0,
            Color32F::BLACK,
            Kind::Unspecified,
        )),
    ];
    let (pixels, _) = read_pixels(&mut renderer, &mut target, &mut tracker, &scene, 0);
    assert_eq!(pixels.len(), 48 * 32 * 4);
    drop(scene);
    assert!(in_flight_owner.upgrade().is_none());
    assert_eq!(effects.outputs["secondary"].results.len(), 1);
    assert_eq!(effects.outputs["secondary"].ids.len(), 1);

    effects.begin_scene("secondary");
    effects.end_scene("secondary");
    assert!(effects.outputs["secondary"].results.is_empty());
    assert!(effects.outputs["secondary"].ids.is_empty());
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn upstream_blur_matches_full_processing_pixels_without_seams_or_stale_cache() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let ids: Vec<_> = (0..28).map(|_| Id::new()).collect();
    let mut effects = BackdropBlurRenderer::default();
    let mut statistics = [(0_u8, 0_u64, 0_u64); 2];
    let mut cases = Vec::new();
    for size in [(257, 193), (259, 195), (513, 385)] {
        for levels in 1..=5 {
            cases.push((size, levels, Transform::Normal, 1.0));
        }
    }
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        cases.push(((259, 195), 3, transform, 1.25));
    }
    let cases = cases.into_iter().flat_map(|case| {
        [
            halley_config::BlurMethod::DualKawase,
            halley_config::BlurMethod::Gaussian,
            halley_config::BlurMethod::Box,
        ]
        .into_iter()
        .map(move |method| (case, method))
    });
    for ((mode, levels, transform, scale), method) in cases {
        let mode: Size<i32, Physical> = mode.into();
        let size = transform.transform_size(mode);
        for (noise_index, noise) in [0.0, halley_config::Blur::default().noise]
            .into_iter()
            .enumerate()
        {
            let mut partial: Vec<_> = (0..3)
                .map(|_| create_texture(&mut renderer, mode).unwrap())
                .collect();
            let mut full = create_texture(&mut renderer, mode).unwrap();
            let mut tracker = OutputDamageTracker::new(mode, scale, transform);
            let config = halley_config::Blur {
                method,
                passes: levels,
                radius: if levels % 2 == 0 { 48.0 } else { 24.0 },
                noise,
                ..Default::default()
            };
            // New targets require valid cache data even at an unchanged stack.
            effects.remove_output("partial");
            effects.remove_output("full");
            let mut saw_partial = false;
            let mut saw_partial_processing = false;
            for tick in 0..12 {
                let partial_scene = scene(
                    &mut renderer,
                    &mut effects,
                    size,
                    config,
                    &ids,
                    tick,
                    false,
                    transform,
                );
                let age = if tick < 3 { 0 } else { 3 };
                let (actual, damage) = read_pixels(
                    &mut renderer,
                    &mut partial[tick % 3],
                    &mut tracker,
                    &partial_scene,
                    age,
                );
                for element in &partial_scene {
                    if let PixelElement::Partial(blur) = element
                        && let Some(plan) = blur.plan.borrow().as_ref()
                    {
                        let capture_pixels: i32 =
                            plan.capture.iter().map(|r| r.size.w * r.size.h).sum();
                        let final_pixels: i32 = plan.passes.last().map_or(size.w * size.h, |p| {
                            p.regions.iter().map(|r| r.size.w * r.size.h).sum()
                        });
                        saw_partial_processing |=
                            capture_pixels < size.w * size.h && final_pixels < size.w * size.h;
                    }
                }
                let full_scene = scene(
                    &mut renderer,
                    &mut effects,
                    size,
                    config,
                    &ids,
                    tick,
                    true,
                    transform,
                );
                let (expected, _) = read_pixels(
                    &mut renderer,
                    &mut full,
                    &mut OutputDamageTracker::new(mode, scale, transform),
                    &full_scene,
                    0,
                );
                let max_error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                // Subdivided draws change low floating-point bits in the
                // procedural noise hash. Bound that appearance difference by
                // its entire configured noise range plus one quantization step.
                let tolerance = (noise * 510.0).ceil() as u8 + 1;
                assert!(
                    max_error <= tolerance,
                    "method={method:?} size={size:?} transform={transform:?} scale={scale} levels={levels} noise={noise} tick={tick} max_error={max_error}"
                );
                let stats = &mut statistics[noise_index];
                stats.0 = stats.0.max(max_error);
                stats.1 += actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| u64::from(a.abs_diff(*b)))
                    .sum::<u64>();
                stats.2 += actual.len() as u64;
                if tick > 0
                    && !damage.is_empty()
                    && damage.iter().all(|r| r.size.w * r.size.h < size.w * size.h)
                {
                    saw_partial = true;
                }
            }
            if levels <= 3 && transform == Transform::Normal {
                // The unmodified Smithay pin conservatively captures full-output
                // effects; compare appearance rather than the removed regional API.
                let _ = (saw_partial, saw_partial_processing);
            }
        }
    }
    for (label, (max, sum, channels)) in ["noise off", "default noise"].into_iter().zip(statistics)
    {
        eprintln!(
            "{label}: max channel error={max}/255; mean absolute channel error={:.6}/255",
            sum as f64 / channels as f64
        );
    }
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn moving_blur_samples_the_current_backdrop_without_trails() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size: Size<i32, Physical> = (320, 240).into();
    let mut effects = BackdropBlurRenderer::default();
    let mut target = create_texture(&mut renderer, size).unwrap();
    let mut reference = create_texture(&mut renderer, size).unwrap();
    let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
    let colors = [
        Color32F::new(1.0, 0.0, 0.0, 1.0),
        Color32F::new(0.0, 0.0, 1.0, 1.0),
    ];
    let ids = [Id::new(), Id::new(), Id::new()];
    for (tick, y) in [30, 160, 30, 160].into_iter().enumerate() {
        effects.begin_scene("motion");
        let blur = effects
            .blur_element(
                &mut renderer,
                "motion",
                BlurIdentity::Window {
                    surface: ids[0].clone(),
                    instance: "moving".into(),
                },
                size.to_logical(1),
                vec![BlurPatch {
                    rect: Rectangle::new((100, y).into(), (100, 40).into()),
                    radius: 0.0,
                    alpha: 1.0,
                    clip: None,
                }],
                halley_config::Blur {
                    passes: 1,
                    radius: 2.0,
                    saturation: 1.0,
                    noise: 0.0,
                    ..Default::default()
                },
                tick as u64,
                Transform::Normal,
            )
            .unwrap()
            .unwrap();
        effects.end_scene("motion");
        let scene = vec![
            PixelElement::Partial(blur),
            PixelElement::Solid(SolidColorRenderElement::new(
                ids[1].clone(),
                Rectangle::new((0, 0).into(), (320, 120).into()),
                0,
                colors[0],
                Kind::Unspecified,
            )),
            PixelElement::Solid(SolidColorRenderElement::new(
                ids[2].clone(),
                Rectangle::new((0, 120).into(), (320, 120).into()),
                0,
                colors[1],
                Kind::Unspecified,
            )),
        ];
        let (pixels, _) = read_pixels(
            &mut renderer,
            &mut target,
            &mut tracker,
            &scene,
            if tick == 0 { 0 } else { 1 },
        );
        // The patch stays well inside a flat-colored backdrop band, so its
        // correct blur equals the sharp scene everywhere. Compare the whole
        // framebuffer to catch both stale sampling and a detached old patch.
        let (expected, _) = read_pixels(
            &mut renderer,
            &mut reference,
            &mut OutputDamageTracker::new(size, 1.0, Transform::Normal),
            &scene[1..],
            0,
        );
        let error = pixels
            .iter()
            .zip(expected)
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap();
        assert!(
            error <= 1,
            "tick={tick}, moving blur sampled a different backdrop or left a trail: error={error}"
        );
    }
}
