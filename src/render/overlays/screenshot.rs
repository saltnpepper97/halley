use std::error::Error;

use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::utils::{Logical, Physical, Rectangle};

use super::shell::{OverlayVisuals, fit_middle};
use crate::render::draw::DrawContext;
use crate::render::scene::SceneElement;
use crate::shell::overlay::NotificationSnapshot;
use crate::shell::screenshot::{ScreenshotAction, layout};

pub fn elements(
    ctx: &mut DrawContext<'_>,
    screen: Rectangle<i32, Physical>,
    snapshot: NotificationSnapshot,
    config: halley_config::Notifications,
    visuals: OverlayVisuals,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let preview = snapshot
        .screenshot
        .as_ref()
        .expect("screenshot notification");
    let layout = layout(
        Rectangle::<i32, Logical>::from_size(screen.size.to_logical(1)),
        config,
        snapshot.mix,
        ctx.text.font_size(),
    );
    let physical = |r: Rectangle<i32, Logical>| r.to_physical(1);
    let area = physical(layout.preview);
    let scale = (area.size.w as f64 / preview.size.0 as f64)
        .min(area.size.h as f64 / preview.size.1 as f64);
    let size = (
        (preview.size.0 as f64 * scale).round() as i32,
        (preview.size.1 as f64 * scale).round() as i32,
    );
    let location = (
        f64::from(area.loc.x + (area.size.w - size.0) / 2),
        f64::from(area.loc.y + (area.size.h - size.1) / 2),
    );
    elements.push(SceneElement::ScreenshotPreview(
        MemoryRenderBufferRenderElement::from_buffer(
            ctx.renderer,
            location,
            &preview.buffer,
            Some(snapshot.mix),
            None,
            Some(size.into()),
            Kind::Unspecified,
        )?,
    ));
    elements.push(SceneElement::NodeLabel(ctx.card(
        area,
        visuals.label_chrome(),
        visuals.key_fill,
        snapshot.mix,
    )?));

    for (rect, label, action) in [
        (layout.copy, "Copy", ScreenshotAction::Copy),
        (layout.open, "Open", ScreenshotAction::Open),
    ] {
        let rect = physical(rect);
        let hovered = snapshot.hovered_action == Some(action);
        let fill = if hovered {
            visuals.key_fill.mix(visuals.border, 0.18)
        } else {
            visuals.key_fill
        };
        text(
            ctx,
            rect,
            label,
            visuals.text.bytes(),
            snapshot.mix,
            true,
            elements,
        )?;
        elements.push(SceneElement::NodeLabel(ctx.card(
            rect,
            visuals.label_chrome(),
            fill,
            snapshot.mix,
        )?));
    }
    text(
        ctx,
        physical(layout.title),
        &snapshot.message,
        visuals.text.bytes(),
        snapshot.mix,
        false,
        elements,
    )?;
    let directory = preview
        .path
        .parent()
        .unwrap_or(&preview.path)
        .display()
        .to_string();
    text(
        ctx,
        physical(layout.subtitle),
        &directory,
        visuals.subtext.bytes(),
        snapshot.mix,
        false,
        elements,
    )?;
    elements.push(SceneElement::NodeLabel(ctx.card(
        physical(layout.card),
        visuals,
        visuals.fill,
        snapshot.mix,
    )?));
    Ok(())
}

fn text(
    ctx: &mut DrawContext<'_>,
    rect: Rectangle<i32, Physical>,
    label: &str,
    rgb: [u8; 3],
    alpha: f32,
    centered: bool,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let (label, size) = fit_middle(ctx, label, rgb, rect.size.w - if centered { 12 } else { 0 })?;
    let x = rect.loc.x
        + if centered {
            (rect.size.w - size.w) / 2
        } else {
            0
        };
    if let Some(text) = ctx.text.element(
        ctx.renderer,
        (x, rect.loc.y + (rect.size.h - size.h) / 2).into(),
        &label,
        rgb,
        alpha,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    Ok(())
}
