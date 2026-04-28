use halley_config::ShadowLayerConfig;
use smithay::{
    backend::renderer::{
        Texture,
        gles::{GlesError, GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::render::state::RenderState;

pub(crate) fn draw_shadow_rect(
    frame: &mut GlesFrame<'_, '_>,
    render_state: &RenderState,
    config: ShadowLayerConfig,
    rect: Rectangle<i32, Physical>,
    corner_radius: f32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), GlesError> {
    if !config.enabled
        || config.color.a <= 0.0
        || config.blur_radius <= 0.0
        || alpha <= 0.0
        || rect.size.w <= 0
        || rect.size.h <= 0
    {
        return Ok(());
    }

    let Some(texture) = render_state.gpu.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = render_state.gpu.window_shadow_program.as_ref() else {
        return Ok(());
    };

    let blur_radius = config.blur_radius.max(0.0);
    let spread = config.spread.max(0.0);
    let falloff_extent = (blur_radius * 3.0).ceil().max(1.0) as i32;
    let pad = falloff_extent + spread.ceil() as i32 + 2;
    let offset_x = config.offset_x.round() as i32;
    let offset_y = config.offset_y.round() as i32;
    let dst = Rectangle::<i32, Physical>::new(
        (rect.loc.x + offset_x - pad, rect.loc.y + offset_y - pad).into(),
        (rect.size.w + pad * 2, rect.size.h + pad * 2).into(),
    );
    let Some(visible) = dst.intersection(damage) else {
        return Ok(());
    };
    let local_damage = Rectangle::<i32, Physical>::new(
        (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
        visible.size,
    );

    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    // Position of the shadow inside the padded shadow quad.
    // The quad `dst` is already offset on the screen, so placing the shadow
    // in the center of `dst` will naturally result in an offset shadow.
    let caster_center = (
        pad as f32 + rect.size.w as f32 * 0.5,
        pad as f32 + rect.size.h as f32 * 0.5,
    );

    let uniforms = [
        Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
        Uniform::new("caster_size", (rect.size.w as f32, rect.size.h as f32)),
        Uniform::new("caster_center", caster_center),
        Uniform::new("corner_radius", corner_radius.max(0.0)),
        Uniform::new("spread", spread),
        Uniform::new("shadow_radius", blur_radius),
        Uniform::new(
            "shadow_color",
            (
                config.color.r.clamp(0.0, 1.0),
                config.color.g.clamp(0.0, 1.0),
                config.color.b.clamp(0.0, 1.0),
                config.color.a.clamp(0.0, 1.0),
            ),
        ),
    ];

    frame.render_texture_from_to(
        texture,
        src,
        dst,
        &[local_damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )
}
