use std::f32::consts::TAU;

use smithay::{
    backend::renderer::{Color32F, Frame},
    utils::{Physical, Rectangle},
};

/// Draw an elliptical ring at a fixed screen-space position and radius.
///
/// All coordinates are in physical screen pixels. This stays decoupled from
/// world-space so HUD elements do not scale with the camera zoom.
pub(crate) fn draw_ring<F: Frame>(
    frame: &mut F,
    center_sx: f32,
    center_sy: f32,
    rx: f32,
    ry: f32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    let samples = 224;
    let thickness = 2.0f32;
    let mut prev: Option<(f32, f32)> = None;
    for i in 0..=samples {
        let t = (i as f32 / samples as f32) * TAU;
        let x = center_sx + t.cos() * rx;
        let y = center_sy + t.sin() * ry;
        if let Some((px, py)) = prev {
            let dx = x - px;
            let dy = y - py;
            let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
            for step in 0..=steps {
                let frac = step as f32 / steps as f32;
                let sx = px + dx * frac;
                let sy = py + dy * frac;
                draw_rect(
                    frame,
                    (sx - thickness * 0.5).round() as i32,
                    (sy - thickness * 0.5).round() as i32,
                    thickness.round().max(1.0) as i32,
                    thickness.round().max(1.0) as i32,
                    color,
                    damage,
                )?;
            }
        }
        prev = Some((x, y));
    }
    Ok(())
}

pub(crate) fn draw_rect<F: Frame>(
    frame: &mut F,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    let dst = Rectangle::new((x, y).into(), (w, h).into());
    frame.draw_solid(dst, &[damage], color)
}

pub(crate) fn draw_outline_rect<F: Frame>(
    frame: &mut F,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: Color32F,
    damage: Rectangle<i32, Physical>,
) -> Result<(), F::Error> {
    if w <= 1 || h <= 1 {
        return Ok(());
    }
    draw_rect(frame, x, y, w, 2, color, damage)?;
    draw_rect(frame, x, y + h - 2, w, 2, color, damage)?;
    draw_rect(frame, x, y, 2, h, color, damage)?;
    draw_rect(frame, x + w - 2, y, 2, h, color, damage)
}
