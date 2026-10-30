use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle, Size};
use smithay::wayland::background_effect::BackgroundEffectSurfaceCachedState;
use smithay::wayland::compositor::{RectangleKind, RegionAttributes, with_states};

/// Resolves the committed protocol region into disjoint, surface-local
/// rectangles clipped to the current surface bounds.
///
/// Keeping the region math here makes the renderer consume ordinary
/// rectangles and, unlike old Halley, preserves ordered wl_region subtraction
/// instead of treating any non-empty request as "blur the whole surface".
pub fn blur_rects(
    surface: &WlSurface,
    surface_size: Size<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    let region = with_states(surface, |states| {
        if !states
            .cached_state
            .has::<BackgroundEffectSurfaceCachedState>()
        {
            return None;
        }
        states
            .cached_state
            .get::<BackgroundEffectSurfaceCachedState>()
            .current()
            .blur_region
            .clone()
    });
    region
        .map(|region| resolve_region(&region, Rectangle::from_size(surface_size)))
        .unwrap_or_default()
}

fn resolve_region(
    region: &RegionAttributes,
    bounds: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    let mut resolved = Vec::new();
    for (kind, rect) in &region.rects {
        let Some(rect) = rect.intersection(bounds) else {
            continue;
        };
        match kind {
            RectangleKind::Add => {
                let additions = Rectangle::subtract_rects_many([rect], resolved.iter().copied());
                resolved.extend(additions);
            }
            RectangleKind::Subtract => {
                resolved = Rectangle::subtract_rects_many(resolved, [rect]);
            }
        }
    }
    resolved.retain(|rect| !rect.is_empty());
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_subtraction_is_preserved_and_clipped() {
        let region = RegionAttributes {
            rects: vec![
                (
                    RectangleKind::Add,
                    Rectangle::new((-10, -10).into(), (120, 120).into()),
                ),
                (
                    RectangleKind::Subtract,
                    Rectangle::new((25, 25).into(), (50, 50).into()),
                ),
            ],
        };

        let resolved = resolve_region(&region, Rectangle::new((0, 0).into(), (100, 100).into()));
        assert_eq!(resolved.len(), 4);
        assert_eq!(
            resolved
                .iter()
                .map(|rect| rect.size.w * rect.size.h)
                .sum::<i32>(),
            7_500
        );
        assert!(resolved.iter().all(|rect| !rect.contains((50, 50))));
    }
}
