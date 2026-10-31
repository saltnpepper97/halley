use super::*;

fn cluster(raw: u64) -> ClusterId {
    ClusterId::new(raw)
}

fn owners(pairs: &[(u64, &str)]) -> Vec<(ClusterId, String)> {
    pairs
        .iter()
        .map(|(id, output)| (cluster(*id), (*output).to_string()))
        .collect()
}

fn reduce(
    requests: &[(ClusterId, PendingAction)],
    owners: &[(ClusterId, String)],
    active: &[(String, Option<ClusterId>)],
) -> TransactionPlan {
    reduce_transaction(
        requests,
        |_| true,
        |id| {
            owners
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .map(|(_, output)| output.clone())
        },
        |output| {
            active
                .iter()
                .find(|(candidate, _)| candidate == output)
                .and_then(|(_, selection)| *selection)
        },
    )
}

#[test]
fn repeated_activation_collapses_to_one_selection() {
    let owners = owners(&[(1, "DP-1")]);
    let active = vec![("DP-1".to_string(), None)];

    let plan = reduce(
        &[
            (cluster(1), PendingAction::Activate),
            (cluster(1), PendingAction::Activate),
        ],
        &owners,
        &active,
    );

    assert_eq!(
        plan.selections,
        vec![("DP-1".to_string(), Some(cluster(1)))]
    );
    assert_eq!(plan.focus, Some(("DP-1".to_string(), cluster(1))));
}

#[test]
fn activating_the_already_active_workspace_is_a_no_op() {
    let owners = owners(&[(1, "DP-1")]);
    let active = vec![("DP-1".to_string(), Some(cluster(1)))];

    let plan = reduce(&[(cluster(1), PendingAction::Activate)], &owners, &active);

    assert!(plan.is_empty());
    assert_eq!(plan.focus, None);
}

#[test]
fn final_activation_per_output_wins() {
    let owners = owners(&[(1, "DP-1"), (2, "DP-1")]);
    let active = vec![("DP-1".to_string(), None)];

    let plan = reduce(
        &[
            (cluster(1), PendingAction::Activate),
            (cluster(2), PendingAction::Activate),
        ],
        &owners,
        &active,
    );

    assert_eq!(
        plan.selections,
        vec![("DP-1".to_string(), Some(cluster(2)))]
    );
    assert_eq!(plan.focus, Some(("DP-1".to_string(), cluster(2))));
}

#[test]
fn deactivating_an_unrelated_workspace_leaves_the_selection_alone() {
    let owners = owners(&[(1, "DP-1"), (2, "DP-1")]);
    let active = vec![("DP-1".to_string(), Some(cluster(1)))];

    let plan = reduce(&[(cluster(2), PendingAction::Deactivate)], &owners, &active);

    assert!(plan.is_empty());
    assert_eq!(plan.focus, None);
}

#[test]
fn deactivating_the_active_workspace_clears_the_output() {
    let owners = owners(&[(1, "DP-1")]);
    let active = vec![("DP-1".to_string(), Some(cluster(1)))];

    let plan = reduce(&[(cluster(1), PendingAction::Deactivate)], &owners, &active);

    assert_eq!(plan.selections, vec![("DP-1".to_string(), None)]);
    assert_eq!(plan.focus, None);
}

#[test]
fn deactivating_field_mode_is_a_no_op() {
    let owners = owners(&[(1, "DP-1")]);
    let active = vec![("DP-1".to_string(), None)];

    let plan = reduce(&[(cluster(1), PendingAction::Deactivate)], &owners, &active);

    assert!(plan.is_empty());
}

#[test]
fn deactivation_after_activation_clears_the_same_output() {
    let owners = owners(&[(1, "DP-1")]);
    let active = vec![("DP-1".to_string(), None)];

    let plan = reduce(
        &[
            (cluster(1), PendingAction::Activate),
            (cluster(1), PendingAction::Deactivate),
        ],
        &owners,
        &active,
    );

    assert!(plan.is_empty(), "the transaction nets out to no change");
}

#[test]
fn a_multi_output_transaction_switches_every_output_but_focuses_one() {
    let owners = owners(&[(1, "DP-1"), (2, "DP-2"), (3, "DP-1")]);
    let active = vec![
        ("DP-1".to_string(), Some(cluster(1))),
        ("DP-2".to_string(), None),
    ];

    let plan = reduce(
        &[
            (cluster(2), PendingAction::Activate),
            (cluster(3), PendingAction::Activate),
        ],
        &owners,
        &active,
    );

    assert_eq!(
        plan.selections,
        vec![
            ("DP-2".to_string(), Some(cluster(2))),
            ("DP-1".to_string(), Some(cluster(3))),
        ],
        "both changed outputs are switched"
    );
    assert_eq!(
        plan.focus,
        Some(("DP-1".to_string(), cluster(3))),
        "only the final applicable activation takes the keyboard"
    );
}

#[test]
fn dead_targets_are_discarded() {
    let owners = owners(&[(1, "DP-1")]);
    let active = [("DP-1".to_string(), None)];

    let plan = reduce_transaction(
        &[
            (cluster(1), PendingAction::Activate),
            (cluster(9), PendingAction::Activate),
        ],
        |id| id != cluster(1),
        |id| {
            owners
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .map(|(_, output)| output.clone())
        },
        |output| {
            active
                .iter()
                .find(|(candidate, _)| candidate == output)
                .and_then(|(_, selection)| *selection)
        },
    );

    assert!(plan.is_empty(), "a destroyed target must not be applied");
    assert_eq!(plan.focus, None);
}

#[test]
fn coordinates_are_a_single_dense_zero_based_dimension() {
    assert_eq!(coordinates(1), 0u32.to_ne_bytes().to_vec());
    assert_eq!(coordinates(3), 2u32.to_ne_bytes().to_vec());
}

#[test]
fn only_active_workspaces_carry_a_state_bit() {
    assert_eq!(
        state_of(true).bits(),
        ext_workspace_handle_v1::State::Active.bits()
    );
    assert_eq!(state_of(false).bits(), 0);
}

#[test]
fn advertised_capabilities_are_activate_and_deactivate_only() {
    assert_eq!(GROUP_CAPABILITIES.bits(), 0);
    assert_eq!(
        WORKSPACE_CAPABILITIES.bits(),
        ext_workspace_handle_v1::WorkspaceCapabilities::Activate.bits()
            | ext_workspace_handle_v1::WorkspaceCapabilities::Deactivate.bits()
    );
    assert!(
        !WORKSPACE_CAPABILITIES
            .contains(ext_workspace_handle_v1::WorkspaceCapabilities::Remove)
    );
    assert!(
        !WORKSPACE_CAPABILITIES
            .contains(ext_workspace_handle_v1::WorkspaceCapabilities::Assign)
    );
}
