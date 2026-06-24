use halley_core::field::NodeId;
use smithay::backend::renderer::Texture;
use smithay::utils::{Logical, Rectangle};

use super::OverlayView;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowPreviewSource {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

impl WindowPreviewSource {
    pub(super) fn aspect(self) -> f32 {
        self.w.max(1.0) / self.h.max(1.0)
    }
}

pub(super) fn window_preview_source_rect(
    overlay: &OverlayView<'_>,
    node_id: NodeId,
    bbox: Rectangle<i32, Logical>,
) -> WindowPreviewSource {
    let bbox_w = bbox.size.w.max(1) as f32;
    let bbox_h = bbox.size.h.max(1) as f32;
    let full = WindowPreviewSource {
        x: 0.0,
        y: 0.0,
        w: bbox_w,
        h: bbox_h,
    };
    let node_aspect = overlay
        .last_active_size
        .get(&node_id)
        .copied()
        .or_else(|| overlay.field.node(node_id).map(|node| node.footprint))
        .or_else(|| overlay.field.node(node_id).map(|node| node.intrinsic_size))
        .map(|size| size.x.max(1.0) / size.y.max(1.0))
        .filter(|aspect| aspect.is_finite() && *aspect >= 0.25 && *aspect <= 4.5);
    let intrinsic_aspect = overlay
        .field
        .node(node_id)
        .map(|node| node.intrinsic_size.x.max(1.0) / node.intrinsic_size.y.max(1.0))
        .filter(|aspect| aspect.is_finite() && *aspect >= 0.25 && *aspect <= 4.5);

    // A fullscreen window has no CSD inset — its whole surface is the content — so
    // the `window_geometry` crop must not apply. Crucially, right after going
    // fullscreen the captured texture is already fullscreen-size while the cached
    // `window_geometry` still lags at the windowed rect; cropping to it would sample
    // a tiny windowed sub-rect at the old offset (small, bottom-right). Use the full
    // bbox for fullscreen tiles instead.
    if !overlay.node_is_fullscreen(node_id)
        && let Some((geo_x, geo_y, geo_w, geo_h)) = overlay
            .render_state
            .cache
            .window_geometry
            .get(&node_id)
            .copied()
        && geo_w >= 1.0
        && geo_h >= 1.0
    {
        let left = (geo_x - bbox.loc.x as f32).clamp(0.0, bbox_w);
        let top = (geo_y - bbox.loc.y as f32).clamp(0.0, bbox_h);
        let right = (geo_x + geo_w - bbox.loc.x as f32).clamp(0.0, bbox_w);
        let bottom = (geo_y + geo_h - bbox.loc.y as f32).clamp(0.0, bbox_h);
        let source = WindowPreviewSource {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        };
        if source.w >= 1.0 && source.h >= 1.0 && !source_approximately_full(source, full) {
            return source;
        }
    }

    if let Some(aspect) = node_aspect.or(intrinsic_aspect)
        && (full.aspect() - aspect).abs() > 0.05
    {
        return center_crop_to_aspect(full, aspect);
    }

    full
}

/// `src_uv_offset` / `src_uv_scale` for the `window_rounded_texture` shader, mapping
/// the cropped `src` sub-rect back to [0,1] across the dst quad so the rounding SDF
/// stays centred. Without this, CSD/GTK apps whose preview is inset to the window
/// geometry render with square corners.
pub(super) fn preview_src_uv<T: Texture>(
    texture: &T,
    source: WindowPreviewSource,
) -> ((f32, f32), (f32, f32)) {
    let size = texture.size();
    let tex_w = size.w.max(1) as f32;
    let tex_h = size.h.max(1) as f32;
    (
        (source.x / tex_w, source.y / tex_h),
        (source.w.max(1.0) / tex_w, source.h.max(1.0) / tex_h),
    )
}

fn source_approximately_full(source: WindowPreviewSource, full: WindowPreviewSource) -> bool {
    source.x.abs() <= 1.0
        && source.y.abs() <= 1.0
        && (source.w - full.w).abs() <= 1.0
        && (source.h - full.h).abs() <= 1.0
}

fn center_crop_to_aspect(source: WindowPreviewSource, target_aspect: f32) -> WindowPreviewSource {
    let target_aspect = target_aspect.clamp(0.25, 4.5);
    let source_aspect = source.aspect();
    if (source_aspect - target_aspect).abs() <= 0.01 {
        return source;
    }
    if source_aspect > target_aspect {
        let w = (source.h * target_aspect).clamp(1.0, source.w);
        WindowPreviewSource {
            x: source.x + (source.w - w) * 0.5,
            y: source.y,
            w,
            h: source.h,
        }
    } else {
        let h = (source.w / target_aspect).clamp(1.0, source.h);
        WindowPreviewSource {
            x: source.x,
            y: source.y + (source.h - h) * 0.5,
            w: source.w,
            h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_square_bbox_to_wide_aspect() {
        let source = center_crop_to_aspect(
            WindowPreviewSource {
                x: 0.0,
                y: 0.0,
                w: 1024.0,
                h: 1024.0,
            },
            16.0 / 9.0,
        );

        assert_eq!(source.x, 0.0);
        assert!((source.y - 224.0).abs() <= 0.5);
        assert_eq!(source.w, 1024.0);
        assert!((source.h - 576.0).abs() <= 0.5);
    }

    #[test]
    fn crops_square_bbox_to_portrait_aspect() {
        let source = center_crop_to_aspect(
            WindowPreviewSource {
                x: 0.0,
                y: 0.0,
                w: 1024.0,
                h: 1024.0,
            },
            9.0 / 16.0,
        );

        assert!((source.x - 224.0).abs() <= 0.5);
        assert_eq!(source.y, 0.0);
        assert!((source.w - 576.0).abs() <= 0.5);
        assert_eq!(source.h, 1024.0);
    }
}
