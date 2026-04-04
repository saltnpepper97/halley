use std::error::Error;
use std::f32::consts::{PI, TAU};

use smithay::{
    backend::renderer::{
        Color32F, Texture,
        gles::{GlesFrame, Uniform},
    },
    utils::{Buffer, Physical, Rectangle, Transform},
};

use crate::compositor::root::Halley;
use crate::overlay::{ClusterBloomAnimSnapshot, OverlayView};
use crate::render::app_icon::ensure_app_icon_resources_for_node_ids;
use crate::render::text::{draw_ui_text_in, ui_text_size_in};

#[derive(Clone, Copy)]
pub(crate) struct BloomTokenLayout {
    pub(crate) cluster_id: halley_core::cluster::ClusterId,
    pub(crate) member_id: halley_core::field::NodeId,
    pub(crate) center_sx: i32,
    pub(crate) center_sy: i32,
    pub(crate) token_radius: i32,
    pub(crate) core_sx: i32,
    pub(crate) core_sy: i32,
}

pub(crate) fn ensure_cluster_bloom_icon_resources(
    renderer: &mut smithay::backend::renderer::gles::GlesRenderer,
    st: &mut Halley,
    monitor: &str,
) -> Result<(), Box<dyn Error>> {
    let overlay = OverlayView::from_halley(st);
    let ids = cluster_bloom_layouts(&overlay, 1, 1, monitor)
        .into_iter()
        .map(|layout| layout.member_id);
    ensure_app_icon_resources_for_node_ids(renderer, st, ids)
}

pub(crate) fn cluster_bloom_layouts(
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
) -> Vec<BloomTokenLayout> {
    let Some(cid) = overlay
        .cluster_state
        .cluster_bloom_open
        .get(monitor)
        .copied()
    else {
        return Vec::new();
    };
    cluster_bloom_layouts_for_cluster(
        overlay,
        screen_w,
        screen_h,
        ClusterBloomAnimSnapshot {
            cluster_id: cid,
            mix: 1.0,
        },
        monitor,
    )
}

fn cluster_bloom_layouts_for_cluster(
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    snapshot: ClusterBloomAnimSnapshot,
    monitor: &str,
) -> Vec<BloomTokenLayout> {
    if overlay
        .cluster_state
        .active_cluster_workspaces
        .get(monitor)
        .is_some()
    {
        return Vec::new();
    }
    let Some(cluster) = overlay.field.cluster(snapshot.cluster_id) else {
        return Vec::new();
    };
    if !cluster.is_collapsed() {
        return Vec::new();
    }
    let Some(core_id) = cluster.core else {
        return Vec::new();
    };
    let Some(core) = overlay.field.node(core_id) else {
        return Vec::new();
    };

    let (core_sx, core_sy) =
        overlay.world_to_screen(screen_w.max(1), screen_h.max(1), core.pos.x, core.pos.y);
    let mut members = cluster.members().to_vec();
    members.sort_by_key(|id| id.as_u64());
    let count = members.len().max(1);
    let token_radius = 24;
    let token_diameter = token_radius as f32 * 2.0;
    let min_slots = 10usize;
    let slots = count.max(min_slots);
    let angle_step = TAU / slots as f32;
    let min_chord = token_diameter + 18.0;
    let bloom_radius = (min_chord / (2.0 * (angle_step * 0.5).sin()).max(0.20)).max(84.0)
        + (count as f32 - 1.0).min(5.0) * 3.0;
    let direction = match overlay.tuning.cluster_bloom_direction {
        halley_config::ClusterBloomDirection::Clockwise => 1.0,
        halley_config::ClusterBloomDirection::CounterClockwise => -1.0,
    };
    let mix = snapshot.mix.clamp(0.0, 1.0);
    let eased = mix * mix * (3.0 - 2.0 * mix);

    members
        .into_iter()
        .enumerate()
        .map(|(index, member_id)| {
            let angle = -PI * 0.5 + direction * (angle_step * index as f32);
            let radial_dx = angle.cos() * bloom_radius * eased;
            let radial_dy = angle.sin() * bloom_radius * eased;
            let center_sx = (core_sx as f32 + radial_dx).round() as i32;
            let center_sy = (core_sy as f32 + radial_dy).round() as i32;
            BloomTokenLayout {
                cluster_id: snapshot.cluster_id,
                member_id,
                center_sx,
                center_sy,
                token_radius,
                core_sx,
                core_sy,
            }
        })
        .collect()
}

pub(crate) fn bloom_token_hit_test(
    st: &Halley,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
    sx: f32,
    sy: f32,
) -> Option<BloomTokenLayout> {
    let overlay = OverlayView::from_halley(st);
    cluster_bloom_layouts(&overlay, screen_w, screen_h, monitor)
        .into_iter()
        .find(|layout| {
            let dx = sx - layout.center_sx as f32;
            let dy = sy - layout.center_sy as f32;
            dx * dx + dy * dy <= (layout.token_radius * layout.token_radius) as f32
        })
}

pub(crate) fn draw_cluster_bloom(
    frame: &mut GlesFrame<'_, '_>,
    st: &mut Halley,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(snapshot) = st.ui.render_state.cluster_bloom_snapshot_for_monitor(
        monitor,
        st.model
            .cluster_state
            .cluster_bloom_open
            .get(monitor)
            .copied(),
    ) else {
        let overlay = OverlayView::from_halley(st);
        draw_cluster_join_affordance(frame, &overlay, screen_w, screen_h, monitor, damage)?;
        return Ok(());
    };
    let overlay = OverlayView::from_halley(st);
    let bloom_alpha = snapshot.mix.clamp(0.0, 1.0);
    for layout in cluster_bloom_layouts_for_cluster(&overlay, screen_w, screen_h, snapshot, monitor)
    {
        draw_bloom_token(frame, &overlay, &layout, bloom_alpha, damage)?;
    }
    draw_cluster_join_affordance(frame, &overlay, screen_w, screen_h, monitor, damage)?;
    Ok(())
}

fn draw_bloom_token(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    layout: &BloomTokenLayout,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    if alpha <= 0.01 {
        return Ok(());
    }
    let preview = overlay
        .interaction_state
        .bloom_pull_preview
        .as_ref()
        .filter(|preview| {
            preview.monitor == overlay.monitor_state.current_monitor
                && preview.cluster_id == layout.cluster_id
                && preview.member_id == layout.member_id
        })
        .cloned();
    let display_offset = preview
        .as_ref()
        .map(|preview| preview.display_offset)
        .unwrap_or(halley_core::field::Vec2 { x: 0.0, y: 0.0 });
    let tether_origin = preview
        .as_ref()
        .map(|preview| preview.slot_screen)
        .unwrap_or(halley_core::field::Vec2 {
            x: layout.center_sx as f32,
            y: layout.center_sy as f32,
        });
    let pull_mix = crate::compositor::interaction::state::bloom_pull_display_mix(display_offset);
    let hold_progress = preview
        .as_ref()
        .map(|preview| preview.hold_progress)
        .unwrap_or(0.0);
    let pre_release_mix = bloom_pre_release_mix(hold_progress);
    let radius = (layout.token_radius as f32
        + 5.0 * pull_mix
        + 1.5 * hold_progress
        + 12.0 * pre_release_mix)
        .round()
        .max(1.0) as i32;
    let center_sx = layout.center_sx + display_offset.x.round() as i32;
    let center_sy = layout.center_sy + display_offset.y.round() as i32;
    if pull_mix > 0.01 {
        draw_bloom_anchor_dot(
            frame,
            overlay,
            tether_origin.x.round() as i32,
            tether_origin.y.round() as i32,
            alpha,
            pull_mix,
            pre_release_mix,
            damage,
        )?;
    }
    let Some(texture) = overlay.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = overlay.render_state.node_circle_program.as_ref() else {
        return Ok(());
    };
    let diameter = radius * 2;
    let dest = Rectangle::<i32, Physical>::new(
        (center_sx - radius, center_sy - radius).into(),
        (diameter, diameter).into(),
    );
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("node_color", (0.12f32, 0.16f32, 0.20f32, 0.0f32)),
        Uniform::new("fill_color", (0.95f32, 0.97f32, 0.99f32, 1.0f32)),
    ];
    frame.render_texture_from_to(
        texture,
        src,
        dest,
        &[damage],
        &[],
        Transform::Normal,
        alpha,
        Some(program),
        &uniforms,
    )?;

    if overlay.tuning.cluster_show_icons
        && let Some(crate::render::state::NodeAppIconCacheEntry::Ready(icon)) =
            overlay.node_app_icon_entry(layout.member_id)
    {
        let side = (diameter as f32 * 0.64).round() as i32;
        let icon_dest = Rectangle::<i32, Physical>::new(
            (center_sx - side / 2, center_sy - side / 2).into(),
            (side, side).into(),
        );
        let icon_src = Rectangle::<f64, Buffer>::new(
            (0.0, 0.0).into(),
            (icon.width as f64, icon.height as f64).into(),
        );
        frame.render_texture_from_to(
            &icon.texture,
            icon_src,
            icon_dest,
            &[damage],
            &[],
            Transform::Normal,
            alpha,
            None,
            &[],
        )?;
        return Ok(());
    }

    let fallback = overlay
        .node_app_ids
        .get(&layout.member_id)
        .map(String::as_str)
        .or_else(|| {
            overlay
                .field
                .node(layout.member_id)
                .map(|n| n.label.as_str())
        })
        .unwrap_or("?");
    let glyph = fallback
        .chars()
        .find(|ch| ch.is_ascii_alphanumeric())
        .unwrap_or('?')
        .to_ascii_uppercase()
        .to_string();
    let scale = 2;
    let (text_w, text_h) =
        ui_text_size_in(overlay.render_state, &overlay.tuning.font, &glyph, scale);
    draw_ui_text_in(
        frame,
        overlay.render_state,
        &overlay.tuning.font,
        center_sx - text_w / 2,
        center_sy - text_h / 2,
        &glyph,
        scale,
        Color32F::new(0.16, 0.20, 0.24, alpha),
        damage,
    )?;
    Ok(())
}

fn bloom_pre_release_mix(hold_progress: f32) -> f32 {
    let t = ((hold_progress - 0.62) / 0.38).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn draw_bloom_anchor_dot(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    x: i32,
    y: i32,
    alpha: f32,
    pull_mix: f32,
    pre_release_mix: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(texture) = overlay.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = overlay.render_state.node_circle_program.as_ref() else {
        return Ok(());
    };
    let radius = (5.0 + 2.0 * pull_mix + 2.5 * pre_release_mix).round() as i32;
    let rect = Rectangle::<i32, Physical>::new(
        (x - radius, y - radius).into(),
        ((radius * 2).max(1), (radius * 2).max(1)).into(),
    );
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let dot_alpha = alpha * (0.84 + 0.10 * pull_mix + 0.12 * pre_release_mix).clamp(0.0, 1.0);
    let uniforms = [
        Uniform::new("node_color", (0.20f32, 0.24f32, 0.29f32, 0.0f32)),
        Uniform::new("fill_color", (0.80f32, 0.85f32, 0.90f32, 1.0f32)),
    ];
    frame.render_texture_from_to(
        texture,
        src,
        rect,
        &[damage],
        &[],
        Transform::Normal,
        dot_alpha,
        Some(program),
        &uniforms,
    )?;
    Ok(())
}

fn draw_cluster_join_affordance(
    frame: &mut GlesFrame<'_, '_>,
    overlay: &OverlayView<'_>,
    screen_w: i32,
    screen_h: i32,
    monitor: &str,
    damage: Rectangle<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    let Some(candidate) = overlay.interaction_state.cluster_join_candidate.as_ref() else {
        return Ok(());
    };
    if candidate.monitor != monitor {
        return Ok(());
    }
    if !candidate.ready {
        return Ok(());
    }
    let Some(cluster) = overlay.field.cluster(candidate.cluster_id) else {
        return Ok(());
    };
    let Some(core_id) = cluster.core else {
        return Ok(());
    };
    let Some(core) = overlay.field.node(core_id) else {
        return Ok(());
    };
    let (sx, sy) = overlay.world_to_screen(screen_w, screen_h, core.pos.x, core.pos.y);
    let radius = 30;
    let rect = Rectangle::<i32, Physical>::new(
        (sx - radius, sy - radius).into(),
        (radius * 2, radius * 2).into(),
    );
    let Some(texture) = overlay.render_state.node_circle_texture.as_ref() else {
        return Ok(());
    };
    let Some(program) = overlay.render_state.node_circle_program.as_ref() else {
        return Ok(());
    };
    let tex_size: smithay::utils::Size<i32, Buffer> = texture.size();
    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (tex_size.w as f64, tex_size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("node_color", (0.17f32, 0.77f32, 0.70f32, 0.08f32)),
        Uniform::new("fill_color", (0.17f32, 0.77f32, 0.70f32, 0.05f32)),
    ];
    frame.render_texture_from_to(
        texture,
        src,
        rect,
        &[damage],
        &[],
        Transform::Normal,
        0.9,
        Some(program),
        &uniforms,
    )?;
    Ok(())
}
