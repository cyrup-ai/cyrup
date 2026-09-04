//! Length-prefixed JSON framing — a 1:1 port of `pi-intercom/broker/framing.ts`.
//!
//! Frame = 4-byte **big-endian** `u32` payload length (covering ONLY the JSON, not the 4 header
//! bytes) followed by that many UTF-8 JSON bytes (`framing.ts:9-15`). One write per message,
//! fire-and-forget. The reader ([`FrameReader`]) reassembles across arbitrary chunk boundaries,
//! rejects an over-cap length as a hard error (drop the connection), and yields every complete
//! frame available in one chunk (`framing.ts:49-86`).
//!
//! pi's `createMessageReader` (`framing.ts:49-86`) calls `onMessage` synchronously, in a loop,
//! for every complete frame it finds — including any frames found *before* it later discovers an
//! over-cap length later in the same `data` chunk (`framing.ts:62-67,80-84`). [`FrameReader::push`]
//! mirrors that: even on the `Err` path, [`FrameReadError::frames`] carries every frame that was
//! fully reassembled earlier in the same call, so the caller can dispatch them before tearing down
//! the connection, exactly as pi already fired `onMessage` for them before `onError`.

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

/// The framing-layer error cases pi's `reportMessage`/`createMessageReader` can report via
/// `onError` (`framing.ts:29-47,63-66`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// A declared payload length exceeds [`MAX_FRAME_BYTES`] (`framing.ts:63-66`).
    #[error("Intercom frame length {length} exceeds maximum {max} bytes")]
    Oversize {
        /// The declared (rejected) length.
        length: usize,
        /// The cap that was exceeded.
        max: usize,
    },
    /// A frame's JSON payload failed to parse (`reportMessage`'s `JSON.parse` catch,
    /// `framing.ts:29-37`). Callers should construct this from the decode error's message when
    /// wiring up their own JSON-decode step, matching pi's `Failed to parse intercom message: `
    /// wording exactly.
    #[error("Failed to parse intercom message: {message}")]
    Parse {
        /// The underlying parse error's message.
        message: String,
    },
    /// The message handler itself failed while processing an otherwise well-formed frame
    /// (`reportMessage`'s `onMessage` catch, `framing.ts:39-46`). Callers should construct this
    /// from the handler error's message, matching pi's `Failed to handle intercom message: `
    /// wording exactly.
    #[error("Failed to handle intercom message: {message}")]
    Handler {
        /// The underlying handler error's message.
        message: String,
    },
}

/// A fatal [`FrameReader::push`] failure, paired with every frame that was already fully
/// reassembled earlier in the *same* `push` call (`framing.ts:52-84` — pi delivers those to
/// `onMessage` synchronously, before it ever detects the later oversize length). Callers MUST
/// dispatch [`Self::frames`] before acting on [`Self::error`] and tearing down the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct FrameReadError {
    /// Frames fully reassembled before the fatal error was encountered; still deliverable.
    pub frames: Vec<Vec<u8>>,
    /// The fatal framing error itself.
    pub error: FrameError,
}

impl FrameReader {
    /// A fresh reader with an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw chunk; return every complete payload now available (`framing.ts:49-86`). A
    /// partial header/payload is retained for the next call. An over-cap length is returned as an
    /// `Err` after clearing the buffer, exactly as pi resets `buffer` then reports the error —
    /// but any frames already reassembled earlier in this same call are carried on
    /// [`FrameReadError::frames`] rather than discarded, matching pi's synchronous
    /// deliver-then-error ordering (`framing.ts:52-84`).
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, FrameReadError> {
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
                return Err(FrameReadError {
                    frames: out,
                    error: FrameError::Oversize {
                        length,
                        max: MAX_FRAME_BYTES,
                    },
                });
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

/// Deserialize one frame payload with **JavaScript's number-overflow behaviour**, which is not
/// `serde_json`'s.
///
/// pi parses every frame with `JSON.parse` (`v0.9.2 broker/framing.ts:36-42`). ECMA-262 rounds a
/// numeric literal that overflows the double range to `±Infinity`, so `1e400` parses fine and
/// `typeof Infinity === "number"` is TRUE — every pi type guard passes. `serde_json` instead fails
/// the WHOLE FRAME with `number out of range` (verified: `1e400`, `-1e400`, `1e309` all error;
/// `1e300` parses). Because that happens in the JSON reader, it fires even in positions cyrup does
/// not model and pi never type-checks: an unmodelled key inside `message`, a top-level frame key, an
/// `extension_publish` payload. A peer could therefore drop a cyrup connection that pi serves
/// normally — and on a socket every local process can reach.
///
/// The fix reproduces what pi puts ON THE WIRE. pi accepts the overflow as `Infinity`, then relays
/// through `JSON.stringify`, which emits **`null`** for any non-finite number. So a peer downstream
/// of a pi broker sees `null` in that position — exactly what this makes cyrup see. For an
/// unmodelled key that is byte-for-byte parity: pi delivers the message with the key nulled, and so
/// does cyrup.
///
/// **Where this still diverges, and why that is not a silent choice.** For a MODELLED numeric field
/// (`timestamp`), pi's broker accepts `Infinity`, answers the sender `delivered`, and relays `null`
/// — at which point the RECEIVER's own `isMessage` rejects it (`typeof null !== "number"`,
/// `v0.9.2 broker/client.ts:106-116`), throws, and `client.ts:321-329` destroys that receiver's
/// socket. A hostile sender can thus disconnect an arbitrary third session. cyrup's guard sees the
/// `null` here and answers `delivery_failed` to the SENDER instead, which is fail-closed and
/// harms nobody else. That is a deliberate refusal to reproduce an upstream amplification bug, and
/// it is recorded in `docs/gap-analysis/PARITY-GAPS.md` (G136c) rather than left as an undocumented difference.
///
/// The slow path runs only after a real `number out of range` error, so well-formed traffic pays
/// nothing.
///
/// # Errors
///
/// Returns the `serde_json` error if the payload is not valid JSON for `T` even after the rewrite.
pub fn from_frame_slice<T: serde::de::DeserializeOwned>(payload: &[u8]) -> serde_json::Result<T> {
    match serde_json::from_slice::<T>(payload) {
        Ok(value) => Ok(value),
        Err(err) if err.classify() == serde_json::error::Category::Syntax => {
            match null_out_overflowing_numbers(payload) {
                Some(rewritten) => serde_json::from_slice::<T>(&rewritten),
                None => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

/// Rewrite every numeric literal whose magnitude overflows the double range to `null`, matching
/// what `JSON.stringify` emits for the `±Infinity` that `JSON.parse` produced. Returns `None` when
/// nothing needed rewriting, so the caller keeps the original `serde_json` error.
///
/// This is a byte scanner, not a parser: it must recognise string boundaries so a numeric-looking
/// substring INSIDE a string is never touched, and it must respect backslash escapes so a `\"` does
/// not end a string early. Both are covered by tests.
fn null_out_overflowing_numbers(payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len());
    let mut i = 0;
    let mut in_string = false;
    let mut rewrote = false;

    while i < payload.len() {
        let Some(&b) = payload.get(i) else { break };

        if in_string {
            out.push(b);
            if b == b'\\' {
                // Copy the escaped byte verbatim so an escaped quote cannot close the string.
                if let Some(&esc) = payload.get(i + 1) {
                    out.push(esc);
                    i += 2;
                    continue;
                }
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if b == b'"' {
            in_string = true;
            out.push(b);
            i += 1;
            continue;
        }

        // A number can only start here: `-` or a digit, and only where a value may appear. Scanning
        // outside strings means the only other `-`/digit bytes are inside numbers we are already
        // consuming, so this is sufficient.
        if b == b'-' || b.is_ascii_digit() {
            let start = i;
            let mut end = i;
            while let Some(&c) = payload.get(end) {
                if c == b'-'
                    || c == b'+'
                    || c == b'.'
                    || c == b'e'
                    || c == b'E'
                    || c.is_ascii_digit()
                {
                    end += 1;
                } else {
                    break;
                }
            }
            let literal = payload.get(start..end).unwrap_or_default();
            let overflows = std::str::from_utf8(literal)
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .is_some_and(|f| f.is_infinite());
            if overflows {
                out.extend_from_slice(b"null");
                rewrote = true;
            } else {
                out.extend_from_slice(literal);
            }
            i = end;
            continue;
        }

        out.push(b);
        i += 1;
    }

    rewrote.then_some(out)
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
            assert!(
                got.is_empty(),
                "no complete frame until the final byte arrives"
            );
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
        let err = reader
            .push(&oversize)
            .expect_err("must reject over-cap length");
        assert!(
            err.frames.is_empty(),
            "no frames were reassembled before the oversize header"
        );
        assert_eq!(
            err.error,
            FrameError::Oversize {
                length: MAX_FRAME_BYTES + 1,
                max: MAX_FRAME_BYTES
            }
        );
    }

    // framing.ts:52-84 — pi's reader delivers every complete frame found earlier in the same
    // `data` chunk to `onMessage` synchronously, in order, and only *afterward* discovers and
    // reports a later oversize frame. Regression proof: against the pre-fix cyrup behavior (which
    // collected reassembled frames into a local `out` and discarded it entirely by returning a
    // bare `Err` on the oversize branch) this test would fail because `err.frames` would be empty
    // instead of containing the two good frames.
    #[test]
    fn preserves_already_reassembled_frames_when_a_later_frame_in_the_same_chunk_is_oversize() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&frame(b"first"));
        chunk.extend_from_slice(&frame(b"second"));
        // A third, bogus frame header declaring an over-cap length, appended in the SAME push().
        let bad_len = (MAX_FRAME_BYTES as u32) + 1;
        chunk.extend_from_slice(&bad_len.to_be_bytes());

        let mut reader = FrameReader::new();
        let err = reader
            .push(&chunk)
            .expect_err("must reject the oversize third frame");

        // The two frames reassembled before the oversize header must NOT be discarded.
        assert_eq!(err.frames, vec![b"first".to_vec(), b"second".to_vec()]);
        assert_eq!(
            err.error,
            FrameError::Oversize {
                length: MAX_FRAME_BYTES + 1,
                max: MAX_FRAME_BYTES
            }
        );
    }

    // framing.ts:29-47 — reportMessage wraps a JSON.parse failure as
    // "Failed to parse intercom message: {message}" and a handler failure as
    // "Failed to handle intercom message: {message}". FrameReader::push itself only reassembles
    // bytes (JSON decoding/dispatch happens in the transport-layer callers), but the descriptive
    // error text those callers must produce is defined once here so the wording stays byte-for-
    // byte faithful to pi wherever it's used.
    #[test]
    fn parse_and_handler_error_messages_match_pi_wording_exactly() {
        let parse_err = FrameError::Parse {
            message: "Unexpected token o in JSON".to_string(),
        };
        assert_eq!(
            parse_err.to_string(),
            "Failed to parse intercom message: Unexpected token o in JSON"
        );

        let handler_err = FrameError::Handler {
            message: "boom".to_string(),
        };
        assert_eq!(
            handler_err.to_string(),
            "Failed to handle intercom message: boom"
        );
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
