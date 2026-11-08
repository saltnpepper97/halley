//! Create a cluster from overlapping windows through the real keyboard and
//! pointer. Creation lays the candidates out without overlap over the Field.
use super::fullscreen_multi_output::Input;
use super::*;
use serde_json::Value;

fn tile_rect(state: &Value, id: u64) -> Option<[i64; 4]> {
    let tile = state["composer"]["tiles"]
        .as_array()?
        .iter()
        .find(|tile| tile["id"] == id)?;
    let values = tile["rect"].as_array()?;
    Some(std::array::from_fn(|index| values[index].as_i64().unwrap()))
}

fn overlaps(a: [i64; 4], b: [i64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

fn tap(input: &Input, code: u32) {
    input.key(code, true);
    input.key(code, false);
}

fn new_cluster_members(fixture: &mut Fixture) -> Option<usize> {
    let Response::ClusterList(list) =
        fixture.request(Request::Cluster(halley_ipc::ClusterRequest::List {
            output: None,
        }))
    else {
        return None;
    };
    list.outputs
        .into_iter()
        .flat_map(|group| group.clusters)
        .find(|cluster| cluster.name != "Disconnect regression")
        .map(|cluster| cluster.member_count)
}

#[test]
#[ignore = "requires Wayland, XWayland and HALLEY_TEST_WINIT_SERVER"]
fn cluster_mode_lays_out_overlapping_windows_and_forms_the_cluster() {
    let mut fixture = Fixture::with_two_outputs("");
    let input = Input::new(&fixture);
    // New windows open at the same place, so these two overlap exactly.
    let (mut queue_a, mut client_a) = native_window_named(
        &fixture,
        "Field cluster A",
        "halley.test.field-cluster-a",
        [u32::MAX, 0, 0, u32::MAX],
    );
    let (mut queue_b, mut client_b) = native_window_named(
        &fixture,
        "Field cluster B",
        "halley.test.field-cluster-b",
        [0, u32::MAX, 0, u32::MAX],
    );
    let a = fixture.node("Field cluster A").id;
    let b = fixture.node("Field cluster B").id;

    // F6 is bound to cluster-mode in this fixture.
    tap(&input, 64);
    let (rect_a, rect_b) = wait_for("both windows laid out", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        let state = input.state();
        (state["composer"]["interactive"] == true)
            .then(|| tile_rect(&state, a).zip(tile_rect(&state, b)))
            .flatten()
    });
    assert!(
        !overlaps(rect_a, rect_b),
        "overlapping windows are laid out apart: {rect_a:?} {rect_b:?}"
    );
    for rect in [rect_a, rect_b] {
        input.motion(
            (rect[0] + rect[2] / 2) as f64,
            (rect[1] + rect[3] / 2) as f64,
        );
        input.button(272, true);
        input.button(272, false);
    }
    // Enter names the cluster with its default name, Enter again confirms.
    tap(&input, 28);
    tap(&input, 28);

    wait_for("the new cluster holds both windows", || {
        queue_a.roundtrip(&mut client_a).unwrap();
        queue_b.roundtrip(&mut client_b).unwrap();
        (new_cluster_members(&mut fixture) == Some(2)).then_some(())
    });
}
