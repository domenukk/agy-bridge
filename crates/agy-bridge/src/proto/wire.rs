//! Binary Protobuf wire-format encoder and decoder for the `localharness` stdio startup handshake.
//!
//! Only two messages in the entire `localharness` protocol use binary Protobuf wire encoding
//! (`InputConfig` on `stdin` and `OutputConfig` on `stdout`); all conversation messages over
//! the WebSocket transport use JSON (`serde_json`).

use std::fmt;

use super::localharness::{ClientInfo, InputConfig, OutputConfig};

const WIRE_TYPE_VARINT: u32 = 0;
const WIRE_TYPE_64BIT: u32 = 1;
const WIRE_TYPE_LEN: u32 = 2;
const WIRE_TYPE_32BIT: u32 = 5;

const MAX_VARINT_BYTES: usize = 10;

/// Error returned when encoding or decoding a binary Protobuf handshake payload fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireError {
    message: String,
}

impl WireError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WireError {}

fn encode_varint(mut value: u64, buf: &mut Vec<u8>) {
    while value >= 0x80 {
        let low_byte = (value & 0x7F).to_le_bytes()[0] | 0x80;
        buf.push(low_byte);
        value >>= 7;
    }
    buf.push(value.to_le_bytes()[0]);
}

fn encode_tag(field_number: u32, wire_type: u32, buf: &mut Vec<u8>) {
    encode_varint(u64::from((field_number << 3) | wire_type), buf);
}

fn encode_string_field(field_number: u32, value: &str, buf: &mut Vec<u8>) {
    if value.is_empty() {
        return;
    }
    encode_tag(field_number, WIRE_TYPE_LEN, buf);
    encode_varint(value.len() as u64, buf);
    buf.extend_from_slice(value.as_bytes());
}

fn encode_bytes_field(field_number: u32, value: &[u8], buf: &mut Vec<u8>) {
    encode_tag(field_number, WIRE_TYPE_LEN, buf);
    encode_varint(value.len() as u64, buf);
    buf.extend_from_slice(value);
}

fn decode_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, WireError> {
    let mut result = 0u64;
    let mut shift = 0u32;
    for i in 0..MAX_VARINT_BYTES {
        let Some(&byte) = bytes.get(*pos) else {
            return Err(WireError::new("unexpected EOF while decoding varint"));
        };
        *pos += 1;
        if i == MAX_VARINT_BYTES - 1 && byte > 1 {
            return Err(WireError::new("varint overflow"));
        }
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
    Err(WireError::new("varint exceeds 10 bytes"))
}

fn decode_tag(bytes: &[u8], pos: &mut usize) -> Result<(u32, u32), WireError> {
    let tag = decode_varint(bytes, pos)?;
    let field_number = u32::try_from(tag >> 3)
        .map_err(|e| WireError::new(format!("field number overflow: {e}")))?;
    let wire_type = u32::from((tag & 0x07).to_le_bytes()[0]);
    Ok((field_number, wire_type))
}

fn decode_len_slice<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8], WireError> {
    let len_u64 = decode_varint(bytes, pos)?;
    let len = usize::try_from(len_u64)
        .map_err(|e| WireError::new(format!("length exceeds usize: {e}")))?;
    let end = pos
        .checked_add(len)
        .ok_or_else(|| WireError::new("length overflow"))?;
    let slice = bytes
        .get(*pos..end)
        .ok_or_else(|| WireError::new("unexpected EOF in length-delimited field"))?;
    *pos = end;
    Ok(slice)
}

fn decode_string(bytes: &[u8], pos: &mut usize) -> Result<String, WireError> {
    let slice = decode_len_slice(bytes, pos)?;
    std::str::from_utf8(slice)
        .map(str::to_owned)
        .map_err(|e| WireError::new(format!("invalid UTF-8 in protobuf string: {e}")))
}

fn skip_field(wire_type: u32, bytes: &[u8], pos: &mut usize) -> Result<(), WireError> {
    match wire_type {
        WIRE_TYPE_VARINT => {
            decode_varint(bytes, pos)?;
            Ok(())
        }
        WIRE_TYPE_64BIT => {
            let end = pos
                .checked_add(8)
                .ok_or_else(|| WireError::new("64-bit skip overflow"))?;
            if end > bytes.len() {
                return Err(WireError::new("unexpected EOF skipping 64-bit field"));
            }
            *pos = end;
            Ok(())
        }
        WIRE_TYPE_LEN => {
            decode_len_slice(bytes, pos)?;
            Ok(())
        }
        WIRE_TYPE_32BIT => {
            let end = pos
                .checked_add(4)
                .ok_or_else(|| WireError::new("32-bit skip overflow"))?;
            if end > bytes.len() {
                return Err(WireError::new("unexpected EOF skipping 32-bit field"));
            }
            *pos = end;
            Ok(())
        }
        other => Err(WireError::new(format!(
            "unsupported protobuf wire type: {other}"
        ))),
    }
}

impl ClientInfo {
    fn encode_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        encode_string_field(1, &self.language, &mut buf);
        encode_string_field(2, &self.version, &mut buf);
        encode_string_field(3, &self.language_version, &mut buf);
        encode_string_field(4, &self.os, &mut buf);
        encode_string_field(5, &self.os_version, &mut buf);
        buf
    }

    fn decode_slice(bytes: &[u8]) -> Result<Self, WireError> {
        let mut info = Self::default();
        let mut pos = 0;
        while pos < bytes.len() {
            let (field_number, wire_type) = decode_tag(bytes, &mut pos)?;
            match (field_number, wire_type) {
                (1, WIRE_TYPE_LEN) => info.language = decode_string(bytes, &mut pos)?,
                (2, WIRE_TYPE_LEN) => info.version = decode_string(bytes, &mut pos)?,
                (3, WIRE_TYPE_LEN) => info.language_version = decode_string(bytes, &mut pos)?,
                (4, WIRE_TYPE_LEN) => info.os = decode_string(bytes, &mut pos)?,
                (5, WIRE_TYPE_LEN) => info.os_version = decode_string(bytes, &mut pos)?,
                _ => skip_field(wire_type, bytes, &mut pos)?,
            }
        }
        Ok(info)
    }
}

fn decode_map_entry(bytes: &[u8]) -> Result<(String, String), WireError> {
    let mut key = String::new();
    let mut val = String::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let (field_number, wire_type) = decode_tag(bytes, &mut pos)?;
        match (field_number, wire_type) {
            (1, WIRE_TYPE_LEN) => key = decode_string(bytes, &mut pos)?,
            (2, WIRE_TYPE_LEN) => val = decode_string(bytes, &mut pos)?,
            _ => skip_field(wire_type, bytes, &mut pos)?,
        }
    }
    Ok((key, val))
}

impl InputConfig {
    /// Encodes this `InputConfig` in binary Protobuf wire format into `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`WireError`] if encoding fails.
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<(), WireError> {
        encode_string_field(1, &self.storage_directory, buf);
        if self.port != 0 {
            encode_tag(2, WIRE_TYPE_VARINT, buf);
            encode_varint(u64::from(self.port), buf);
        }
        encode_string_field(3, &self.bind_address, buf);
        if let Some(ref client_info) = self.client_info {
            let nested = client_info.encode_to_vec();
            encode_bytes_field(4, &nested, buf);
        }
        for (k, v) in &self.env {
            let mut entry = Vec::new();
            encode_string_field(1, k, &mut entry);
            encode_string_field(2, v, &mut entry);
            encode_bytes_field(5, &entry, buf);
        }
        if self.use_interactions_api {
            encode_tag(6, WIRE_TYPE_VARINT, buf);
            encode_varint(1, buf);
        }
        Ok(())
    }

    /// Decodes an `InputConfig` from a binary Protobuf wire slice.
    ///
    /// # Errors
    ///
    /// Returns [`WireError`] if `bytes` is malformed or truncated.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut config = Self::default();
        let mut pos = 0;
        while pos < bytes.len() {
            let (field_number, wire_type) = decode_tag(bytes, &mut pos)?;
            match (field_number, wire_type) {
                (1, WIRE_TYPE_LEN) => {
                    config.storage_directory = decode_string(bytes, &mut pos)?;
                }
                (2, WIRE_TYPE_VARINT) => {
                    let v = decode_varint(bytes, &mut pos)?;
                    config.port = u32::try_from(v)
                        .map_err(|e| WireError::new(format!("port exceeds u32: {e}")))?;
                }
                (3, WIRE_TYPE_LEN) => {
                    config.bind_address = decode_string(bytes, &mut pos)?;
                }
                (4, WIRE_TYPE_LEN) => {
                    let slice = decode_len_slice(bytes, &mut pos)?;
                    config.client_info = Some(ClientInfo::decode_slice(slice)?);
                }
                (5, WIRE_TYPE_LEN) => {
                    let slice = decode_len_slice(bytes, &mut pos)?;
                    let (k, v) = decode_map_entry(slice)?;
                    config.env.insert(k, v);
                }
                (6, WIRE_TYPE_VARINT) => {
                    config.use_interactions_api = decode_varint(bytes, &mut pos)? != 0;
                }
                _ => skip_field(wire_type, bytes, &mut pos)?,
            }
        }
        Ok(config)
    }
}

impl OutputConfig {
    /// Encodes this `OutputConfig` in binary Protobuf wire format into `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`WireError`] if encoding fails.
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<(), WireError> {
        if self.port != 0 {
            encode_tag(1, WIRE_TYPE_VARINT, buf);
            encode_varint(i64::from(self.port).cast_unsigned(), buf);
        }
        encode_string_field(2, &self.api_key, buf);
        Ok(())
    }

    /// Decodes an `OutputConfig` from a binary Protobuf wire slice.
    ///
    /// # Errors
    ///
    /// Returns [`WireError`] if `bytes` is malformed or truncated.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut config = Self::default();
        let mut pos = 0;
        while pos < bytes.len() {
            let (field_number, wire_type) = decode_tag(bytes, &mut pos)?;
            match (field_number, wire_type) {
                (1, WIRE_TYPE_VARINT) => {
                    let v = decode_varint(bytes, &mut pos)?;
                    let low = v.to_le_bytes();
                    let low_u32 = u32::from_le_bytes([low[0], low[1], low[2], low[3]]);
                    config.port = low_u32.cast_signed();
                }
                (2, WIRE_TYPE_LEN) => {
                    config.api_key = decode_string(bytes, &mut pos)?;
                }
                _ => skip_field(wire_type, bytes, &mut pos)?,
            }
        }
        Ok(config)
    }
}
