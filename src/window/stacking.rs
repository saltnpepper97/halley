//! Parent-child constraints shared by desktop stacking, rendering and input.

use smithay::desktop::{Space, Window};

pub(crate) fn parent_window(space: &Space<Window>, window: &Window) -> Option<Window> {
    if crate::xwayland::is_override_redirect(window) {
        return None;
    }
    if let Some(toplevel) = window.toplevel() {
        let parent = toplevel.parent()?;
        return space
            .elements()
            .find(|candidate| {
                candidate
                    .toplevel()
                    .is_some_and(|t| t.wl_surface() == &parent)
            })
            .cloned();
    }
    crate::xwayland::parent_window(space, window)
}

pub(crate) fn is_descendant_of(space: &Space<Window>, child: &Window, parent: &Window) -> bool {
    let mut current = child.clone();
    // Clients can supply cyclic X11 relationships. Never recurse indefinitely.
    for _ in 0..space.elements().count() {
        let Some(ancestor) = parent_window(space, &current) else {
            return false;
        };
        if &ancestor == parent {
            return child != parent;
        }
        current = ancestor;
    }
    false
}

/// Stable bottom-to-top topological order. A blocked child waits for its
/// parent; unrelated ready entries retain their order. Ignore edges inside
/// malformed cycles while retaining the constraints on their descendants.
pub(super) fn parent_order(parents: &[Option<usize>]) -> Vec<usize> {
    let mut parents = parents.to_vec();
    for child in 0..parents.len() {
        let mut current = parents[child];
        for _ in 0..parents.len() {
            let Some(parent) = current else { break };
            if parent == child {
                parents[child] = None;
                break;
            }
            current = parents[parent];
        }
    }
    let mut order = Vec::with_capacity(parents.len());
    let mut placed = vec![false; parents.len()];
    while order.len() < parents.len() {
        let next = (0..parents.len())
            .find(|&i| !placed[i] && parents[i].is_none_or(|parent| placed[parent]))
            .expect("cycles were removed");
        placed[next] = true;
        order.push(next);
    }
    order
}

/// Applies the same constraints after presentation-specific sorting. Entries
/// without a live window (nodes, snapshots and cluster cores) stay independent.
pub(crate) fn sort_above_parents<T>(
    space: &Space<Window>,
    entries: &mut Vec<T>,
    window: impl Fn(&T) -> Option<&Window>,
) -> bool {
    let parents = entries
        .iter()
        .map(|entry| {
            let parent = parent_window(space, window(entry)?)?;
            entries
                .iter()
                .position(|entry| window(entry) == Some(&parent))
        })
        .collect::<Vec<_>>();
    if parents
        .iter()
        .enumerate()
        .all(|(i, parent)| parent.is_none_or(|p| p < i))
    {
        return false;
    }
    let order = parent_order(&parents);
    if order.iter().copied().eq(0..entries.len()) {
        return false;
    }
    let mut previous = std::mem::take(entries)
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    entries.extend(order.into_iter().map(|i| previous[i].take().unwrap()));
    true
}

#[cfg(test)]
mod tests {
    use super::parent_order;

    #[test]
    fn raising_a_parent_brings_its_nested_dialogs_above_it() {
        // grandchild, child, unrelated foreground, newly raised parent
        assert_eq!(parent_order(&[Some(1), Some(3), None, None]), [2, 3, 1, 0]);
    }

    #[test]
    fn siblings_keep_their_depth_and_unrelated_windows_can_cover_the_family() {
        assert_eq!(parent_order(&[Some(2), Some(2), None, None]), [2, 0, 1, 3]);
        assert_eq!(parent_order(&[None, Some(0), Some(0), None]), [0, 1, 2, 3]);
    }

    #[test]
    fn late_parent_changes_repair_the_stack() {
        assert_eq!(parent_order(&[Some(2), None, None, Some(0)]), [1, 2, 0, 3]);
    }

    #[test]
    fn missing_parents_and_empty_stacks_need_no_reordering() {
        assert_eq!(parent_order(&[None, None]), [0, 1]);
        assert!(parent_order(&[]).is_empty());
    }

    #[test]
    fn cyclic_clients_do_not_hang_or_lose_their_descendants() {
        assert_eq!(parent_order(&[Some(1), Some(0), Some(1)]), [0, 1, 2]);
        assert_eq!(parent_order(&[Some(0)]), [0]);
    }
}
