//! The renderers compositor-drawn UI needs, borrowed together.
//!
//! Overlay cards, badges and dialogs all draw with the same three renderers:
//! the GLES renderer, the node renderer that owns the card and label shaders,
//! and the UI text renderer. Passing them as one value keeps signatures short
//! and lets a fourth renderer be added in one place instead of at every call.

use std::error::Error;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Size};

use super::node::{LabelRenderElement, NodeRenderer};
use super::overlays::shell::{OverlayRgb, OverlayVisuals, card_element, label_card_element};
use super::scene::SceneElement;
use super::text::UiTextRenderer;

pub(crate) struct DrawContext<'a> {
    pub renderer: &'a mut GlesRenderer,
    pub node: &'a mut NodeRenderer,
    pub text: &'a mut UiTextRenderer,
}

impl DrawContext<'_> {
    /// The size `text` would draw at, or zero when it draws nothing.
    pub fn text_size(
        &mut self,
        text: &str,
        color: [u8; 3],
    ) -> Result<Size<i32, Buffer>, Box<dyn Error>> {
        Ok(self
            .text
            .measure(self.renderer, text, color)?
            .unwrap_or((0, 0).into()))
    }

    /// Draw `text` with its top-left corner at `origin`.
    pub fn push_text(
        &mut self,
        elements: &mut Vec<SceneElement>,
        origin: Point<i32, Physical>,
        text: &str,
        color: [u8; 3],
        alpha: f32,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(text) = self
            .text
            .element(self.renderer, origin, text, color, alpha)?
        {
            elements.push(SceneElement::UiText(text.element));
        }
        Ok(())
    }

    /// An overlay card in the shared overlay style; see [`card_element`].
    pub fn card(
        &mut self,
        destination: Rectangle<i32, Physical>,
        visuals: OverlayVisuals,
        fill: OverlayRgb,
        alpha: f32,
    ) -> Result<LabelRenderElement, Box<dyn Error>> {
        card_element(self.renderer, self.node, destination, visuals, fill, alpha)
    }

    /// A borderless caption or badge card; see [`label_card_element`].
    pub fn label_card(
        &mut self,
        destination: Rectangle<i32, Physical>,
        visuals: OverlayVisuals,
        fill: OverlayRgb,
        alpha: f32,
    ) -> Result<LabelRenderElement, Box<dyn Error>> {
        label_card_element(self.renderer, self.node, destination, visuals, fill, alpha)
    }
}
