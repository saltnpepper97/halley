//! Compositor-owned PNG selections stay available after their toast disappears.
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub enum SelectionData {
    #[default]
    X11,
    Png(Arc<[u8]>),
}

/// A client can delay reading its pipe indefinitely. Transfer away from the
/// compositor loop, and bound the time spent waiting on an unresponsive reader.
pub fn send(png: Arc<[u8]>, fd: OwnedFd) {
    if let Err(err) = std::thread::Builder::new()
        .name("halley-screenshot-clipboard".into())
        .spawn(move || {
            if let Err(err) = write_png(&png, fd) {
                eventline::debug!("screenshot clipboard transfer: {err}");
            }
        })
    {
        eventline::warn!("screenshot clipboard worker: {err}");
    }
}

fn write_png(mut bytes: &[u8], fd: OwnedFd) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: raw remains owned by fd/file throughout this transfer.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut file = std::fs::File::from(fd);
    while !bytes.is_empty() {
        match file.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => bytes = &bytes[written..],
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: one initialized pollfd, valid until the call returns.
                let ready = unsafe { libc::poll(&mut poll, 1, 5_000) };
                if ready == 0 {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                if ready < 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    #[test]
    fn large_png_transfers_exact_bytes_without_blocking_the_caller() {
        let png: Arc<[u8]> = (0..1_000_000)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>()
            .into();
        let (mut reader, writer) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        send(png.clone(), writer.into());
        let mut received = Vec::new();
        reader.read_to_end(&mut received).unwrap();
        assert_eq!(&*png, received);
    }

    #[test]
    fn abandoned_receiver_closes_the_transfer() {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        assert!(write_png(&[1; 4096], writer.into()).is_err());
    }
}
