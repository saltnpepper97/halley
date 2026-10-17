//! Drive the production output-management path in the disposable nested session.

use super::*;
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1 as configured_head,
    zwlr_output_configuration_v1 as configuration, zwlr_output_head_v1 as head,
    zwlr_output_manager_v1 as manager, zwlr_output_mode_v1 as mode,
};

#[derive(Default)]
pub(super) struct Observations {
    head: Option<head::ZwlrOutputHeadV1>,
    serial: u32,
    scale: f64,
    results: Vec<&'static str>,
}

impl Dispatch<manager::ZwlrOutputManagerV1, ()> for NativeState {
    wayland_client::event_created_child!(NativeState, manager::ZwlrOutputManagerV1, [
        manager::EVT_HEAD_OPCODE => (head::ZwlrOutputHeadV1, ())
    ]);

    fn event(
        state: &mut Self,
        _: &manager::ZwlrOutputManagerV1,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager::Event::Head { head } => state.output_management.head = Some(head),
            manager::Event::Done { serial } => state.output_management.serial = serial,
            _ => {}
        }
    }
}

impl Dispatch<head::ZwlrOutputHeadV1, ()> for NativeState {
    wayland_client::event_created_child!(NativeState, head::ZwlrOutputHeadV1, [
        head::EVT_MODE_OPCODE => (mode::ZwlrOutputModeV1, ())
    ]);

    fn event(
        state: &mut Self,
        _: &head::ZwlrOutputHeadV1,
        event: head::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let head::Event::Scale { scale } = event {
            state.output_management.scale = scale;
        }
    }
}

impl Dispatch<configuration::ZwlrOutputConfigurationV1, ()> for NativeState {
    fn event(
        state: &mut Self,
        _: &configuration::ZwlrOutputConfigurationV1,
        event: configuration::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.output_management.results.push(match event {
            configuration::Event::Succeeded => "succeeded",
            configuration::Event::Failed => "failed",
            configuration::Event::Cancelled => "cancelled",
            _ => unreachable!(),
        });
    }
}

delegate_noop!(NativeState: ignore mode::ZwlrOutputModeV1);
delegate_noop!(NativeState: ignore configured_head::ZwlrOutputConfigurationHeadV1);

struct Client {
    queue: EventQueue<NativeState>,
    state: NativeState,
    manager: manager::ZwlrOutputManagerV1,
}

impl Client {
    fn new(fixture: &Fixture) -> Self {
        let (mut queue, mut state) = native_connection(fixture);
        let manager = state.registry.as_ref().unwrap().bind(
            state.globals["zwlr_output_manager_v1"],
            4,
            &queue.handle(),
            (),
        );
        queue.roundtrip(&mut state).unwrap();
        assert!(state.output_management.head.is_some());
        Self {
            queue,
            state,
            manager,
        }
    }

    fn configure(
        &self,
    ) -> (
        configuration::ZwlrOutputConfigurationV1,
        configured_head::ZwlrOutputConfigurationHeadV1,
    ) {
        let configuration = self.manager.create_configuration(
            self.state.output_management.serial,
            &self.queue.handle(),
            (),
        );
        let head = configuration.enable_head(
            self.state.output_management.head.as_ref().unwrap(),
            &self.queue.handle(),
            (),
        );
        (configuration, head)
    }

    fn result(&mut self, configuration: configuration::ZwlrOutputConfigurationV1, expected: &str) {
        self.state.output_management.results.clear();
        self.queue.roundtrip(&mut self.state).unwrap();
        assert_eq!(self.state.output_management.results, [expected]);
        configuration.destroy();
        self.queue.roundtrip(&mut self.state).unwrap();
    }
}

fn outputs(fixture: &mut Fixture) -> halley_ipc::OutputsResponse {
    let Response::Outputs(outputs) = fixture.request(Request::Outputs) else {
        panic!("missing outputs");
    };
    outputs
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn fractional_scale_test_apply_updates_clients_and_native_capture() {
    let mut fixture = Fixture::new("stacking");
    let original_config = fs::read(fixture.path.join("halley.rune")).unwrap();
    let mut client = Client::new(&fixture);
    let (mut queue, mut state) = native_window(&fixture);
    fixture.node("surviving native window");

    for scale in [1.25, 1.5, 2.0, 1.333, 1.0] {
        let before = outputs(&mut fixture);
        let (test, head) = client.configure();
        head.set_scale(scale);
        test.test();
        client.result(test, "succeeded");
        assert_eq!(outputs(&mut fixture), before, "test must not apply scale");

        let (apply, head) = client.configure();
        head.set_scale(scale);
        apply.apply();
        client.result(apply, "succeeded");
        let effective = halley_config::output::normalize_scale(scale).unwrap();
        let after = outputs(&mut fixture);
        assert_eq!(after.outputs[0].scale, effective);
        assert!((client.state.output_management.scale - effective).abs() <= 1.0 / 256.0);
        let mut expected = before;
        expected.outputs[0].scale = effective;
        assert_eq!(after, expected, "mode and position must stay unchanged");
        wait_for("preferred scale after protocol apply", || {
            queue.roundtrip(&mut state).unwrap();
            (state.fractional_scales.last() == Some(&((effective * 120.0).round() as u32)))
                .then_some(())
        });
        assert_eq!(
            state.integer_scales.last(),
            Some(&(effective.ceil() as i32))
        );

        let mode = after.outputs[0].modes[after.outputs[0].current_mode.unwrap()];
        let expected_width = (240.0 * effective).round() as usize;
        wait_for("scaled pixels in a native-resolution capture", || {
            let (pixels, width) = capture_frame_pixels(&mut fixture);
            assert_eq!(width, mode.width as usize);
            assert_eq!(pixels.len(), mode.width as usize * mode.height as usize * 4);
            let red_width = pixels
                .chunks_exact(width * 4)
                .map(|row| {
                    row.chunks_exact(4)
                        .filter(|pixel| pixel[2] > 245 && pixel[1] < 5 && pixel[0] < 5)
                        .count()
                })
                .max()
                .unwrap();
            (red_width.abs_diff(expected_width) <= 2).then_some(())
        });
    }
    assert_eq!(
        fs::read(fixture.path.join("halley.rune")).unwrap(),
        original_config
    );
    assert!(fixture.process.try_wait().unwrap().is_none());
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn rejected_scale_and_host_changes_leave_output_unchanged() {
    let mut fixture = Fixture::new("stacking");
    let mut client = Client::new(&fixture);
    let before = outputs(&mut fixture);
    for scale in [0.125, 10.5] {
        for apply in [false, true] {
            let (configuration, head) = client.configure();
            head.set_scale(scale);
            if apply {
                configuration.apply();
            } else {
                configuration.test();
            }
            client.result(configuration, "failed");
            assert_eq!(outputs(&mut fixture), before);
        }
    }
    // An otherwise valid scale must not relax host-owned geometry restrictions.
    let (configuration, head) = client.configure();
    head.set_scale(1.5);
    head.set_position(123, 456);
    configuration.apply();
    client.result(configuration, "failed");
    assert_eq!(outputs(&mut fixture), before);

    let stale = client.manager.create_configuration(
        client.state.output_management.serial.wrapping_sub(1),
        &client.queue.handle(),
        (),
    );
    let head = stale.enable_head(
        client.state.output_management.head.as_ref().unwrap(),
        &client.queue.handle(),
        (),
    );
    head.set_scale(1.5);
    stale.apply();
    client.result(stale, "cancelled");
    assert_eq!(outputs(&mut fixture), before);
}

#[test]
#[ignore = "requires a Wayland desktop and XWayland; starts an isolated nested compositor"]
fn nonpositive_and_duplicate_scales_are_protocol_errors() {
    let mut fixture = Fixture::new("stacking");
    let before = outputs(&mut fixture);
    for scale in [0.0, -1.0, 1.5] {
        let mut client = Client::new(&fixture);
        let (configuration, head) = client.configure();
        head.set_scale(scale);
        if scale > 0.0 {
            head.set_scale(2.0);
        }
        configuration.apply();
        assert!(client.queue.roundtrip(&mut client.state).is_err());
        assert_eq!(outputs(&mut fixture), before);
        assert!(fixture.process.try_wait().unwrap().is_none());
    }
}
