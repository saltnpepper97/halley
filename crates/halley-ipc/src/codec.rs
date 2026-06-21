use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use crate::error::CodecError;
use halley_api::{Request, Response};

const MAX_FRAME_LEN: u32 = 1024 * 1024; // 1 MiB

pub fn encode_request(req: &Request) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(req).map_err(CodecError::Encode)
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, CodecError> {
    postcard::from_bytes(bytes).map_err(CodecError::Decode)
}

pub fn encode_response(resp: &Response) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(resp).map_err(CodecError::Encode)
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, CodecError> {
    postcard::from_bytes(bytes).map_err(CodecError::Decode)
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), CodecError> {
    let len = payload.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, CodecError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;

    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(len));
    }

    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

/// Write a length-prefixed frame and pass file descriptors with SCM_RIGHTS.
///
/// This is the foundation needed for cross-process DMA-BUF screencast buffers:
/// PipeWire owns/allocates the DMA-BUFs in the portal process, while the
/// renderer lives in the compositor process. The fds must therefore travel over
/// the IPC Unix socket, not through postcard payload bytes.
pub fn write_frame_with_fds(
    stream: &UnixStream,
    payload: &[u8],
    fds: &[RawFd],
) -> Result<(), CodecError> {
    if payload.len() > MAX_FRAME_LEN as usize {
        return Err(CodecError::FrameTooLarge(payload.len() as u32));
    }
    if fds.is_empty() {
        let mut stream = stream;
        return write_frame(&mut stream, payload);
    }

    let len = (payload.len() as u32).to_le_bytes();
    let iov = [
        libc::iovec {
            iov_base: len.as_ptr() as *mut _,
            iov_len: len.len(),
        },
        libc::iovec {
            iov_base: payload.as_ptr() as *mut _,
            iov_len: payload.len(),
        },
    ];

    let control_len =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as libc::c_uint) as usize };
    let mut control = vec![0u8; control_len];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_ptr() as *mut _;
    msg.msg_iovlen = iov.len();
    msg.msg_control = control.as_mut_ptr() as *mut _;
    msg.msg_controllen = control.len();

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(std::io::Error::other("failed to allocate fd control message").into());
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as libc::c_uint) as usize;
        std::ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(cmsg).cast::<u8>(),
            std::mem::size_of_val(fds),
        );
    }

    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, 0) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let total_len = len.len() + payload.len();
    let sent = sent as usize;
    if sent < total_len {
        let mut frame = Vec::with_capacity(total_len);
        frame.extend_from_slice(&len);
        frame.extend_from_slice(payload);
        let mut stream = stream;
        stream.write_all(&frame[sent..])?;
        stream.flush()?;
    }

    Ok(())
}

/// Read a length-prefixed frame and receive up to `max_fds` SCM_RIGHTS file
/// descriptors attached to it.
pub fn read_frame_with_fds(
    stream: &UnixStream,
    max_fds: usize,
) -> Result<(Vec<u8>, Vec<OwnedFd>), CodecError> {
    let control_len = if max_fds == 0 {
        0
    } else {
        unsafe {
            libc::CMSG_SPACE((max_fds * std::mem::size_of::<RawFd>()) as libc::c_uint) as usize
        }
    };
    let mut control = vec![0u8; control_len];
    let mut initial = vec![0u8; 4 + MAX_FRAME_LEN as usize];
    let mut iov = [libc::iovec {
        iov_base: initial.as_mut_ptr().cast(),
        iov_len: initial.len(),
    }];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = iov.len();
    if !control.is_empty() {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len();
    }

    let read = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if read < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if read == 0 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "empty ipc frame").into(),
        );
    }
    if (msg.msg_flags & libc::MSG_CTRUNC) != 0 {
        return Err(std::io::Error::other("ipc fd control message truncated").into());
    }

    let mut fds = Vec::new();
    if max_fds > 0 {
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let data_len = (*cmsg).cmsg_len.saturating_sub(libc::CMSG_LEN(0) as usize);
                    let fd_count = data_len / std::mem::size_of::<RawFd>();
                    let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                    for idx in 0..fd_count.min(max_fds.saturating_sub(fds.len())) {
                        fds.push(OwnedFd::from_raw_fd(*data.add(idx)));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
    }

    let read = read as usize;
    let mut header = [0u8; 4];
    let header_from_initial = read.min(4);
    header[..header_from_initial].copy_from_slice(&initial[..header_from_initial]);
    if header_from_initial < 4 {
        let mut stream = stream;
        stream.read_exact(&mut header[header_from_initial..])?;
    }

    let len = u32::from_le_bytes(header);
    if len > MAX_FRAME_LEN {
        return Err(CodecError::FrameTooLarge(len));
    }

    let mut payload = vec![0u8; len as usize];
    let initial_payload = read.saturating_sub(4).min(payload.len());
    if initial_payload > 0 {
        payload[..initial_payload].copy_from_slice(&initial[4..4 + initial_payload]);
    }
    if initial_payload < payload.len() {
        let mut stream = stream;
        stream.read_exact(&mut payload[initial_payload..])?;
    }

    Ok((payload, fds))
}
