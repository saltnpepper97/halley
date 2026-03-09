use std::io::{Read, Write};

use crate::error::CodecError;
use crate::protocol::{Request, Response};

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
