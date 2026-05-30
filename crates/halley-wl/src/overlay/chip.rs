use std::error::Error;

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
