//! AWS event-stream framing (`application/vnd.amazon.eventstream`).

use serde_json::Value;
use std::collections::BTreeMap;

/// One decoded event-stream message: its headers and its (JSON) payload.
pub(super) struct EventFrame {
    headers: BTreeMap<String, String>,
    payload: Vec<u8>,
}

impl EventFrame {
    pub(super) fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).cloned()
    }

    pub(super) fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.payload).ok()
    }
}

/// Incremental decoder for the AWS binary event-stream framing the SDK hides from upstream.
///
/// Frame layout (`vnd.amazon.eventstream`):
/// `[total_len u32][headers_len u32][prelude_crc u32][headers][payload][message_crc u32]`, all
/// big-endian. Both CRCs are CRC-32 (IEEE) and both are verified: a corrupted frame must not be
/// silently interpreted, because the SDK would have rejected it.
#[derive(Default)]
pub(super) struct EventStreamDecoder {
    buffer: Vec<u8>,
}

/// The largest frame accepted, guarding a corrupt length prefix from provoking a huge allocation.
/// AWS's own limit for an event-stream message is 16 MiB.
const MAX_EVENT_FRAME_BYTES: usize = 16 * 1024 * 1024;

impl EventStreamDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Pop the next complete frame, or `Ok(None)` when more bytes are needed.
    pub(super) fn next_frame(&mut self) -> Result<Option<EventFrame>, String> {
        if self.buffer.len() < 12 {
            return Ok(None);
        }
        let total_len = be_u32(&self.buffer, 0).ok_or("truncated event-stream prelude")? as usize;
        let headers_len = be_u32(&self.buffer, 4).ok_or("truncated event-stream prelude")? as usize;
        let prelude_crc = be_u32(&self.buffer, 8).ok_or("truncated event-stream prelude")?;
        if !(16..=MAX_EVENT_FRAME_BYTES).contains(&total_len) || headers_len > total_len - 16 {
            return Err(format!("invalid event-stream frame length {total_len}"));
        }
        if self.buffer.len() < total_len {
            return Ok(None);
        }
        let prelude = self
            .buffer
            .get(..8)
            .ok_or("truncated event-stream prelude")?;
        if crc32(prelude) != prelude_crc {
            return Err("event-stream prelude checksum mismatch".to_string());
        }
        let message = self
            .buffer
            .get(..total_len - 4)
            .ok_or("truncated event-stream message")?;
        let message_crc =
            be_u32(&self.buffer, total_len - 4).ok_or("truncated event-stream message")?;
        if crc32(message) != message_crc {
            return Err("event-stream message checksum mismatch".to_string());
        }

        let headers_bytes = self
            .buffer
            .get(12..12 + headers_len)
            .ok_or("truncated event-stream headers")?
            .to_vec();
        let payload = self
            .buffer
            .get(12 + headers_len..total_len - 4)
            .ok_or("truncated event-stream payload")?
            .to_vec();
        self.buffer.drain(..total_len);

        Ok(Some(EventFrame {
            headers: parse_event_headers(&headers_bytes)?,
            payload,
        }))
    }
}

/// Read a big-endian `u32` at `offset`, or `None` when out of range (no indexing — the workspace
/// denies `clippy::indexing_slicing`).
fn be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    let mut buf = [0u8; 4];
    for (dst, src) in buf.iter_mut().zip(slice.iter()) {
        *dst = *src;
    }
    Some(u32::from_be_bytes(buf))
}

/// Decode the event-stream header block. Only string-valued headers are surfaced (the ones the
/// protocol uses for `:message-type` / `:event-type` / `:exception-type` / `:content-type`), but
/// every value type is *sized* correctly so the walk never desynchronises.
pub(super) fn parse_event_headers(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let name_len = *bytes.get(i).ok_or("truncated event-stream header")? as usize;
        i += 1;
        let name = bytes.get(i..i + name_len).ok_or("truncated header name")?;
        let name = String::from_utf8_lossy(name).to_string();
        i += name_len;
        let value_type = *bytes.get(i).ok_or("truncated header type")?;
        i += 1;
        match value_type {
            // bool true / bool false — no payload.
            0 | 1 => {}
            // byte / short / integer / long.
            2 => i += 1,
            3 => i += 2,
            4 => i += 4,
            5 => i += 8,
            // byte array / string — u16 length prefix.
            6 | 7 => {
                let len = u16::from_be_bytes([
                    *bytes.get(i).ok_or("truncated header length")?,
                    *bytes.get(i + 1).ok_or("truncated header length")?,
                ]) as usize;
                i += 2;
                let value = bytes.get(i..i + len).ok_or("truncated header value")?;
                if value_type == 7 {
                    out.insert(name, String::from_utf8_lossy(value).to_string());
                }
                i += len;
            }
            // timestamp (i64 millis) / uuid (16 bytes).
            8 => i += 8,
            9 => i += 16,
            other => return Err(format!("unknown event-stream header type {other}")),
        }
    }
    Ok(out)
}

/// CRC-32 (IEEE 802.3, reflected, poly `0xEDB88320`) — the checksum the event-stream framing uses.
pub(super) fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
