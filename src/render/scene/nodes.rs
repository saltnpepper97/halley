use super::*;

pub(super) struct NodeElementContext<'a> {
    pub(super) output: &'a Output,
    pub(super) output_geometry: Rectangle<i32, Logical>,
    pub(super) nodes: &'a crate::nodes::NodesState,
    pub(super) node_grab_active: bool,
    pub(super) cameras: &'a crate::presentation::camera::OutputCameras,
    pub(super) decorations: &'a halley_config::Decorations,
    pub(super) pins: &'a halley_config::Pins,
    pub(super) overlays: &'a halley_config::Overlays,
    pub(super) pin_renderer: &'a mut crate::render::pin::PinRenderer,
    pub(super) debug: halley_config::Debug,
    pub(super) shadow_config: halley_config::ShadowLayer,
    pub(super) shadow_renderer: &'a mut crate::render::effects::shadow::ShadowRenderer,
    pub(super) now: std::time::Duration,
}

pub(super) fn node_elements(
    renderer: &mut GlesRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    context: NodeElementContext<'_>,
) -> Result<NodeScene, Box<dyn Error>> {
    let NodeElementContext {
        output,
        output_geometry,
        nodes,
        node_grab_active,
        cameras,
        decorations,
        pins,
        overlays,
        pin_renderer,
        debug,
        shadow_config,
        shadow_renderer,
        now,
    } = context;
    let Some(camera) = cameras.get(&output.name()) else {
        return Ok(NodeScene::default());
    };
    let focused = nodes.config.border_color_highlighted;
    let mut overlay = Vec::new();

    // Resolve node icons while their windows are still live. Otherwise the
    // first decay starts the asynchronous load only after the node appears;
    // when that load completes after the fade window, the icon pops in at full
    // opacity. Later decays only looked correct because they reused the cache.
    if nodes.config.show_app_icons != halley_config::NodeDisplayPolicy::Off {
        for app_id in nodes.records().filter_map(|record| {
            (record.attached && record.output == output.name())
                .then_some(record.app_id.as_deref())
                .flatten()
        }) {
            node_renderer.request_app_icon(renderer, app_id);
        }
    }

    if debug.show_focus_ring || nodes.ring_is_previewed(&output.name(), now) {
        let focus_ring = nodes.focus_ring_for_output(&output.name());
        let scale = crate::presentation::camera::scale(camera);
        let center = (
            output_geometry.size.w as f32 / 2.0 + focus_ring.offset_x * scale,
            output_geometry.size.h as f32 / 2.0 + focus_ring.offset_y * scale,
        );
        overlay.push(SceneElement::FocusRing(node_renderer.focus_ring_element(
            renderer,
            &output.name(),
            center,
            (focus_ring.radius_x * scale, focus_ring.radius_y * scale),
            (focused.r, focused.g, focused.b),
            0.82,
        )?));
    }

    let mut records = nodes
        .collapsed_on_output(&output.name())
        .collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.id.as_u64()));
    let label_hovered = nodes.label_hovered_on_output(&output.name(), now);
    let mut groups = Vec::new();

    for record in records {
        let Some(node) = nodes.field.node(record.id) else {
            continue;
        };
        let mut icons = Vec::new();
        let mut markers = Vec::new();
        let landmark_position = nodes.landmark_position(record.id, node.pos, now);
        let center = crate::nodes::screen_from_world(landmark_position, camera, output_geometry);
        let local = center - output_geometry.loc;
        let progress = if nodes.animations_enabled
            && nodes.animation.enabled
            && nodes.animation.duration_ms > 0
        {
            (now.saturating_sub(record.collapsed_at).as_secs_f32()
                / (nodes.animation.duration_ms as f32 / 1_000.0))
                .clamp(0.0, 1.0)
        } else {
            1.0
        };
        let eased = 1.0 - (1.0 - progress).powi(3);
        let side = (crate::nodes::NODE_DIAMETER_PX * (0.24 + 0.76 * eased))
            .round()
            .max(1.0) as i32;
        let destination = Rectangle::<i32, Physical>::new(
            (local.x - side / 2, local.y - side / 2).into(),
            (side, side).into(),
        );
        let hovered = nodes.hovered == Some(record.id);
        let highlighted = hovered || nodes.focused() == Some(record.id);
        let ring = node_ring_color(nodes.config, highlighted);
        let fill = node_fill_color(nodes.config, ring);
        markers.push(SceneElement::Node(node_renderer.element(
            renderer,
            destination,
            nodes.config.shape,
            crate::render::node::NodeStyle {
                border_rgb: ring,
                fill_rgb: fill,
                opacity: nodes.config.opacity,
                flat_fill: !matches!(
                    nodes.config.background_color,
                    halley_config::NodeBackgroundColor::Auto
                        | halley_config::NodeBackgroundColor::System
                ),
            },
        )?));
        if let Some(shadow) = shadow_renderer.element(
            renderer,
            format!("{}:node:{}", output.name(), record.id.as_u64()),
            destination,
            match nodes.config.shape {
                halley_config::NodeShape::Square => 0.0,
                halley_config::NodeShape::Squircle => side as f32 * 0.21,
            },
            nodes.config.opacity * eased,
            shadow_config,
        )? {
            markers.push(SceneElement::Shadow(shadow));
        }

        let elapsed_ms = now.saturating_sub(record.collapsed_at).as_millis() as f32;
        let icon_alpha = (((elapsed_ms - 1_000.0) / 220.0).clamp(0.0, 1.0) * nodes.config.opacity)
            .clamp(0.0, 1.0);
        let allow_real = nodes.config.show_app_icons == halley_config::NodeDisplayPolicy::Always
            || (nodes.config.show_app_icons == halley_config::NodeDisplayPolicy::Hover
                && highlighted);
        if allow_real && let Some(app_id) = record.app_id.as_deref() {
            node_renderer.request_app_icon(renderer, app_id);
        }
        if icon_alpha > 0.001 {
            let icon_side = ((crate::nodes::NODE_DIAMETER_PX * nodes.config.icon_size).round()
                as i32)
                .clamp(16, 42);
            let real_icon = allow_real
                .then_some(record.app_id.as_deref())
                .flatten()
                .and_then(|app_id| {
                    node_renderer.app_icon_element(
                        renderer,
                        app_id,
                        Rectangle::new(
                            (local.x - icon_side / 2, local.y - icon_side / 2).into(),
                            (icon_side, icon_side).into(),
                        ),
                        icon_alpha,
                    )
                });
            if let Some(icon) = real_icon {
                icons.push(SceneElement::NodeTexture(icon));
            }
        }

        let hover_mix = match (node_grab_active, nodes.config.show_labels) {
            (true, halley_config::NodeDisplayPolicy::Hover) => {
                nodes.label_hover_mix(record.id, false)
            }
            (true, _) | (_, halley_config::NodeDisplayPolicy::Off) => 0.0,
            (false, halley_config::NodeDisplayPolicy::Hover) => {
                nodes.label_hover_mix(record.id, label_hovered == Some(record.id))
            }
            (false, halley_config::NodeDisplayPolicy::Always) => 1.0,
        };
        let mut elements = landmark_label_elements(
            renderer,
            node_renderer,
            ui_text,
            LandmarkLabel {
                center: (local.x, local.y),
                marker_side: side,
                output_size: (output_geometry.size.w, output_geometry.size.h),
                text: &node.label,
                shape: nodes.config.label_shape,
                fill,
                ring,
                hover_mix,
                alpha: eased,
            },
        )?;
        if node.pinned
            && let Some(pin) = pin_renderer.element(
                renderer,
                &output.name(),
                crate::render::pin::PinSlot::Node(record.id.as_u64()),
                crate::render::pin::landmark_badge_rect(pins, (local.x, local.y), side),
                eased,
                pins,
                overlays,
                decorations,
            )
        {
            elements.insert(0, SceneElement::Closing(pin));
        }
        elements.extend(icons);
        elements.extend(markers);
        groups.push(StackGroup {
            window: None,
            stack_index: record.collapsed_stack_index.unwrap_or(usize::MAX),
            // Live windows use u64::MAX. At an equal shifted stack index this
            // lower order keeps the node and its shrinking snapshot behind
            // the same window that was above it before unmapping.
            order: 0,
            elements,
        });
    }
    Ok(NodeScene { overlay, groups })
}

#[derive(Default)]
pub(super) struct NodeScene {
    pub(super) overlay: Vec<SceneElement>,
    pub(super) groups: Vec<StackGroup>,
}

pub(crate) struct LandmarkLabel<'a> {
    pub(crate) center: (i32, i32),
    pub(crate) marker_side: i32,
    pub(crate) output_size: (i32, i32),
    pub(crate) text: &'a str,
    pub(crate) shape: halley_config::NodeShape,
    pub(crate) fill: (f32, f32, f32),
    pub(crate) ring: (f32, f32, f32),
    pub(crate) hover_mix: f32,
    pub(crate) alpha: f32,
}

pub(crate) fn landmark_label_elements(
    renderer: &mut GlesRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    label: LandmarkLabel<'_>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    landmark_label_elements_inner(renderer, node_renderer, ui_text, label, None)
}

pub(crate) fn landmark_label_elements_avoiding(
    renderer: &mut GlesRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    label: LandmarkLabel<'_>,
    obstacles: &[Rectangle<i32, Physical>],
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    landmark_label_elements_inner(renderer, node_renderer, ui_text, label, Some(obstacles))
}

fn landmark_label_elements_inner(
    renderer: &mut GlesRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    label: LandmarkLabel<'_>,
    obstacles: Option<&[Rectangle<i32, Physical>]>,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let reveal = ease_in_out_cubic(label.hover_mix * label.hover_mix * label.hover_mix);
    let fade = ((reveal - 0.30) / 0.55).clamp(0.0, 1.0);
    if fade <= 0.01 {
        return Ok(Vec::new());
    }
    let slide = ((reveal - 0.15) / 0.65).clamp(0.0, 1.0);
    let grow = ((reveal - 0.40) / 0.55).clamp(0.0, 1.0);
    let base_width = ((label.text.chars().count() as f32 * 9.5).round() as i32).clamp(72, 420);
    let width = even(((base_width as f32 * (1.0 + 0.80 * grow)).round() as i32).clamp(72, 240));
    let height = even((26.0 * (1.0 + 0.55 * grow)).round() as i32);
    let gap = (14.0 * (1.0 + 0.45 * grow)).round() as i32;
    let target_width = even(((base_width as f32 * 1.80).round() as i32).clamp(72, 240));
    let target_height = even((26.0_f32 * 1.55).round() as i32);
    let target_gap = (14.0_f32 * 1.45).round() as i32;
    let margin = 12;
    let placement = if let Some(obstacles) = obstacles {
        choose_label_placement(
            label.center,
            label.marker_side,
            label.output_size,
            (target_width, target_height),
            target_gap,
            obstacles,
        )
    } else if label.center.0 + label.marker_side / 2 + gap.max(10) + target_width + margin
        > label.output_size.0
    {
        LabelPlacement::Left
    } else {
        LabelPlacement::Right
    };
    let mut destination = label_candidate(
        placement,
        label.center,
        label.marker_side,
        (width, height),
        gap,
    );
    let slide_offset = placement.slide_offset();
    destination.loc.x += (slide_offset.0 as f32 * (1.0 - slide)).round() as i32;
    destination.loc.y += (slide_offset.1 as f32 * (1.0 - slide)).round() as i32;
    let destination = Rectangle::<i32, Physical>::new(
        (
            destination
                .loc
                .x
                .clamp(margin, (label.output_size.0 - width - margin).max(margin)),
            destination
                .loc
                .y
                .clamp(margin, (label.output_size.1 - height - margin).max(margin)),
        )
            .into(),
        (width, height).into(),
    );
    let label_fill = label_fill_color(label.fill, label.ring);
    let text_rgb = contrast_text_rgb(label_fill);
    let (text, text_size) = fit_node_label(renderer, ui_text, label.text, text_rgb, width - 20)?;
    let mut elements = Vec::new();
    if !text.is_empty()
        && let Some(prepared) = ui_text.element(
            renderer,
            (
                destination.loc.x + (width - text_size.w).max(0) / 2,
                destination.loc.y + (height - text_size.h).max(0) / 2,
            )
                .into(),
            &text,
            text_rgb,
            0.94 * label.alpha * fade,
        )?
    {
        elements.push(SceneElement::UiText(prepared.element));
    }
    elements.push(SceneElement::NodeLabel(node_renderer.label_element(
        renderer,
        destination,
        label.shape,
        label_fill,
        1.0,
    )?));
    Ok(elements)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LabelPlacement {
    Right,
    Left,
    Down,
    Up,
    DownRight,
    DownLeft,
    UpRight,
    UpLeft,
}

impl LabelPlacement {
    const PREFERENCE: [Self; 8] = [
        Self::Right,
        Self::Left,
        Self::Down,
        Self::Up,
        Self::DownRight,
        Self::DownLeft,
        Self::UpRight,
        Self::UpLeft,
    ];

    fn slide_offset(self) -> (i32, i32) {
        match self {
            Self::Right => (-44, 10),
            Self::Left => (44, 10),
            Self::Down => (0, -44),
            Self::Up => (0, 44),
            Self::DownRight => (-31, -31),
            Self::DownLeft => (31, -31),
            Self::UpRight => (-31, 31),
            Self::UpLeft => (31, 31),
        }
    }
}

fn label_candidate(
    placement: LabelPlacement,
    center: (i32, i32),
    marker_side: i32,
    label_size: (i32, i32),
    gap: i32,
) -> Rectangle<i32, Physical> {
    let (width, height) = label_size;
    let offset = marker_side / 2 + gap.max(10);
    let centered_x = center.0 - width / 2;
    let centered_y = center.1 - height / 2;
    let left = center.0 - offset - width;
    let right = center.0 + offset;
    let up = center.1 - offset - height;
    let down = center.1 + offset;
    let location = match placement {
        LabelPlacement::Right => (right, centered_y),
        LabelPlacement::Left => (left, centered_y),
        LabelPlacement::Down => (centered_x, down),
        LabelPlacement::Up => (centered_x, up),
        LabelPlacement::DownRight => (right, down),
        LabelPlacement::DownLeft => (left, down),
        LabelPlacement::UpRight => (right, up),
        LabelPlacement::UpLeft => (left, up),
    };
    Rectangle::new(location.into(), label_size.into())
}

fn choose_label_placement(
    center: (i32, i32),
    marker_side: i32,
    output_size: (i32, i32),
    label_size: (i32, i32),
    gap: i32,
    obstacles: &[Rectangle<i32, Physical>],
) -> LabelPlacement {
    const MARGIN: i32 = 12;
    const OBSTACLE_CLEARANCE: i32 = 8;

    LabelPlacement::PREFERENCE
        .into_iter()
        .enumerate()
        .map(|(preference, placement)| {
            let candidate = label_candidate(placement, center, marker_side, label_size, gap);
            let overflow = i64::from((MARGIN - candidate.loc.x).max(0))
                + i64::from((MARGIN - candidate.loc.y).max(0))
                + i64::from((candidate.loc.x + candidate.size.w + MARGIN - output_size.0).max(0))
                + i64::from((candidate.loc.y + candidate.size.h + MARGIN - output_size.1).max(0));
            let overlap = obstacles
                .iter()
                .map(|obstacle| {
                    Rectangle::<i32, Physical>::new(
                        (
                            obstacle.loc.x - OBSTACLE_CLEARANCE,
                            obstacle.loc.y - OBSTACLE_CLEARANCE,
                        )
                            .into(),
                        (
                            obstacle.size.w + OBSTACLE_CLEARANCE * 2,
                            obstacle.size.h + OBSTACLE_CLEARANCE * 2,
                        )
                            .into(),
                    )
                })
                .filter_map(|obstacle| candidate.intersection(obstacle))
                .map(|overlap| i64::from(overlap.size.w) * i64::from(overlap.size.h))
                .sum::<i64>();
            ((overflow, overlap, preference), placement)
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, placement)| placement)
        .unwrap_or(LabelPlacement::Right)
}

pub(super) fn fit_node_label(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    source: &str,
    rgb: [u8; 3],
    available: i32,
) -> Result<(String, smithay::utils::Size<i32, smithay::utils::Buffer>), Box<dyn Error>> {
    fit_ui_text(renderer, ui_text, source, rgb, available)
}

pub(super) fn fit_ui_text(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    source: &str,
    rgb: [u8; 3],
    available: i32,
) -> Result<(String, smithay::utils::Size<i32, smithay::utils::Buffer>), Box<dyn Error>> {
    let text = source.trim();
    let Some(size) = ui_text.measure(renderer, text, rgb)? else {
        return Ok((String::new(), (0, 0).into()));
    };
    if size.w <= available {
        return Ok((text.to_string(), size));
    }

    let characters = text.chars().collect::<Vec<_>>();
    for keep in (0..characters.len()).rev() {
        let candidate = characters[..keep]
            .iter()
            .copied()
            .chain(std::iter::once('…'))
            .collect::<String>();
        let Some(size) = ui_text.measure(renderer, &candidate, rgb)? else {
            continue;
        };
        if size.w <= available {
            return Ok((candidate, size));
        }
    }
    Ok((String::new(), (0, 0).into()))
}

pub(crate) fn node_ring_color(nodes: halley_config::Nodes, highlighted: bool) -> (f32, f32, f32) {
    let color = if highlighted {
        nodes.border_color_highlighted
    } else {
        nodes.border_color
    };
    (color.r, color.g, color.b)
}

pub(crate) fn node_fill_color(
    config: halley_config::Nodes,
    ring: (f32, f32, f32),
) -> (f32, f32, f32) {
    match config.background_color {
        halley_config::NodeBackgroundColor::Auto | halley_config::NodeBackgroundColor::System => (
            0.94 * 0.86 + ring.0 * 0.14,
            0.96 * 0.86 + ring.1 * 0.14,
            0.985 * 0.86 + ring.2 * 0.14,
        ),
        halley_config::NodeBackgroundColor::Light => (0.92, 0.95, 0.98),
        halley_config::NodeBackgroundColor::Dark => (0.15, 0.18, 0.22),
        halley_config::NodeBackgroundColor::Fixed(r, g, b) => (r, g, b),
    }
}

pub(super) fn label_fill_color(fill: (f32, f32, f32), ring: (f32, f32, f32)) -> (f32, f32, f32) {
    (
        fill.0 * 0.90 + ring.0 * 0.10,
        fill.1 * 0.90 + ring.1 * 0.10,
        fill.2 * 0.90 + ring.2 * 0.10,
    )
}

pub(crate) fn contrast_text_rgb(fill: (f32, f32, f32)) -> [u8; 3] {
    let luminance = fill.0 * 0.2126 + fill.1 * 0.7152 + fill.2 * 0.0722;
    let rgb = if luminance >= 0.45 {
        (0.08, 0.10, 0.12)
    } else {
        (0.96, 0.98, 1.0)
    };
    [
        (rgb.0 * 255.0) as u8,
        (rgb.1 * 255.0) as u8,
        (rgb.2 * 255.0) as u8,
    ]
}

pub(super) fn ease_in_out_cubic(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value < 0.5 {
        4.0 * value * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(3) / 2.0
    }
}

pub(super) fn even(value: i32) -> i32 {
    (value + 1) & !1
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: (i32, i32) = (500, 300);
    const OUTPUT: (i32, i32) = (1_000, 600);
    const LABEL: (i32, i32) = (180, 40);

    fn obstacle(location: (i32, i32), size: (i32, i32)) -> Rectangle<i32, Physical> {
        Rectangle::new(location.into(), size.into())
    }

    #[test]
    fn adaptive_label_keeps_the_normal_right_side_when_it_is_clear() {
        assert_eq!(
            choose_label_placement(CENTER, 64, OUTPUT, LABEL, 20, &[]),
            LabelPlacement::Right
        );
    }

    #[test]
    fn adaptive_label_uses_the_other_side_when_a_window_blocks_the_right() {
        let obstacles = [obstacle((540, 260), (260, 80))];
        assert_eq!(
            choose_label_placement(CENTER, 64, OUTPUT, LABEL, 20, &obstacles),
            LabelPlacement::Left
        );
    }

    #[test]
    fn adaptive_label_uses_vertical_space_when_both_sides_are_blocked() {
        let obstacles = [
            obstacle((540, 270), (260, 60)),
            obstacle((200, 270), (260, 60)),
        ];
        assert_eq!(
            choose_label_placement(CENTER, 64, OUTPUT, LABEL, 20, &obstacles),
            LabelPlacement::Down
        );
    }

    #[test]
    fn adaptive_label_stays_inside_the_output_before_using_preference_order() {
        assert_eq!(
            choose_label_placement((900, 300), 64, OUTPUT, LABEL, 20, &[]),
            LabelPlacement::Left
        );
        assert_eq!(
            choose_label_placement((500, 590), 64, OUTPUT, LABEL, 20, &[]),
            LabelPlacement::Up
        );
    }

    #[test]
    fn adaptive_label_chooses_the_least_obstructed_on_screen_fallback() {
        let obstacles = [
            obstacle((540, 260), (260, 80)),
            obstacle((250, 260), (210, 80)),
            obstacle((400, 340), (200, 120)),
        ];
        assert_eq!(
            choose_label_placement(CENTER, 64, OUTPUT, LABEL, 20, &obstacles),
            LabelPlacement::Up
        );
    }
}
