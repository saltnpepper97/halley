use std::time::Instant;

use smithay::wayland::compositor::{SurfaceAttributes, with_states};
use smithay::{
    backend::renderer::{
        element::Kind, element::surface::render_elements_from_surface_tree, gles::GlesRenderer,
    },
    utils::{Physical, Rectangle},
};

use super::super::bearings::{BearingChipLayout, collect_bearing_layouts};
use super::super::cursor_surface_hotspot;
use super::super::layer_shell::collect_layer_surfaces;
use super::super::node::{NodeSnapshot, collect_hover_preview};
use super::super::state::ClosingWindowAnimationSnapshot;
use crate::compositor::interaction::ResizeCtx;
use crate::compositor::root::Halley;
use crate::window::{
    ActiveBorderRect, CroppedClippedSurfaceElement, OffscreenNodeTexture, StackWindowDrawUnit,
    collect_active_surfaces,
};

pub(super) type SurfaceElement =
    smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement<GlesRenderer>;

pub(super) struct PreparedFrameState {
    pub(super) damage: Rectangle<i32, Physical>,
    pub(super) now: Instant,
}

pub(super) struct SceneCollections {
    pub(super) session_lock_elements: Vec<SurfaceElement>,
    pub(super) layer_background_elements: Vec<SurfaceElement>,
    pub(super) layer_bottom_elements: Vec<SurfaceElement>,
    pub(super) layer_top_elements: Vec<SurfaceElement>,
    pub(super) layer_overlay_elements: Vec<SurfaceElement>,
    pub(super) active_elements: Vec<CroppedClippedSurfaceElement>,
    pub(super) resized_active_elements: Vec<CroppedClippedSurfaceElement>,
    pub(super) fullscreen_active_elements: Vec<CroppedClippedSurfaceElement>,
    pub(super) above_fullscreen_active_elements: Vec<CroppedClippedSurfaceElement>,
    pub(super) offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) resized_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) fullscreen_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) above_fullscreen_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) popup_elements:
        Vec<smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>>,
    pub(super) fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) fullscreen_popup_elements:
        Vec<smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>>,
    pub(super) above_fullscreen_popup_offscreen_textures: Vec<OffscreenNodeTexture>,
    pub(super) above_fullscreen_popup_elements:
        Vec<smithay::backend::renderer::element::utils::CropRenderElement<SurfaceElement>>,
    pub(super) stack_window_units: Vec<StackWindowDrawUnit>,
    pub(super) border_rects: Vec<ActiveBorderRect>,
    pub(super) resized_border_rects: Vec<ActiveBorderRect>,
    pub(super) above_fullscreen_border_rects: Vec<ActiveBorderRect>,
    pub(super) closing_window_animations: Vec<ClosingWindowAnimationSnapshot>,
    pub(super) overlap_overlay_rects: Vec<(i32, i32, i32, i32)>,
    pub(super) hover_preview_rect: Option<(i32, i32, i32, i32)>,
    pub(super) hover_preview_elements: Vec<SurfaceElement>,
    pub(super) render_nodes: Vec<NodeSnapshot>,
    pub(super) bearing_layouts: Vec<BearingChipLayout>,
}

pub(super) struct CursorScene {
    pub(super) cursor_status: smithay::input::pointer::CursorImageStatus,
    pub(super) cursor_surface_elements: Vec<SurfaceElement>,
}

pub(super) fn prepare_debug_frame_state(
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
) -> PreparedFrameState {
    let now = Instant::now();
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        crate::protocol::wayland::session_lock::configure_surfaces(st);
    }
    if !st.input.interaction_state.suppress_layer_shell_configure {
        crate::compositor::monitor::layer_shell::configure_layer_shell_surfaces(
            st,
            (size.w, size.h).into(),
        );
    }

    PreparedFrameState {
        damage: Rectangle::<i32, Physical>::from_size(size),
        now,
    }
}

pub(super) fn collect_debug_frame_scene(
    renderer: &mut GlesRenderer,
    st: &mut Halley,
    size: smithay::utils::Size<i32, Physical>,
    resize_preview: Option<ResizeCtx>,
    hover_node: Option<halley_core::field::NodeId>,
    preview_hover_node: Option<halley_core::field::NodeId>,
    now: Instant,
) -> SceneCollections {
    if crate::protocol::wayland::session_lock::session_lock_active(st) {
        let session_lock_elements =
            crate::protocol::wayland::session_lock::current_monitor_surfaces(st)
                .into_iter()
                .flat_map(|surface| {
                    render_elements_from_surface_tree(
                        renderer,
                        &surface,
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                })
                .collect();
        return SceneCollections {
            session_lock_elements,
            layer_background_elements: Vec::new(),
            layer_bottom_elements: Vec::new(),
            layer_top_elements: Vec::new(),
            layer_overlay_elements: Vec::new(),
            active_elements: Vec::new(),
            resized_active_elements: Vec::new(),
            fullscreen_active_elements: Vec::new(),
            above_fullscreen_active_elements: Vec::new(),
            offscreen_textures: Vec::new(),
            resized_offscreen_textures: Vec::new(),
            fullscreen_offscreen_textures: Vec::new(),
            above_fullscreen_offscreen_textures: Vec::new(),
            popup_offscreen_textures: Vec::new(),
            popup_elements: Vec::new(),
            fullscreen_popup_offscreen_textures: Vec::new(),
            fullscreen_popup_elements: Vec::new(),
            above_fullscreen_popup_offscreen_textures: Vec::new(),
            above_fullscreen_popup_elements: Vec::new(),
            stack_window_units: Vec::new(),
            border_rects: Vec::new(),
            resized_border_rects: Vec::new(),
            above_fullscreen_border_rects: Vec::new(),
            closing_window_animations: Vec::new(),
            overlap_overlay_rects: Vec::new(),
            hover_preview_rect: None,
            hover_preview_elements: Vec::new(),
            render_nodes: Vec::new(),
            bearing_layouts: Vec::new(),
        };
    }

    let render_monitor = st.model.monitor_state.current_monitor.clone();
    let bearings_mix = st
        .ui
        .render_state
        .bearings_mix_for_monitor(render_monitor.as_str());
    let (
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
    ) = collect_layer_surfaces(renderer, st, size, now);

    let (
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        above_fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        above_fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        above_fullscreen_popup_offscreen_textures,
        above_fullscreen_popup_elements,
        node_surface_map,
        stack_window_units,
        border_rects,
        resized_border_rects,
        above_fullscreen_border_rects,
        overlap_overlay_rects,
    ) = collect_active_surfaces(renderer, st, size, resize_preview, now);
    let closing_window_animations = if st.runtime.tuning.window_close_animation_enabled() {
        st.ui
            .render_state
            .closing_window_animation_snapshots(render_monitor.as_str(), now)
    } else {
        Vec::new()
    };

    let hovered_preview_id = preview_hover_node.and_then(|id| {
        st.model.field.node(id).and_then(|n| {
            matches!(
                n.state,
                halley_core::field::NodeState::Node | halley_core::field::NodeState::Core
            )
            .then_some(id)
        })
    });
    let overlay_hover_preview = st
        .input
        .interaction_state
        .overlay_hover_target
        .as_ref()
        .filter(|target| {
            target.monitor == render_monitor
                && preview_hover_node == Some(target.node_id)
                && st
                    .input
                    .interaction_state
                    .cluster_overflow_drag_preview
                    .is_none()
        })
        .map(|target| (target.node_id, target.screen_anchor, target.prefer_left));
    let (hover_preview_rect, hover_preview_elements) = collect_hover_preview(
        renderer,
        st,
        size,
        render_monitor.as_str(),
        &node_surface_map,
        hovered_preview_id,
        overlay_hover_preview,
        hover_node,
        now,
    );

    let render_nodes = st
        .model
        .field
        .nodes()
        .keys()
        .copied()
        .into_iter()
        .filter_map(|id| {
            let node = st.model.field.node(id)?;
            if !st.model.field.participates_in_field_view(id)
                || !st.model.field.is_visible(id)
                || !st.node_visible_on_current_monitor(id)
            {
                return None;
            }
            Some(NodeSnapshot {
                id,
                state: node.state.clone(),
                pos: node.pos,
                intrinsic_size: node.intrinsic_size,
                label: node.label.clone(),
            })
        })
        .collect();
    let bearing_layouts =
        collect_bearing_layouts(st, size.w, size.h, render_monitor.as_str(), bearings_mix);

    SceneCollections {
        session_lock_elements: Vec::new(),
        layer_background_elements,
        layer_bottom_elements,
        layer_top_elements,
        layer_overlay_elements,
        active_elements,
        resized_active_elements,
        fullscreen_active_elements,
        above_fullscreen_active_elements,
        offscreen_textures,
        resized_offscreen_textures,
        fullscreen_offscreen_textures,
        above_fullscreen_offscreen_textures,
        popup_offscreen_textures,
        popup_elements,
        fullscreen_popup_offscreen_textures,
        fullscreen_popup_elements,
        above_fullscreen_popup_offscreen_textures,
        above_fullscreen_popup_elements,
        stack_window_units,
        border_rects,
        resized_border_rects,
        above_fullscreen_border_rects,
        closing_window_animations,
        overlap_overlay_rects,
        hover_preview_rect,
        hover_preview_elements,
        render_nodes,
        bearing_layouts,
    }
}

pub(super) fn collect_cursor_scene(
    renderer: &mut GlesRenderer,
    cursor_screen: Option<(f32, f32)>,
    cursor_image: Option<&smithay::input::pointer::CursorImageStatus>,
) -> CursorScene {
    let cursor_status = cursor_image
        .cloned()
        .unwrap_or_else(smithay::input::pointer::CursorImageStatus::default_named);

    let mut cursor_surface_elements = Vec::new();
    if let (Some((sx, sy)), smithay::input::pointer::CursorImageStatus::Surface(surface)) =
        (cursor_screen, cursor_status.clone())
    {
        let scale = with_states(&surface, |states| {
            states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .buffer_scale as f64
        });
        let (hotspot_x, hotspot_y) = cursor_surface_hotspot(&surface);
        let loc = (sx.round() as i32 - hotspot_x, sy.round() as i32 - hotspot_y);
        cursor_surface_elements = render_elements_from_surface_tree(
            renderer,
            &surface,
            loc,
            scale,
            1.0,
            Kind::Unspecified,
        );
    }

    CursorScene {
        cursor_status,
        cursor_surface_elements,
    }
}
