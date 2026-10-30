use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufBlocker, DmabufSource};
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::element::utils::select_dmabuf_feedback;
use smithay::desktop::layer_map_for_output;
use smithay::desktop::utils::send_dmabuf_feedback_surface_tree;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use smithay::reexports::wayland_server::{DisplayHandle, GlobalDispatch};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufGlobalData, DmabufHandler, DmabufState,
};

use crate::backend::dmabuf::{DmabufCapabilities, SurfaceDmabufFeedback};
use crate::wayland::{WaylandState, window_is_on_output};

/// Keep an unfinished implicit-sync buffer out of committed renderer state.
/// Already-ready buffers need no blocker. A registration failure is a separate
/// error: the caller must reject the commit instead of rendering an unfinished
/// buffer or installing an orphan blocker.
pub fn defer_until_readable(
    dmabuf: &Dmabuf,
    register: impl FnOnce(DmabufSource) -> bool,
) -> std::io::Result<Option<DmabufBlocker>> {
    let Ok((blocker, source)) = dmabuf.generate_blocker(calloop::Interest::READ) else {
        return Ok(None);
    };
    if !register(source) {
        return Err(std::io::Error::other(
            "failed to register implicit buffer readiness source",
        ));
    }
    Ok(Some(blocker))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Advertisement {
    None,
    Version3,
    Version5(libc::dev_t),
}

fn advertisement(capabilities: &DmabufCapabilities) -> Advertisement {
    if capabilities.is_empty() {
        Advertisement::None
    } else if let Some(main_device) = capabilities.main_device() {
        Advertisement::Version5(main_device)
    } else {
        Advertisement::Version3
    }
}

/// Advertises the strongest truthful linux-dmabuf global for one renderer.
///
/// Feedback-capable version 5 needs a device-backed table. Renderers whose EGL
/// device cannot be resolved still support the version 3 format list.
pub fn create_global<D>(
    state: &mut DmabufState,
    display_handle: &DisplayHandle,
    capabilities: &DmabufCapabilities,
) -> Option<DmabufGlobal>
where
    D: DmabufHandler + GlobalDispatch<ZwpLinuxDmabufV1, DmabufGlobalData> + 'static,
{
    let formats: Vec<_> = capabilities.formats().iter().copied().collect();
    let main_device = match advertisement(capabilities) {
        Advertisement::None => {
            eventline::warn!(
                "DMA-BUF: renderer reported no importable formats; global not advertised"
            );
            return None;
        }
        Advertisement::Version3 => {
            eventline::debug!(
                "DMA-BUF: render device unavailable; advertising version 3 with {} formats",
                formats.len()
            );
            return Some(state.create_global::<D>(display_handle, formats));
        }
        Advertisement::Version5(main_device) => main_device,
    };

    match DmabufFeedbackBuilder::new(main_device, formats.iter().copied()).build() {
        Ok(feedback) => {
            eventline::debug!(
                "DMA-BUF: advertising version 5 with {} renderer formats",
                formats.len()
            );
            Some(state.create_global_with_default_feedback::<D>(display_handle, &feedback))
        }
        Err(err) => {
            eventline::warn!(
                "DMA-BUF: failed to build version 5 feedback, falling back to version 3: {err}"
            );
            Some(state.create_global::<D>(display_handle, formats))
        }
    }
}

pub fn send_output_feedback(
    wayland: &WaylandState,
    output: &Output,
    primary_output: &Output,
    session_lock: &crate::wayland::session_lock::State,
    feedback: &SurfaceDmabufFeedback,
    element_states: &RenderElementStates,
) {
    if session_lock.active() {
        for surface in session_lock.surfaces_for_output(output) {
            send_dmabuf_feedback_surface_tree(
                surface.wl_surface(),
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        }
        return;
    }

    wayland
        .space
        .elements()
        .filter(|window| window_is_on_output(window, output, primary_output))
        .for_each(|window| {
            window.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        element_states,
                        &feedback.render,
                        &feedback.scanout,
                    )
                },
            );
        });

    let map = layer_map_for_output(output);
    for layer in map.layers() {
        layer.send_dmabuf_feedback(
            output,
            |_, _| Some(output.clone()),
            |surface, _| {
                select_dmabuf_feedback(surface, element_states, &feedback.render, &feedback.scanout)
            },
        );
    }

    if let Some(icon) = wayland.dnd_icon.as_ref() {
        send_dmabuf_feedback_surface_tree(
            &icon.surface,
            output,
            |_, _| Some(output.clone()),
            |surface, _| {
                select_dmabuf_feedback(surface, element_states, &feedback.render, &feedback.scanout)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::{Format, Fourcc, Modifier};

    use super::*;

    // Pollable socket descriptors stand in for DMA-BUF plane fences here.
    // No fake buffer is imported into a renderer; this exercises readiness,
    // all-plane completion and event-loop liveness without a GPU dependency.
    fn pending_planes() -> (Dmabuf, [std::os::unix::net::UnixStream; 2]) {
        use smithay::backend::allocator::dmabuf::DmabufFlags;
        use std::os::unix::net::UnixStream;
        let (read_a, write_a) = UnixStream::pair().unwrap();
        let (read_b, write_b) = UnixStream::pair().unwrap();
        let mut builder = Dmabuf::builder(
            (16, 16),
            Fourcc::Argb8888,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        assert!(builder.add_plane(std::os::fd::OwnedFd::from(read_a), 0, 64));
        assert!(builder.add_plane(std::os::fd::OwnedFd::from(read_b), 0, 64));
        (builder.build().unwrap(), [write_a, write_b])
    }

    #[test]
    fn pending_buffer_does_not_block_loop_and_releases_only_after_all_planes() {
        use smithay::wayland::compositor::{Blocker, BlockerState};
        use std::io::Write;
        use std::time::Duration;
        let (dmabuf, mut writers) = pending_planes();
        let mut event_loop = calloop::EventLoop::<(usize, usize)>::try_new().unwrap();
        let handle = event_loop.handle();
        let blocker = defer_until_readable(&dmabuf, |source| {
            handle
                .insert_source(source, |_, _, state| {
                    state.0 += 1;
                    Ok(())
                })
                .is_ok()
        })
        .unwrap()
        .unwrap();
        handle
            .insert_source(calloop::timer::Timer::immediate(), |_, _, state| {
                state.1 += 1;
                calloop::timer::TimeoutAction::Drop
            })
            .unwrap();
        let mut state = (0, 0);
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(
            state,
            (0, 1),
            "unrelated work must run while the buffer is pending"
        );
        assert_eq!(blocker.state(), BlockerState::Pending);
        writers[0].write_all(&[1]).unwrap();
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.0, 0);
        assert_eq!(blocker.state(), BlockerState::Pending);
        writers[1].write_all(&[1]).unwrap();
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.0, 1);
        assert_eq!(blocker.state(), BlockerState::Released);
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.0, 1, "readiness notification is one-shot");
    }

    #[test]
    fn ready_buffers_do_not_register_a_source_or_install_a_blocker() {
        use std::io::Write;
        let (dmabuf, mut writers) = pending_planes();
        for writer in &mut writers {
            writer.write_all(&[1]).unwrap();
        }
        assert!(
            defer_until_readable(&dmabuf, |_| panic!("already ready"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_source_registration_is_an_error_instead_of_a_ready_buffer_or_orphan_blocker() {
        let (dmabuf, _writers) = pending_planes();
        assert!(defer_until_readable(&dmabuf, |_| false).is_err());
    }

    fn capabilities(main_device: Option<libc::dev_t>) -> DmabufCapabilities {
        let formats = [Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Linear,
        }]
        .into_iter()
        .collect();
        DmabufCapabilities::new(main_device, formats)
    }

    #[test]
    fn omits_global_without_renderer_formats() {
        assert_eq!(
            advertisement(&DmabufCapabilities::new(None, Default::default())),
            Advertisement::None
        );
    }

    #[test]
    fn falls_back_to_version_three_without_a_device() {
        assert_eq!(advertisement(&capabilities(None)), Advertisement::Version3);
    }

    #[test]
    fn selects_version_five_when_feedback_has_a_device() {
        assert_eq!(
            advertisement(&capabilities(Some(42))),
            Advertisement::Version5(42)
        );
    }
}
