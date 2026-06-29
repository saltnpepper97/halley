use super::*;

use std::collections::HashSet;
use std::io;
use std::path::Path;

use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, Offscreen, Renderer};
use smithay::utils::{Buffer, Rectangle, Transform};

use crate::compositor::spawn::state::{node_rule_opacity, node_wants_blur};
use crate::render::{
    draw_offscreen_textures, draw_window_borders, ensure_node_circle_resources,
    ensure_window_texture_program,
};

fn preview_offscreen_clip(
    st: &Halley,
    node_id: NodeId,
) -> Option<(
    Rectangle<i32, smithay::utils::Logical>,
    f32,
    smithay::backend::renderer::gles::GlesTexProgram,
)> {
    // A fullscreen-active surface already IS the whole content (no window
    // chrome/border to clip away), and its xdg window-geometry cache may still
    // hold the stale windowed rect from before the client went fullscreen.
    // Skip the clip so the entire fullscreen surface is captured.
    if st.is_fullscreen_active(node_id) {
        return None;
    }
    let (x, y, w, h) = window_geometry_for_node(st, node_id)?;
    let program = st
        .ui
        .render_state
        .gpu
        .surface_clip_program
        .as_ref()?
        .clone();
    Some((
        Rectangle::<i32, smithay::utils::Logical>::new(
            (x.round() as i32, y.round() as i32).into(),
            (w.round().max(1.0) as i32, h.round().max(1.0) as i32).into(),
        ),
        st.runtime.tuning.window_border_radius_px() as f32,
        program,
    ))
}

pub(super) fn render_view_for_monitor(st: &Halley, monitor: &str) -> (Vec2, Vec2, Vec2) {
    if st.model.monitor_state.current_monitor == monitor {
        return (
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        );
    }

    st.model
        .monitor_state
        .monitors
        .get(monitor)
        .map(|space| {
            (
                space.viewport.center,
                space.viewport.size,
                space.zoom_ref_size,
            )
        })
        .unwrap_or((
            st.model.viewport.center,
            st.model.viewport.size,
            st.model.zoom_ref_size,
        ))
}

pub(super) fn world_to_screen_for_view(
    view_center: Vec2,
    view_size: Vec2,
    output_w: i32,
    output_h: i32,
    x: f32,
    y: f32,
) -> (i32, i32) {
    let vw = view_size.x.max(1.0);
    let vh = view_size.y.max(1.0);
    let nx = ((x - view_center.x) / vw) + 0.5;
    let ny = ((y - view_center.y) / vh) + 0.5;

    (
        (nx * output_w as f32).round() as i32,
        (ny * output_h as f32).round() as i32,
    )
}

pub(crate) fn capture_closing_window_animation(
    st: &Halley,
    monitor: &str,
    node_id: NodeId,
) -> Option<(Vec<ActiveBorderRect>, Vec<OffscreenNodeTexture>, f32, f32)> {
    let node = st.model.field.node(node_id)?;
    // The open grow-in is driven by the active-transition system (not the Animator). Capture the
    // window's live render scale/alpha so the close tween continues from its current on-screen
    // size instead of snapping to full size first. Mirrors window/layout.rs.
    let now = Instant::now();
    let anim = crate::frame_loop::anim_style_for(st, node_id, node.state.clone(), now);
    let transition_alpha =
        crate::compositor::workspace::state::active_transition_alpha(st, node_id, now);
    let open_scale = crate::animation::active_surface_render_scale(
        anim.scale,
        st.active_zoom_lock_scale(),
        node.intrinsic_size.x,
        node.intrinsic_size.y,
        transition_alpha,
    );
    let live_ramp = if transition_alpha > 0.0 {
        crate::animation::ease_out_back((1.0 - transition_alpha).clamp(0.0, 1.0), 1.42)
            .clamp(0.0, 1.08)
    } else {
        let live_t = ((anim.scale - 0.44) / (1.0 - 0.44)).clamp(0.0, 1.0);
        crate::animation::ease_in_out_cubic(live_t).clamp(0.0, 1.0)
    };
    let open_alpha = (anim.alpha * live_ramp).clamp(0.0, 1.0);
    let cache = st
        .ui
        .render_state
        .cache
        .window_offscreen_cache
        .get(&node_id)?;
    let texture = cache.texture.clone()?;
    let ob = cache.bbox?;
    if !cache.has_content {
        return None;
    }

    let output_size = layer_output_size_for_monitor(st, monitor);
    if output_size.w <= 0 || output_size.h <= 0 {
        return None;
    }
    let output_clip = Rectangle::<i32, Physical>::new(
        (0, 0).into(),
        (output_size.w.max(1), output_size.h.max(1)).into(),
    );

    let (view_center, viewport_size, view_size) = render_view_for_monitor(st, monitor);
    let render_scale = (viewport_size.x.max(1.0) / view_size.x.max(1.0)).max(0.01);
    // For a fullscreen surface this returns None (no CSD crop; cached geometry can be
    // stale), so we fall back to the live cache bbox `ob` — same rule the field render
    // uses — instead of over-scaling a window closing straight out of fullscreen.
    let local_geo = render_window_geometry_for_node(st, node_id).unwrap_or((
        ob.loc.x as f32,
        ob.loc.y as f32,
        ob.size.w.max(1) as f32,
        ob.size.h.max(1) as f32,
    ));
    let maximized_visual =
        crate::compositor::workspace::state::maximized_visual_for_node_on_monitor_at(
            st, node_id, monitor, now,
        );
    let tile_visual = crate::animation::cluster_tile_rect_for(
        st.ui.render_state.cluster_tile_tracks(),
        node_id,
        now,
    );
    let visual_pos = tile_visual
        .map(|rect| rect.center)
        .or_else(|| maximized_visual.map(|(pos, _)| pos))
        .unwrap_or(node.pos);
    let (cx, cy) = world_to_screen_for_view(
        view_center,
        view_size,
        output_size.w,
        output_size.h,
        visual_pos.x,
        visual_pos.y,
    );
    let (visual_w, visual_h) = tile_visual
        .map(|rect| (rect.size.x, rect.size.y))
        .or_else(|| maximized_visual.map(|(_, size)| (size.x, size.y)))
        .unwrap_or((local_geo.2, local_geo.3));
    let gw = (visual_w * render_scale).round().max(1.0) as i32;
    let gh = (visual_h * render_scale).round().max(1.0) as i32;
    let gx = cx - (gw / 2);
    let gy = cy - (gh / 2);
    let fullscreen_on_monitor = st
        .fullscreen_monitor_for_node(node_id)
        .is_some_and(|fullscreen_monitor| fullscreen_monitor == monitor);
    let decoration_metrics = if fullscreen_on_monitor {
        window_decoration_metrics(0, 0, 0, 0)
    } else {
        window_decoration_metrics(
            scaled_window_border_px(st.runtime.tuning.window_border_radius_px(), render_scale),
            scaled_window_border_px(
                st.runtime.tuning.window_primary_border_size_px(),
                render_scale,
            ),
            scaled_window_border_px(
                st.runtime.tuning.window_secondary_border_gap_px(),
                render_scale,
            ),
            scaled_window_border_px(
                st.runtime.tuning.window_secondary_border_size_px(),
                render_scale,
            ),
        )
    };
    let preserve_visual_margin = false;
    let lock_dst_to_geometry = decoration_metrics.content_corner_radius_px > 0;
    let (src_x, src_y, src_w, src_h, dst_x, dst_y, dst_w, dst_h, clip_x, clip_y, clip_w, clip_h) =
        offscreen_visual_crop_and_dst(
            ob.loc.x,
            ob.loc.y,
            ob.size.w.max(1),
            ob.size.h.max(1),
            local_geo.0,
            local_geo.1,
            local_geo.2,
            local_geo.3,
            gx,
            gy,
            gw.max(1),
            gh.max(1),
            render_scale,
            output_clip,
            preserve_visual_margin,
            lock_dst_to_geometry,
            false,
        );
    let (geo_offset_x, geo_offset_y, geo_w_px, geo_h_px) = if lock_dst_to_geometry {
        (0.0, 0.0, dst_w.max(1) as f32, dst_h.max(1) as f32)
    } else {
        let src_scale_x = if src_w > 0.0 {
            dst_w as f32 / src_w as f32
        } else {
            1.0
        };
        let src_scale_y = if src_h > 0.0 {
            dst_h as f32 / src_h as f32
        } else {
            1.0
        };
        let geo_local_x = local_geo.0 - ob.loc.x as f32;
        let geo_local_y = local_geo.1 - ob.loc.y as f32;
        let geo_src_x = (geo_local_x - src_x as f32).max(0.0);
        let geo_src_y = (geo_local_y - src_y as f32).max(0.0);
        (
            (geo_src_x * src_scale_x).max(0.0),
            (geo_src_y * src_scale_y).max(0.0),
            (local_geo.2 * src_scale_x).min(dst_w as f32).max(1.0),
            (local_geo.3 * src_scale_y).min(dst_h as f32).max(1.0),
        )
    };
    let opacity = node_rule_opacity(st, node_id);
    let offscreen = OffscreenNodeTexture {
        texture,
        alpha: opacity,
        blur: node_wants_blur(st, node_id),
        blur_alpha: 1.0,
        corner_radius: decoration_metrics.content_corner_radius_px as f32,
        src_x,
        src_y,
        src_w,
        src_h,
        dst_x,
        dst_y,
        dst_w,
        dst_h,
        clip_x,
        clip_y,
        clip_w,
        clip_h,
        geo_offset_x,
        geo_offset_y,
        geo_w: geo_w_px,
        geo_h: geo_h_px,
    };
    let border_rects = build_window_border_rects(
        st,
        node_id,
        gx,
        gy,
        gw.max(1),
        gh.max(1),
        opacity,
        render_scale,
        fullscreen_on_monitor,
    );

    Some((border_rects, vec![offscreen], open_scale, open_alpha))
}

pub(crate) fn capture_window_to_png_via_renderer(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    monitor: &str,
    node_id: NodeId,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let previous_monitor = st.begin_temporary_render_monitor(monitor);
    let result = (|| {
        let now = Instant::now();
        ensure_node_circle_resources(renderer, st)?;
        ensure_window_texture_program(renderer, st);
        prewarm_visible_close_animation_snapshots(renderer, st, now);

        let (mut border_rects, mut offscreen_textures, _, _) =
            capture_closing_window_animation(st, monitor, node_id).ok_or_else(|| {
                io::Error::other(format!(
                    "unable to prepare window capture for node {} on {monitor}",
                    node_id.as_u64()
                ))
            })?;
        let bounds = window_capture_bounds(&border_rects, &offscreen_textures)
            .ok_or_else(|| io::Error::other("window capture bounds are empty"))?;

        translate_window_capture_primitives(
            &mut border_rects,
            &mut offscreen_textures,
            bounds.loc.x,
            bounds.loc.y,
            bounds.size.w,
            bounds.size.h,
        );
        let buffer_size: smithay::utils::Size<i32, Buffer> =
            (bounds.size.w.max(1), bounds.size.h.max(1)).into();

        let mut texture = <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            buffer_size,
        )?;
        let damage = Rectangle::from_size(bounds.size);
        {
            let mut target = renderer.bind(&mut texture)?;
            let mut frame = renderer.render(&mut target, bounds.size, Transform::Normal)?;
            frame.clear(Color32F::TRANSPARENT, &[damage])?;
            draw_offscreen_textures(
                &mut frame,
                damage,
                &offscreen_textures,
                st.ui.render_state.gpu.window_texture_program.as_ref(),
                None,
            )?;
            draw_window_borders(&mut frame, bounds.size, damage, &border_rects, st)?;
            let _ = frame.finish()?;
        }

        let capture_region = Rectangle::<i32, Buffer>::from_size(buffer_size);
        let mapping = renderer.copy_texture(&texture, capture_region, Fourcc::Abgr8888)?;
        let bytes = renderer.map_texture(&mapping)?.to_vec();
        save_window_capture_png(
            output_path,
            bounds.size.w as u32,
            bounds.size.h as u32,
            bytes,
        )?;
        Ok(())
    })();
    st.end_temporary_render_monitor(previous_monitor);
    result
}

fn window_capture_bounds(
    border_rects: &[ActiveBorderRect],
    offscreen_textures: &[OffscreenNodeTexture],
) -> Option<Rectangle<i32, Physical>> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for rect in border_rects {
        let border_px = rect.border_px.max(0.0).round() as i32;
        min_x = min_x.min(rect.x - border_px);
        min_y = min_y.min(rect.y - border_px);
        max_x = max_x.max(rect.x + rect.w + border_px);
        max_y = max_y.max(rect.y + rect.h + border_px);
    }
    for tex in offscreen_textures {
        min_x = min_x.min(tex.dst_x);
        min_y = min_y.min(tex.dst_y);
        max_x = max_x.max(tex.dst_x + tex.dst_w.max(1));
        max_y = max_y.max(tex.dst_y + tex.dst_h.max(1));
    }

    (min_x < max_x && min_y < max_y).then(|| {
        Rectangle::<i32, Physical>::new(
            (min_x, min_y).into(),
            ((max_x - min_x).max(1), (max_y - min_y).max(1)).into(),
        )
    })
}

fn translate_window_capture_primitives(
    border_rects: &mut [ActiveBorderRect],
    offscreen_textures: &mut [OffscreenNodeTexture],
    offset_x: i32,
    offset_y: i32,
    clip_w: i32,
    clip_h: i32,
) {
    for rect in border_rects {
        rect.x -= offset_x;
        rect.y -= offset_y;
    }
    for tex in offscreen_textures {
        tex.dst_x -= offset_x;
        tex.dst_y -= offset_y;
        tex.clip_x = 0;
        tex.clip_y = 0;
        tex.clip_w = clip_w.max(1);
        tex.clip_h = clip_h.max(1);
    }
}

fn save_window_capture_png(
    output_path: &Path,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let image = RgbaImage::from_vec(width.max(1), height.max(1), bytes)
        .ok_or_else(|| io::Error::other("failed to build RGBA image for window capture"))?;
    image.save(output_path)?;
    Ok(())
}

/// Capture offscreen textures for the windows the alt+tab switcher is showing so
/// each card can preview the real window instead of an app icon. Runs in the
/// frame-prep phase (before the output `GlesFrame`) alongside the visible-window
/// prewarm — render-to-texture can't happen mid-frame.
///
/// Hybrid liveness: the centered/selected card refreshes on every committed frame
/// (a playing video animates as you tab), while the neighbouring cards capture
/// once and reuse the still — this caps live render-to-texture to ~1/frame.
/// Windows that must render live (`node_requires_live_surface_render`, e.g. the
/// Wine secondary-monitor surfaces) are skipped and fall back to their icon.
/// Max number of *new* neighbour still captures per frame. The selected card is
/// always captured (it's the live preview); neighbours beyond this budget keep their
/// app-icon fallback and capture on a later frame, so opening the switcher with many
/// windows doesn't do several full render-to-texture passes in one frame.
const FOCUS_CYCLE_NEIGHBOUR_CAPTURE_BUDGET: usize = 1;

pub(crate) fn prewarm_focus_cycle_previews(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    now: Instant,
) {
    let Some(mut slots) = st
        .input
        .interaction_state
        .focus_cycle_session
        .as_ref()
        .filter(|session| session.candidates.len() >= 2)
        .map(|session| session.visible_slots(crate::overlay::FOCUS_CYCLE_VISIBLE_RADIUS))
    else {
        return;
    };

    // Process the selected card (offset 0) first, then the nearest neighbours, so the
    // cards the user is most likely to land on warm before the far ones when the
    // per-frame capture budget defers the rest.
    slots.sort_by_key(|(offset, _)| offset.abs());

    let wanted_nodes: HashSet<NodeId> = slots.iter().map(|(_, node_id)| *node_id).collect();
    let mut node_surfaces: HashMap<NodeId, WlSurface> = HashMap::new();
    for toplevel in st.platform.xdg_shell_state.toplevel_surfaces().iter() {
        let wl = toplevel.wl_surface().clone();
        let Some(node_id) = st.model.surface_to_node.get(&wl.id()).copied() else {
            continue;
        };
        if wanted_nodes.contains(&node_id) {
            node_surfaces.insert(node_id, wl);
            if node_surfaces.len() == wanted_nodes.len() {
                break;
            }
        }
    }

    let mut neighbour_captures = 0usize;
    let mut deferred = false;
    for (offset, node_id) in slots {
        let Some(wl) = node_surfaces.get(&node_id).cloned() else {
            continue;
        };
        // This runs only during an alt+tab cycle, which releases the immersive
        // fullscreen lock (the window is composited, off direct scanout), so even
        // fullscreen/game surfaces are sampleable here — capture them for the
        // preview card instead of falling back to the app icon.
        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        if !matches!(node.kind, halley_core::field::NodeKind::Surface) {
            continue;
        }
        let bbox = sync_node_size_from_surface(st, node_id, &wl);

        // The selected card previews live: it reads (and refreshes) the shared
        // `window_offscreen_cache`, which the main render pass also keeps current
        // for on-screen windows. Neighbours preview a single frozen still kept in a
        // dedicated `focus_cycle_still` map the main pass never touches — sharing
        // the live cache made any neighbour that was also visible on the desktop
        // animate (the main pass overwrote its texture every committed frame).
        let selected = offset == 0;

        if selected {
            // A fresh still is captured when this card is later demoted to a
            // neighbour, so don't leave a stale frozen copy around.
            st.ui.render_state.clear_focus_cycle_still_for(node_id);

            let needs_capture = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get(&node_id)
                .is_none_or(|cache| {
                    !cache.matches_size(bbox.size.w, bbox.size.h)
                        || cache.texture.is_none()
                        || cache.bbox.is_none()
                        || !cache.has_content
                        || cache.dirty
                });
            if !needs_capture {
                continue;
            }

            st.ui.render_state.ensure_window_offscreen_cache(
                node_id,
                bbox.size.w,
                bbox.size.h,
                now,
            );

            // Capture with the same geometry clip the live-window and Apogee
            // prewarms use (`preview_offscreen_clip`); the three paths share one
            // offscreen cache keyed by node id and the live draw path reuses
            // whatever texture is there without re-clipping. Capturing the raw
            // (clip=None) surface left CSD/GTK windows' shadow margins + square
            // corners in the shared cache, so after picking such a window the live
            // path drew it mis-clipped and undersized with backdrop blur showing.
            let clip = preview_offscreen_clip(st, node_id);
            if let Ok(offscreen) = render_surface_tree_to_texture(renderer, &wl, 1.0, clip) {
                let cache = st
                    .ui
                    .render_state
                    .cache
                    .window_offscreen_cache
                    .get_mut(&node_id)
                    .expect("offscreen cache should exist after ensure");
                cache.texture = Some(offscreen.texture);
                cache.bbox = Some(offscreen.bbox);
                cache.has_content = offscreen.has_content;
                cache.mark_clean(now);
            }
        } else {
            // Neighbour: capture the still once, then leave it frozen.
            let needs_capture = st
                .ui
                .render_state
                .cache
                .focus_cycle_still
                .get(&node_id)
                .is_none_or(|cache| {
                    !cache.matches_size(bbox.size.w, bbox.size.h)
                        || cache.texture.is_none()
                        || cache.bbox.is_none()
                        || !cache.has_content
                });
            if !needs_capture {
                continue;
            }

            // Spread neighbour captures across frames so opening the switcher with many
            // windows doesn't stall on several render-to-texture passes at once. Over
            // budget: keep the icon fallback this frame and request another.
            if neighbour_captures >= FOCUS_CYCLE_NEIGHBOUR_CAPTURE_BUDGET {
                deferred = true;
                continue;
            }
            neighbour_captures += 1;

            st.ui
                .render_state
                .ensure_focus_cycle_still(node_id, bbox.size.w, bbox.size.h, now);

            let clip = preview_offscreen_clip(st, node_id);
            if let Ok(offscreen) = render_surface_tree_to_texture(renderer, &wl, 1.0, clip) {
                let cache = st
                    .ui
                    .render_state
                    .cache
                    .focus_cycle_still
                    .get_mut(&node_id)
                    .expect("focus-cycle still should exist after ensure");
                cache.texture = Some(offscreen.texture);
                cache.bbox = Some(offscreen.bbox);
                cache.has_content = offscreen.has_content;
                cache.mark_clean(now);
            }
        }
    }

    // Drop frozen stills for cards no longer visible in the switcher.
    st.ui.render_state.prune_focus_cycle_still(&wanted_nodes);

    // Some neighbour stills were deferred by the per-frame budget; schedule another
    // frame so they fill in even if the switcher is otherwise idle.
    if deferred {
        st.request_maintenance();
    }
}

pub(crate) fn prewarm_apogee_previews(renderer: &mut GlesRenderer, st: &mut Halley, now: Instant) {
    use crate::compositor::overview::{ApogeePhase, ApogeeTileKind};

    const APOGEE_INITIAL_CAPTURE_BUDGET: usize = 4;
    const APOGEE_LIVE_REFRESH_MIN_MS: u64 = 33;

    let Some(session) = st.input.interaction_state.apogee_session.as_ref() else {
        return;
    };
    if session.phase == ApogeePhase::Closing {
        return;
    }
    let current_monitor = st.model.monitor_state.current_monitor.clone();
    let Some(monitor_session) = session.monitor_session(current_monitor.as_str()) else {
        return;
    };
    let tile_node_ids = monitor_session
        .tiles
        .iter()
        .filter(|tile| matches!(tile.kind, ApogeeTileKind::Window))
        .map(|tile| tile.node_id)
        .collect::<Vec<_>>();

    let live = st.runtime.tuning.apogee.live_previews;
    let live_node = live
        .then_some(st.input.interaction_state.apogee_live_preview_node)
        .flatten()
        .filter(|node_id| tile_node_ids.contains(node_id));
    let mut missing = Vec::new();
    for node_id in tile_node_ids {
        let cache = st
            .ui
            .render_state
            .cache
            .window_offscreen_cache
            .get(&node_id);
        let needs_initial = cache.is_none_or(|cache| {
            cache.texture.is_none() || cache.bbox.is_none() || !cache.has_content
        });
        if needs_initial {
            missing.push(node_id);
        }
    }

    if let Some(live_node) = live_node {
        // If the hovered tile still needs its first snapshot, prioritize it so
        // hover-live feels immediate even when many windows are filling in.
        missing.sort_by_key(|node_id| if *node_id == live_node { 0 } else { 1 });
    }

    let mut refresh_live = false;
    let mut targets = missing
        .iter()
        .take(APOGEE_INITIAL_CAPTURE_BUDGET)
        .copied()
        .collect::<Vec<_>>();
    if targets.is_empty()
        && let Some(live_node) = live_node
        && st
            .ui
            .render_state
            .cache
            .window_offscreen_cache
            .get(&live_node)
            .is_some_and(|cache| cache.dirty)
    {
        let due = st
            .input
            .interaction_state
            .apogee_live_preview_last_at
            .is_none_or(|last| {
                now.saturating_duration_since(last).as_millis() as u64 >= APOGEE_LIVE_REFRESH_MIN_MS
            });
        if due {
            targets.push(live_node);
            refresh_live = true;
        }
    }
    if targets.is_empty() {
        return;
    }

    let wanted: HashSet<NodeId> = targets.iter().copied().collect();
    let mut node_surfaces: HashMap<NodeId, WlSurface> = HashMap::new();
    for toplevel in st.platform.xdg_shell_state.toplevel_surfaces().iter() {
        let wl = toplevel.wl_surface().clone();
        let Some(node_id) = st.model.surface_to_node.get(&wl.id()).copied() else {
            continue;
        };
        if wanted.contains(&node_id) {
            node_surfaces.insert(node_id, wl);
            if node_surfaces.len() == wanted.len() {
                break;
            }
        }
    }

    for node_id in targets {
        let Some(wl) = node_surfaces.get(&node_id).cloned() else {
            continue;
        };
        // This runs only while apogee is open, where the fullscreen/game window is
        // soft-suspended (composited, off direct scanout), so its surface is
        // sampleable — capture it for the tile instead of falling back to the icon.
        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        if !matches!(node.kind, halley_core::field::NodeKind::Surface) {
            continue;
        }
        let bbox = sync_node_size_from_surface(st, node_id, &wl);
        let needs_capture = st
            .ui
            .render_state
            .cache
            .window_offscreen_cache
            .get(&node_id)
            .is_none_or(|cache| {
                !cache.matches_size(bbox.size.w, bbox.size.h)
                    || cache.texture.is_none()
                    || cache.bbox.is_none()
                    || !cache.has_content
                    || (live && cache.dirty)
            });
        if !needs_capture {
            continue;
        }
        st.ui
            .render_state
            .ensure_window_offscreen_cache(node_id, bbox.size.w, bbox.size.h, now);
        let clip = preview_offscreen_clip(st, node_id);
        if let Ok(offscreen) = render_surface_tree_to_texture(renderer, &wl, 1.0, clip) {
            let cache = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get_mut(&node_id)
                .expect("offscreen cache should exist after ensure");
            cache.texture = Some(offscreen.texture);
            cache.bbox = Some(offscreen.bbox);
            cache.has_content = offscreen.has_content;
            cache.mark_clean(now);
            if refresh_live && Some(node_id) == live_node {
                st.input.interaction_state.apogee_live_preview_last_at = Some(now);
            }
        }
    }
}

pub(crate) fn prewarm_visible_close_animation_snapshots(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    now: Instant,
) {
    let requested_prewarm_nodes = st
        .ui
        .render_state
        .requested_window_animation_prewarm_nodes(now);
    let mut target_nodes: HashSet<NodeId> = requested_prewarm_nodes.iter().copied().collect();
    if let Some(node_id) = st.model.focus_state.primary_interaction_focus {
        target_nodes.insert(node_id);
    }
    if let Some(node_id) = st.last_focused_surface_node_for_monitor(st.focused_monitor()) {
        target_nodes.insert(node_id);
    }
    if target_nodes.is_empty() {
        return;
    }
    let mut wl_surfaces: Vec<_> = st
        .platform
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|toplevel| {
            let wl = toplevel.wl_surface().clone();
            let node_id = st.model.surface_to_node.get(&wl.id()).copied()?;
            Some((node_id, wl))
        })
        .collect();

    wl_surfaces.sort_by(|(left, _), (right, _)| {
        requested_prewarm_nodes
            .contains(right)
            .cmp(&requested_prewarm_nodes.contains(left))
            .then_with(|| right.as_u64().cmp(&left.as_u64()))
    });

    for (node_id, wl) in wl_surfaces {
        if !target_nodes.contains(&node_id) {
            continue;
        }
        let bbox = sync_node_size_from_surface(st, node_id, &wl);
        let Some(node) = st.model.field.node(node_id) else {
            continue;
        };
        if node.state != halley_core::field::NodeState::Active
            || node_requires_live_surface_render(st, node_id)
        {
            continue;
        }
        // While a tile-open/close transition is animating, freeze the snapshot
        // texture. Snapshot consumers scale this single capture, so
        // re-capturing on every intermediate size the client commits as it settles
        // into the final tile is both wasted GPU work (grows to ~5ms/frame —
        // enough to miss vblanks on a 180Hz output → the choppy slide) and the
        // source of the mid-slide "displaced texture" (a lagging capture drawn
        // against the moved geometry). Reuse the existing texture; the normal
        // rebuild resumes once the transition settles.
        let tile_transition_active = crate::animation::cluster_tile_rect_for(
            st.ui.render_state.cluster_tile_tracks(),
            node_id,
            now,
        )
        .is_some();
        if tile_transition_active
            && st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get(&node_id)
                .is_some_and(|cache| {
                    cache.texture.is_some() && cache.bbox.is_some() && cache.has_content
                })
        {
            continue;
        }

        let cache_missing = st
            .ui
            .render_state
            .cache
            .window_offscreen_cache
            .get(&node_id)
            .is_none_or(|cache| {
                !cache.matches_size(bbox.size.w, bbox.size.h)
                    || cache.texture.is_none()
                    || cache.bbox.is_none()
                    || !cache.has_content
            });
        if !cache_missing {
            continue;
        }

        let cache = st.ui.render_state.ensure_window_offscreen_cache(
            node_id,
            bbox.size.w,
            bbox.size.h,
            now,
        );
        if !cache.dirty && cache.texture.is_some() && cache.bbox.is_some() && cache.has_content {
            continue;
        }

        let clip = preview_offscreen_clip(st, node_id);
        if let Ok(offscreen) = render_surface_tree_to_texture(renderer, &wl, 1.0, clip) {
            let cache = st
                .ui
                .render_state
                .cache
                .window_offscreen_cache
                .get_mut(&node_id)
                .expect("offscreen cache should exist after prewarm ensure");
            cache.texture = Some(offscreen.texture);
            cache.bbox = Some(offscreen.bbox);
            cache.has_content = offscreen.has_content;
            cache.mark_clean(now);
            if cache.has_content {
                st.ui.render_state.finish_window_animation_prewarm(node_id);
            }
        }
    }
}
