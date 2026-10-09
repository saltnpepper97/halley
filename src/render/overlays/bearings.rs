use std::cmp::Ordering;
use std::error::Error;

use halley_core::bearings::{Bearing, bearing_to_point};
use halley_core::field::Vec2;
use halley_core::viewport::Viewport;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Rectangle};

use crate::render::scene::SceneElement;
use crate::shell::bearings::BearingTarget;

const CHIP_PAD_X: i32 = 10;
const CHIP_PAD_Y: i32 = 7;
const EDGE_PAD: i32 = 16;
const ICON_SIZE: i32 = 16;
const ICON_TEXT_GAP: i32 = 6;
const PIN_BADGE_RADIUS: i32 = 7;
const PIN_BADGE_GAP: i32 = 7;
const DISTANCE_GAP: i32 = 4;
const META_PAD_X: i32 = 7;
const META_PAD_Y: i32 = 4;
const GROUP_GAP: i32 = 10;
const MAX_LABEL_CHARS: usize = 24;
const MIN_DISTANCE_ALPHA: f32 = 0.34;
const DISTANCE_HIDE_MULTIPLIER: f32 = 1.5;
const TEXT_RGB: [u8; 3] = [238, 242, 249];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Lane {
    NW,
    N,
    NE,
    W,
    E,
    SW,
    S,
    SE,
}

impl Lane {
    fn horizontal(self) -> bool {
        matches!(self, Self::N | Self::S)
    }
}

impl From<Bearing> for Lane {
    fn from(value: Bearing) -> Self {
        match value {
            Bearing::NW => Self::NW,
            Bearing::N => Self::N,
            Bearing::NE => Self::NE,
            Bearing::W => Self::W,
            Bearing::E => Self::E,
            Bearing::SW => Self::SW,
            Bearing::S => Self::S,
            Bearing::SE => Self::SE,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Size {
    chip_w: i32,
    chip_h: i32,
    distance_w: i32,
    distance_h: i32,
    show_icon: bool,
    crowding_w: i32,
}

impl Size {
    fn total_h(self) -> i32 {
        self.chip_h
            + if self.distance_h > 0 {
                self.distance_h + DISTANCE_GAP
            } else {
                0
            }
    }

    fn extent(self, lane: Lane) -> i32 {
        if lane.horizontal() {
            self.crowding_w
        } else {
            self.total_h()
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    target: BearingTarget,
    lane: Lane,
    projected: f32,
    distance: f32,
    label: String,
    icon: Option<IconKind>,
    pinned: bool,
    size: Size,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum IconKind {
    App(String),
    Cluster,
}

#[derive(Clone, Debug)]
struct Group {
    target: BearingTarget,
    lane: Lane,
    projected: f32,
    distance: f32,
    label: String,
    icon: Option<IconKind>,
    pinned: bool,
    alpha: f32,
    size: Size,
}

#[derive(Clone, Debug)]
struct Layout {
    target: BearingTarget,
    chip: Rectangle<i32, Physical>,
    icon: Option<Rectangle<i32, Physical>>,
    label: String,
    distance: Option<(String, Rectangle<i32, Physical>)>,
    icon_kind: Option<IconKind>,
    pinned: bool,
    alpha: f32,
}

#[derive(Clone, Copy)]
struct LayoutContext<'a> {
    output_name: &'a str,
    output_geometry: Rectangle<i32, Logical>,
    config: halley_config::Bearings,
    mix: f32,
    nodes: &'a crate::nodes::NodesState,
    clusters: &'a crate::clusters::ClusterSystem,
    fullscreen: &'a crate::wayland::fullscreen::FullscreenManager,
    camera: &'a halley_core::camera::Camera,
}

#[allow(clippy::too_many_arguments)]
pub fn elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    bearings: &crate::shell::bearings::BearingsState,
    nodes: &crate::nodes::NodesState,
    clusters: &crate::clusters::ClusterSystem,
    fullscreen: &crate::wayland::fullscreen::FullscreenManager,
    cameras: &crate::presentation::camera::OutputCameras,
    blur_config: halley_config::Blur,
    backdrop_blur_renderer: &mut crate::render::effects::backdrop_blur::BackdropBlurRenderer,
    node_renderer: &mut crate::render::node::NodeRenderer,
    cluster_renderer: &mut crate::clusters::render::ClusterRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    overlay_config: &halley_config::Overlays,
    decorations: &halley_config::Decorations,
    pins: &halley_config::Pins,
    pin_renderer: &mut crate::render::pin::PinRenderer,
) -> Result<Vec<SceneElement>, Box<dyn Error>> {
    let output_name = output.name();
    let mix = bearings.mix(&output_name);
    if mix <= 0.002 {
        bearings.set_hitboxes(&output_name, Vec::new());
        return Ok(Vec::new());
    }
    let Some(camera) = cameras.get(&output_name) else {
        return Ok(Vec::new());
    };
    let overlay_visuals = crate::render::overlays::shell::resolve_visuals(overlay_config);
    let layouts = collect_layouts(
        renderer,
        LayoutContext {
            output_name: &output_name,
            output_geometry,
            config: bearings.config,
            mix,
            nodes,
            clusters,
            fullscreen,
            camera,
        },
        ui_text,
    )?;
    bearings.set_hitboxes(
        &output_name,
        layouts
            .iter()
            .map(|layout| crate::shell::bearings::BearingHitbox {
                target: layout.target,
                rect: layout.chip.to_logical(1),
            })
            .collect(),
    );

    let blur_patches = if bearings.config.blur && blur_config.overlays {
        layouts
            .iter()
            .filter(|layout| layout.alpha >= 0.04)
            .map(|layout| crate::render::effects::backdrop_blur::BlurPatch {
                rect: layout.chip,
                radius: overlay_visuals.radius
                    + if overlay_visuals.radius > 0.0 {
                        overlay_visuals.border_px
                    } else {
                        0.0
                    },
                alpha: layout.alpha,
                clip: None,
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut foreground = Vec::new();
    let mut backgrounds = Vec::new();
    for layout in layouts {
        let label_size = ui_text
            .measure(renderer, &layout.label, overlay_visuals.text.bytes())?
            .unwrap_or((0, 0).into());
        let text_x = layout.chip.loc.x
            + CHIP_PAD_X
            + if layout.icon.is_some() {
                ICON_SIZE + ICON_TEXT_GAP
            } else {
                0
            };
        if let Some(text) = ui_text.element(
            renderer,
            (
                text_x,
                layout.chip.loc.y + (layout.chip.size.h - label_size.h) / 2,
            )
                .into(),
            &layout.label,
            overlay_visuals.text.bytes(),
            layout.alpha,
        )? {
            foreground.push(SceneElement::UiText(text.element));
        }

        if let (Some(icon), Some(kind)) = (layout.icon, layout.icon_kind.as_ref()) {
            match kind {
                IconKind::App(app_id) => {
                    node_renderer.request_app_icon(renderer, app_id);
                    if let Some(icon) =
                        node_renderer.app_icon_element(renderer, app_id, icon, layout.alpha)
                    {
                        foreground.push(SceneElement::NodeTexture(icon));
                    }
                }
                IconKind::Cluster => {
                    let [r, g, b] = overlay_visuals.text.bytes();
                    foreground.push(SceneElement::ClusterIcon(cluster_renderer.icon(
                        renderer,
                        icon,
                        false,
                        [[r, g, b, 255]; 2],
                        layout.alpha,
                    )?));
                }
            }
        }

        if layout.pinned {
            let slot = match layout.target {
                BearingTarget::Node(id) => crate::render::pin::PinSlot::BearingNode(id.as_u64()),
                BearingTarget::ClusterCore { cluster, .. } => {
                    crate::render::pin::PinSlot::BearingCluster(cluster.as_u64())
                }
            };
            let cx = layout.chip.loc.x + layout.chip.size.w - CHIP_PAD_X - PIN_BADGE_RADIUS;
            let cy = layout.chip.loc.y + layout.chip.size.h / 2;
            if let Some(pin) = pin_renderer.element(
                renderer,
                &output_name,
                slot,
                Rectangle::new(
                    (cx - PIN_BADGE_RADIUS, cy - PIN_BADGE_RADIUS).into(),
                    (PIN_BADGE_RADIUS * 2, PIN_BADGE_RADIUS * 2).into(),
                ),
                layout.alpha,
                pins,
                overlay_config,
                decorations,
            ) {
                foreground.push(SceneElement::Closing(pin));
            }
        }

        if let Some((distance, rect)) = layout.distance {
            let text_size = ui_text
                .measure(renderer, &distance, overlay_visuals.text.bytes())?
                .unwrap_or((0, 0).into());
            if let Some(text) = ui_text.element(
                renderer,
                (
                    rect.loc.x + (rect.size.w - text_size.w) / 2,
                    rect.loc.y + (rect.size.h - text_size.h) / 2,
                )
                    .into(),
                &distance,
                overlay_visuals.text.bytes(),
                layout.alpha * 0.96,
            )? {
                foreground.push(SceneElement::UiText(text.element));
            }
            backgrounds.push(SceneElement::NodeLabel(
                crate::render::overlays::shell::label_card_element(
                    renderer,
                    node_renderer,
                    rect,
                    overlay_visuals,
                    overlay_visuals.fill,
                    layout.alpha * 0.88,
                )?,
            ));
        }
        backgrounds.push(SceneElement::NodeLabel(
            crate::render::overlays::shell::label_card_element(
                renderer,
                node_renderer,
                layout.chip,
                overlay_visuals,
                overlay_visuals.fill,
                layout.alpha * 0.92,
            )?,
        ));
    }
    foreground.extend(backgrounds);
    if let Some(blur) = backdrop_blur_renderer.blur_element(
        renderer,
        &output_name,
        crate::render::effects::backdrop_blur::BlurIdentity::Overlay("bearings"),
        output_geometry.size,
        blur_patches,
        blur_config,
        0,
        output.current_transform(),
    )? {
        foreground.push(SceneElement::BackdropBlur(blur));
    }
    Ok(foreground)
}

fn collect_layouts(
    renderer: &mut GlesRenderer,
    context: LayoutContext<'_>,
    ui_text: &mut crate::render::text::UiTextRenderer,
) -> Result<Vec<Layout>, Box<dyn Error>> {
    let LayoutContext {
        output_name,
        output_geometry,
        config,
        mix,
        nodes,
        clusters,
        fullscreen,
        camera,
    } = context;
    let center = crate::presentation::camera::global_center(
        (camera.center.x, camera.center.y).into(),
        output_geometry,
    );
    let viewport = Viewport::new(
        Vec2 {
            x: center.x,
            y: center.y,
        },
        camera.view_size,
    );
    let screen_w = output_geometry.size.w;
    let screen_h = output_geometry.size.h;
    let mut candidates = Vec::new();

    for record in nodes.records().filter(|record| {
        record.attached && record.output == output_name && !clusters.is_member(record.id)
    }) {
        let Some(node) = nodes.field.node(record.id) else {
            continue;
        };
        let field_visible = nodes.field.is_visible(record.id);
        let parked_fullscreen = fullscreen.is_presentation_paused(&record.surface);
        if !bearing_node_is_eligible(field_visible, parked_fullscreen)
            || intersects_view(node.pos, node.footprint, &viewport)
        {
            continue;
        }
        let Some(bearing) = bearing_to_point(&viewport, node.pos) else {
            continue;
        };
        let lane = Lane::from(bearing);
        let distance = offscreen_distance(node.pos, node.footprint, &viewport);
        let pinned = node.pinned;
        let label = bearing_label(record);
        let distance_text = config
            .show_distance
            .then(|| format!("{:.0}px", distance.round()));
        let show_icon = config.show_icons && record.app_id.is_some();
        let size = measure_size(
            renderer,
            ui_text,
            &label,
            show_icon,
            config.show_icons,
            distance_text.as_deref(),
            pinned,
        )?;
        candidates.push(Candidate {
            target: BearingTarget::Node(record.id),
            lane,
            projected: projected_anchor(node.pos, &viewport, lane, screen_w, screen_h),
            distance,
            label,
            icon: record.app_id.clone().map(IconKind::App),
            pinned,
            size,
        });
    }

    let scale = crate::presentation::camera::scale(camera).max(0.05);
    let now = crate::frame_clock::monotonic_now();
    let core_footprint = Vec2 {
        x: crate::clusters::CORE_DIAMETER_PX / scale,
        y: crate::clusters::CORE_DIAMETER_PX / scale,
    };
    for (_, cluster, metadata) in clusters.clusters_for_output(output_name) {
        let Some(core) = clusters.core_node(cluster) else {
            continue;
        };
        let position = nodes.landmark_position(core, metadata.core_position, now);
        if intersects_view(position, core_footprint, &viewport) {
            continue;
        }
        let Some(bearing) = bearing_to_point(&viewport, position) else {
            continue;
        };
        let lane = Lane::from(bearing);
        let distance = offscreen_distance(position, core_footprint, &viewport);
        let pinned = clusters
            .registry()
            .cluster(cluster)
            .is_some_and(|cluster| cluster.pinned);
        let label = truncate_label(&metadata.name, || format!("Cluster {}", cluster.as_u64()));
        let distance_text = config
            .show_distance
            .then(|| format!("{:.0}px", distance.round()));
        let show_icon = config.show_icons;
        let size = measure_size(
            renderer,
            ui_text,
            &label,
            show_icon,
            show_icon,
            distance_text.as_deref(),
            pinned,
        )?;
        candidates.push(Candidate {
            target: BearingTarget::ClusterCore { cluster, core },
            lane,
            projected: projected_anchor(position, &viewport, lane, screen_w, screen_h),
            distance,
            label,
            icon: show_icon.then_some(IconKind::Cluster),
            pinned,
            size,
        });
    }

    // One sort by (lane, order), then walk the contiguous run per lane. The
    // previous shape made eight filtering passes over every candidate and
    // cloned each one it kept, so every chip's label String was reallocated
    // eight times a frame per output.
    candidates.sort_by(|left, right| {
        left.lane
            .cmp(&right.lane)
            .then(candidate_order(left, right))
    });
    let mut layouts = Vec::new();
    let mut remaining = candidates;
    while !remaining.is_empty() {
        let lane = remaining[0].lane;
        let count = remaining.partition_point(|candidate| candidate.lane == lane);
        let tail = remaining.split_off(count);
        let lane_candidates = std::mem::replace(&mut remaining, tail);
        let groups = group_candidates(renderer, ui_text, config, mix, lane_candidates)?;
        layouts.extend(layout_groups(lane, groups, screen_w, screen_h));
    }
    Ok(layouts)
}

fn candidate_order(left: &Candidate, right: &Candidate) -> Ordering {
    left.projected
        .partial_cmp(&right.projected)
        .unwrap_or(Ordering::Equal)
        .then(stable_target_key(left.target).cmp(&stable_target_key(right.target)))
}

fn stable_target_key(target: BearingTarget) -> (u8, u64) {
    match target {
        BearingTarget::Node(id) => (0, id.as_u64()),
        BearingTarget::ClusterCore { cluster, .. } => (1, cluster.as_u64()),
    }
}

fn group_candidates(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    config: halley_config::Bearings,
    mix: f32,
    candidates: Vec<Candidate>,
) -> Result<Vec<Group>, Box<dyn Error>> {
    let mut groups = Vec::new();
    let mut current: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if let Some(previous) = current.last()
            && starts_new_group(previous, &candidate)
            && let Some(group) =
                finalize_group(renderer, ui_text, config, mix, std::mem::take(&mut current))?
        {
            groups.push(group);
        }
        current.push(candidate);
    }
    if !current.is_empty()
        && let Some(group) = finalize_group(renderer, ui_text, config, mix, current)?
    {
        groups.push(group);
    }
    Ok(groups)
}

fn starts_new_group(previous: &Candidate, candidate: &Candidate) -> bool {
    candidate.projected - previous.projected
        > separation(candidate.lane, previous.size, candidate.size)
}

fn finalize_group(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    config: halley_config::Bearings,
    mix: f32,
    members: Vec<Candidate>,
) -> Result<Option<Group>, Box<dyn Error>> {
    let nearest = members
        .iter()
        .min_by(|left, right| {
            left.distance
                .partial_cmp(&right.distance)
                .unwrap_or(Ordering::Equal)
                .then(stable_target_key(left.target).cmp(&stable_target_key(right.target)))
        })
        .expect("non-empty bearing group");
    let count = members.len();
    let label = if count == 1 {
        nearest.label.clone()
    } else {
        format!("{count} nodes")
    };
    let pinned = members.iter().any(|member| member.pinned);
    // Count the whole crowded group before applying its nearest target's fade.
    // Fully faded groups must not create layouts or invisible hitboxes.
    let alpha = if pinned && config.show_pinned {
        mix
    } else {
        mix * distance_alpha(config.fade_distance, nearest.distance)
    };
    if alpha <= 0.002 {
        return Ok(None);
    }
    let distance_text = config
        .show_distance
        .then(|| format!("{:.0}px", nearest.distance.round()));
    let show_icon = count == 1 && config.show_icons && nearest.icon.is_some();
    let size = measure_size(
        renderer,
        ui_text,
        &label,
        show_icon,
        show_icon,
        distance_text.as_deref(),
        pinned,
    )?;
    Ok(Some(Group {
        target: nearest.target,
        lane: nearest.lane,
        projected: members.iter().map(|member| member.projected).sum::<f32>() / count as f32,
        distance: nearest.distance,
        label,
        icon: (count == 1).then(|| nearest.icon.clone()).flatten(),
        pinned,
        alpha,
        size,
    }))
}

fn layout_groups(lane: Lane, mut groups: Vec<Group>, screen_w: i32, screen_h: i32) -> Vec<Layout> {
    groups.sort_by(|left, right| {
        left.projected
            .partial_cmp(&right.projected)
            .unwrap_or(Ordering::Equal)
            .then(stable_target_key(left.target).cmp(&stable_target_key(right.target)))
    });
    let mut centers = groups
        .iter()
        .map(|group| {
            let (min, max) = center_bounds(lane, group.size, screen_w, screen_h);
            group.projected.clamp(min, max)
        })
        .collect::<Vec<_>>();
    for index in 1..centers.len() {
        centers[index] = centers[index]
            .max(centers[index - 1] + separation(lane, groups[index - 1].size, groups[index].size));
    }
    for index in (0..centers.len().saturating_sub(1)).rev() {
        let (_, max) = center_bounds(lane, groups[index].size, screen_w, screen_h);
        centers[index] = centers[index]
            .min(max)
            .min(centers[index + 1] - separation(lane, groups[index].size, groups[index + 1].size));
    }
    for index in 1..centers.len() {
        centers[index] = centers[index]
            .max(centers[index - 1] + separation(lane, groups[index - 1].size, groups[index].size));
    }
    groups
        .into_iter()
        .zip(centers)
        .map(|(group, center)| build_layout(group, center, screen_w, screen_h))
        .collect()
}

fn build_layout(group: Group, center: f32, screen_w: i32, screen_h: i32) -> Layout {
    let distance_block = if group.size.distance_h > 0 {
        group.size.distance_h + DISTANCE_GAP
    } else {
        0
    };
    let vertical_y = center.round() as i32 - group.size.total_h() / 2 + distance_block;
    let (x, y) = match group.lane {
        Lane::N => (
            center.round() as i32 - group.size.chip_w / 2,
            EDGE_PAD + distance_block,
        ),
        Lane::S => (
            center.round() as i32 - group.size.chip_w / 2,
            screen_h - EDGE_PAD - group.size.chip_h,
        ),
        Lane::W | Lane::NW | Lane::SW => (EDGE_PAD, vertical_y),
        Lane::E | Lane::NE | Lane::SE => (screen_w - EDGE_PAD - group.size.chip_w, vertical_y),
    };
    let chip = Rectangle::new((x, y).into(), (group.size.chip_w, group.size.chip_h).into());
    let icon = group.size.show_icon.then(|| {
        Rectangle::new(
            (x + CHIP_PAD_X, y + (group.size.chip_h - ICON_SIZE) / 2).into(),
            (ICON_SIZE, ICON_SIZE).into(),
        )
    });
    let distance = (group.size.distance_h > 0).then(|| {
        let text = format!("{:.0}px", group.distance.round());
        let rect = Rectangle::new(
            (
                x + (group.size.chip_w - group.size.distance_w) / 2,
                y - DISTANCE_GAP - group.size.distance_h,
            )
                .into(),
            (group.size.distance_w, group.size.distance_h).into(),
        );
        (text, rect)
    });
    Layout {
        target: group.target,
        chip,
        icon,
        label: group.label,
        distance,
        icon_kind: group.icon,
        pinned: group.pinned,
        alpha: group.alpha.clamp(0.0, 1.0),
    }
}

fn measure_size(
    renderer: &mut GlesRenderer,
    ui_text: &mut crate::render::text::UiTextRenderer,
    label: &str,
    show_icon: bool,
    reserve_icon_for_crowding: bool,
    distance: Option<&str>,
    pinned: bool,
) -> Result<Size, Box<dyn Error>> {
    let label_size = ui_text
        .measure(renderer, label, TEXT_RGB)?
        .unwrap_or((0, 0).into());
    let chip_w = (CHIP_PAD_X * 2
        + label_size.w
        + if show_icon {
            ICON_SIZE + ICON_TEXT_GAP
        } else {
            0
        }
        + if pinned {
            PIN_BADGE_RADIUS * 2 + PIN_BADGE_GAP
        } else {
            0
        })
    .max(44);
    let chip_h = (CHIP_PAD_Y * 2 + label_size.h.max(if show_icon { ICON_SIZE } else { 0 })).max(24);
    let distance_size = match distance {
        Some(distance) => ui_text
            .measure(renderer, distance, TEXT_RGB)?
            .unwrap_or((0, 0).into()),
        None => (0, 0).into(),
    };
    Ok(Size {
        chip_w,
        chip_h,
        distance_w: if distance.is_some() {
            distance_size.w + META_PAD_X * 2
        } else {
            0
        },
        distance_h: if distance.is_some() {
            distance_size.h + META_PAD_Y * 2
        } else {
            0
        },
        show_icon,
        // Old Halley always measured node bearings with their configured icon
        // slot, even when the icon later fell back to a generated glyph. Keep
        // that footprint for aggregation while leaving the visible Next chip
        // unchanged when no app icon is available.
        crowding_w: if reserve_icon_for_crowding && !show_icon {
            chip_w + ICON_SIZE + ICON_TEXT_GAP
        } else {
            chip_w
        },
    })
}

fn separation(lane: Lane, left: Size, right: Size) -> f32 {
    (left.extent(lane) + right.extent(lane)) as f32 * 0.5 + GROUP_GAP as f32
}

fn center_bounds(lane: Lane, size: Size, screen_w: i32, screen_h: i32) -> (f32, f32) {
    let (extent, available) = if lane.horizontal() {
        (size.chip_w, screen_w)
    } else {
        (size.total_h(), screen_h)
    };
    let half = extent as f32 * 0.5;
    (
        EDGE_PAD as f32 + half,
        available as f32 - EDGE_PAD as f32 - half,
    )
}

fn projected_anchor(
    pos: Vec2,
    viewport: &Viewport,
    lane: Lane,
    screen_w: i32,
    screen_h: i32,
) -> f32 {
    let dx = pos.x - viewport.center.x;
    let dy = pos.y - viewport.center.y;
    let half_w = viewport.size.x * 0.5;
    let half_h = viewport.size.y * 0.5;
    let tx = if dx.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        half_w / dx.abs()
    };
    let ty = if dy.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        half_h / dy.abs()
    };
    let t = tx.min(ty);
    if lane.horizontal() {
        ((viewport.center.x + dx * t - (viewport.center.x - half_w)) / viewport.size.x.max(1.0))
            * screen_w as f32
    } else {
        ((viewport.center.y + dy * t - (viewport.center.y - half_h)) / viewport.size.y.max(1.0))
            * screen_h as f32
    }
}

fn bearing_node_is_eligible(field_visible: bool, parked_fullscreen: bool) -> bool {
    field_visible || parked_fullscreen
}

fn intersects_view(pos: Vec2, footprint: Vec2, viewport: &Viewport) -> bool {
    let view = viewport.rect();
    let half = Vec2 {
        x: footprint.x * 0.5,
        y: footprint.y * 0.5,
    };
    pos.x + half.x > view.min.x
        && pos.x - half.x < view.max.x
        && pos.y + half.y > view.min.y
        && pos.y - half.y < view.max.y
}

fn offscreen_distance(pos: Vec2, footprint: Vec2, viewport: &Viewport) -> f32 {
    let view = viewport.rect();
    let half = Vec2 {
        x: footprint.x * 0.5,
        y: footprint.y * 0.5,
    };
    let overflow_x = if pos.x + half.x <= view.min.x {
        view.min.x - (pos.x + half.x)
    } else if pos.x - half.x >= view.max.x {
        pos.x - half.x - view.max.x
    } else {
        0.0
    };
    let overflow_y = if pos.y + half.y <= view.min.y {
        view.min.y - (pos.y + half.y)
    } else if pos.y - half.y >= view.max.y {
        pos.y - half.y - view.max.y
    } else {
        0.0
    };
    overflow_x.hypot(overflow_y)
}

fn distance_alpha(fade_distance: f32, distance: f32) -> f32 {
    let fade_end = fade_distance * DISTANCE_HIDE_MULTIPLIER;
    if distance <= fade_distance {
        let t = (distance / fade_distance).clamp(0.0, 1.0);
        return MIN_DISTANCE_ALPHA + (1.0 - t) * (1.0 - MIN_DISTANCE_ALPHA);
    }
    if distance >= fade_end {
        return 0.0;
    }
    MIN_DISTANCE_ALPHA * (1.0 - (distance - fade_distance) / (fade_end - fade_distance))
}

fn bearing_label(record: &crate::nodes::NodeRecord) -> String {
    let base = if !record.title.trim().is_empty() {
        record.title.trim()
    } else if let Some(app_id) = record.app_id.as_deref() {
        app_id
    } else {
        return format!("Node {}", record.id.as_u64());
    };
    truncate_label(base, || format!("Node {}", record.id.as_u64()))
}

fn truncate_label(base: &str, fallback: impl FnOnce() -> String) -> String {
    let base = if base.trim().is_empty() {
        return fallback();
    } else {
        base.trim()
    };
    let mut chars = base.chars();
    let prefix = chars.by_ref().take(MAX_LABEL_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn parked_fullscreen_remains_bearing_eligible_while_active_fullscreen_does_not() {
        assert!(bearing_node_is_eligible(false, true));
        assert!(!bearing_node_is_eligible(false, false));
        assert!(bearing_node_is_eligible(true, false));
    }

    use super::*;

    fn candidate(projected: f32, size: Size) -> Candidate {
        Candidate {
            target: BearingTarget::Node(halley_core::field::NodeId::new(projected as u64 + 1)),
            lane: Lane::N,
            projected,
            distance: 100.0,
            label: "node".into(),
            icon: None,
            pinned: false,
            size,
        }
    }

    #[test]
    fn icon_fallback_footprint_preserves_old_halley_node_aggregation() {
        let without_legacy_slot = Size {
            chip_w: 44,
            chip_h: 24,
            distance_w: 0,
            distance_h: 0,
            show_icon: false,
            crowding_w: 44,
        };
        let with_legacy_slot = Size {
            crowding_w: 44 + ICON_SIZE + ICON_TEXT_GAP,
            ..without_legacy_slot
        };

        let gap = 70.0;
        assert!(starts_new_group(
            &candidate(0.0, without_legacy_slot),
            &candidate(gap, without_legacy_slot),
        ));
        assert!(!starts_new_group(
            &candidate(0.0, with_legacy_slot),
            &candidate(gap, with_legacy_slot),
        ));
    }

    #[test]
    fn distance_fade_matches_old_halley_thresholds() {
        assert_eq!(distance_alpha(1_200.0, 0.0), 1.0);
        assert!((distance_alpha(1_200.0, 1_200.0) - MIN_DISTANCE_ALPHA).abs() < 0.0001);
        assert!((distance_alpha(1_200.0, 1_500.0) - MIN_DISTANCE_ALPHA * 0.5).abs() < 0.0001);
        assert_eq!(distance_alpha(1_200.0, 1_800.0), 0.0);
        assert_eq!(distance_alpha(1_200.0, 100_000.0), 0.0);
    }

    #[test]
    #[ignore = "requires surfaceless GLES; run with LIBGL_ALWAYS_SOFTWARE=1 and --ignored"]
    fn four_window_group_counts_far_members_and_still_disappears_at_distance_cutoff() {
        use smithay::backend::egl::{EGLContext, EGLDisplay, native::EGLSurfacelessDisplay};
        let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.unwrap();
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context) }.unwrap();
        let mut text = crate::render::text::UiTextRenderer::default();
        let config = halley_config::Bearings::default();
        let viewport = Viewport::new(
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 {
                x: 1_920.0,
                y: 1_200.0,
            },
        );
        let footprint = Vec2 {
            x: 1_000.0,
            y: 640.0,
        };
        let size = Size {
            chip_w: 44,
            chip_h: 24,
            distance_w: 0,
            distance_h: 0,
            show_icon: false,
            crowding_w: 44,
        };
        let mut candidates = (0..4)
            .map(|index| {
                let position = Vec2 {
                    x: -1_580.0 - index as f32 * 1_100.0,
                    y: 0.0,
                };
                assert!(!intersects_view(position, footprint, &viewport));
                assert_eq!(bearing_to_point(&viewport, position), Some(Bearing::W));
                Candidate {
                    target: BearingTarget::Node(halley_core::field::NodeId::new(index + 1)),
                    lane: Lane::W,
                    distance: offscreen_distance(position, footprint, &viewport),
                    ..candidate(
                        projected_anchor(position, &viewport, Lane::W, 1_920, 1_200),
                        size,
                    )
                }
            })
            .collect::<Vec<_>>();
        let groups =
            group_candidates(&mut renderer, &mut text, config, 1.0, candidates.clone()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "4 nodes");
        assert_eq!(groups[0].target, candidates[0].target);
        assert!(groups[0].alpha > 0.002);

        // An isolated far target disappears rather than becoming a permanent chip.
        assert!(
            group_candidates(
                &mut renderer,
                &mut text,
                config,
                1.0,
                vec![candidates[3].clone()]
            )
            .unwrap()
            .is_empty()
        );
        for candidate in &mut candidates {
            candidate.distance += 2_000.0;
        }
        assert!(
            group_candidates(&mut renderer, &mut text, config, 1.0, candidates.clone())
                .unwrap()
                .is_empty()
        );

        // Preserve the existing explicit exemption for pinned targets.
        candidates[3].pinned = true;
        let groups =
            group_candidates(&mut renderer, &mut text, config, 1.0, candidates.clone()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "4 nodes");
        assert_eq!(groups[0].alpha, 1.0);
        let config = halley_config::Bearings {
            show_pinned: false,
            ..config
        };
        assert!(
            group_candidates(&mut renderer, &mut text, config, 1.0, candidates)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn long_labels_are_unicode_safe_and_truncated() {
        let source = "abcdefghijklmnopqrstuvwxYZ";
        let mut chars = source.chars();
        let prefix = chars.by_ref().take(MAX_LABEL_CHARS).collect::<String>();
        assert_eq!(format!("{prefix}…"), "abcdefghijklmnopqrstuvwx…");
    }

    #[test]
    fn clusters_have_their_own_stable_target_and_configured_label() {
        let cluster = halley_core::cluster::ClusterId::new(4);
        let core = halley_core::field::NodeId::new(40);
        let target = BearingTarget::ClusterCore { cluster, core };

        assert_eq!(stable_target_key(target), (1, 4));
        assert_eq!(truncate_label("Research", || "fallback".into()), "Research");
        assert_eq!(IconKind::Cluster, IconKind::Cluster);
    }
}
