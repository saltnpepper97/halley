//! Order ordinary window scenes and separate their popup and exclusive planes.

use super::*;
use smithay::desktop::{Space, Window};

type IndexedWindow = (usize, LiveWindowScene);

pub(super) struct WindowStackPlanes {
    pub desktop: Vec<SceneElement>,
    pub popups: Vec<StackGroup>,
    pub floating: Vec<IndexedWindow>,
}

pub(super) fn arrange(
    space: &Space<Window>,
    mut stack: Vec<StackGroup>,
    live_windows: Vec<IndexedWindow>,
    exclusive_windows: &[IndexedWindow],
) -> WindowStackPlanes {
    let exclusive_anchor = exclusive_windows
        .iter()
        .map(|(stack_index, _)| *stack_index)
        .max();
    let (floating_foreground, mut live_windows) = match exclusive_anchor {
        Some(exclusive_anchor) => live_windows.into_iter().partition(|(stack_index, scene)| {
            cluster_float_is_above_exclusive(scene.cluster_floating, *stack_index, exclusive_anchor)
        }),
        None => (Vec::new(), live_windows),
    };
    // A cluster workspace is one coherent stack. Preserve its position
    // relative to non-cluster windows at the topmost member's existing Space
    // slot, then use the layout's explicit depth for member overlap.
    let cluster_anchor = live_windows
        .iter()
        .filter_map(|(stack_index, scene)| scene.cluster_depth.map(|_| *stack_index))
        .max();
    let mut popup_stack = Vec::new();
    let landmark_ceiling = live_windows
        .iter()
        .filter(|(_, scene)| scene.above_field_landmarks)
        .map(|(stack_index, _)| *stack_index)
        .min();
    for (stack_index, window_scene) in &mut live_windows {
        if !window_scene.popup_elements.is_empty() {
            popup_stack.push(StackGroup {
                window: Some(window_scene.window.clone()),
                stack_index: window_scene
                    .cluster_depth
                    .and(cluster_anchor)
                    .unwrap_or(*stack_index),
                order: window_scene
                    .cluster_depth
                    .map_or(u64::MAX, |depth| depth as u64),
                elements: std::mem::take(&mut window_scene.popup_elements),
            });
        }
    }
    for (stack_index, window_scene) in live_windows {
        stack.push(StackGroup {
            window: Some(window_scene.window),
            stack_index: window_scene
                .cluster_depth
                .and(cluster_anchor)
                .unwrap_or(stack_index),
            order: window_scene
                .cluster_depth
                .map_or(u64::MAX, |depth| depth as u64),
            elements: window_scene.elements,
        });
    }
    if let Some(ceiling) = landmark_ceiling {
        // Collapsed nodes remember their old Space slot, which can now be
        // higher than every remaining live window. Temporarily place desktop
        // objects below presentations without changing their saved Field depth.
        for group in stack.iter_mut().filter(|group| group.window.is_none()) {
            group.stack_index = group.stack_index.min(ceiling);
        }
    }
    sort_stack_groups(&mut stack);
    crate::window::stacking::sort_above_parents(space, &mut stack, |group| group.window.as_ref());
    let desktop = stack
        .into_iter()
        .rev()
        .flat_map(|group| group.elements)
        .collect();

    WindowStackPlanes {
        desktop,
        popups: popup_stack,
        floating: floating_foreground,
    }
}
