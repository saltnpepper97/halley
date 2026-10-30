use super::*;

// Wheel bindings and cluster scrolling get first refusal before client delivery.
pub(super) fn handle<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
    socket_name: &OsStr,
) {
    let pointer_handle = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    if let InputEvent::PointerAxis { event: axis_event } = event {
        if super::super::gesture::handle_axis_pan(session, axis_event) {
            super::super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
        let route =
            super::super::pointer::route_for_discrete_input(session, axis_event.time().millis());
        let output_name = route.as_ref().map(|route| route.output.name().to_string());
        let bindings_enabled = bindings_enabled(session);
        let modifiers = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup")
            .modifier_state();
        let sides = session.keyboard.side_modifiers;
        let binding_context = binding_context_for_output(session, output_name.as_deref());
        let result = process_wheel_bindings(
            axis_event,
            &mut session.interactions.wheel_accumulator,
            bindings_enabled,
            |direction| {
                match_wheel_bind(
                    &session.keyboard.binds,
                    &modifiers,
                    sides,
                    binding_context,
                    direction,
                )
            },
        );
        let bound_action = !result.actions.is_empty();
        for (direction, action) in result.actions {
            eventline::debug!("keybinds: wheel {direction:?} + {modifiers:?} -> {action:?}");
            actions::dispatch(
                session,
                action,
                socket_name,
                output_name.as_deref(),
                None,
                actions::DispatchOrigin::Other,
            );
        }

        if !bound_action
            && scroll_cluster_overflow_at_pointer(
                session,
                axis_event,
                crate::frame_clock::monotonic_now(),
            )
        {
            session.request_redraw();
            super::super::pointer::finish_frame(session, &pointer_handle);
            return;
        }

        if result.forward_horizontal || result.forward_vertical {
            let frame = axis_frame_filtered(
                axis_event,
                result.forward_horizontal,
                result.forward_vertical,
            );
            pointer_handle.axis(session, frame);
        }
        super::super::pointer::finish_frame(session, &pointer_handle);
    }
}
