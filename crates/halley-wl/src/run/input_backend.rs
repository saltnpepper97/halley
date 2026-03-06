use super::*;
pub(super) struct DirectLibinputInterface;

impl LibinputInterface for DirectLibinputInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<std::os::fd::OwnedFd, i32> {
        use rustix::fs::{Mode, OFlags, open};
        open(path, OFlags::from_bits_retain(flags as u32), Mode::empty())
            .map_err(|err| err.raw_os_error())
    }

    fn close_restricted(&mut self, _fd: std::os::fd::OwnedFd) {
        // Drop closes the fd.
    }
}

pub(super) fn build_direct_libinput_backend() -> Result<LibinputInputBackend, Box<dyn Error>> {
    let seat = env::var("XDG_SEAT").unwrap_or_else(|_| "seat0".to_string());
    preflight_direct_input_access(seat.as_str())?;
    let mut context = Libinput::new_with_udev(DirectLibinputInterface);
    context
        .udev_assign_seat(seat.as_str())
        .map_err(|_| io::Error::other(format!("libinput seat assign failed for {}", seat)))?;
    Ok(LibinputInputBackend::new(context))
}

pub(super) fn preflight_direct_input_access(seat: &str) -> Result<(), Box<dyn Error>> {
    use rustix::fs::{Mode, OFlags, open};

    let xdg_seat = env::var("XDG_SEAT").unwrap_or_else(|_| "<unset>".to_string());
    info!(
        "tty direct-libinput preflight: seat={} XDG_SEAT={}",
        seat, xdg_seat
    );

    let mut event_nodes: Vec<PathBuf> = fs::read_dir("/dev/input")
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect();
    event_nodes.sort();

    if event_nodes.is_empty() {
        warn!("tty direct-libinput preflight: no /dev/input/event* nodes found");
        return Ok(());
    }

    let mut readable = 0usize;
    for path in &event_nodes {
        match open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(_fd) => readable += 1,
            Err(err) => {
                let (mode, uid, gid) = match fs::metadata(path) {
                    Ok(meta) => (meta.mode() & 0o777, meta.uid(), meta.gid()),
                    Err(_) => (0, 0, 0),
                };
                warn!(
                    "tty direct-libinput preflight: cannot open {} (mode={:o} uid={} gid={}): {}",
                    path.display(),
                    mode,
                    uid,
                    gid,
                    err
                );
            }
        }
    }
    info!(
        "tty direct-libinput preflight: readable input event nodes={}/{}",
        readable,
        event_nodes.len()
    );
    if readable == 0 {
        let msg = "no readable /dev/input/event* devices; tty input will not work in direct-libinput mode (fix permissions/group membership, run as root, or enable session-libseat)";
        warn!("{}", msg);
        warn!("continuing without input devices");
    }
    Ok(())
}

#[cfg(feature = "session-libseat")]
pub(super) fn build_tty_libinput_backend(
    session: Rc<RefCell<LibSeatSession>>,
    seat: &str,
) -> Result<(LibinputInputBackend, Rc<RefCell<Libinput>>), Box<dyn Error>> {
    let mut context = Libinput::new_with_udev(LibinputSessionInterface::from(session));
    context
        .udev_assign_seat(seat)
        .map_err(|_| io::Error::other(format!("libinput seat assign failed for {}", seat)))?;
    let context_handle = Rc::new(RefCell::new(context.clone()));
    Ok((LibinputInputBackend::new(context), context_handle))
}
