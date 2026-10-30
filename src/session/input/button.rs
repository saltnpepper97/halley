use super::*;

// Resolve press/release ownership before forwarding anything to a client.
pub(super) fn handle<D: SessionDriver, B: InputBackend>(
    session: &mut Session<D>,
    event: &InputEvent<B>,
    steam_close_press: Option<Window>,
    socket_name: &OsStr,
    position_after: (f64, f64),
) {
    let pointer_handle = session
        .seat
        .get_pointer()
        .expect("pointer capability added at seat setup");
    if let InputEvent::PointerButton {
        event: button_event,
    } = event
    {
        let button = button_event.button_code();
        let state = button_event.state();
        let time = button_event.time().millis();
        let serial = SERIAL_COUNTER.next_serial();
        if let crate::input::grab::Grab::MovePopup { button: owner, .. } =
            &session.interactions.grab
        {
            if *owner == button && state == ButtonState::Released {
                session.interactions.grab = crate::input::grab::Grab::None;
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
                super::super::pointer::update_client_state(session, time);
                session.request_redraw();
            }
            // The app received neither the initiating press nor any held motion.
            super::super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
        if matches!(
            &session.interactions.grab,
            crate::input::grab::Grab::PendingWindowMove(pending)
                if pending.client_owned && releases_pending_window_move(
                    pending.button,
                    button,
                    state == ButtonState::Released,
                )
        ) {
            // No compositor move began, so leave this release unconsumed. It
            // completes the client's click and lets its double-click handler
            // decide whether to toggle maximize.
            session.interactions.grab = crate::input::grab::Grab::None;
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Grab, None);
        }
        if button == BTN_LEFT && state == ButtonState::Released {
            if session.clusters.bloom_pull().is_some() {
                session.clusters.clear_bloom_pull();
                session.clusters.set_overlay_hovered(None);
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
                session.request_redraw();
                super::super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            if let Some((drag_output, drag)) = session.clusters.take_overflow_drag() {
                let now = crate::frame_clock::monotonic_now();
                let output = session
                    .wayland
                    .space
                    .outputs()
                    .find(|output| output.name() == drag_output)
                    .cloned();
                let mut changed = false;
                if let Some(output) = output.as_ref()
                    && let Some(geometry) = session.wayland.space.output_geometry(output)
                {
                    let local = Point::<f64, Logical>::from((
                        position_after.0 - f64::from(geometry.loc.x),
                        position_after.1 - f64::from(geometry.loc.y),
                    ));
                    let work_area =
                        smithay::desktop::layer_map_for_output(output).non_exclusive_zone();
                    let moved = (local.x - drag.press_local.x).hypot(local.y - drag.press_local.y)
                        >= NODE_DRAG_THRESHOLD_PX;
                    let strip_slot = moved
                        .then(|| {
                            session.clusters.overflow_strip_slot(
                                &drag_output,
                                work_area,
                                local,
                                now,
                            )
                        })
                        .flatten();
                    let target = (moved && strip_slot.is_none())
                        .then(|| {
                            session
                                .clusters
                                .visible_tile_hit_test(&drag_output, work_area, local)
                        })
                        .flatten();
                    changed = if let Some(target_slot) = strip_slot {
                        session.clusters.reorder_overflow_member(
                            &drag_output,
                            drag.member_id,
                            target_slot,
                            now,
                        )
                    } else if let Some(target) = target {
                        session.clusters.swap_overflow_member(
                            &drag_output,
                            drag.member_id,
                            target,
                            work_area,
                            now,
                        )
                    } else {
                        session.clusters.promote_overflow_member(
                            &drag_output,
                            drag.member_id,
                            work_area,
                            now,
                        )
                    };
                    session.clusters.reveal_overflow(&drag_output, now);
                }
                if changed
                    && let Some(window) = session
                        .nodes
                        .record(drag.member_id)
                        .map(|record| record.window.clone())
                {
                    super::super::focus_window(session, &window, serial);
                }
                session.clusters.set_overlay_hovered(None);
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
                session.request_redraw();
                super::super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            match &session.interactions.grab {
                crate::input::grab::Grab::PendingNode { id, .. } => {
                    let id = *id;
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    let _ = crate::nodes::restore(session, id, serial);
                    super::super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::MoveNode { id, .. } => {
                    let id = *id;
                    let now = crate::frame_clock::monotonic_now();
                    if session.nodes.physics.enabled {
                        let _ = crate::nodes::tick_physics(session, now);
                    }
                    let active_workspace_release = (|| {
                        let (output, output_geometry) =
                            output_at_pointer(&session.wayland.space, session.pointer.position())?;
                        session.clusters.active_on(&output.name())?;
                        let work_area =
                            smithay::desktop::layer_map_for_output(&output).non_exclusive_zone();
                        let global_work_area =
                            Rectangle::new(output_geometry.loc + work_area.loc, work_area.size);
                        if !global_work_area
                            .to_f64()
                            .contains(Point::<f64, Logical>::from(session.pointer.position()))
                        {
                            return None;
                        }
                        let center = session.nodes.field.node(id)?.pos;
                        let size = session.nodes.record(id)?.geometry.size;
                        let origin = collapsed_node_drop_origin(center, size, output_geometry);
                        Some((output, work_area, origin))
                    })();
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    session.nodes.clear_direct_motion(id);
                    session.clusters.cancel_join_candidate();
                    if let Some((output, work_area, origin)) = active_workspace_release {
                        let output_name = output.name();
                        crate::nodes::set_collapsed_output(session, id, &output);
                        if crate::nodes::restore_for_cluster_join(session, id, serial)
                            && session.clusters.join_active_member_front(
                                &mut session.nodes.field,
                                &output_name,
                                id,
                                work_area,
                                origin,
                                now,
                            )
                        {
                            super::super::reconcile_cluster_surfaces(session, &output_name);
                        }
                    }
                    session.request_redraw();
                    super::super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::PendingClusterCore { id, output, .. } => {
                    let id = *id;
                    let output_name = output.clone();
                    let now = crate::frame_clock::monotonic_now();
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(BTN_LEFT);
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    let output = {
                        session
                            .wayland
                            .space
                            .outputs()
                            .find(|candidate| candidate.name() == output_name)
                            .cloned()
                    };
                    let owned_focus = cluster_owns_focus(session, id);
                    if session.clusters.activate(&output_name, id, now)
                        && let Some(output) = output
                    {
                        sync_cluster_activation_focus(session, &output, id, owned_focus, serial);
                    }
                    session.request_redraw();
                    super::super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                crate::input::grab::Grab::MoveClusterCore { .. } => {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(BTN_LEFT);
                    session.interactions.grab = crate::input::grab::Grab::None;
                    session
                        .cursor
                        .set_override(crate::cursor::OverrideSource::Grab, None);
                    session.request_redraw();
                    super::super::pointer::finish_frame(session, &pointer_handle);
                    return;
                }
                _ => {}
            }
        }
        let mut route = super::super::pointer::route_for_discrete_input(session, time);
        let native_popup_grab = pointer_handle
            .with_grab(|_, grab| {
                grab.downcast_ref::<smithay::desktop::PopupPointerGrab<Session<D>>>()
                    .is_some()
            })
            .unwrap_or(false);
        if native_popup_grab && let Some(mut grab) = session.popup_grab.take() {
            let popup_roots = grab
                .pointer_grab_start_data()
                .focus
                .as_ref()
                .map(|(root, _)| {
                    smithay::desktop::PopupManager::popups_for_surface(root)
                        .map(|(popup, _)| popup.wl_surface().clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let target_root = route
                .as_ref()
                .and_then(|route| route.focus.as_ref())
                .map(|(surface, _)| wayland::compositor::root_surface(surface));
            if !grab.has_ended()
                && wayland::popup::outside_popup_press(
                    state == ButtonState::Pressed,
                    target_root.as_ref(),
                    &popup_roots,
                )
            {
                grab.ungrab(smithay::desktop::PopupUngrabStrategy::All);
                // Motion retires Smithay's ended pointer/keyboard grabs and
                // restores the actual outside target before normal dispatch.
                // Do not deliver a synthetic button or consume the real click.
                route = super::super::pointer::route_for_motion(session, time);
                session.request_redraw();
            } else if !grab.has_ended() {
                session.popup_grab = Some(grab);
                // The popup owns this event, including releases. Compositor
                // bindings and desktop interactions must not steal it.
                pointer_handle.button(
                    session,
                    &ButtonEvent {
                        serial,
                        time: smithay::backend::input::InputTime::from_millis(time),
                        button,
                        state,
                    },
                );
                super::super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
        } else if !native_popup_grab {
            session.popup_grab = None;
        }
        dismiss_lift_on_outside_press(session, route.as_ref(), button, state);
        if session.clusters.accepts_modal_input() {
            let naming_output = session
                .clusters
                .creation()
                .filter(|creation| creation.naming)
                .map(|creation| creation.output.clone());
            if let Some(output) = naming_output {
                if button == BTN_LEFT {
                    match state {
                        ButtonState::Pressed => {
                            match session
                                .render
                                .cluster_creation_overlay
                                .hit_test(&output, session.pointer.position())
                            {
                                Some(
                                    crate::render::overlays::cluster_creation::CreationOverlayHit::ConfirmButton,
                                ) => {
                                    finish_cluster_creation(session);
                                }
                                Some(
                                    crate::render::overlays::cluster_creation::CreationOverlayHit::InputCaret(caret),
                                ) => {
                                    session.clusters.begin_name_selection(caret);
                                }
                                None => {}
                            }
                            session.interactions.suppressed_buttons.suppress(button);
                        }
                        ButtonState::Released => {
                            session
                                .interactions
                                .suppressed_buttons
                                .release_is_suppressed(button);
                            session.clusters.end_name_selection();
                        }
                    }
                    session.request_redraw();
                } else if state == ButtonState::Pressed {
                    session.interactions.suppressed_buttons.suppress(button);
                } else {
                    session
                        .interactions
                        .suppressed_buttons
                        .release_is_suppressed(button);
                }
                super::super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
            if button == BTN_LEFT
                && state == ButtonState::Pressed
                && let Some(crate::input::pointer::PointerRoute {
                    output,
                    target: crate::input::pointer::PointerTarget::Window(window),
                    ..
                }) = route.as_ref()
                && let Some(surface) = window.wl_surface()
                && let Some(id) = session.nodes.id_for_surface(surface.as_ref())
                && session.clusters.toggle_creation_member(id, &output.name())
            {
                wayland::focus::select_output(&mut session.wayland, output);
                session.request_redraw();
            }
            if state == ButtonState::Pressed {
                session.interactions.suppressed_buttons.suppress(button);
            } else {
                session
                    .interactions
                    .suppressed_buttons
                    .release_is_suppressed(button);
            }
            super::super::pointer::finish_frame(session, &pointer_handle);
            return;
        }
        if state == ButtonState::Pressed
            && matches!(
                route.as_ref().map(|route| &route.target),
                Some(crate::input::pointer::PointerTarget::Decoration { .. })
            )
            && bindings_enabled(session)
        {
            let modifiers = session
                .seat
                .get_keyboard()
                .expect("keyboard capability added at seat setup")
                .modifier_state();
            let context = binding_context_for_output(
                session,
                route.as_ref().map(|route| route.output.name()).as_deref(),
            );
            let action = match_pointer_bind(
                &session.keyboard.binds,
                &modifiers,
                session.keyboard.side_modifiers,
                context,
                button,
            );
            if action.as_ref().is_some_and(|action| {
                matches!(
                    action,
                    halley_config::Action::PointerMoveWindow
                        | halley_config::Action::PointerResizeWindow
                        | halley_config::Action::PointerPanFieldAlways
                )
            }) && dispatch_pointer_grab_action(
                session,
                action.as_ref().expect("checked above"),
                route.as_ref(),
                button,
                serial,
            ) {
                super::super::pointer::finish_frame(session, &pointer_handle);
                return;
            }
        }
        if button == BTN_LEFT {
            match state {
                ButtonState::Pressed => {
                    if let Some(crate::input::pointer::PointerRoute {
                        target: crate::input::pointer::PointerTarget::Decoration { window, hit },
                        ..
                    }) = route.as_ref()
                    {
                        if matches!(
                            hit,
                            crate::titlebar::Hit::Control(crate::titlebar::Control::Close)
                        ) && crate::titlebar::control_enabled(
                            window,
                            crate::titlebar::Control::Close,
                        ) {
                            super::super::closing::capture_close_control(session, window);
                        }
                        super::super::focus::focus_window_from_pointer(session, window, serial);
                        match hit {
                            crate::titlebar::Hit::Control(control) => {
                                session.interactions.titlebar_pressed =
                                    Some(crate::titlebar::ButtonTarget {
                                        window: window.clone(),
                                        control: *control,
                                    });
                            }
                            crate::titlebar::Hit::Drag => {
                                let _ = super::super::begin_titlebar_pointer_move(
                                    session, window, serial, button,
                                );
                            }
                            crate::titlebar::Hit::Resize(handle) => {
                                let location =
                                    route.as_ref().expect("matched decoration route").location;
                                let world = halley_core::field::Vec2 {
                                    x: location.x as f32,
                                    y: location.y as f32,
                                };
                                let visual_geometry = route
                                    .as_ref()
                                    .and_then(|route| route.visual_geometry)
                                    .or_else(|| session.wayland.space.element_geometry(window))
                                    .unwrap_or_else(|| window.geometry());
                                let _ = super::super::begin_window_resize(
                                    session,
                                    window,
                                    *handle,
                                    button,
                                    world,
                                    visual_geometry,
                                    serial,
                                );
                            }
                        }
                        session.request_redraw();
                        super::super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                }
                ButtonState::Released => {
                    if let Some(pressed) = session.interactions.titlebar_pressed.take() {
                        let activates = route.as_ref().is_some_and(|route| {
                            matches!(
                                &route.target,
                                crate::input::pointer::PointerTarget::Decoration {
                                    window,
                                    hit: crate::titlebar::Hit::Control(control),
                                } if window == &pressed.window && control == &pressed.control
                            )
                        });
                        if activates {
                            super::super::activate_titlebar_control(session, &pressed, serial);
                        } else if pressed.control == crate::titlebar::Control::Close {
                            super::super::closing::discard_close_control(session, &pressed.window);
                        }
                        session.request_redraw();
                        super::super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                    let pending_titlebar = match &session.interactions.grab {
                        crate::input::grab::Grab::PendingWindowMove(pending)
                            if !pending.client_owned && pending.button == button =>
                        {
                            Some(pending.clone())
                        }
                        _ => None,
                    };
                    if let Some(pending) = pending_titlebar {
                        session.interactions.grab = crate::input::grab::Grab::None;
                        session
                            .cursor
                            .set_override(crate::cursor::OverrideSource::Grab, None);
                        let clicked_same_drag_region = route.as_ref().is_some_and(|route| {
                            matches!(
                                &route.target,
                                crate::input::pointer::PointerTarget::Decoration {
                                    window,
                                    hit: crate::titlebar::Hit::Drag,
                                } if window == &pending.window
                            )
                        });
                        if clicked_same_drag_region
                            && let Some(surface) = pending.window.wl_surface()
                        {
                            let now = crate::frame_clock::monotonic_now();
                            let double_click = session
                                .interactions
                                .titlebar_last_click
                                .as_ref()
                                .is_some_and(|last| {
                                    &last.surface == surface.as_ref()
                                        && now.saturating_sub(last.at)
                                            <= std::time::Duration::from_millis(500)
                                });
                            if double_click {
                                session.interactions.titlebar_last_click = None;
                                super::super::activate_titlebar_control(
                                    session,
                                    &crate::titlebar::ButtonTarget {
                                        window: pending.window.clone(),
                                        control: crate::titlebar::Control::Maximize,
                                    },
                                    serial,
                                );
                            } else {
                                session.interactions.titlebar_last_click =
                                    Some(crate::titlebar::LastClick {
                                        surface: surface.into_owned(),
                                        at: now,
                                    });
                            }
                        }
                        session.request_redraw();
                        super::super::pointer::finish_frame(session, &pointer_handle);
                        return;
                    }
                }
            }
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && let Some(route) = route.as_ref()
        {
            wayland::focus::select_output(&mut session.wayland, &route.output);
        }
        let mut intercepted = false;
        let mut finishing_client_move = false;
        let now = crate::frame_clock::monotonic_now();
        let action_target = (button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open())
        .then(|| cluster_action_at_pointer(session, now))
        .flatten();
        if let Some((cluster, control, output)) = action_target {
            let output_name = output.name();
            wayland::focus::select_output(&mut session.wayland, &output);
            session.clusters.close_bloom(&output_name, now);
            session.clusters.set_hovered_core(None, now);
            match control {
                crate::clusters::ClusterActionControl::Close => {
                    session.request_cluster_dissolution(cluster);
                }
                crate::clusters::ClusterActionControl::Edit => {
                    if session.clusters.begin_rename(cluster) {
                        session.cursor.set_override(
                            crate::cursor::OverrideSource::Modal,
                            Some(smithay::input::pointer::CursorIcon::Default),
                        );
                    } else {
                        session.shell.overlays.show_error(
                            output_name,
                            "Cluster name could not be edited",
                            3_000,
                            now,
                        );
                    }
                }
            }
            session.interactions.suppressed_buttons.suppress(button);
            session.request_redraw();
            intercepted = true;
        }
        let bloom_token = (button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted)
            .then(|| cluster_bloom_at_pointer(session, now))
            .flatten();
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && let Some((token, output)) = bloom_token
            && session.clusters.begin_bloom_pull(token, output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            session.interactions.suppressed_buttons.suppress(button);
            session.clusters.set_overlay_hovered(None);
            session.cursor.set_override(
                crate::cursor::OverrideSource::Grab,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted
            && let Some((member, output, local)) =
                cluster_overflow_at_pointer(session, crate::frame_clock::monotonic_now())
            && session.clusters.begin_overflow_drag(
                &output.name(),
                member,
                local,
                crate::frame_clock::monotonic_now(),
            )
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            session.interactions.suppressed_buttons.suppress(button);
            session.clusters.set_overlay_hovered(None);
            session.cursor.set_override(
                crate::cursor::OverrideSource::Grab,
                Some(smithay::input::pointer::CursorIcon::Grabbing),
            );
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && !intercepted
            && let Some((id, output)) = cluster_at_pointer(session)
            && let Some(metadata) = session.clusters.metadata(id)
            && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
            && let Some(camera) = session.cameras.get(&output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            if let Some(core) = session.clusters.core_node(id) {
                session
                    .nodes
                    .focus(Some(core), session.start_time.elapsed().as_millis() as u64);
            }
            let center =
                crate::nodes::screen_from_world(metadata.core_position, camera, output_geometry);
            let screen_offset = halley_core::field::Vec2 {
                x: center.x as f32 - position_after.0 as f32,
                y: center.y as f32 - position_after.1 as f32,
            };
            session
                .clusters
                .close_bloom(&output.name(), crate::frame_clock::monotonic_now());
            session
                .clusters
                .set_hovered_core(None, crate::frame_clock::monotonic_now());
            let modifiers = session
                .seat
                .get_keyboard()
                .expect("keyboard capability added at seat setup")
                .modifier_state();
            if crate::input::mod_key_held(
                &modifiers,
                session.keyboard.side_modifiers,
                session.keyboard.effective_mod,
            ) {
                if session
                    .clusters
                    .core_node(id)
                    .is_none_or(|core| !crate::session::node_user_pinned(session, core))
                {
                    begin_cluster_core_direct_motion(
                        &mut session.nodes,
                        session.clusters.core_node(id),
                    );
                    session.interactions.grab =
                        crate::input::grab::Grab::MoveClusterCore { id, screen_offset };
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Grab,
                        Some(smithay::input::pointer::CursorIcon::Grabbing),
                    );
                }
            } else {
                session.interactions.grab = crate::input::grab::Grab::PendingClusterCore {
                    id,
                    output: output.name(),
                    press_screen: Point::<f64, Logical>::from(position_after),
                    screen_offset,
                };
                session
                    .cursor
                    .set_override(crate::cursor::OverrideSource::Grab, None);
            }
            session.interactions.suppressed_buttons.suppress(button);
            session.request_redraw();
            intercepted = true;
        }
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && !session.shell.focus_cycle.is_open()
            && let Some((target, output)) = bearing_at_pointer(
                &session.pointer,
                &session.wayland.space,
                &session.shell.bearings,
            )
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            let revealed = match target {
                crate::shell::bearings::BearingTarget::Node(id) => {
                    crate::nodes::focus_or_reveal_node(session, id, serial, true)
                }
                crate::shell::bearings::BearingTarget::ClusterCore { core, .. } => {
                    crate::nodes::reveal_cluster_core(session, core, serial, true)
                }
            };
            if revealed {
                session.interactions.suppressed_buttons.suppress(button);
                intercepted = true;
            }
        }
        let modifiers = session
            .seat
            .get_keyboard()
            .expect("keyboard capability added at seat setup")
            .modifier_state();
        let bindings_enabled = bindings_enabled(session);
        let on_background = route.as_ref().is_some_and(|route| {
            matches!(
                &route.target,
                crate::input::pointer::PointerTarget::Background
            )
        });
        let binding_context = binding_context_for_output(
            session,
            route.as_ref().map(|route| route.output.name()).as_deref(),
        );
        if !intercepted {
            match process_pointer_binding(
                &session.keyboard.binds,
                &modifiers,
                session.keyboard.side_modifiers,
                binding_context,
                button,
                state,
                bindings_enabled,
                &mut session.interactions.suppressed_buttons,
            ) {
                PointerBindingResult::Action(action) => {
                    let pointer_grab = matches!(
                        action,
                        halley_config::Action::PointerMoveWindow
                            | halley_config::Action::PointerResizeWindow
                            | halley_config::Action::PointerPanField
                            | halley_config::Action::PointerPanFieldAlways
                            | halley_config::Action::PointerDragPan
                    );
                    let handled = if pointer_grab {
                        dispatch_pointer_grab_action(
                            session,
                            &action,
                            route.as_ref(),
                            button,
                            serial,
                        )
                    } else {
                        let output_name =
                            route.as_ref().map(|route| route.output.name().to_string());
                        actions::dispatch(
                            session,
                            action,
                            socket_name,
                            output_name.as_deref(),
                            None,
                            actions::DispatchOrigin::Other,
                        );
                        true
                    };
                    if pointer_grab || !handled {
                        session
                            .interactions
                            .suppressed_buttons
                            .release_is_suppressed(button);
                    }
                    intercepted = handled;
                }
                PointerBindingResult::SuppressedRelease => intercepted = true,
                PointerBindingResult::Unhandled => {}
            }
        }

        if !intercepted
            && button == BTN_LEFT
            && state == ButtonState::Pressed
            && let Some((id, output)) = node_at_pointer(session)
            && let Some(record) = session.nodes.record(id).cloned()
            && let Some(node_position) = session.nodes.field.node(id).map(|node| node.pos)
            && let Some(output_geometry) = session.wayland.space.output_geometry(&output)
            && let Some(camera) = session.cameras.get(&output.name())
        {
            wayland::focus::select_output(&mut session.wayland, &output);
            let center = crate::nodes::screen_from_world(node_position, camera, output_geometry);
            let screen_offset = halley_core::field::Vec2 {
                x: center.x as f32 - position_after.0 as f32,
                y: center.y as f32 - position_after.1 as f32,
            };
            session
                .nodes
                .clear_hover(crate::frame_clock::monotonic_now());
            super::super::focus::focus_node_from_pointer(session, id, &output, serial);
            let mod_held = crate::input::mod_key_held(
                &modifiers,
                session.keyboard.side_modifiers,
                session.keyboard.effective_mod,
            );
            if mod_held {
                if !crate::session::node_user_pinned(session, id) {
                    session.nodes.clear_direct_motion(id);
                    session.interactions.grab = crate::input::grab::Grab::MoveNode {
                        id,
                        surface: record.surface,
                        screen_offset,
                        last_world: node_position,
                        last_update: crate::frame_clock::monotonic_now(),
                        velocity: halley_core::field::Vec2 { x: 0.0, y: 0.0 },
                    };
                    session.cursor.set_override(
                        crate::cursor::OverrideSource::Grab,
                        Some(smithay::input::pointer::CursorIcon::Grabbing),
                    );
                }
            } else {
                session.interactions.grab = crate::input::grab::Grab::PendingNode {
                    id,
                    surface: record.surface,
                    press_screen: Point::<f64, Logical>::from(position_after),
                    screen_offset,
                };
            }
            intercepted = true;
        }

        // Old Halley kept a bloom open while another window or compositor
        // marker was clicked or grabbed. Only an otherwise-unhandled press on
        // the empty Field dismissed it; core presses close their own bloom in
        // the dedicated core path above.
        if button == BTN_LEFT
            && state == ButtonState::Pressed
            && plain_background_press_dismisses_bloom(intercepted, on_background)
            && let Some(route) = route.as_ref()
        {
            let now = crate::frame_clock::monotonic_now();
            if session.clusters.close_bloom(&route.output.name(), now) {
                session.clusters.set_overlay_hovered(None);
                session.clusters.set_hovered_core(None, now);
                session.request_redraw();
            }
        }

        if !intercepted && button == BTN_LEFT {
            match state {
                ButtonState::Pressed => match route.as_ref().map(|route| &route.target) {
                    Some(crate::input::pointer::PointerTarget::Window(window)) => {
                        super::super::focus::focus_window_from_pointer(session, window, serial);
                    }
                    Some(crate::input::pointer::PointerTarget::Layer(layer)) => {
                        super::super::focus::focus_layer(session, Some(layer.clone()), serial);
                    }
                    Some(crate::input::pointer::PointerTarget::Decoration { .. }) => {
                        intercepted = true;
                    }
                    Some(crate::input::pointer::PointerTarget::Background) => {
                        super::super::focus::focus_layer(session, None, serial);
                    }
                    None => {}
                },
                ButtonState::Released => {
                    let released_window = match &session.interactions.grab {
                        crate::input::grab::Grab::MoveWindow {
                            id,
                            window,
                            cluster_drag,
                            button: move_button,
                            client_owned,
                            last_world,
                            ..
                        } if *move_button == button => Some((
                            *id,
                            window.clone(),
                            cluster_drag.clone(),
                            *last_world,
                            *client_owned,
                        )),
                        _ => None,
                    };
                    if let Some((id, window, cluster_drag, last_world, client_owned)) =
                        released_window
                    {
                        finishing_client_move = client_owned;
                        let now = crate::frame_clock::monotonic_now();
                        if session.nodes.physics.enabled {
                            let _ = crate::nodes::tick_physics(session, now);
                        }
                        let cluster_release = cluster_drag.as_ref().and_then(|drag| {
                            let output = session
                                .wayland
                                .space
                                .outputs()
                                .find(|output| output.name() == drag.output)?
                                .clone();
                            let output_geometry = session.wayland.space.output_geometry(&output)?;
                            let work_area = smithay::desktop::layer_map_for_output(&output)
                                .non_exclusive_zone();
                            let origin =
                                super::super::presented_window_rect(session, &window, &output, now)
                                    .map(|geometry| geometry.to_logical(1))
                                    .or_else(|| {
                                        let geometry =
                                            session.wayland.space.element_geometry(&window)?;
                                        Some(Rectangle::new(
                                            geometry.loc - output_geometry.loc,
                                            geometry.size,
                                        ))
                                    })?;
                            let global_work_area =
                                Rectangle::new(output_geometry.loc + work_area.loc, work_area.size);
                            let pointer_inside = global_work_area.to_f64().contains(Point::<
                                f64,
                                Logical,
                            >::from(
                                session.pointer.position(),
                            ));
                            Some((
                                drag.clone(),
                                output.name(),
                                work_area,
                                origin,
                                pointer_inside,
                            ))
                        });
                        let active_workspace_release =
                            (cluster_drag.is_none())
                                .then(|| {
                                    let (output, output_geometry) = output_at_pointer(
                                        &session.wayland.space,
                                        session.pointer.position(),
                                    )?;
                                    session.clusters.active_on(&output.name())?;
                                    let work_area = smithay::desktop::layer_map_for_output(&output)
                                        .non_exclusive_zone();
                                    let global_work_area = Rectangle::new(
                                        output_geometry.loc + work_area.loc,
                                        work_area.size,
                                    );
                                    if !global_work_area.to_f64().contains(
                                        Point::<f64, Logical>::from(session.pointer.position()),
                                    ) {
                                        return None;
                                    }
                                    let origin = super::super::presented_window_rect(
                                        session, &window, &output, now,
                                    )
                                    .map(|geometry| geometry.to_logical(1))
                                    .or_else(|| {
                                        let geometry =
                                            session.wayland.space.element_geometry(&window)?;
                                        Some(Rectangle::new(
                                            geometry.loc - output_geometry.loc,
                                            geometry.size,
                                        ))
                                    })?;
                                    Some((output.name(), work_area, origin))
                                })
                                .flatten();
                        let joined = if cluster_drag.is_none() && active_workspace_release.is_none()
                        {
                            id.and_then(|member| {
                                session
                                    .clusters
                                    .commit_join_candidate(&mut session.nodes.field, member)
                            })
                        } else {
                            None
                        };
                        session.interactions.grab = crate::input::grab::Grab::None;
                        session
                            .cursor
                            .set_override(crate::cursor::OverrideSource::Grab, None);
                        let mut cluster_drop_handled = false;
                        if let (
                            Some(id),
                            Some((drag, member_output, work_area, origin, pointer_inside)),
                        ) = (id, cluster_release)
                        {
                            match drag.kind {
                                crate::input::grab::ClusterWindowDragKind::Floating
                                    if pointer_inside =>
                                {
                                    cluster_drop_handled =
                                        session.clusters.finish_floating_member_drag(
                                            &drag.output,
                                            &member_output,
                                            id,
                                            work_area,
                                            origin,
                                        );
                                }
                                crate::input::grab::ClusterWindowDragKind::Layout(_)
                                    if pointer_inside =>
                                {
                                    cluster_drop_handled = session.clusters.finish_workspace_drag(
                                        &drag.output,
                                        id,
                                        work_area,
                                        origin,
                                        now,
                                    );
                                }
                                crate::input::grab::ClusterWindowDragKind::Floating
                                | crate::input::grab::ClusterWindowDragKind::Layout(_) => {
                                    cluster_drop_handled =
                                        session.clusters.detach_active_member_for_drag(
                                            &mut session.nodes.field,
                                            &drag.output,
                                            crate::clusters::ClusterDragMember {
                                                cluster_id: drag.cluster_id,
                                                node_id: id,
                                            },
                                            work_area,
                                            last_world,
                                            now,
                                        );
                                }
                            }
                            if cluster_drop_handled {
                                session.nodes.clear_direct_motion(id);
                                super::super::reconcile_cluster_surfaces(session, &drag.output);
                                if member_output != drag.output {
                                    super::super::reconcile_cluster_surfaces(
                                        session,
                                        &member_output,
                                    );
                                }
                                session.request_redraw();
                            }
                        }
                        if let (Some(id), Some((output, work_area, origin))) =
                            (id, active_workspace_release)
                            && session.clusters.join_active_member_front(
                                &mut session.nodes.field,
                                &output,
                                id,
                                work_area,
                                origin,
                                now,
                            )
                        {
                            cluster_drop_handled = true;
                            session.nodes.clear_direct_motion(id);
                            super::super::reconcile_cluster_surfaces(session, &output);
                            session.request_redraw();
                        }
                        let drag_presentation_released = session.clusters.cancel_window_drag();
                        if cluster_drag.is_some() && !cluster_drop_handled {
                            if let (Some(id), Some(drag)) = (id, cluster_drag.as_ref()) {
                                let output = session
                                    .wayland
                                    .space
                                    .outputs()
                                    .find(|output| output.name() == drag.output)
                                    .cloned();
                                if let Some(output) = output {
                                    crate::nodes::set_collapsed_output(session, id, &output);
                                    super::super::reconcile_cluster_surfaces(session, &drag.output);
                                }
                            }
                            session.request_redraw();
                        } else if joined.is_some() {
                            if let Some(id) = id {
                                session.nodes.clear_direct_motion(id);
                            }
                            session.request_redraw();
                        } else if !cluster_drop_handled
                            && session.nodes.physics.enabled
                            && let Some(id) = id
                        {
                            session.nodes.lock_released_window(id, now);
                            session.request_redraw();
                        } else if drag_presentation_released {
                            session.request_redraw();
                        }
                        intercepted = true;
                    }
                }
            }
        }

        // A camera pan ends with the release of the button that began it, which
        // may be any pointer button a `pan-field`/`pan-field-always` chord uses.
        if !intercepted
            && releases_field_pan(
                &session.interactions.grab,
                button,
                state == ButtonState::Released,
            )
        {
            session.interactions.grab = crate::input::grab::Grab::None;
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Grab, None);
            intercepted = true;
        }

        if !intercepted
            && state == ButtonState::Released
            && matches!(
                &session.interactions.grab,
                crate::input::grab::Grab::ResizeWindow(resize) if resize.button == button
            )
        {
            session.interactions.grab = crate::input::grab::Grab::None;
            crate::input::grab::release_resize_anchor(&mut session.interactions.resize_anchor);
            session
                .cursor
                .set_override(crate::cursor::OverrideSource::Grab, None);
            intercepted = true;
        }

        if forward_pointer_button(intercepted, finishing_client_move) {
            if let Some(window) = forwarded_steam_close_button(
                &mut session.interactions.steam_close_pressed,
                steam_close_press,
                button,
                state,
                steam_client_close_target(route.as_ref()),
            ) {
                super::super::closing::start_steam_client_close_control(session, &window);
            }
            let had_click_grab = super::super::pointer::client_click_grab_active(session);
            pointer_handle.button(
                session,
                &ButtonEvent {
                    serial,
                    time: smithay::backend::input::InputTime::from_millis(time),
                    button,
                    state,
                },
            );
            if had_click_grab && !super::super::pointer::client_click_grab_active(session) {
                // The last release goes to the owner, then normal hit-testing
                // resumes even if the physical pointer stops moving.
                super::super::pointer::route_for_motion(session, time);
            }
        }
        super::super::pointer::finish_frame(session, &pointer_handle);
    }
}
