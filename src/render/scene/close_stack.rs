//! Compose closing-window snapshots into the desktop stack.

use super::*;

pub(super) struct ClosingSceneContext<'a> {
    pub animations: &'a mut crate::render::close::WindowCloseAnimations,
    pub shaders: &'a mut crate::render::window_shader::WindowAnimationShaders,
    pub decorations: &'a mut crate::render::window_decoration::WindowDecorationRenderer,
    pub shadow_renderer: &'a mut crate::render::effects::shadow::ShadowRenderer,
    pub shadow_style: halley_config::ShadowLayer,
    pub cameras: &'a crate::presentation::camera::OutputCameras,
    pub now: std::time::Duration,
    pub redact: bool,
}

pub(super) fn stack_groups(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    context: ClosingSceneContext<'_>,
) -> Result<Vec<StackGroup>, Box<dyn Error>> {
    let stack = context
        .animations
        .renders_for_output(
            renderer,
            context.shaders,
            output,
            output_geometry,
            context.cameras,
            context.now,
        )
        .into_iter()
        .map(|closing| -> Result<StackGroup, Box<dyn Error>> {
            if context.redact {
                return Ok(StackGroup {
                    window: None,
                    stack_index: closing.stack_index,
                    order: closing.order,
                    elements: Vec::new(),
                });
            }
            let mut elements = Vec::new();
            let shader = closing.shader;
            let shader_active = shader.is_some();
            let shadow_alpha = closing.texture.alpha();
            let border_width = closing.border.map(|border| border.width).unwrap_or(0);
            let rounded = shader.is_none()
                && closing.content_radius > 0.0
                && context.decorations.available(renderer);
            if let Some(border) = closing.border {
                if rounded
                    && let Some(border) = context.decorations.border_element(
                        renderer,
                        closing.border_id.clone(),
                        closing.destination,
                        border.width,
                        closing.content_radius,
                        border.color,
                        1.0,
                    )
                {
                    elements.push(SceneElement::WindowBorder(border));
                } else {
                    elements.extend(
                        crate::render::border_strips(
                            std::array::from_fn(|index| closing.border_id.namespaced(100 + index)),
                            closing.destination,
                            border.width,
                            border.color,
                        )
                        .into_iter()
                        .map(SceneElement::Border),
                    );
                }
            }
            if let Some(shader) = shader {
                elements.push(SceneElement::WindowShader(shader));
            } else if rounded {
                let texture = context
                    .decorations
                    .texture_element(
                        renderer,
                        closing.texture,
                        closing.source_texture,
                        closing.destination,
                        closing.content_radius,
                    )
                    .expect("rounded resources were checked above");
                elements.push(SceneElement::RoundedTexture(texture));
            } else {
                elements.push(SceneElement::Closing(closing.texture));
            }
            let caster = Rectangle::new(
                (
                    closing.destination.loc.x - border_width,
                    closing.destination.loc.y - border_width,
                )
                    .into(),
                (
                    closing.destination.size.w + border_width * 2,
                    closing.destination.size.h + border_width * 2,
                )
                    .into(),
            );
            // The rectangular caster cannot follow an arbitrary shader's
            // fragment silhouette. Let the shader own the complete close
            // visual instead of leaving a detached shadow on the trajectory.
            let mut shadow_config = context.shadow_style;
            if let Some(enabled) = closing.shadow {
                shadow_config.enabled = enabled;
            }
            if !shader_active
                && let Some(shadow) = context.shadow_renderer.element(
                    renderer,
                    format!("{}:closing:{}", output.name(), closing.order),
                    caster,
                    if rounded {
                        closing.content_radius + border_width as f32
                    } else {
                        0.0
                    },
                    shadow_alpha,
                    shadow_config,
                )?
            {
                elements.push(SceneElement::Shadow(shadow));
            }
            Ok(StackGroup {
                window: None,
                stack_index: closing.stack_index,
                order: closing.order,
                elements,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(stack)
}
