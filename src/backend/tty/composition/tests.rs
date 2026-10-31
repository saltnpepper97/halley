use std::cell::RefCell;
use std::rc::Rc;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Element, Id, RenderElementPresentationState};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{ContextId, DebugFlags, Frame, RendererSuper, TextureFilter};
use smithay::utils::user_data::UserDataMap;

use super::*;
use crate::session::tty::FrameDemand;

mod blur_damage;
mod local_animation;

const RED: Color32F = Color32F::new(1.0, 0.0, 0.0, 1.0);
const GREEN: Color32F = Color32F::new(0.0, 1.0, 0.0, 1.0);
const WHITE: Color32F = Color32F::new(1.0, 1.0, 1.0, 1.0);

// Run the production composition and Smithay's output damage tracker against
// actual pixels, without needing a display server or two physical GPUs.
#[derive(Debug, Clone)]
struct RasterTexture {
    size: Size<i32, Buffer>,
    pixels: Rc<RefCell<Vec<Color32F>>>,
}

impl RasterTexture {
    fn new(size: (i32, i32)) -> Self {
        Self {
            size: size.into(),
            pixels: Rc::new(RefCell::new(vec![
                Color32F::TRANSPARENT;
                (size.0 * size.1) as usize
            ])),
        }
    }

    fn pixel(&self, x: i32, y: i32) -> Color32F {
        self.pixels.borrow()[(y * self.size.w + x) as usize]
    }

    fn paint(&self, rect: Rectangle<i32, Physical>, color: Color32F, blend: bool) {
        let Some(rect) = rect.intersection(Rectangle::from_size((self.size.w, self.size.h).into()))
        else {
            return;
        };
        let mut pixels = self.pixels.borrow_mut();
        for y in rect.loc.y..rect.loc.y + rect.size.h {
            for x in rect.loc.x..rect.loc.x + rect.size.w {
                let pixel = &mut pixels[(y * self.size.w + x) as usize];
                *pixel = if blend {
                    let remaining = 1.0 - color.a();
                    Color32F::new(
                        color.r() + pixel.r() * remaining,
                        color.g() + pixel.g() * remaining,
                        color.b() + pixel.b() * remaining,
                        color.a() + pixel.a() * remaining,
                    )
                } else {
                    color
                };
            }
        }
    }
}

impl Texture for RasterTexture {
    fn width(&self) -> u32 {
        self.size.w as u32
    }
    fn height(&self) -> u32 {
        self.size.h as u32
    }
    fn format(&self) -> Option<Fourcc> {
        Some(Fourcc::Abgr8888)
    }
}

#[derive(Debug)]
struct RasterRenderer {
    context: ContextId<RasterTexture>,
    frames: usize,
    fail_finish: bool,
}

impl Default for RasterRenderer {
    fn default() -> Self {
        Self {
            context: ContextId::new(),
            frames: 0,
            fail_finish: false,
        }
    }
}

impl RendererSuper for RasterRenderer {
    type Error = std::io::Error;
    type TextureId = RasterTexture;
    type Framebuffer<'buffer> = RasterTexture;
    type Frame<'frame, 'buffer>
        = RasterFrame
    where
        'buffer: 'frame,
        Self: 'frame;
}

impl Renderer for RasterRenderer {
    fn context_id(&self) -> ContextId<RasterTexture> {
        self.context.clone()
    }
    fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }
    fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_debug_flags(&mut self, _: DebugFlags) {}
    fn debug_flags(&self) -> DebugFlags {
        DebugFlags::empty()
    }
    fn render<'frame, 'buffer>(
        &'frame mut self,
        target: &'frame mut RasterTexture,
        size: Size<i32, Physical>,
        transform: Transform,
    ) -> Result<RasterFrame, Self::Error>
    where
        'buffer: 'frame,
    {
        assert_eq!(transform, Transform::Normal);
        assert_eq!((size.w, size.h), (target.size.w, target.size.h));
        self.frames += 1;
        Ok(RasterFrame {
            target: target.clone(),
            context: self.context.clone(),
            fail_finish: std::mem::take(&mut self.fail_finish),
        })
    }
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        sync.wait().map_err(std::io::Error::other)
    }
}

impl Bind<RasterTexture> for RasterRenderer {
    fn bind(&mut self, target: &mut RasterTexture) -> Result<RasterTexture, Self::Error> {
        Ok(target.clone())
    }
}

struct RasterFrame {
    target: RasterTexture,
    context: ContextId<RasterTexture>,
    fail_finish: bool,
}

impl Frame for RasterFrame {
    type Error = std::io::Error;
    type TextureId = RasterTexture;
    fn context_id(&self) -> ContextId<RasterTexture> {
        self.context.clone()
    }
    fn clear(
        &mut self,
        color: Color32F,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), Self::Error> {
        for rect in damage {
            self.target.paint(*rect, color, false);
        }
        Ok(())
    }
    fn draw_solid(
        &mut self,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        color: Color32F,
    ) -> Result<(), Self::Error> {
        for rect in damage {
            self.target
                .paint(Rectangle::new(dst.loc + rect.loc, rect.size), color, true);
        }
        Ok(())
    }
    fn render_texture_from_to(
        &mut self,
        texture: &RasterTexture,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _: &[Rectangle<i32, Physical>],
        transform: Transform,
        alpha: f32,
    ) -> Result<(), Self::Error> {
        assert_eq!(transform, Transform::Normal);
        for rect in damage {
            for y in rect.loc.y..rect.loc.y + rect.size.h {
                for x in rect.loc.x..rect.loc.x + rect.size.w {
                    let sx = (src.loc.x + f64::from(x) * src.size.w / f64::from(dst.size.w)).floor()
                        as i32;
                    let sy = (src.loc.y + f64::from(y) * src.size.h / f64::from(dst.size.h)).floor()
                        as i32;
                    self.target.paint(
                        Rectangle::new((dst.loc.x + x, dst.loc.y + y).into(), (1, 1).into()),
                        texture.pixel(sx, sy) * alpha,
                        true,
                    );
                }
            }
        }
        Ok(())
    }
    fn transformation(&self) -> Transform {
        Transform::Normal
    }
    fn output_size(&self) -> Size<i32, Physical> {
        (self.target.size.w, self.target.size.h).into()
    }
    fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> {
        sync.wait().map_err(std::io::Error::other)
    }
    fn finish(self) -> Result<SyncPoint, Self::Error> {
        if self.fail_finish {
            Err(std::io::Error::other("injected frame failure"))
        } else {
            Ok(SyncPoint::signaled())
        }
    }
}

fn solid(
    id: &Id,
    rect: (i32, i32, i32, i32),
    commit: usize,
    color: Color32F,
) -> SolidColorRenderElement {
    SolidColorRenderElement::new(
        id.clone(),
        Rectangle::new((rect.0, rect.1).into(), (rect.2, rect.3).into()),
        commit,
        color,
        Kind::Unspecified,
    )
}

fn compose(renderer: &mut RasterRenderer) -> (ComposedFrame<RasterTexture>, RasterTexture) {
    let texture = RasterTexture::new((100, 80));
    (
        ComposedFrame::new(
            renderer,
            texture.clone(),
            Scale::from(1.0),
            Transform::Normal,
        ),
        texture,
    )
}

fn present(
    renderer: &mut RasterRenderer,
    tracker: &mut OutputDamageTracker,
    target: &mut RasterTexture,
    frame: &ComposedFrame<RasterTexture>,
    age: usize,
) -> Vec<Rectangle<i32, Physical>> {
    tracker
        .render_output(
            renderer,
            target,
            age,
            &[frame.element((100, 80).into())],
            Color32F::BLACK,
        )
        .unwrap()
        .damage
        .cloned()
        .unwrap_or_default()
}

#[test]
fn unchanged_scene_skips_composition_and_transfer_for_repeated_pointer_redraws() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, _) = compose(&mut renderer);
    let scene = [solid(&Id::new(), (10, 10, 20, 20), 0, RED)];
    let mut tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut target = RasterTexture::new((100, 80));
    frame
        .render(&mut renderer, &scene, Color32F::BLACK, false)
        .unwrap();
    assert!(!present(&mut renderer, &mut tracker, &mut target, &frame, 0).is_empty());
    let first = frame.element((100, 80).into());
    for _ in 0..100 {
        frame
            .render(&mut renderer, &scene, Color32F::BLACK, false)
            .unwrap();
        assert!(present(&mut renderer, &mut tracker, &mut target, &frame, 1).is_empty());
        let next = frame.element((100, 80).into());
        assert_eq!(first.id(), next.id());
        assert_eq!(first.current_commit(), next.current_commit());
    }
    assert_eq!(renderer.frames, 2);
    assert_eq!(target.pixel(15, 15), RED);
}

#[test]
fn geometry_animation_with_or_without_fps_still_forces_full_primary_and_secondary_repaints() {
    for fps_visible in [false, true] {
        let animation = FrameDemand::new(true, false, fps_visible);
        assert!(animation.keep_redrawing);
        let mut renderer = RasterRenderer::default();
        let (mut frame, texture) = compose(&mut renderer);
        let mut primary_tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
        let mut secondary_tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
        let mut primary = RasterTexture::new((100, 80));
        let mut secondary = RasterTexture::new((100, 80));
        let animated = Id::new();
        for x in [5, 10, 10, 15] {
            let scene = [solid(&animated, (x, 5, 5, 5), 0, WHITE)];
            let age = usize::from(!animation.force_full_repaint);
            let damage = primary_tracker
                .render_output(&mut renderer, &mut primary, age, &scene, Color32F::BLACK)
                .unwrap()
                .damage
                .cloned()
                .unwrap_or_default();
            assert_eq!(damage, [Rectangle::from_size((100, 80).into())]);
            frame
                .render(
                    &mut renderer,
                    &scene,
                    Color32F::BLACK,
                    animation.force_full_repaint,
                )
                .unwrap();
            assert_eq!(
                present(
                    &mut renderer,
                    &mut secondary_tracker,
                    &mut secondary,
                    &frame,
                    age
                ),
                [Rectangle::from_size((100, 80).into())]
            );
            assert_eq!(*primary.pixels.borrow(), *texture.pixels.borrow());
            assert_eq!(*primary.pixels.borrow(), *secondary.pixels.borrow());
            assert_eq!(secondary.pixel(x, 5), WHITE);
        }
        assert_eq!(secondary.pixel(5, 5), Color32F::BLACK);
        assert_eq!(renderer.frames, 12);
    }
}

#[test]
fn cursor_motion_and_removal_restore_pixels_with_partial_transfer() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, texture) = compose(&mut renderer);
    let cursor = Id::new();
    let background = solid(&Id::new(), (0, 0, 100, 80), 0, RED);
    let mut tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut target = RasterTexture::new((100, 80));
    frame
        .render(
            &mut renderer,
            &[solid(&cursor, (5, 5, 3, 3), 0, WHITE), background.clone()],
            Color32F::BLACK,
            false,
        )
        .unwrap();
    present(&mut renderer, &mut tracker, &mut target, &frame, 0);
    frame
        .render(
            &mut renderer,
            &[solid(&cursor, (20, 5, 3, 3), 0, WHITE), background.clone()],
            Color32F::BLACK,
            false,
        )
        .unwrap();
    let damage = present(&mut renderer, &mut tracker, &mut target, &frame, 1);
    assert!(damage.iter().all(|r| r.size.w * r.size.h < 100 * 80));
    assert_eq!(target.pixel(5, 5), RED);
    assert_eq!(target.pixel(20, 5), WHITE);
    assert_eq!(*target.pixels.borrow(), *texture.pixels.borrow());
    // The pointer leaving this output (or becoming hidden) removes its element.
    frame
        .render(
            &mut renderer,
            std::slice::from_ref(&background),
            Color32F::BLACK,
            false,
        )
        .unwrap();
    let damage = present(&mut renderer, &mut tracker, &mut target, &frame, 1);
    assert!(!damage.is_empty());
    assert_eq!(target.pixel(20, 5), RED);
    let settled = renderer.frames;
    frame
        .render(&mut renderer, &[background], Color32F::BLACK, false)
        .unwrap();
    assert!(present(&mut renderer, &mut tracker, &mut target, &frame, 1).is_empty());
    assert_eq!(renderer.frames, settled);
    let frames = renderer.frames;
    frame
        .render::<_, SolidColorRenderElement>(&mut renderer, &[], Color32F::BLACK, false)
        .unwrap();
    present(&mut renderer, &mut tracker, &mut target, &frame, 1);
    assert_eq!(target.pixel(0, 0), Color32F::BLACK);
    assert!(renderer.frames > frames);
}

#[test]
fn same_surface_identity_publishes_changed_content_and_clear_color() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, _) = compose(&mut renderer);
    let id = Id::new();
    frame
        .render(
            &mut renderer,
            &[solid(&id, (10, 10, 20, 20), 0, RED)],
            Color32F::BLACK,
            false,
        )
        .unwrap();
    let before = frame.element((100, 80).into());
    frame
        .render(
            &mut renderer,
            &[solid(&id, (10, 10, 20, 20), 1, GREEN)],
            Color32F::BLACK,
            false,
        )
        .unwrap();
    let after = frame.element((100, 80).into());
    assert_eq!(before.id(), after.id());
    assert_ne!(before.current_commit(), after.current_commit());
    assert_eq!(
        after
            .damage_since(Scale::from(1.0), Some(before.current_commit()))
            .as_ref(),
        &[Rectangle::new((10, 10).into(), (20, 20).into())]
    );
    let mut tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut target = RasterTexture::new((100, 80));
    present(&mut renderer, &mut tracker, &mut target, &frame, 0);
    assert_eq!(target.pixel(15, 15), GREEN);
    frame
        .render(
            &mut renderer,
            &[solid(&id, (10, 10, 20, 20), 1, GREEN)],
            WHITE,
            false,
        )
        .unwrap();
    let damage = present(&mut renderer, &mut tracker, &mut target, &frame, 1);
    assert_eq!(damage, [Rectangle::from_size((100, 80).into())]);
    assert_eq!(target.pixel(0, 0), WHITE);
    assert_eq!(target.pixel(15, 15), GREEN);
}

#[test]
fn output_buffer_age_accumulates_missed_cursor_damage() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, texture) = compose(&mut renderer);
    let cursor = Id::new();
    let mut tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut targets = [RasterTexture::new((100, 80)), RasterTexture::new((100, 80))];
    for i in 0..8 {
        frame
            .render(
                &mut renderer,
                &[solid(&cursor, (5 + i * 5, 5, 3, 3), 0, WHITE)],
                Color32F::BLACK,
                false,
            )
            .unwrap();
        present(
            &mut renderer,
            &mut tracker,
            &mut targets[(i % 2) as usize],
            &frame,
            if i < 2 { 0 } else { 2 },
        );
        assert_eq!(
            *targets[(i % 2) as usize].pixels.borrow(),
            *texture.pixels.borrow()
        );
    }
    // A reset scanout buffer still receives the retained scene, even when
    // composing that unchanged scene produces no new damage.
    let mut fresh = RasterTexture::new((100, 80));
    assert_eq!(
        present(&mut renderer, &mut tracker, &mut fresh, &frame, 0),
        [Rectangle::from_size((100, 80).into())]
    );
    assert_eq!(*fresh.pixels.borrow(), *texture.pixels.borrow());
}

#[test]
fn forced_repaint_and_failed_frame_do_not_suppress_changed_pixels() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, texture) = compose(&mut renderer);
    let id = Id::new();
    let scene = [solid(&id, (10, 10, 20, 20), 0, RED)];
    frame
        .render(&mut renderer, &scene, Color32F::BLACK, false)
        .unwrap();
    let before = frame.element((100, 80).into());
    frame
        .render(&mut renderer, &scene, Color32F::BLACK, true)
        .unwrap();
    let forced = frame.element((100, 80).into());
    assert_eq!(
        forced
            .damage_since(Scale::from(1.0), Some(before.current_commit()))
            .as_ref(),
        &[Rectangle::from_size((100, 80).into())]
    );
    renderer.fail_finish = true;
    let changed = [solid(&id, (10, 10, 20, 20), 1, GREEN)];
    assert!(
        frame
            .render(&mut renderer, &changed, Color32F::BLACK, false)
            .is_err()
    );
    assert_eq!(
        forced.current_commit(),
        frame.element((100, 80).into()).current_commit()
    );
    frame
        .render(&mut renderer, &changed, Color32F::BLACK, false)
        .unwrap();
    assert_eq!(texture.pixel(15, 15), GREEN);
    assert_eq!(
        frame
            .element((100, 80).into())
            .damage_since(Scale::from(1.0), Some(forced.current_commit()))
            .as_ref(),
        &[Rectangle::from_size((100, 80).into())]
    );
}

#[test]
fn geometry_changes_invalidate_and_fractional_rotated_damage_stays_in_scene_space() {
    let mut renderer = RasterRenderer::default();
    for transform in [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::Flipped,
    ] {
        let mode_size: Size<i32, Buffer> = (120, 90).into();
        let scale = Scale::from(1.5);
        let (size, logical_size) =
            super::super::composed_texture_sizes(mode_size, transform, scale);
        let texture = RasterTexture::new((size.w, size.h));
        let mut frame = ComposedFrame::new(&renderer, texture, scale, transform);
        assert!(frame.matches(size, scale, transform));
        assert!(!frame.matches(size, Scale::from(2.0), transform));
        assert!(!frame.matches(
            size,
            scale,
            if transform == Transform::Normal {
                Transform::_180
            } else {
                Transform::Normal
            }
        ));
        assert!(!frame.matches((size.w + 1, size.h).into(), scale, transform));
        let mut target = OutputDamageTracker::new((mode_size.w, mode_size.h), scale, transform);
        let id = Id::new();
        frame
            .render(
                &mut renderer,
                &[solid(&id, (9, 12, 6, 6), 0, RED)],
                Color32F::BLACK,
                false,
            )
            .unwrap();
        target
            .damage_output(0, &[frame.element(logical_size)])
            .unwrap();
        frame
            .render(
                &mut renderer,
                &[solid(&id, (9, 12, 6, 6), 1, GREEN)],
                Color32F::BLACK,
                false,
            )
            .unwrap();
        let (damage, _) = target
            .damage_output(1, &[frame.element(logical_size)])
            .unwrap();
        assert_eq!(
            damage.unwrap(),
            &[Rectangle::new((9, 12).into(), (6, 6).into())]
        );
    }
}

#[test]
fn fractional_partial_updates_match_a_full_repaint_at_odd_output_sizes() {
    let mut renderer = RasterRenderer::default();
    let mode: Size<i32, Buffer> = (101, 79).into();
    for scale in [0.75, 1.25, 1.5, 1.75, 2.0] {
        let scale = Scale::from(scale);
        let (size, logical_size) =
            super::super::composed_texture_sizes(mode, Transform::Normal, scale);
        let mut frame = ComposedFrame::new(
            &renderer,
            RasterTexture::new((size.w, size.h)),
            scale,
            Transform::Normal,
        );
        let mut tracker = OutputDamageTracker::new((mode.w, mode.h), scale, Transform::Normal);
        let mut reference_tracker =
            OutputDamageTracker::new((mode.w, mode.h), scale, Transform::Normal);
        let mut target = RasterTexture::new((size.w, size.h));
        let mut reference = RasterTexture::new((size.w, size.h));
        let cursor = Id::new();
        let background = solid(&Id::new(), (0, 0, size.w, size.h), 0, RED);
        for (x, y) in [(5, 5), (30, 40), (98, 76), (10, 10)] {
            frame
                .render(
                    &mut renderer,
                    &[solid(&cursor, (x, y, 3, 3), 0, WHITE), background.clone()],
                    Color32F::BLACK,
                    false,
                )
                .unwrap();
            tracker
                .render_output(
                    &mut renderer,
                    &mut target,
                    1,
                    &[frame.element(logical_size)],
                    Color32F::BLACK,
                )
                .unwrap();
            reference_tracker
                .render_output(
                    &mut renderer,
                    &mut reference,
                    0,
                    &[frame.element(logical_size)],
                    Color32F::BLACK,
                )
                .unwrap();
            assert_eq!(
                *target.pixels.borrow(),
                *reference.pixels.borrow(),
                "scale={scale:?} cursor=({x}, {y})"
            );
        }
    }
}

#[test]
fn occluded_scene_states_remain_available_on_unchanged_frames() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, _) = compose(&mut renderer);
    let front = Id::new();
    let back = Id::new();
    let scene = [
        solid(&front, (0, 0, 100, 80), 0, RED),
        solid(&back, (0, 0, 100, 80), 0, GREEN),
    ];
    for _ in 0..2 {
        let states = frame
            .render(&mut renderer, &scene, Color32F::BLACK, false)
            .unwrap();
        assert_eq!(
            states
                .element_render_state(front.clone())
                .unwrap()
                .visible_area,
            8_000
        );
        assert_eq!(
            states
                .element_render_state(back.clone())
                .unwrap()
                .presentation_state,
            RenderElementPresentationState::Skipped
        );
    }
    assert_eq!(renderer.frames, 1);
}

struct EffectElement {
    solid: SolidColorRenderElement,
    captures: Option<Rc<RefCell<Vec<Color32F>>>>,
}

impl Element for EffectElement {
    fn id(&self) -> &Id {
        self.solid.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.solid.current_commit()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.solid.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.solid.geometry(scale)
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.solid.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.solid.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.solid.alpha()
    }
    fn kind(&self) -> Kind {
        self.solid.kind()
    }
    fn is_framebuffer_effect(&self) -> bool {
        self.captures.is_some()
    }
}

impl RenderElement<RasterRenderer> for EffectElement {
    fn draw(
        &self,
        frame: &mut RasterFrame,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), std::io::Error> {
        <SolidColorRenderElement as RenderElement<RasterRenderer>>::draw(
            &self.solid,
            frame,
            src,
            dst,
            damage,
            opaque,
            cache,
        )
    }
    fn capture_framebuffer(
        &self,
        frame: &mut RasterFrame,
        _: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        _: &UserDataMap,
    ) -> Result<(), std::io::Error> {
        self.captures
            .as_ref()
            .unwrap()
            .borrow_mut()
            .push(frame.target.pixel(dst.loc.x, dst.loc.y));
        Ok(())
    }
}

#[test]
fn framebuffer_effect_conservatively_recaptures_every_sample() {
    let mut renderer = RasterRenderer::default();
    let (mut frame, _) = compose(&mut renderer);
    let background = Id::new();
    let effect = Id::new();
    let captures = Rc::new(RefCell::new(Vec::new()));
    for (commit, color) in [(0, RED), (0, RED), (1, GREEN), (1, GREEN)] {
        let scene = [
            EffectElement {
                solid: solid(
                    &effect,
                    (10, 10, 20, 20),
                    0,
                    Color32F::new(0.0, 0.0, 0.0, 0.5),
                ),
                captures: Some(captures.clone()),
            },
            EffectElement {
                solid: solid(&background, (0, 0, 100, 80), commit, color),
                captures: None,
            },
        ];
        frame
            .render(&mut renderer, &scene, Color32F::BLACK, false)
            .unwrap();
    }
    assert_eq!(*captures.borrow(), [RED, RED, GREEN, GREEN]);
    assert_eq!(renderer.frames, 4);
}

#[test]
fn fps_with_blur_matches_conservative_primary_and_secondary_outputs() {
    let fps = FrameDemand::new(false, false, true);
    let mut renderer = RasterRenderer::default();
    let (mut frame, texture) = compose(&mut renderer);
    let mut primary_tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut secondary_tracker = OutputDamageTracker::new((100, 80), 1.0, Transform::Normal);
    let mut primary = RasterTexture::new((100, 80));
    let mut secondary = RasterTexture::new((100, 80));
    let primary_captures = Rc::new(RefCell::new(Vec::new()));
    let secondary_captures = Rc::new(RefCell::new(Vec::new()));
    let chip = Id::new();
    let effect = Id::new();
    let patch = Id::new();
    let background = Id::new();
    for (tick, (chip_commit, patch_commit, color)) in [
        (0, 0, RED),
        (0, 0, RED),
        (1, 0, RED),
        (1, 1, GREEN),
        (1, 1, GREEN),
    ]
    .into_iter()
    .enumerate()
    {
        let scene = |captures: Rc<RefCell<Vec<Color32F>>>| {
            vec![
                EffectElement {
                    solid: solid(
                        &chip,
                        (2, 2, 10, 5),
                        chip_commit,
                        if chip_commit == 0 { WHITE } else { GREEN },
                    ),
                    captures: None,
                },
                EffectElement {
                    solid: solid(
                        &effect,
                        (10, 10, 20, 20),
                        0,
                        Color32F::new(0.0, 0.0, 0.0, 0.5),
                    ),
                    captures: Some(captures),
                },
                EffectElement {
                    solid: solid(&patch, (10, 10, 20, 20), patch_commit, color),
                    captures: None,
                },
                EffectElement {
                    solid: solid(&background, (0, 0, 100, 80), 0, RED),
                    captures: None,
                },
            ]
        };
        let age = usize::from(tick != 0 && !fps.force_full_repaint);
        let primary_scene = scene(primary_captures.clone());
        let (prepared, _) = crate::render::conservative::prepare(&primary_scene);
        let damage = primary_tracker
            .render_output(&mut renderer, &mut primary, age, &prepared, Color32F::BLACK)
            .unwrap()
            .damage
            .cloned()
            .unwrap_or_default();
        frame
            .render(
                &mut renderer,
                &scene(secondary_captures.clone()),
                Color32F::BLACK,
                fps.force_full_repaint,
            )
            .unwrap();
        let transfer = present(
            &mut renderer,
            &mut secondary_tracker,
            &mut secondary,
            &frame,
            age,
        );
        if tick != 0 {
            assert!(
                damage
                    .iter()
                    .any(|rect| rect.size.w * rect.size.h == 100 * 80)
            );
            assert!(
                transfer
                    .iter()
                    .any(|rect| rect.size.w * rect.size.h == 100 * 80)
            );
        }
        assert_eq!(*primary.pixels.borrow(), *texture.pixels.borrow());
        assert_eq!(*primary.pixels.borrow(), *secondary.pixels.borrow());
    }
    assert_eq!(*primary_captures.borrow(), [RED, RED, RED, GREEN, GREEN]);
    assert_eq!(*secondary_captures.borrow(), [RED, RED, RED, GREEN, GREEN]);
    assert_eq!(secondary.pixel(15, 15), Color32F::new(0.0, 0.5, 0.0, 1.0));
    assert_eq!(secondary.pixel(50, 50), RED);
    assert_eq!(renderer.frames, 15);
}
