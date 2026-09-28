//! Incremental SSE framing — the WHATWG "event stream interpretation" algorithm over a byte
//! buffer with an advancing cursor.
//!
//! Replaces `eventsource-stream` 0.2.3 (see [`super::sse`]). Same observable frames on every
//! shape the crate's fixtures and the six wire APIs produce, minus two upstream defects: a
//! `&string[1..]` BOM strip that panics on a non-char-boundary, and a per-line
//! `String::split_off` + per-chunk whole-buffer `String::from_utf8` that together make framing
//! quadratic in the byte length of a single buffer.

use super::sse::SseFrame;

/// UTF-8 byte order mark, stripped once at stream start (`event_stream.rs:266-275` upstream).
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Consumed-prefix bytes tolerated before the buffer is compacted. The cursor makes framing
/// O(bytes); this bounds the buffer's *residency* without paying a memmove per line.
const RECLAIM_THRESHOLD: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct SseFramer {
    buf: Vec<u8>,
    /// Cursor into `buf`; everything before it is consumed.
    start: usize,
    bom_checked: bool,
    /// The spec's "event type buffer".
    event: String,
    /// The spec's "data buffer" — every `data:` line appends its value plus one LF.
    data: String,
}

impl SseFramer {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start >= RECLAIM_THRESHOLD {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(chunk);
    }

    /// The next complete frame, or `Ok(None)` when more bytes are needed.
    pub(crate) fn next_frame(&mut self) -> Result<Option<SseFrame>, std::str::Utf8Error> {
        if !self.bom_checked {
            if self.buf.len() >= BOM.len() {
                self.bom_checked = true;
                if self.buf.starts_with(&BOM) {
                    self.start = BOM.len();
                }
            } else if self.buf.iter().any(|b| *b == b'\n' || *b == b'\r') {
                // A complete line shorter than the BOM cannot begin with one.
                self.bom_checked = true;
            } else {
                return Ok(None);
            }
        }
        loop {
            let rest = self.buf.get(self.start..).unwrap_or(&[]);
            let Some(pos) = rest.iter().position(|b| *b == b'\n' || *b == b'\r') else {
                return Ok(None);
            };
            let term_len = match rest.get(pos) {
                Some(b'\n') => 1,
                Some(b'\r') => match rest.get(pos + 1) {
                    Some(b'\n') => 2,
                    Some(_) => 1,
                    // A trailing CR may still become CRLF; upstream reports `Incomplete` here.
                    None => return Ok(None),
                },
                _ => return Ok(None),
            };
            let line_at = self.start;
            self.start += pos + term_len;
            let line = std::str::from_utf8(self.buf.get(line_at..line_at + pos).unwrap_or(&[]))?;

            if line.is_empty() {
                // Dispatch — the same three steps [`Self::flush`] runs, so the blank-line and
                // the end-of-stream paths cannot drift apart.
                match self.flush() {
                    Some(frame) => return Ok(Some(frame)),
                    None => continue,
                }
            }
            Self::consume_line(&mut self.event, &mut self.data, line);
        }
    }

    /// Apply one non-blank line to the event/data buffers (upstream's `decodeSseLine` past its
    /// `line === ""` arm).
    ///
    /// CYRUP-DELTA: takes the two buffers rather than `&mut self`, because the caller's `line`
    /// borrows `self.buf` and a `&mut self` receiver would conflict with it. Copying the line
    /// into a `String` instead would put an allocation on every line of every stream, which is
    /// exactly what this framer exists to avoid.
    fn consume_line(event: &mut String, data: &mut String, line: &str) {
        if line.starts_with(':') {
            return; // comment
        }
        let (name, value) = match line.split_once(':') {
            Some((n, v)) => (n, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match name {
            "event" => {
                event.clear();
                event.push_str(value);
            }
            "data" => {
                data.push_str(value);
                data.push('\n');
            }
            // `id:` and `retry:` are spec fields that `SseFrame` does not carry. Neither
            // arms dispatch: only a blank line dispatches, and only non-empty data emits.
            _ => {}
        }
    }

    /// Dispatch whatever the event/data buffers hold. Both buffers reset EVEN IF nothing is
    /// emitted (upstream's `core::mem::take(self)` in `EventBuilder::dispatch`).
    ///
    /// CYRUP-DELTA: gated on non-empty DATA, whereas pi's `flushSseEvent`
    /// (`anthropic-messages.ts:340-353` @v0.87.1) also fires on an `event:`-only residual. It can:
    /// its `ServerSentEvent` carries `raw`. [`SseFrame`] does not, and every cyrup decoder
    /// dispatches on `data`, so an `event:`-only frame would be indistinguishable from noise.
    pub(crate) fn flush(&mut self) -> Option<SseFrame> {
        let mut data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if data.is_empty() {
            return None;
        }
        if data.ends_with('\n') {
            data.pop();
        }
        Some(SseFrame {
            event: if event.is_empty() {
                "message".to_string()
            } else {
                event
            },
            data,
        })
    }

    /// Terminate the stream: feed any UNTERMINATED residual line through the field parser, then
    /// dispatch. This is upstream's `if (buffer.length > 0) decodeSseLine(buffer, state)` followed
    /// by `flushSseEvent(state)` (`anthropic-messages.ts:454-464` @v0.87.1). Without the residual
    /// step a stream cut mid-`data:` line still loses its payload.
    pub(crate) fn flush_at_eof(&mut self) -> Result<Option<SseFrame>, std::str::Utf8Error> {
        if self.start < self.buf.len() {
            let line = std::str::from_utf8(self.buf.get(self.start..).unwrap_or(&[]))?;
            // Non-empty by construction (no terminator remains), so pi's `line === ""` arm —
            // which would dispatch — is unreachable here.
            Self::consume_line(&mut self.event, &mut self.data, line);
            self.start = self.buf.len();
        }
        Ok(self.flush())
    }
}

/// A framing error: either the byte source failed, or the stream was not UTF-8.
pub(crate) enum FrameError<E> {
    Transport(E),
    Utf8(std::str::Utf8Error),
}

/// Frame a byte stream. `Send`-safe and cancellation-agnostic — the caller keeps its own
/// cancel arm (see [`super::sse::open_sse`]).
pub(crate) fn frame_bytes<S, B, E>(
    inner: S,
    flush_at_eof: bool,
) -> impl futures::Stream<Item = Result<SseFrame, FrameError<E>>> + Send
where
    S: futures::Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send,
    E: Send,
{
    struct St<S> {
        inner: std::pin::Pin<Box<S>>,
        framer: SseFramer,
        done: bool,
        flush_at_eof: bool,
    }
    let st = St {
        inner: Box::pin(inner),
        framer: SseFramer::default(),
        done: false,
        flush_at_eof,
    };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if st.done {
                return None;
            }
            match st.framer.next_frame() {
                Ok(Some(frame)) => return Some((Ok(frame), st)),
                Err(e) => {
                    st.done = true;
                    return Some((Err(FrameError::Utf8(e)), st));
                }
                Ok(None) => {}
            }
            match futures::StreamExt::next(&mut st.inner).await {
                Some(Ok(bytes)) => st.framer.push(bytes.as_ref()),
                Some(Err(e)) => {
                    st.done = true;
                    return Some((Err(FrameError::Transport(e)), st));
                }
                // EOF. Two of pi's adapters terminate the residual frame here and the rest
                // drop it, so the caller chooses:
                //   * `anthropic-messages.ts` `flushSseEvent` (`:461-464` @v0.87.1), and
                //   * `openai-codex-responses.ts` `if (done && buffer.trim()) buffer += "\n\n";`
                //     (`:795` @v0.87.1, added in v0.85.0 / #9047)
                // both flush; the OpenAI-SDK-backed paths have no equivalent and deliberately
                // drop it. Setting `done` before yielding the flushed frame is what stops the
                // unfold from polling the finished inner stream again.
                None if st.flush_at_eof => {
                    st.done = true;
                    return match st.framer.flush_at_eof() {
                        Ok(Some(frame)) => Some((Ok(frame), st)),
                        Ok(None) => None,
                        Err(e) => Some((Err(FrameError::Utf8(e)), st)),
                    };
                }
                None => return None,
            }
        }
    })
}
