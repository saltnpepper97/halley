use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen, Texture};
use smithay::output::{Mode, PhysicalProperties, Subpixel};
use smithay::utils::Transform;

use super::*;
use crate::render::effects::backdrop_blur::BackdropBlurRenderer;
use crate::render::effects::shadow::ShadowRenderer;
use crate::render::node::NodeRenderer;
use crate::render::text::UiTextRenderer;
use crate::session::tty::FrameDemand;
use crate::shell::overlay::{OverlayManager, OverlaySnapshot};

const SIZE: (i32, i32) = (641, 481);

/// Explicit before/after extraction check on the same renderer/font installation.
/// Record before changing a renderer, then compare using the same baseline file.
#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn omitted_blurred_window_reveals_background_and_keeps_foreground() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size = (128, 96);
    let window = Rectangle::<i32, Physical>::new((32, 24).into(), (48, 40).into());
    let mut blur_renderer = BackdropBlurRenderer::default();
    blur_renderer.begin_scene("privacy-test");
    let blur = blur_renderer
        .blur_element(
            &mut renderer,
            "privacy-test",
            crate::render::effects::backdrop_blur::BlurIdentity::Overlay("privacy-test"),
            size.into(),
            vec![crate::render::effects::backdrop_blur::BlurPatch {
                rect: window,
                radius: 0.0,
                alpha: 1.0,
                clip: None,
            }],
            halley_config::Blur::default(),
            0,
            Transform::Normal,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        blur.geometry(Scale::from(1.0)),
        Rectangle::from_size(size.into())
    );
    let mut protected = vec![
        SceneElement::Border(crate::render::solid_color_element(
            Id::new(),
            window,
            Color32F::new(1.0, 0.0, 0.0, 1.0),
        )),
        SceneElement::BackdropBlur(blur),
    ];
    super::omit_elements(&mut protected);
    let foreground = Rectangle::<i32, Physical>::new((40, 30).into(), (8, 8).into());
    protected.insert(
        0,
        SceneElement::Border(crate::render::solid_color_element(
            Id::new(),
            foreground,
            Color32F::new(0.0, 0.0, 1.0, 1.0),
        )),
    );
    protected.push(SceneElement::Border(crate::render::solid_color_element(
        Id::new(),
        Rectangle::from_size(size.into()),
        Color32F::new(0.0, 1.0, 0.0, 1.0),
    )));
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, size.into())
        .unwrap();
    let (pixels, _) = read_pixels(
        &mut renderer,
        &mut target,
        &mut OutputDamageTracker::new(size, 1.0, Transform::Normal),
        &protected,
        0,
    );
    let pixel = |x: usize, y: usize| &pixels[(y * size.0 as usize + x) * 4..][..4];
    assert_eq!(
        pixel(4, 4),
        &[0, 255, 0, 255],
        "wallpaper outside the window must stay visible"
    );
    assert_eq!(
        pixel(35, 27),
        &[0, 255, 0, 255],
        "content behind the protected window must remain visible"
    );
    assert_eq!(
        pixel(42, 32),
        &[0, 0, 255, 255],
        "an unrelated foreground window must stay visible"
    );
}

#[test]
#[ignore = "requires surfaceless GLES and HALLEY_UI_PARITY_BASELINE plus HALLEY_UI_PARITY_MODE=record|compare"]
fn ui_library_migration_matches_recorded_frames() {
    use halley_config::NotificationPosition::*;
    use std::hash::{DefaultHasher, Hash, Hasher};
    let baseline = std::env::var("HALLEY_UI_PARITY_BASELINE").expect("baseline path");
    let mode = std::env::var("HALLEY_UI_PARITY_MODE").expect("record or compare");
    assert!(matches!(mode.as_str(), "record" | "compare"));
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let output = output("ui-extraction");
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, SIZE.into())
        .unwrap();
    let mut fingerprints = String::new();
    for font_size in [11, 18] {
        for position in [
            TopLeft,
            TopCenter,
            TopRight,
            BottomLeft,
            BottomCenter,
            BottomRight,
        ] {
            for kind in ["notification", "screenshot", "zoom"] {
                let config = halley_config::Overlays {
                    notifications: halley_config::Notifications {
                        position,
                        offset_x: -12,
                        offset_y: 16,
                        ..Default::default()
                    },
                    zoom_indicator: halley_config::ZoomIndicator {
                        position,
                        hold_duration_ms: 500,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut overlays = OverlayManager::default();
                let mut resources = SceneResources::new();
                resources.text.reload_font(&halley_config::Font {
                    family: "monospace".into(),
                    size: font_size,
                });
                match kind {
                    "notification" => {
                        overlays.show_config_error(output.name(), 500, Duration::ZERO)
                    }
                    "screenshot" => {
                        let pixels = [30, 160, 210, 255].repeat(160 * 90);
                        let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
                            &pixels, Fourcc::Abgr8888, (160, 90), 1, Transform::Normal, None);
                        overlays.show_screenshot_saved(
                            output.name(),
                            std::path::Path::new("/screenshots"),
                            500,
                            Duration::ZERO,
                        );
                        overlays.attach_screenshot_preview(Some(std::sync::Arc::new(
                            crate::capture::preview::ScreenshotPreview {
                                path: "/screenshots/test.png".into(),
                                buffer,
                                size: (160, 90),
                                png: pixels.into(),
                            },
                        )));
                    }
                    _ => {
                        overlays.show_zoom_indicator(
                            &output.name(),
                            0.75,
                            &config.zoom_indicator,
                            Duration::ZERO,
                        );
                    }
                }
                for millis in [0, 45, 90, 180, 500, 590, 700] {
                    let snapshot = overlays.snapshot(&output.name(), Duration::from_millis(millis));
                    let scene = resources.scene(&mut renderer, &output, snapshot, &config, None);
                    let (pixels, _) = read_pixels(
                        &mut renderer,
                        &mut target,
                        &mut OutputDamageTracker::new(SIZE, 1.0, Transform::Normal),
                        &scene,
                        0,
                    );
                    let mut hash = DefaultHasher::new();
                    pixels.hash(&mut hash);
                    fingerprints.push_str(&format!(
                        "{font_size} {position:?} {kind} {millis} {:016x}\n",
                        hash.finish()
                    ));
                }
            }
        }
    }
    if mode == "record" {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(baseline)
            .expect("record requires a new baseline file; existing evidence is preserved")
            .write_all(fingerprints.as_bytes())
            .unwrap();
    } else {
        assert_eq!(
            fingerprints,
            std::fs::read_to_string(baseline).unwrap(),
            "UI extraction changed rendered pixels"
        );
    }
}

fn output(name: &str) -> Output {
    let output = Output::new(
        name.into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: name.into(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: SIZE.into(),
            refresh: 60_000,
        }),
        Some(Transform::Normal),
        Some(smithay::output::Scale::Integer(1)),
        Some((0, 0).into()),
    );
    output
}

struct SceneResources {
    nodes: NodeRenderer,
    text: UiTextRenderer,
    blur: BackdropBlurRenderer,
    shadow: ShadowRenderer,
    background: [Id; 3],
}

impl SceneResources {
    fn new() -> Self {
        Self {
            nodes: NodeRenderer::default(),
            text: UiTextRenderer::new(&halley_config::Font::default()),
            blur: BackdropBlurRenderer::default(),
            shadow: ShadowRenderer::default(),
            background: std::array::from_fn(|_| Id::new()),
        }
    }

    fn scene(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        snapshot: OverlaySnapshot,
        config: &halley_config::Overlays,
        label_mix: Option<f32>,
    ) -> Vec<SceneElement> {
        let scale = output.current_scale().fractional_scale();
        self.text.set_display_scale(scale);
        self.blur.set_display_scale(
            &output.name(),
            scale,
            crate::render::output_physical_size(output),
        );
        self.nodes.begin_scene(&output.name());
        self.text.begin_scene();
        self.blur.begin_scene(&output.name());
        let mut elements = crate::render::overlays::shell::elements(
            renderer,
            Rectangle::from_size(SIZE.into()),
            snapshot,
            config,
            &mut self.nodes,
            &mut self.text,
        )
        .unwrap();
        if let Some(hover_mix) = label_mix {
            elements.extend(
                nodes::landmark_label_elements_avoiding(
                    renderer,
                    &mut self.nodes,
                    &mut self.text,
                    nodes::LandmarkLabel {
                        center: (220, 220),
                        marker_side: 64,
                        output_size: SIZE,
                        text: "Work cluster",
                        shape: halley_config::NodeShape::Squircle,
                        fill: (0.15, 0.18, 0.22),
                        ring: (0.6, 0.3, 0.1),
                        hover_mix,
                        alpha: 0.9,
                    },
                    &[],
                )
                .unwrap(),
            );
        }
        append_compositor_overlay_blur(
            renderer,
            output,
            OverlayEffectStyle {
                output_size: SIZE.into(),
                identity: "animation-test",
                // Remove procedural noise so the pixel comparison isolates
                // stale fade/geometry/shadow damage, with one rounding step.
                blur: halley_config::Blur {
                    noise: 0.0,
                    ..Default::default()
                },
                shadow: halley_config::Shadows::default().overlay,
            },
            &mut self.blur,
            &mut self.shadow,
            &mut elements,
        )
        .unwrap();
        for (id, rect, color) in [
            (
                &self.background[0],
                Rectangle::new((210, 0).into(), (100, SIZE.1).into()),
                Color32F::new(0.1, 0.7, 0.2, 1.0),
            ),
            (
                &self.background[1],
                Rectangle::new((0, 320).into(), (SIZE.0, 45).into()),
                Color32F::new(0.8, 0.2, 0.1, 1.0),
            ),
            (
                &self.background[2],
                Rectangle::from_size(SIZE.into()),
                Color32F::new(0.1, 0.2, 0.4, 1.0),
            ),
        ] {
            elements.push(SceneElement::Border(crate::render::solid_color_element(
                id.clone(),
                rect,
                color,
            )));
        }
        elements
            .into_iter()
            .map(|element| crate::render::display_scale::element(element, scale, false))
            .collect()
    }
}

fn read_pixels<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    tracker: &mut OutputDamageTracker,
    scene: &[E],
    age: usize,
) -> (Vec<u8>, Vec<Rectangle<i32, Physical>>) {
    let size = target.size();
    let mut fbo = renderer.bind(target).unwrap();
    let result = tracker
        .render_output(renderer, &mut fbo, age, scene, Color32F::BLACK)
        .unwrap();
    result.sync.wait().unwrap();
    let damage = result.damage.cloned().unwrap_or_default();
    let mapping = renderer
        .copy_framebuffer(&fbo, Rectangle::from_size(size), Fourcc::Abgr8888)
        .unwrap();
    (renderer.map_texture(&mapping).unwrap().to_vec(), damage)
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn apogee_dimming_preserves_wallpaper_hue_and_fades_without_a_brightness_jump() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size = (4, 4);
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, size.into())
        .unwrap();
    let backdrop_id = Id::new();
    let wallpaper_id = Id::new();
    let mut frames = 0;
    for wallpaper in [0_u8, 0x11, 0x40, 0x80, 0xff] {
        for dim in [0.0, 0.5, 0.85, 1.0] {
            let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
            // Open, hold, and close with the same buffer to exercise opacity damage.
            for (frame, step) in (0..=10).chain((0..=10).rev()).enumerate() {
                let progress = step as f32 / 10.0;
                let value = wallpaper as f32 / 255.0;
                let scene = [
                    crate::render::solid_color_element(
                        backdrop_id.clone(),
                        Rectangle::from_size(size.into()),
                        overview::apogee_backdrop_color(dim, progress),
                    ),
                    crate::render::solid_color_element(
                        wallpaper_id.clone(),
                        Rectangle::from_size(size.into()),
                        Color32F::new(value, value, value, 1.0),
                    ),
                ];
                let (pixels, _) = read_pixels(
                    &mut renderer,
                    &mut target,
                    &mut tracker,
                    &scene,
                    usize::from(frame > 0),
                );
                let expected = (wallpaper as f32 * (1.0 - dim * progress)).round() as u8;
                for pixel in pixels.chunks_exact(4) {
                    assert_eq!(pixel[0], pixel[1], "red/green hue shift: {pixel:?}");
                    assert_eq!(pixel[1], pixel[2], "green/blue hue shift: {pixel:?}");
                    assert!(
                        pixel[0].abs_diff(expected) <= 1,
                        "wallpaper={wallpaper}, dim={dim}, progress={progress}: {pixel:?}, expected={expected}"
                    );
                    if dim == 0.0 || progress == 0.0 {
                        assert_eq!(
                            pixel[0], wallpaper,
                            "transparent backdrop changed wallpaper"
                        );
                    }
                    assert_eq!(pixel[3], 255);
                }
                frames += 1;
            }
        }
    }
    println!("verified {frames} Apogee dimming frames with reused-buffer damage");
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn conservative_animation_pixels_match_full_repaint_with_blur_shadows_and_reused_buffers() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let actual_output = output("actual");
    let reference_output = output("reference");
    let mut comparisons = 0;
    for buffers in 1..=3 {
        for case in ["notification", "zoom", "cluster-label"] {
            let config = halley_config::Overlays {
                zoom_indicator: halley_config::ZoomIndicator {
                    hold_duration_ms: 120,
                    fade_duration_ms: 180,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut overlays = OverlayManager::default();
            if case == "notification" {
                overlays.show_config_error("actual".into(), 300, Duration::ZERO);
            } else if case == "zoom" {
                overlays.show_zoom_indicator(
                    "actual",
                    0.75,
                    &config.zoom_indicator,
                    Duration::ZERO,
                );
            }
            let clusters = crate::clusters::ClusterSystem::new(
                halley_config::Clusters::default(),
                halley_config::ClusterAnimation::default(),
            );
            let mut actual_resources = SceneResources::new();
            let mut reference_resources = SceneResources::new();
            let mut targets: Vec<GlesTexture> = (0..buffers)
                .map(|_| {
                    renderer
                        .create_buffer(Fourcc::Abgr8888, SIZE.into())
                        .unwrap()
                })
                .collect();
            let mut reference = renderer
                .create_buffer(Fourcc::Abgr8888, SIZE.into())
                .unwrap();
            let mut tracker = OutputDamageTracker::new(SIZE, 1.0, Transform::Normal);
            let mut saw_forced_full = false;
            let mut saw_reused = false;
            let mut saw_settled = false;
            let mut saw_visible = false;
            // Include enough settled frames to drain damage history for all buffer ages.
            let ticks = if case == "cluster-label" { 160 } else { 42 };
            for tick in 0..ticks {
                let now = Duration::from_millis(tick as u64 * 15);
                overlays.wakeup(now);
                let label_mix = (case == "cluster-label").then(|| {
                    clusters.label_hover_mix(halley_core::cluster::ClusterId::new(1), tick < 90)
                });
                let label_animating =
                    label_mix.is_some_and(|mix| mix != if tick < 90 { 1.0 } else { 0.0 });
                let local_animating =
                    overlays.animating_on_output("actual", now) || label_animating;
                let demand = FrameDemand::new(false, local_animating, false);
                assert_eq!(demand.keep_redrawing, local_animating);
                assert_eq!(demand.force_full_repaint, local_animating);
                let snapshot = overlays.snapshot("actual", now);
                let scene = actual_resources.scene(
                    &mut renderer,
                    &actual_output,
                    snapshot.clone(),
                    &config,
                    label_mix,
                );
                saw_visible |= scene
                    .iter()
                    .any(|e| matches!(e, SceneElement::UiText(_)) && e.alpha() > 0.1);
                // Use the same upstream blur wrapper and age reset as both host backends.
                let (prepared, blur) = crate::render::conservative::prepare(&scene);
                let forced_full = demand.force_full_repaint || blur;
                let age = if tick < buffers || forced_full {
                    0
                } else {
                    buffers
                };
                saw_reused |= age == buffers;
                let (actual, damage) = read_pixels(
                    &mut renderer,
                    &mut targets[tick % buffers],
                    &mut tracker,
                    &prepared,
                    age,
                );
                // Independent effect caches keep the always-full reference from
                // refreshing or repairing the scene under test.
                let reference_scene = reference_resources.scene(
                    &mut renderer,
                    &reference_output,
                    snapshot,
                    &config,
                    label_mix,
                );
                let (reference_prepared, _) =
                    crate::render::conservative::prepare(&reference_scene);
                let (expected, _) = read_pixels(
                    &mut renderer,
                    &mut reference,
                    &mut OutputDamageTracker::new(SIZE, 1.0, Transform::Normal),
                    &reference_prepared,
                    0,
                );
                let max_error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    max_error <= 1,
                    "case={case} buffers={buffers} tick={tick} max_error={max_error}"
                );
                if tick >= buffers {
                    if forced_full {
                        assert!(
                            damage
                                .iter()
                                .any(|r| r.size.w * r.size.h == SIZE.0 * SIZE.1),
                            "animated/blurred {case} must repaint the entire output"
                        );
                        saw_forced_full = true;
                    } else {
                        saw_settled |= !demand.keep_redrawing && damage.is_empty();
                    }
                }
                comparisons += 1;
            }
            assert!(
                saw_visible && saw_forced_full && saw_reused && saw_settled,
                "case={case} buffers={buffers}"
            );
        }
    }
    eprintln!(
        "{comparisons} conservative-animation GLES comparisons passed, buffer ages 1-3, max permitted channel error=1/255"
    );
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn screenshot_preview_pixels_and_fade_match_full_repaint() {
    use halley_config::NotificationPosition::*;
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let actual_output = output("actual");
    let reference_output = output("reference");
    let directory =
        std::env::temp_dir().join(format!("halley-preview-pixels-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("test.png");
    let pixels = [30, 160, 210, 255].repeat(160 * 90);
    image::save_buffer(&path, &pixels, 160, 90, image::ColorType::Rgba8).unwrap();
    let preview = crate::capture::preview::prepare(path.clone(), 160, 90, pixels).unwrap();
    let mut comparisons = 0;
    for position in [
        TopLeft,
        TopCenter,
        TopRight,
        BottomLeft,
        BottomCenter,
        BottomRight,
    ] {
        for buffers in 1..=3 {
            let config = halley_config::Overlays {
                notifications: halley_config::Notifications {
                    position,
                    offset_x: -12,
                    offset_y: 16,
                    success_duration_ms: 300,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut overlays = OverlayManager::default();
            overlays.show_screenshot_saved("actual".into(), &directory, 300, Duration::ZERO);
            overlays.attach_screenshot_preview(Some(preview.clone()));
            let mut actual_resources = SceneResources::new();
            let mut reference_resources = SceneResources::new();
            let font = halley_config::Font {
                size: 16,
                ..Default::default()
            };
            actual_resources.text.reload_font(&font);
            reference_resources.text.reload_font(&font);
            let mut targets: Vec<GlesTexture> = (0..buffers)
                .map(|_| {
                    renderer
                        .create_buffer(Fourcc::Abgr8888, SIZE.into())
                        .unwrap()
                })
                .collect();
            let mut reference = renderer
                .create_buffer(Fourcc::Abgr8888, SIZE.into())
                .unwrap();
            let mut tracker = OutputDamageTracker::new(SIZE, 1.0, Transform::Normal);
            for tick in 0..42 {
                let now = Duration::from_millis(tick as u64 * 15);
                overlays.wakeup(now);
                let demand =
                    FrameDemand::new(false, overlays.animating_on_output("actual", now), false);
                let snapshot = overlays.snapshot("actual", now);
                let scene = actual_resources.scene(
                    &mut renderer,
                    &actual_output,
                    snapshot.clone(),
                    &config,
                    None,
                );
                let (prepared, blur) = crate::render::conservative::prepare(&scene);
                let age = if tick < buffers || demand.force_full_repaint || blur {
                    0
                } else {
                    buffers
                };
                let (actual, _) = read_pixels(
                    &mut renderer,
                    &mut targets[tick % buffers],
                    &mut tracker,
                    &prepared,
                    age,
                );
                if tick == 12 {
                    let layout = crate::shell::screenshot::layout(
                        Rectangle::from_size(SIZE.into()),
                        config.notifications,
                        1.0,
                        font.size,
                    );
                    let x = layout.preview.loc.x + layout.preview.size.w / 2;
                    let y = layout.preview.loc.y + layout.preview.size.h / 2;
                    let start = ((y * SIZE.0 + x) * 4) as usize;
                    assert_eq!(
                        &actual[start..start + 4],
                        &[30, 160, 210, 255],
                        "preview channels and position"
                    );
                    if position == TopRight
                        && buffers == 1
                        && let Ok(path) = std::env::var("HALLEY_SCREENSHOT_TEST_RENDER")
                    {
                        image::save_buffer(
                            path,
                            &actual,
                            SIZE.0 as u32,
                            SIZE.1 as u32,
                            image::ColorType::Rgba8,
                        )
                        .unwrap();
                    }
                }
                let reference_scene = reference_resources.scene(
                    &mut renderer,
                    &reference_output,
                    snapshot,
                    &config,
                    None,
                );
                let (reference_prepared, _) =
                    crate::render::conservative::prepare(&reference_scene);
                let (expected, _) = read_pixels(
                    &mut renderer,
                    &mut reference,
                    &mut OutputDamageTracker::new(SIZE, 1.0, Transform::Normal),
                    &reference_prepared,
                    0,
                );
                let max_error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    max_error <= 1,
                    "position={position:?} buffers={buffers} tick={tick} max_error={max_error}"
                );
                comparisons += 1;
            }
            assert!(
                overlays
                    .snapshot("actual", Duration::from_millis(600))
                    .notification
                    .is_none()
            );
        }
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
    eprintln!("screenshot preview: {comparisons} rendered frame comparisons");
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn display_scaling_matches_full_repaint_with_text_blur_shadows_and_buffer_reuse() {
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let mut comparisons = 0;
    for scale in [0.75, 1.0, 1.25, 1.5, 2.0] {
        let size = (
            (SIZE.0 as f64 * scale).round() as i32,
            (SIZE.1 as f64 * scale).round() as i32,
        );
        let actual_output = output("scaled-actual");
        let reference_output = output("scaled-reference");
        for output in [&actual_output, &reference_output] {
            output.change_current_state(
                Some(Mode {
                    size: size.into(),
                    refresh: 60_000,
                }),
                None,
                Some(smithay::output::Scale::Fractional(scale)),
                None,
            );
        }
        let mut actual_resources = SceneResources::new();
        let mut reference_resources = SceneResources::new();
        let mut actual_target = renderer
            .create_buffer(Fourcc::Abgr8888, size.into())
            .unwrap();
        let mut reference_target = renderer
            .create_buffer(Fourcc::Abgr8888, size.into())
            .unwrap();
        let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        let config = halley_config::Overlays::default();
        let mut overlays = OverlayManager::default();
        overlays.show_config_error("scaled-actual".into(), 300, Duration::ZERO);
        for tick in 0..45 {
            let now = Duration::from_millis(tick * 20);
            overlays.wakeup(now);
            let snapshot = overlays.snapshot("scaled-actual", now);
            let scene = actual_resources.scene(
                &mut renderer,
                &actual_output,
                snapshot.clone(),
                &config,
                None,
            );
            let reference_scene = reference_resources.scene(
                &mut renderer,
                &reference_output,
                snapshot,
                &config,
                None,
            );
            let (prepared, blur) = crate::render::conservative::prepare(&scene);
            let (reference_prepared, _) = crate::render::conservative::prepare(&reference_scene);
            let age = usize::from(
                tick > 0 && !blur && !overlays.animating_on_output("scaled-actual", now),
            );
            let (actual, _) = read_pixels(
                &mut renderer,
                &mut actual_target,
                &mut tracker,
                &prepared,
                age,
            );
            let (reference, _) = read_pixels(
                &mut renderer,
                &mut reference_target,
                &mut OutputDamageTracker::new(size, 1.0, Transform::Normal),
                &reference_prepared,
                0,
            );
            assert_eq!(actual, reference, "scale={scale} tick={tick}");
            // A stripe at logical x=210..310 must retain the configured width and position.
            let (x, y) = (
                (250.0 * scale).round() as usize,
                (100.0 * scale).round() as usize,
            );
            let pixel = &actual[(y * size.0 as usize + x) * 4..][..4];
            assert!(
                pixel[1] > 150 && pixel[0] < 40 && pixel[2] < 80,
                "stripe was misplaced at scale={scale}: {pixel:?}"
            );
            comparisons += 1;
        }
    }
    println!("verified {comparisons} scaled frames with independent full-repaint references");
}

#[test]
#[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
fn display_scale_preserves_native_client_pixels_without_double_scaling() {
    use smithay::backend::renderer::element::memory::{
        MemoryRenderBuffer, MemoryRenderBufferRenderElement,
    };
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
    let size = (32, 32);
    let pixels = (0..size.1)
        .flat_map(|_| {
            (0..size.0).flat_map(|x| {
                let value = if x % 2 == 0 { 0 } else { 255 };
                [value, value, value, 255]
            })
        })
        .collect::<Vec<u8>>();
    let buffer =
        MemoryRenderBuffer::from_slice(&pixels, Fourcc::Abgr8888, size, 2, Transform::Normal, None);
    let inner = MemoryRenderBufferRenderElement::from_buffer(
        &mut renderer,
        (0.0, 0.0),
        &buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .unwrap();
    let element =
        crate::render::display_scale::element(SceneElement::ScreenshotPreview(inner), 2.0, false);
    assert_eq!(element.geometry(1.0.into()).size, size.into());
    assert_eq!(element.geometry(2.0.into()).size, size.into());
    let mut target = renderer
        .create_buffer(Fourcc::Abgr8888, size.into())
        .unwrap();
    let (actual, _) = read_pixels(
        &mut renderer,
        &mut target,
        &mut OutputDamageTracker::new(size, 2.0, Transform::Normal),
        &[element],
        0,
    );
    assert_eq!(actual, pixels, "native one-pixel stripes must stay sharp");
}
