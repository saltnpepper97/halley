use std::error::Error;
use std::{cell::Cell, ptr};

use smithay::{
    backend::renderer::{
        Color32F, Texture,
        gles::{GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::render::shadow::draw_shadow_rect;
use crate::render::state::RenderState;

use super::OverlayVisuals;

thread_local! {
    static OVERLAY_BLUR_CONTEXT: Cell<*mut ()> = const { Cell::new(ptr::null_mut()) };
}

pub(crate) fn with_overlay_blur_context<R>(
    ctx: Option<&mut crate::render::frame::draw::FrameBlurContext<'_>>,
    f: impl FnOnce() -> R,
) -> R {
    OVERLAY_BLUR_CONTEXT.with(|slot| {
        let previous = slot.replace(
            ctx.map(|ctx| ctx as *mut crate::render::frame::draw::FrameBlurContext<'_> as *mut ())
                .unwrap_or_else(ptr::null_mut),
        );
        let result = f();
        slot.set(previous);
        result
    })
}

pub(crate) fn draw_overlay_backdrop_blur(
    frame: &mut GlesFrame<'_, '_>,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    OVERLAY_BLUR_CONTEXT.with(|slot| {
        let ptr = slot.get();
        if ptr.is_null() {
            return Ok(());
        }
        let ctx =
            unsafe { &mut *(ptr as *mut crate::render::frame::draw::FrameBlurContext<'static>) };
        if let Err(err) = ctx.draw_patch(frame, damage, rect, corner_radius, alpha) {
            eventline::warn!("overlay blur skipped this frame: {err}");
        }
        Ok(())
    })
}

pub(super) fn draw_overlay_chip(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    draw_border: bool,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    draw_overlay_chip_with_border_color(
        frame,
        render_state,
        visuals,
        rect,
        corner_radius,
        fill_color,
        visuals.palette.border.alpha(1.0),
        draw_border,
        damage,
        alpha,
    )
}

pub(super) fn draw_overlay_chip_with_border_color(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    border_color: Color32F,
    draw_border: bool,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    draw_overlay_chip_impl(
        frame,
        render_state,
        visuals,
        rect,
        corner_radius,
        fill_color,
        border_color,
        draw_border,
        true,
        true,
        damage,
        alpha,
    )
}

/// Draw just a rounded, theme-coloured stroke (transparent interior, no shadow, no
/// backdrop blur). Used for hover/focus rings drawn over existing content.
pub(super) fn draw_overlay_ring(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    border_color: Color32F,
    border_px: f32,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let ring_visuals = OverlayVisuals {
        border_px,
        ..*visuals
    };
    draw_overlay_chip_impl(
        frame,
        render_state,
        &ring_visuals,
        rect,
        corner_radius,
        Color32F::new(0.0, 0.0, 0.0, 0.0),
        border_color,
        true,
        false,
        false,
        damage,
        alpha,
    )
}

pub(super) fn draw_overlay_chip_without_shadow(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    draw_border: bool,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    draw_overlay_chip_impl(
        frame,
        render_state,
        visuals,
        rect,
        corner_radius,
        fill_color,
        visuals.palette.border.alpha(1.0),
        draw_border,
        false,
        true,
        damage,
        alpha,
    )
}

fn draw_overlay_chip_impl(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    visuals: &OverlayVisuals,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    fill_color: Color32F,
    border_color: Color32F,
    draw_border: bool,
    draw_shadow: bool,
    draw_blur: bool,
    damage: Rectangle<i32, Physical>,
    alpha: f32,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = render_state.gpu.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = render_state.ui_rect_program(visuals.rounded) else {
        return Ok(());
    };
    if draw_shadow {
        draw_shadow_rect(
            frame,
            render_state,
            visuals.shadow,
            rect,
            if visuals.rounded { corner_radius } else { 0.0 },
            alpha,
            damage,
        )?;
    }
    if draw_blur {
        draw_overlay_backdrop_blur(
            frame,
            rect,
            if visuals.rounded { corner_radius } else { 0.0 },
            damage,
            alpha,
        )?;
    }
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let border_px = if draw_border { visuals.border_px } else { 0.0 };
    let uniforms = [
        Uniform::new(
            "node_color",
            (border_color.r(), border_color.g(), border_color.b(), 1.0f32),
        ),
        Uniform::new(
            "fill_color",
            (
                fill_color.r(),
                fill_color.g(),
                fill_color.b(),
                fill_color.a(),
            ),
        ),
        Uniform::new("rect_size", (rect.size.w as f32, rect.size.h as f32)),
        Uniform::new(
            "inner_rect_size",
            (
                (rect.size.w as f32 - border_px * 2.0).max(1.0),
                (rect.size.h as f32 - border_px * 2.0).max(1.0),
            ),
        ),
        Uniform::new(
            "inner_rect_offset",
            (border_px.max(0.0), border_px.max(0.0)),
        ),
        Uniform::new("corner_radius", corner_radius),
        Uniform::new("inner_corner_radius", (corner_radius - border_px).max(0.0)),
        Uniform::new("border_px", border_px),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        rect,
        &[damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )?;
    Ok(())
}
