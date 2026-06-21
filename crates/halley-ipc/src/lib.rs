pub mod codec;
pub mod error;

pub use codec::{
    decode_request, decode_response, encode_request, encode_response, read_frame,
    read_frame_with_fds, write_frame, write_frame_with_fds,
};
pub use error::CodecError;

use std::env;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use halley_api::{Request, Response};

pub fn default_socket_path() -> io::Result<PathBuf> {
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    Ok(PathBuf::from(runtime_dir)
        .join("halley")
        .join("halley.sock"))
}

pub fn send_request(req: &Request) -> Result<Response, CodecError> {
    let path = default_socket_path()?;
    send_request_to(&path, req)
}

pub fn send_request_to(path: &std::path::Path, req: &Request) -> Result<Response, CodecError> {
    let mut stream = UnixStream::connect(path)?;
    let bytes = encode_request(req)?;
    write_frame(&mut stream, &bytes)?;

    let resp_bytes = read_frame(&mut stream)?;
    decode_response(&resp_bytes)
}
