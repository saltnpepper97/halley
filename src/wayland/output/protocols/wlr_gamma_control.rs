use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::mpsc::{SyncSender, sync_channel};

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};
use wayland_protocols_wlr::gamma_control::v1::server::{
    zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1},
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

use crate::session::{Session, SessionDriver};

const VERSION: u32 = 1;

pub struct GlobalData {
    enabled: bool,
}

pub struct State {
    _global: GlobalId,
    controls: HashMap<Output, ZwlrGammaControlV1>,
    reader: Option<SyncSender<ReadJob>>,
}

#[derive(Debug)]
pub struct ControlData {
    gamma_size: u32,
}

impl State {
    pub fn new<D>(display: &DisplayHandle, enabled: bool) -> Self
    where
        D: GlobalDispatch<ZwlrGammaControlManagerV1, GlobalData> + 'static,
    {
        Self {
            _global: display
                .create_global::<D, ZwlrGammaControlManagerV1, _>(VERSION, GlobalData { enabled }),
            controls: HashMap::new(),
            reader: None,
        }
    }

    pub fn output_disabled(&mut self, output: &Output) -> bool {
        self.controls
            .remove(output)
            .inspect(|control| control.failed())
            .is_some()
    }
}

struct ReadJob {
    file: File,
    gamma_size: u32,
    output: Output,
    control: ZwlrGammaControlV1,
}

/// A single bounded worker isolates even slow filesystem descriptors from the
/// compositor. Never spawn a thread per client request or block when queuing.
pub fn init_reader<D: SessionDriver>(
    session: &mut Session<D>,
    handle: &calloop::LoopHandle<'_, Session<D>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (jobs, requests) = sync_channel::<ReadJob>(8);
    let (done, completions) = calloop::channel::channel();
    std::thread::Builder::new()
        .name("halley-gamma-reader".into())
        .spawn(move || {
            while let Ok(job) = requests.recv() {
                let result = read_ramp(job.file, job.gamma_size);
                if done.send((job.output, job.control, result)).is_err() {
                    break;
                }
            }
        })?;
    handle.insert_source(completions, |event, _, session| {
        if let calloop::channel::Event::Msg((output, control, result)) = event {
            finish_read(session, output, control, result);
        }
    })?;
    session.wayland.wlr_gamma_control_state.reader = Some(jobs);
    Ok(())
}

fn finish_read<D: SessionDriver>(
    session: &mut Session<D>,
    output: Output,
    control: ZwlrGammaControlV1,
    result: Result<Vec<u16>, String>,
) {
    // A destroyed/replaced control or disabled output invalidates pending work.
    if session
        .wayland
        .wlr_gamma_control_state
        .controls
        .get(&output)
        != Some(&control)
    {
        return;
    }
    let result = result.and_then(|ramp| session.driver.set_gamma(&output, Some(ramp)));
    if let Err(err) = result {
        eventline::warn!("gamma control for {:?} failed: {err}", output.name());
        control.failed();
        session
            .wayland
            .wlr_gamma_control_state
            .controls
            .remove(&output);
        let _ = session.driver.set_gamma(&output, None);
    }
}

impl<D: SessionDriver> GlobalDispatch<ZwlrGammaControlManagerV1, GlobalData, Session<D>>
    for Session<D>
{
    fn bind(
        _session: &mut Session<D>,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _global_data: &GlobalData,
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(_client: Client, global_data: &GlobalData) -> bool {
        global_data.enabled
    }
}

impl<D: SessionDriver> Dispatch<ZwlrGammaControlManagerV1, (), Session<D>> for Session<D> {
    fn request(
        session: &mut Session<D>,
        _client: &Client,
        _manager: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Session<D>>,
    ) {
        match request {
            zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } => {
                let output = Output::from_resource(&output);
                let gamma_size = output
                    .as_ref()
                    .filter(|output| {
                        !session
                            .wayland
                            .wlr_gamma_control_state
                            .controls
                            .contains_key(*output)
                    })
                    .and_then(|output| session.driver.gamma_size(output).ok());
                let control = data_init.init(
                    id,
                    ControlData {
                        gamma_size: gamma_size.unwrap_or(0),
                    },
                );
                if let (Some(output), Some(gamma_size)) = (output, gamma_size) {
                    control.gamma_size(gamma_size);
                    session
                        .wayland
                        .wlr_gamma_control_state
                        .controls
                        .insert(output, control);
                } else {
                    control.failed();
                }
            }
            zwlr_gamma_control_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D: SessionDriver> Dispatch<ZwlrGammaControlV1, ControlData, Session<D>> for Session<D> {
    fn request(
        session: &mut Session<D>,
        _client: &Client,
        resource: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        data: &ControlData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Session<D>>,
    ) {
        match request {
            zwlr_gamma_control_v1::Request::SetGamma { fd } => {
                let output = session
                    .wayland
                    .wlr_gamma_control_state
                    .controls
                    .iter()
                    .find_map(|(output, control)| (control == resource).then(|| output.clone()));
                let Some(output) = output else {
                    return;
                };
                let job = ReadJob {
                    file: File::from(fd),
                    gamma_size: data.gamma_size,
                    output: output.clone(),
                    control: resource.clone(),
                };
                let queued = session
                    .wayland
                    .wlr_gamma_control_state
                    .reader
                    .as_ref()
                    .is_some_and(|reader| reader.try_send(job).is_ok());
                if !queued {
                    finish_read(
                        session,
                        output,
                        resource.clone(),
                        Err("gamma reader unavailable or queue full".into()),
                    );
                }
            }
            zwlr_gamma_control_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        session: &mut Session<D>,
        _client: ClientId,
        resource: &ZwlrGammaControlV1,
        _data: &ControlData,
    ) {
        let output = session
            .wayland
            .wlr_gamma_control_state
            .controls
            .iter()
            .find_map(|(output, control)| (control == resource).then(|| output.clone()));
        if let Some(output) = output {
            session
                .wayland
                .wlr_gamma_control_state
                .controls
                .remove(&output);
            if let Err(err) = session.driver.set_gamma(&output, None) {
                eventline::warn!("failed to reset gamma for {:?}: {err}", output.name());
            }
        }
    }
}

fn read_ramp(file: File, gamma_size: u32) -> Result<Vec<u16>, String> {
    let entries = usize::try_from(gamma_size)
        .ok()
        .and_then(|size| size.checked_mul(3))
        .ok_or_else(|| "gamma ramp size overflow".to_string())?;
    let byte_len = entries
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| "gamma ramp byte size overflow".to_string())?;
    // The protocol requires a fixed-size file. Reject streams before reading;
    // metadata and reads both happen on the worker, including slow FUSE files.
    let metadata = file
        .metadata()
        .map_err(|err| format!("gamma file metadata: {err}"))?;
    if !metadata.is_file() || metadata.len() != byte_len as u64 {
        return Err("gamma ramp must be a regular file of the advertised size".into());
    }
    let mut bytes = vec![0; byte_len];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|err| format!("failed to read gamma ramp: {err}"))?;
    Ok(bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use smithay::reexports::rustix;
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::OwnedFd;

    fn ramp_file(bytes: &[u8]) -> File {
        let fd = rustix::fs::memfd_create(
            CString::new("gamma-test").unwrap(),
            rustix::fs::MemfdFlags::CLOEXEC,
        )
        .unwrap();
        let mut file = File::from(fd);
        file.write_all(bytes).unwrap();
        file
    }

    use super::read_ramp;

    #[test]
    fn ramp_parser_preserves_native_channel_order() {
        let values = [1u16, 2, 3, 4, 5, 6];
        let bytes = values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect::<Vec<_>>();
        assert_eq!(read_ramp(ramp_file(&bytes), 2).unwrap(), values);
    }

    #[test]
    fn ramp_parser_rejects_open_stream_without_waiting_for_eof() {
        let (reader, _writer): (OwnedFd, OwnedFd) = rustix::pipe::pipe().unwrap();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(read_ramp(File::from(reader), 2)).unwrap());
        assert!(
            result
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("reader blocked on pipe")
                .is_err()
        );
    }

    #[test]
    fn ramp_parser_ignores_and_preserves_shared_file_offset() {
        let mut file = ramp_file(&[0; 12]);
        file.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(read_ramp(file.try_clone().unwrap(), 2).unwrap(), [0; 6]);
        assert_eq!(file.stream_position().unwrap(), 5);
    }

    #[test]
    fn ramp_parser_rejects_short_and_trailing_data() {
        assert!(read_ramp(ramp_file(&[0; 11]), 2).is_err());
        assert!(read_ramp(ramp_file(&[0; 13]), 2).is_err());
    }
}
