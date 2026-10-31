use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Physical, Rectangle, Size};

use super::shell::{card_element, resolve_visuals};
use crate::render::node::NodeRenderer;
use crate::render::scene::SceneElement;
use crate::render::text::UiTextRenderer;

const FPS_EDGE_PAD: i32 = 20;
const FPS_PAD_X: i32 = 18;
const FPS_PAD_Y: i32 = 10;
const FPS_CORNER_RADIUS: f32 = 14.0;
const FPS_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
struct FpsSampler {
    sampled_at: Duration,
    frames: u32,
    fps: f32,
}

#[derive(Default)]
pub struct DebugFpsOverlay {
    samplers: HashMap<String, FpsSampler>,
}

impl DebugFpsOverlay {
    fn sample(&mut self, output: &str, now: Duration) -> f32 {
        let sampler = self
            .samplers
            .entry(output.to_string())
            .or_insert(FpsSampler {
                sampled_at: now,
                frames: 0,
                fps: 0.0,
            });
        sampler.frames = sampler.frames.saturating_add(1);
        let elapsed = now.saturating_sub(sampler.sampled_at);
        if elapsed >= FPS_SAMPLE_INTERVAL {
            sampler.fps = sampler.frames as f32 / elapsed.as_secs_f32().max(0.001);
            sampler.frames = 0;
            sampler.sampled_at = now;
        }
        sampler.fps
    }
}

#[allow(clippy::too_many_arguments)]
pub fn elements(
    renderer: &mut GlesRenderer,
    output: &str,
    now: Duration,
    state: &mut DebugFpsOverlay,
    overlay_config: &halley_config::Overlays,
    _decorations: &halley_config::Decorations,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let label = fps_label(state.sample(output, now));
    let mut visuals = resolve_visuals(overlay_config);
    visuals.radius = FPS_CORNER_RADIUS;
    let Some(text_size) = ui_text.measure(renderer, &label, visuals.text.bytes())? else {
        return Ok(Vec::new());
    };
    let card = fps_card_rect(text_size);
    let mut elements = Vec::with_capacity(2);
    if let Some(text) = ui_text.element(
        renderer,
        (card.loc.x + FPS_PAD_X, card.loc.y + FPS_PAD_Y).into(),
        &label,
        visuals.text.bytes(),
        1.0,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    let mut fill = visuals.fill;
    fill.a *= 0.88;
    elements.push(SceneElement::NodeLabel(card_element(
        renderer,
        node_renderer,
        card,
        visuals,
        fill,
        1.0,
    )?));
    Ok(elements)
}

fn fps_label(fps: f32) -> String {
    format!("{:.0} FPS", fps.clamp(0.0, 999.0))
}

fn fps_card_rect(text_size: Size<i32, Buffer>) -> Rectangle<i32, Physical> {
    Rectangle::new(
        (FPS_EDGE_PAD, FPS_EDGE_PAD).into(),
        (
            text_size.w.saturating_add(FPS_PAD_X * 2).max(1),
            text_size.h.saturating_add(FPS_PAD_Y * 2).max(1),
        )
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuous_samples_keep_the_displayed_rate_live_when_frame_cadence_changes() {
        let mut state = DebugFpsOverlay::default();
        state.sample("DP-1", Duration::ZERO);
        for tick in 1..=50 {
            state.sample("DP-1", Duration::from_millis(tick * 10));
        }
        assert_eq!(fps_label(state.samplers["DP-1"].fps), "100 FPS");
        for tick in 1..=12 {
            assert_eq!(
                state.sample("DP-1", Duration::from_millis(500 + tick * 20)),
                100.0
            );
        }
        assert_eq!(
            fps_label(state.sample("DP-1", Duration::from_millis(760))),
            "50 FPS"
        );
    }
}
