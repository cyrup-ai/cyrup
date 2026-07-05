//! Length-prefixed JSON framing — a 1:1 port of `pi-intercom/broker/framing.ts`.
//!
//! Frame = 4-byte **big-endian** `u32` payload length (covering ONLY the JSON, not the 4 header
//! bytes) followed by that many UTF-8 JSON bytes (`framing.ts:9-15`). One write per message,
//! fire-and-forget. The reader ([`FrameReader`]) reassembles across arbitrary chunk boundaries,
//! rejects an over-cap length as a hard error (drop the connection), and yields every complete
//! frame available in one chunk (`framing.ts:49-86`).

use bytes::{Buf, BytesMut};

/// `MAX_FRAME_BYTES = 1024 * 1024` (`framing.ts:3`) — 1 MiB payload cap.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Encode one message: 4-byte BE payload length + the payload bytes (`writeMessage`,
/// `framing.ts:9-15`). Serialize the message to JSON first and pass its UTF-8 bytes here.
#[must_use]
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Serialize `value` to JSON and encode it as one length-prefixed frame.
///
/// # Errors
/// Returns the underlying `serde_json` error if `value` cannot be serialized.
pub fn encode_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    Ok(encode_frame(&serde_json::to_vec(value)?))
}

/// A streaming length-prefixed frame reassembler (`createMessageReader`, `framing.ts:22-87`).
/// Feed it raw socket chunks with [`FrameReader::push`]; it returns every complete JSON payload
/// available so far. An over-cap length is a hard [`FrameError::Oversize`] — the caller MUST drop
/// the connection (pi `socket.destroy`), never resync.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: BytesMut,
}

/// The single fatal framing error: a declared payload length exceeding [`MAX_FRAME_BYTES`]
/// (`framing.ts:63-66`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("intercom frame length {length} exceeds maximum {max} bytes")]
pub struct FrameError {
    /// The declared (rejected) length.
    pub length: usize,
    /// The cap that was exceeded.
    pub max: usize,
}

impl FrameReader {
    /// A fresh reader with an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw chunk; return every complete payload now available (`framing.ts:49-86`). A
    /// partial header/payload is retained for the next call. An over-cap length is returned as an
    /// `Err` after clearing the buffer, exactly as pi resets `buffer` then reports the error.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        self.buffer.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            // Peek the 4-byte BE length header without consuming it (pi reads
            // `buffer.readUInt32BE(0)` before it necessarily has the full frame). `first_chunk`
            // avoids any slice indexing (workspace-denied `clippy::indexing_slicing`).
            let Some(header) = self.buffer.first_chunk::<4>() else {
                return Ok(out);
            };
            let length = u32::from_be_bytes(*header) as usize;
            if length > MAX_FRAME_BYTES {
                self.buffer.clear();
                return Err(FrameError { length, max: MAX_FRAME_BYTES });
            }
            if self.buffer.len() < 4 + length {
                return Ok(out);
            }
            // Consume the 4-byte header + `length` payload bytes.
            self.buffer.advance(4);
            let payload = self.buffer.split_to(length);
            out.push(payload.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn frame(payload: &[u8]) -> Vec<u8> {
        encode_frame(payload)
    }

    // framing.test.ts:11-29 — a frame split across arbitrary chunk boundaries reassembles.
    #[test]
    fn reassembles_a_frame_fragmented_across_chunks() {
        let payload = br#"{"type":"hello","n":1}"#;
        let full = frame(payload);
        let mut reader = FrameReader::new();

        // Feed the header one byte at a time, then the payload in two halves.
        for i in 0..full.len() - 1 {
            let got = reader.push(&full[i..=i]).expect("no oversize");
            assert!(got.is_empty(), "no complete frame until the final byte arrives");
        }
        let got = reader.push(&full[full.len() - 1..]).expect("no oversize");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], payload);
    }

    // framing.test.ts — multiple back-to-back frames in one chunk all surface.
    #[test]
    fn yields_multiple_frames_in_one_chunk() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&frame(b"aaa"));
        chunk.extend_from_slice(&frame(b"bbbb"));
        chunk.extend_from_slice(&frame(b"c"));
        let mut reader = FrameReader::new();
        let got = reader.push(&chunk).expect("no oversize");
        assert_eq!(got, vec![b"aaa".to_vec(), b"bbbb".to_vec(), b"c".to_vec()]);
    }

    // framing.test.ts:31-82 — an over-cap length is a hard error; the buffer is cleared.
    #[test]
    fn rejects_an_oversize_frame_as_a_hard_error() {
        let mut oversize = Vec::new();
        let bad_len = (MAX_FRAME_BYTES as u32) + 1;
        oversize.extend_from_slice(&bad_len.to_be_bytes());
        let mut reader = FrameReader::new();
        let err = reader.push(&oversize).expect_err("must reject over-cap length");
        assert_eq!(err.length, MAX_FRAME_BYTES + 1);
        assert_eq!(err.max, MAX_FRAME_BYTES);
    }

    #[test]
    fn round_trips_through_encode_json() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct M {
            a: u32,
        }
        let encoded = encode_json(&M { a: 7 }).expect("serializes");
        let mut reader = FrameReader::new();
        let frames = reader.push(&encoded).expect("no oversize");
        assert_eq!(frames.len(), 1);
        let back: M = serde_json::from_slice(&frames[0]).expect("deserializes");
        assert_eq!(back, M { a: 7 });
    }
}
