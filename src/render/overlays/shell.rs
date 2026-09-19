use std::error::Error;

use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Buffer, Logical, Physical, Rectangle};

use crate::render::node::{LabelRenderElement, NodeRenderer, OverlayCardStyle};
use crate::render::scene::SceneElement;
use crate::render::text::UiTextRenderer;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayRgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl OverlayRgb {
    fn premultiplied_tuple(self) -> (f32, f32, f32, f32) {
        (self.r * self.a, self.g * self.a, self.b * self.a, self.a)
    }

    pub fn bytes(self) -> [u8; 3] {
        [
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    pub fn mix(self, other: Self, amount: f32) -> Self {
        let t = amount.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    fn luminance(self) -> f32 {
        self.r * 0.2126 + self.g * 0.7152 + self.b * 0.0722
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayVisuals {
    pub fill: OverlayRgb,
    pub text: OverlayRgb,
    pub error: OverlayRgb,
    pub subtext: OverlayRgb,
    pub key_fill: OverlayRgb,
    pub border: OverlayRgb,
    pub border_px: f32,
    pub radius: f32,
}
impl OverlayVisuals {
    pub(crate) fn label_chrome(mut self) -> Self {
        self.border_px = 0.0;
        self
    }
}

pub const fn backdrop_dim(alpha: f32) -> Color32F {
    Color32F::new(0.02, 0.03, 0.05, alpha)
}

const LIGHT_FILL: OverlayRgb = OverlayRgb {
    r: 0.92,
    g: 0.95,
    b: 0.98,
    a: 1.0,
};
const DARK_FILL: OverlayRgb = OverlayRgb {
    r: 0.15,
    g: 0.18,
    b: 0.22,
    a: 1.0,
};
const LIGHT_TEXT: OverlayRgb = OverlayRgb {
    r: 0.08,
    g: 0.10,
    b: 0.12,
    a: 1.0,
};
const DARK_TEXT: OverlayRgb = OverlayRgb {
    r: 0.94,
    g: 0.96,
    b: 0.98,
    a: 1.0,
};
const HALLEY_ACCENT: OverlayRgb = OverlayRgb {
    r: 0xd6 as f32 / 255.0,
    g: 0x5d as f32 / 255.0,
    b: 0x26 as f32 / 255.0,
    a: 1.0,
};

pub fn resolve_visuals(config: &halley_config::Overlays) -> OverlayVisuals {
    let fill = resolve_fill(config.background_color);
    let text = resolve_text(config.text_color, fill);
    let error = resolve_error(config.error_color);
    let border = resolve_border(config.border_color);
    OverlayVisuals {
        fill,
        text,
        error,
        subtext: text.mix(fill, 0.20),
        key_fill: fill.mix(text, 0.10),
        border,
        border_px: if config.borders {
            config.border_size_px.max(0) as f32
        } else {
            0.0
        },
        radius: config.radius_px.max(0) as f32,
    }
}

fn resolve_fill(mode: halley_config::OverlayColorMode) -> OverlayRgb {
    match mode {
        halley_config::OverlayColorMode::Auto
        | halley_config::OverlayColorMode::System
        | halley_config::OverlayColorMode::Light => LIGHT_FILL,
        halley_config::OverlayColorMode::Dark => DARK_FILL,
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
    }
}

fn resolve_text(mode: halley_config::OverlayColorMode, fill: OverlayRgb) -> OverlayRgb {
    match mode {
        halley_config::OverlayColorMode::Auto | halley_config::OverlayColorMode::System => {
            if fill.luminance() < 0.45 {
                DARK_TEXT
            } else {
                LIGHT_TEXT
            }
        }
        halley_config::OverlayColorMode::Light => LIGHT_TEXT,
        halley_config::OverlayColorMode::Dark => DARK_TEXT,
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
    }
}

fn resolve_error(mode: halley_config::OverlayColorMode) -> OverlayRgb {
    match mode {
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
        halley_config::OverlayColorMode::Auto
        | halley_config::OverlayColorMode::System
        | halley_config::OverlayColorMode::Light
        | halley_config::OverlayColorMode::Dark => OverlayRgb {
            r: 0xfb as f32 / 255.0,
            g: 0x49 as f32 / 255.0,
            b: 0x34 as f32 / 255.0,
            a: 1.0,
        },
    }
}

fn resolve_border(mode: halley_config::OverlayColorMode) -> OverlayRgb {
    match mode {
        halley_config::OverlayColorMode::Fixed { r, g, b, a } => OverlayRgb { r, g, b, a },
        halley_config::OverlayColorMode::Auto
        | halley_config::OverlayColorMode::System
        | halley_config::OverlayColorMode::Light
        | halley_config::OverlayColorMode::Dark => HALLEY_ACCENT,
    }
}

pub fn card_element(
    renderer: &mut GlesRenderer,
    node_renderer: &mut NodeRenderer,
    destination: Rectangle<i32, Physical>,
    visuals: OverlayVisuals,
    fill: OverlayRgb,
    alpha: f32,
) -> Result<LabelRenderElement, Box<dyn Error>> {
    node_renderer.overlay_card_element(
        renderer,
        destination,
        OverlayCardStyle {
            content_radius: visuals.radius,
            // Smithay composites GLES elements as premultiplied RGBA. The
            // configured colours are straight alpha, so premultiply their RGB
            // channels before the card shader applies its geometric mask.
            fill: fill.premultiplied_tuple(),
            border: visuals.border.premultiplied_tuple(),
            border_px: visuals.border_px,
            alpha,
        },
    )
}

/// Internal caption bands and badges were borderless in old Halley. They use
/// the shared overlay palette and shape, but never inherit the container
/// border setting.
pub fn label_card_element(
    renderer: &mut GlesRenderer,
    node_renderer: &mut NodeRenderer,
    destination: Rectangle<i32, Physical>,
    visuals: OverlayVisuals,
    fill: OverlayRgb,
    alpha: f32,
) -> Result<LabelRenderElement, Box<dyn Error>> {
    card_element(
        renderer,
        node_renderer,
        destination,
        visuals.label_chrome(),
        fill,
        alpha,
    )
}

pub fn elements(
    renderer: &mut GlesRenderer,
    output_geometry: Rectangle<i32, Logical>,
    snapshot: crate::shell::overlay::OverlaySnapshot,
    config: &halley_config::Overlays,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let visuals = resolve_visuals(config);
    let screen = Rectangle::<i32, Physical>::from_size(output_geometry.size.to_physical(1));
    let mut elements = Vec::new();
    if let Some(basics) = snapshot.basics {
        super::basics::elements(
            renderer,
            screen,
            basics,
            visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if let Some(mix) = snapshot.exit_mix {
        confirmation_elements(
            renderer,
            screen,
            mix,
            "Are you sure you want to leave?",
            None,
            "leave",
            visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if let Some(confirmation) = snapshot.confirmation {
        confirmation_elements(
            renderer,
            screen,
            1.0,
            &confirmation.title,
            Some(&confirmation.message),
            confirmation.confirm_label,
            visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if let Some(notification) = snapshot.notification {
        notification_elements(
            renderer,
            screen,
            notification,
            config.notifications.position,
            visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if let Some(indicator) = snapshot.cluster_indicator {
        cluster_indicator_elements(
            renderer,
            screen,
            indicator,
            visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    if config.zoom_indicator.enabled
        && let Some(zoom_indicator) = snapshot.zoom_indicator
    {
        let zoom_visuals =
            resolve_zoom_indicator_visuals(config.zoom_indicator, visuals, config.border_size_px);
        zoom_indicator_elements(
            renderer,
            screen,
            zoom_indicator,
            config.zoom_indicator,
            zoom_visuals,
            node_renderer,
            ui_text,
            &mut elements,
        )?;
    }
    Ok(elements)
}

#[allow(clippy::too_many_arguments)]
fn confirmation_elements(
    renderer: &mut GlesRenderer,
    screen: Rectangle<i32, Physical>,
    mix: f32,
    title: &str,
    message: Option<&str>,
    confirm_label: &str,
    visuals: OverlayVisuals,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let actions = [("Enter", confirm_label), ("Esc", "cancel")];
    let title_size = ui_text
        .measure(renderer, title, visuals.text.bytes())?
        .unwrap_or((0, 0).into());
    let message_size = message
        .map(|message| ui_text.measure(renderer, message, visuals.subtext.bytes()))
        .transpose()?
        .flatten()
        .unwrap_or((0, 0).into());
    let mut action_width = 0;
    let mut action_height = 0;
    let mut action_sizes = Vec::new();
    for (key, label) in actions {
        let key_size = ui_text
            .measure(renderer, key, visuals.text.bytes())?
            .unwrap_or((0, 0).into());
        let label_size = ui_text
            .measure(renderer, label, visuals.subtext.bytes())?
            .unwrap_or((0, 0).into());
        let chip_width = key_size.w + 16;
        action_width += chip_width + 8 + label_size.w + 22;
        action_height = action_height.max(key_size.h.max(label_size.h) + 8);
        action_sizes.push((key, label, key_size, label_size, chip_width));
    }
    action_width = action_width.saturating_sub(22);
    let card_width = (title_size.w.max(message_size.w).max(action_width) + 48)
        .max(280)
        .min((screen.size.w - 36).max(1));
    let message_block_height = message.map_or(0, |_| message_size.h + 12);
    let card_height = title_size.h + message_block_height + action_height + 54;
    let card = Rectangle::new(
        (
            screen.loc.x + (screen.size.w - card_width) / 2,
            screen.loc.y + (screen.size.h - card_height) / 2,
        )
            .into(),
        (card_width, card_height).into(),
    );
    let title_x = card.loc.x + 24;
    let title_y = card.loc.y + 18;
    if let Some(text) = ui_text.element(
        renderer,
        (title_x, title_y).into(),
        title,
        visuals.text.bytes(),
        mix,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    if let Some(message) = message
        && let Some(text) = ui_text.element(
            renderer,
            (title_x, title_y + title_size.h + 12).into(),
            message,
            visuals.subtext.bytes(),
            mix,
        )?
    {
        elements.push(SceneElement::UiText(text.element));
    }
    let mut x = card.loc.x + 24;
    let action_y = card.loc.y + card.size.h - action_height - 16;
    for (key, label, key_size, label_size, chip_width) in action_sizes {
        let chip = Rectangle::new((x, action_y).into(), (chip_width, action_height).into());
        if let Some(text) = ui_text.element(
            renderer,
            (chip.loc.x + 8, chip.loc.y + (chip.size.h - key_size.h) / 2).into(),
            key,
            visuals.text.bytes(),
            mix,
        )? {
            elements.push(SceneElement::UiText(text.element));
        }
        elements.push(SceneElement::NodeLabel(card_element(
            renderer,
            node_renderer,
            chip,
            OverlayVisuals {
                border_px: 0.0,
                ..visuals
            },
            visuals.key_fill,
            0.96 * mix,
        )?));
        x += chip_width + 8;
        if let Some(text) = ui_text.element(
            renderer,
            (x, action_y + (action_height - label_size.h) / 2).into(),
            label,
            visuals.subtext.bytes(),
            mix,
        )? {
            elements.push(SceneElement::UiText(text.element));
        }
        x += label_size.w + 22;
    }
    elements.push(SceneElement::NodeLabel(card_element(
        renderer,
        node_renderer,
        card,
        visuals,
        visuals.fill,
        0.97 * mix,
    )?));
    let color = backdrop_dim(0.62 * mix);
    elements.push(SceneElement::Border(crate::render::solid_color_element(
        node_renderer.active_slot_id(crate::render::node::NodeSlot::ShellBackdrop),
        screen,
        color,
    )));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn notification_elements(
    renderer: &mut GlesRenderer,
    screen: Rectangle<i32, Physical>,
    notification: crate::shell::overlay::NotificationSnapshot,
    position: halley_config::NotificationPosition,
    visuals: OverlayVisuals,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let max_text_width = ((screen.size.w as f32 * 0.70).round() as i32 - 32).max(80);
    let color = match notification.kind {
        crate::shell::overlay::NotificationKind::Success => visuals.text,
        crate::shell::overlay::NotificationKind::Error => visuals.error,
    };
    let (message, text_size) = fit_middle(
        renderer,
        ui_text,
        &notification.message,
        color.bytes(),
        max_text_width,
    )?;
    let card =
        Rectangle::<i32, Physical>::new((0, 0).into(), (text_size.w + 32, text_size.h + 16).into());
    let margin = 24;
    let slide = ((1.0 - notification.mix) * 8.0).round() as i32;
    let x = match position {
        halley_config::NotificationPosition::TopLeft
        | halley_config::NotificationPosition::BottomLeft => margin,
        halley_config::NotificationPosition::TopCenter
        | halley_config::NotificationPosition::BottomCenter => (screen.size.w - card.size.w) / 2,
        halley_config::NotificationPosition::TopRight
        | halley_config::NotificationPosition::BottomRight => screen.size.w - card.size.w - margin,
    };
    let y = match position {
        halley_config::NotificationPosition::TopLeft
        | halley_config::NotificationPosition::TopCenter
        | halley_config::NotificationPosition::TopRight => margin - slide,
        halley_config::NotificationPosition::BottomLeft
        | halley_config::NotificationPosition::BottomCenter
        | halley_config::NotificationPosition::BottomRight => {
            screen.size.h - card.size.h - margin + slide
        }
    };
    let card = Rectangle::new((x, y).into(), card.size);
    if let Some(text) = ui_text.element(
        renderer,
        (
            card.loc.x + 16,
            card.loc.y + (card.size.h - text_size.h) / 2,
        )
            .into(),
        &message,
        color.bytes(),
        notification.mix,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    elements.push(SceneElement::NodeLabel(card_element(
        renderer,
        node_renderer,
        card,
        visuals,
        visuals.fill,
        0.97 * notification.mix,
    )?));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cluster_indicator_elements(
    renderer: &mut GlesRenderer,
    screen: Rectangle<i32, Physical>,
    indicator: crate::shell::overlay::ClusterIndicatorSnapshot,
    visuals: OverlayVisuals,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let max_text_width = ((screen.size.w as f32 * 0.70).round() as i32 - 40).max(80);
    let (label, text_size) = fit_middle(
        renderer,
        ui_text,
        &indicator.label,
        visuals.text.bytes(),
        max_text_width,
    )?;
    let card_size =
        smithay::utils::Size::<i32, Physical>::from((text_size.w + 40, text_size.h + 24));
    let card = Rectangle::<i32, Physical>::new(
        (
            (screen.size.w - card_size.w) / 2,
            (screen.size.h - card_size.h) / 2,
        )
            .into(),
        card_size,
    );
    if let Some(text) = ui_text.element(
        renderer,
        (
            card.loc.x + 20,
            card.loc.y + (card.size.h - text_size.h) / 2,
        )
            .into(),
        &label,
        visuals.text.bytes(),
        visuals.text.a * indicator.mix,
    )? {
        elements.push(SceneElement::UiText(text.element));
    }
    elements.push(SceneElement::NodeLabel(card_element(
        renderer,
        node_renderer,
        card,
        visuals,
        visuals.fill,
        0.97 * indicator.mix,
    )?));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn zoom_indicator_elements(
    renderer: &mut GlesRenderer,
    screen: Rectangle<i32, Physical>,
    indicator: crate::shell::overlay::ZoomIndicatorSnapshot,
    config: halley_config::ZoomIndicator,
    visuals: OverlayVisuals,
    node_renderer: &mut NodeRenderer,
    ui_text: &mut UiTextRenderer,
    elements: &mut Vec<SceneElement>,
) -> Result<(), Box<dyn Error>> {
    let label = zoom_indicator_label(indicator.scale);
    let measured = match config.text_size {
        Some(size_px) => {
            ui_text.measure_at_size(renderer, &label, visuals.text.bytes(), size_px)?
        }
        None => ui_text.measure(renderer, &label, visuals.text.bytes())?,
    };
    let Some(text_size) = measured else {
        return Ok(());
    };
    let card = zoom_indicator_card_rect(screen, text_size, config.position);
    let origin = (
        card.loc.x + 16,
        card.loc.y + (card.size.h - text_size.h) / 2,
    )
        .into();
    let alpha = indicator.mix * config.opacity;
    let text = match config.text_size {
        Some(size_px) => ui_text.element_at_size(
            renderer,
            origin,
            &label,
            visuals.text.bytes(),
            visuals.text.a * alpha,
            size_px,
        )?,
        None => ui_text.element(
            renderer,
            origin,
            &label,
            visuals.text.bytes(),
            visuals.text.a * alpha,
        )?,
    };
    if let Some(text) = text {
        elements.push(SceneElement::UiText(text.element));
    }
    if config.background {
        elements.push(SceneElement::NodeLabel(card_element(
            renderer,
            node_renderer,
            card,
            visuals,
            visuals.fill,
            0.97 * alpha,
        )?));
    }
    Ok(())
}

fn resolve_zoom_indicator_visuals(
    config: halley_config::ZoomIndicator,
    mut visuals: OverlayVisuals,
    border_size_px: i32,
) -> OverlayVisuals {
    if let Some(mode) = config.background_color {
        visuals.fill = resolve_fill(mode);
    }
    if let Some(mode) = config.text_color {
        visuals.text = resolve_text(mode, visuals.fill);
    }
    if let Some(mode) = config.border_color {
        visuals.border = resolve_border(mode);
    }
    if let Some(borders) = config.borders {
        visuals.border_px = if borders {
            border_size_px.max(0) as f32
        } else {
            0.0
        };
    }
    if let Some(radius_px) = config.radius_px {
        visuals.radius = radius_px.max(0) as f32;
    }
    visuals
}

fn zoom_indicator_label(scale: f32) -> String {
    format!("{:.2}x", scale.clamp(0.0, 1.0))
}

fn zoom_indicator_card_rect(
    screen: Rectangle<i32, Physical>,
    text_size: smithay::utils::Size<i32, Buffer>,
    position: halley_config::NotificationPosition,
) -> Rectangle<i32, Physical> {
    let card_size: smithay::utils::Size<i32, Physical> =
        (text_size.w + 32, text_size.h + 16).into();
    let margin = 24;
    let x = match position {
        halley_config::NotificationPosition::TopLeft
        | halley_config::NotificationPosition::BottomLeft => margin,
        halley_config::NotificationPosition::TopCenter
        | halley_config::NotificationPosition::BottomCenter => (screen.size.w - card_size.w) / 2,
        halley_config::NotificationPosition::TopRight
        | halley_config::NotificationPosition::BottomRight => screen.size.w - card_size.w - margin,
    };
    let y = match position {
        halley_config::NotificationPosition::TopLeft
        | halley_config::NotificationPosition::TopCenter
        | halley_config::NotificationPosition::TopRight => margin,
        halley_config::NotificationPosition::BottomLeft
        | halley_config::NotificationPosition::BottomCenter
        | halley_config::NotificationPosition::BottomRight => screen.size.h - card_size.h - margin,
    };
    Rectangle::new((x, y).into(), card_size)
}

fn fit_middle(
    renderer: &mut GlesRenderer,
    ui_text: &mut UiTextRenderer,
    value: &str,
    color: [u8; 3],
    max_width: i32,
) -> Result<(String, smithay::utils::Size<i32, Buffer>), Box<dyn Error>> {
    let measured = ui_text
        .measure(renderer, value, color)?
        .unwrap_or((0, 0).into());
    if measured.w <= max_width {
        return Ok((value.to_string(), measured));
    }
    let chars = value.chars().collect::<Vec<_>>();
    for keep in (1..chars.len()).rev() {
        let left = keep.div_ceil(2);
        let right = keep / 2;
        let candidate = format!(
            "{}…{}",
            chars[..left].iter().collect::<String>(),
            chars[chars.len() - right..].iter().collect::<String>()
        );
        let measured = ui_text
            .measure(renderer, &candidate, color)?
            .unwrap_or((0, 0).into());
        if measured.w <= max_width {
            return Ok((candidate, measured));
        }
    }
    Ok(("…".to_string(), (max_width.min(8), measured.h).into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_auto_palette_is_light_with_dark_text() {
        let visuals = resolve_visuals(&halley_config::Overlays::default());
        assert_eq!(visuals.fill, LIGHT_FILL);
        assert_eq!(visuals.text, LIGHT_TEXT);
        assert_eq!(visuals.radius, 8.0);
    }

    #[test]
    fn dark_background_makes_auto_text_light() {
        let config = halley_config::Overlays {
            background_color: halley_config::OverlayColorMode::Dark,
            ..halley_config::Overlays::default()
        };
        assert_eq!(resolve_visuals(&config).text, DARK_TEXT);
    }

    #[test]
    fn overlay_border_size_does_not_inherit_window_border_width() {
        let config = halley_config::Overlays {
            border_size_px: 7,
            ..halley_config::Overlays::default()
        };

        assert_eq!(resolve_visuals(&config).border_px, 7.0);
    }

    #[test]
    fn internal_label_chrome_never_inherits_container_borders() {
        let visuals = resolve_visuals(&halley_config::Overlays::default());

        assert!(visuals.border_px > 0.0);
        assert_eq!(visuals.label_chrome().border_px, 0.0);
    }

    #[test]
    fn zoom_indicator_label_has_two_decimal_places() {
        assert_eq!(zoom_indicator_label(1.0), "1.00x");
        assert_eq!(zoom_indicator_label(0.754), "0.75x");
        assert_eq!(zoom_indicator_label(0.756), "0.76x");
    }

    #[test]
    fn translucent_overlay_colours_are_premultiplied_for_smithay() {
        let white_at_half_alpha = OverlayRgb {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 128.0 / 255.0,
        };

        assert_eq!(
            white_at_half_alpha.premultiplied_tuple(),
            (128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0,)
        );
    }

    #[test]
    fn zoom_indicator_visual_overrides_are_independent() {
        let shared = resolve_visuals(&halley_config::Overlays::default());
        let config = halley_config::ZoomIndicator {
            background_color: Some(halley_config::OverlayColorMode::Dark),
            text_color: Some(halley_config::OverlayColorMode::Auto),
            borders: Some(false),
            radius_px: Some(20),
            ..halley_config::ZoomIndicator::default()
        };

        let visuals = resolve_zoom_indicator_visuals(
            config,
            shared,
            halley_config::Overlays::default().border_size_px,
        );
        assert_eq!(visuals.fill, DARK_FILL);
        assert_eq!(visuals.text, DARK_TEXT);
        assert_eq!(visuals.border_px, 0.0);
        assert_eq!(visuals.radius, 20.0);
    }

    #[test]
    fn zoom_indicator_border_override_uses_overlay_border_size() {
        let shared_config = halley_config::Overlays {
            borders: false,
            border_size_px: 7,
            ..halley_config::Overlays::default()
        };
        let shared = resolve_visuals(&shared_config);
        let zoom = halley_config::ZoomIndicator {
            borders: Some(true),
            ..halley_config::ZoomIndicator::default()
        };

        assert_eq!(
            resolve_zoom_indicator_visuals(zoom, shared, shared_config.border_size_px).border_px,
            7.0
        );
    }

    #[test]
    fn zoom_indicator_visual_defaults_inherit_shared_style() {
        let shared_config = halley_config::Overlays {
            background_color: halley_config::OverlayColorMode::Dark,
            radius_px: 14,
            borders: false,
            ..halley_config::Overlays::default()
        };
        let shared = resolve_visuals(&shared_config);

        assert_eq!(
            resolve_zoom_indicator_visuals(
                halley_config::ZoomIndicator::default(),
                shared,
                shared_config.border_size_px,
            )
            .fill,
            shared.fill
        );
        assert_eq!(
            resolve_zoom_indicator_visuals(
                halley_config::ZoomIndicator::default(),
                shared,
                shared_config.border_size_px,
            )
            .radius,
            14.0
        );
    }

    #[test]
    fn zoom_indicator_card_uses_each_configured_anchor() {
        let screen = Rectangle::from_size((1_000, 800).into());
        let text_size = (48, 18).into();
        let expected = [
            (halley_config::NotificationPosition::TopLeft, (24, 24)),
            (halley_config::NotificationPosition::TopCenter, (460, 24)),
            (halley_config::NotificationPosition::TopRight, (896, 24)),
            (halley_config::NotificationPosition::BottomLeft, (24, 742)),
            (
                halley_config::NotificationPosition::BottomCenter,
                (460, 742),
            ),
            (halley_config::NotificationPosition::BottomRight, (896, 742)),
        ];
        for (position, location) in expected {
            let card = zoom_indicator_card_rect(screen, text_size, position);
            assert_eq!(card.loc, location.into());
            assert_eq!(card.size, (80, 34).into());
        }
    }
}
