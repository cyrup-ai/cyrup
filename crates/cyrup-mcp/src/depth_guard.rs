//! MCP-630 — a message nested deeper than this port reads is refused at once, not dropped.
//!
//! serde_json refuses the 128th nested array or object (`remaining_depth: 128`, `de.rs:63`
//! @1.0.150), and reports it as a syntax error. rmcp's line codec drops a line with a syntax error
//! (`transport/async_rw.rs:164-172` @3.1.4, "Ignoring unparsable incoming message"), and its HTTP
//! client treats a JSON body that does not parse as an accepted POST
//! (`common/reqwest/streamable_http_client.rs:308-318`) and skips an SSE event that does not
//! (`common/client_side_sse.rs:409-417`). A reply nested that deep was therefore never seen, and the
//! request it answered ran to its timeout, 60 s by default ([`crate::runtime::DEFAULT_REQUEST_TIMEOUT`]).
//!
//! # Refused, not read [CYRUP-DELTA]
//!
//! A JavaScript client reads such a reply (`JSON.parse` has no depth limit). This port refuses it:
//! 127 levels is the limit of every serde_json reader on a result's path, and of one that this crate
//! cannot reach. Measured (MCP-637), reader by reader, transport to session to cache:
//!
//! | reader | nests the reply's JSON | limit |
//! |---|---|---|
//! | this guard, before any parse | as the reply | 127 ([`MAX_READ_DEPTH`]) |
//! | rmcp's stdio line codec (`transport/async_rw.rs:164-172` @3.1.4) | as the reply | serde_json's 127 |
//! | the HTTP JSON body, parsed here ([`read_json_body`]) | as the reply | serde_json's 127 |
//! | an SSE event, parsed by rmcp (`common/client_side_sse.rs:409-417`) | as the reply | serde_json's 127, out of this crate's reach |
//! | the session loader (`cyrup-session` `manager/load.rs`), on the tool result's line | the reply **+ 2** (`details.mcpResult`, measured by `cyrup-it`'s `deep_result`) | serde_json's 127; an unreadable line is skipped |
//! | the cache readers (`dirs::load_metadata_cache`, `registration::load_metadata_cache`), on `mcp-cache.json` | a `tools/list` reply **+ 1** | serde_json's 127; an unreadable file is absent |
//! | a codemode script (V8's `JSON.parse`, `prelude.js` `settle`) | the result | none |
//!
//! Nothing in the workspace disables the limit (`disable_recursion_limit` needs serde_json's
//! `unbounded_depth`, which is off) or grows the stack under a deep parse (no `serde_stacker`).
//! Reading past 127 would need all three: rmcp's SSE parse, which is not this crate's; a parse with
//! no limit that does not overflow a worker's stack on a hostile reply, which aborts the process
//! rather than failing one request; and a session loader that reads the entry.
//!
//! Within 127, what is read is persisted so that it reads back: the result's `details.mcpResult` is
//! summarized past [`crate::renderers::MCP_RESULT_MAX_DEPTH`] (a reply nested 126 or 127 made a
//! 128- or 129-level line, which the loader skipped, and the tool result vanished on resume), and
//! a catalogue that would nest `mcp-cache.json` past 127 is not cached (a 127-level `tools/list`
//! reply made a 128-level file, which neither reader parsed, and every server's catalogue went
//! with it).

//! # What a refusal does
//!
//! Each incoming message is measured before rmcp parses it ([`nesting_depth`], one pass, string
//! aware). One within [`MAX_READ_DEPTH`] goes through untouched. One deeper is read only far enough
//! to find its `id` and `method` — serde_json skips the fields it is not asked for iteratively and
//! without its depth limit (`ignore_value`, `de.rs:1102`) — and then:
//!
//! - a **reply** (an `id`, no `method`) is replaced by a JSON-RPC error carrying the same `id`, so
//!   the request it answers fails at once, saying how deep the reply was ([`refusal_message`]);
//! - a **request or notification from the server**, or a message with no `id` to answer, is dropped,
//!   as rmcp drops any message it cannot parse. Nothing on this side waits on one. A server request
//!   dropped here is left to the server's own timeout: the reader cannot write the error back.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::StreamExt as _;
use futures::stream::BoxStream;
use serde::Deserialize;
use serde_json::Value;
use sse_stream::Sse;
use tokio::io::{AsyncRead, ReadBuf};

/// The deepest JSON this port reads, in nested arrays and objects: serde_json refuses the next one.
pub const MAX_READ_DEPTH: usize = 127;

/// JSON-RPC's "Parse error", the code a refused reply is answered with.
const PARSE_ERROR: i32 = -32700;

/// The UTF-8 byte-order mark rmcp strips before it parses a line (`async_rw.rs`).
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// The nesting of the JSON seen so far, carried across the chunks one message arrives in.
#[derive(Debug, Default, Clone, Copy)]
struct Scan {
    depth: usize,
    deepest: usize,
    in_string: bool,
    escaped: bool,
}

impl Scan {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => self.in_string = true,
                b'{' | b'[' => {
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                _ => {}
            }
        }
    }
}

/// How many arrays and objects deep `text` nests at its deepest. Brackets inside strings do not
/// count.
#[must_use]
pub fn nesting_depth(text: &[u8]) -> usize {
    let mut scan = Scan::default();
    scan.feed(text);
    scan.deepest
}

/// What becomes of a message nested deeper than [`MAX_READ_DEPTH`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A reply, answered in its place with this JSON-RPC error, which carries the reply's `id`.
    Answer(String),
    /// A request or notification from the server, or a message with no `id`: dropped.
    Drop,
}

/// `None` when `message` can be read as it is; what to do with it otherwise.
#[must_use]
pub fn refuse(message: &[u8]) -> Option<Refusal> {
    let depth = nesting_depth(message);
    (depth > MAX_READ_DEPTH).then(|| refusal(message, depth))
}

/// The text a refused reply fails its request with — what the user reads after
/// `Failed to call tool: `, or as the reason a server failed to connect.
#[must_use]
pub fn refusal_message(depth: usize) -> String {
    format!(
        "the MCP server's reply is nested {depth} levels deep, and cyrup reads JSON nested at most \
         {MAX_READ_DEPTH} levels deep"
    )
}

fn refusal(message: &[u8], depth: usize) -> Refusal {
    /// Only the two fields that decide the refusal; every other field is skipped unread.
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        id: Option<Value>,
        #[serde(default)]
        method: Option<String>,
    }

    let body = message.strip_prefix(UTF8_BOM).unwrap_or(message);
    let envelope = match serde_json::from_slice::<Envelope>(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            tracing::warn!(
                "MCP: dropped a message nested {depth} levels deep (cyrup reads at most \
                 {MAX_READ_DEPTH}) that has no id to answer: {error}"
            );
            return Refusal::Drop;
        }
    };
    match (envelope.id, envelope.method) {
        (Some(id), None) if id.is_number() || id.is_string() => {
            tracing::warn!(
                "MCP: refused a reply nested {depth} levels deep (cyrup reads at most \
                 {MAX_READ_DEPTH}); its request fails"
            );
            Refusal::Answer(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": PARSE_ERROR, "message": refusal_message(depth) },
                })
                .to_string(),
            )
        }
        (_, method) => {
            tracing::warn!(
                "MCP: dropped a server {} nested {depth} levels deep (cyrup reads at most \
                 {MAX_READ_DEPTH})",
                method.map_or_else(|| "message".to_string(), |method| format!("`{method}`")),
            );
            Refusal::Drop
        }
    }
}

/// One SSE event with its `data` refused in place: replaced by the answer, or emptied, which rmcp
/// skips as it skips an event with no data. The event's `id` is kept, so a resumed stream still
/// starts after it.
fn guard_sse_event(mut event: Sse) -> Sse {
    if let Some(refusal) = event
        .data
        .as_deref()
        .and_then(|data| refuse(data.as_bytes()))
    {
        event.data = match refusal {
            Refusal::Answer(answer) => Some(answer),
            Refusal::Drop => None,
        };
    }
    event
}

/// An SSE stream with every event's `data` measured before rmcp parses it.
pub fn guard_sse<E: Send + 'static>(
    stream: BoxStream<'static, Result<Sse, E>>,
) -> BoxStream<'static, Result<Sse, E>> {
    stream.map(|event| event.map(guard_sse_event)).boxed()
}

/// The `initialize` (or `server/discover`) POST's SSE stream with every event's `data` measured
/// before rmcp parses it: a **reply** nested past what cyrup reads ends the stream, with
/// [`refusal_message`] as its error; a deep request or notification is dropped, as [`guard_sse`]
/// drops it.
///
/// Not [`guard_sse`], because rmcp reads this stream with `expect_initialized`
/// (`transport/streamable_http_client.rs:257-288` @3.1.4), which returns on the first `Response`
/// and skips every other message, the error reply [`guard_sse`] puts in a deep reply's place
/// included: the handshake would drain past it and wait out its budget, or fail with `empty sse
/// stream`. An error on the stream ends it at once (`let event = event?;`, `:267`).
///
/// rmcp's worker logs that error and closes the connection, so the reason is also handed to
/// `refused`, for the connect to report.
pub fn guard_handshake_sse(
    stream: BoxStream<'static, Result<Sse, sse_stream::Error>>,
    refused: impl Fn(String) + Send + 'static,
) -> BoxStream<'static, Result<Sse, sse_stream::Error>> {
    stream
        .map(move |event| {
            let mut event = event?;
            let Some(data) = event.data.as_deref() else {
                return Ok(event);
            };
            match refuse(data.as_bytes()) {
                Some(Refusal::Answer(_)) => {
                    let reason = refusal_message(nesting_depth(data.as_bytes()));
                    refused(reason.clone());
                    Err(sse_stream::Error::Body(Box::new(std::io::Error::other(
                        reason,
                    ))))
                }
                Some(Refusal::Drop) => {
                    event.data = None;
                    Ok(event)
                }
                None => Ok(event),
            }
        })
        .boxed()
}

/// A JSON body as rmcp's HTTP client would read it, measured first: the message, the refusal's
/// answer in its place, or `None` for a body that is neither — which rmcp treats as an accepted
/// POST.
#[must_use]
pub fn read_json_body(body: &[u8]) -> Option<rmcp::model::ServerJsonRpcMessage> {
    match refuse(body) {
        Some(Refusal::Answer(answer)) => serde_json::from_str(&answer).ok(),
        Some(Refusal::Drop) => None,
        None => serde_json::from_slice(body).ok(),
    }
}

/// The child's stdout as rmcp's line codec reads it, one line at a time, each measured first.
///
/// A line within [`MAX_READ_DEPTH`] passes through byte for byte. A deeper one is replaced by its
/// [`Refusal::Answer`] or left out. A line is handed on only once it is complete, which costs
/// nothing: the codec parses only complete lines.
/// What a [`DepthGuard`] keeps of a buffer between lines. A line longer than this (a tool returning a
/// screenshot) grows the buffers for itself, and they are given back once it has been handed on, so
/// one large reply does not hold its size for the life of the connection.
const RETAINED_CAPACITY: usize = 64 * 1024;

/// Give back what a buffer grew past [`RETAINED_CAPACITY`] for one large line.
fn release(buffer: &mut Vec<u8>) {
    if buffer.capacity() > RETAINED_CAPACITY {
        buffer.shrink_to(RETAINED_CAPACITY);
    }
}

pub struct DepthGuard<R> {
    inner: R,
    /// The line being read, up to its newline.
    line: Vec<u8>,
    scan: Scan,
    /// Lines ready for the reader, and how much of them it has taken.
    ready: Vec<u8>,
    handed: usize,
    eof: bool,
}

impl<R> DepthGuard<R> {
    /// Measure what `inner` produces.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            line: Vec::new(),
            scan: Scan::default(),
            ready: Vec::new(),
            handed: 0,
            eof: false,
        }
    }

    fn take(&mut self, mut bytes: &[u8]) {
        while let Some(at) = bytes.iter().position(|&byte| byte == b'\n') {
            let (head, rest) = bytes.split_at(at);
            self.push(head);
            self.finish(true);
            bytes = rest.get(1..).unwrap_or_default();
        }
        self.push(bytes);
    }

    fn push(&mut self, bytes: &[u8]) {
        self.scan.feed(bytes);
        self.line.extend_from_slice(bytes);
    }

    fn finish(&mut self, newline: bool) {
        let deepest = std::mem::take(&mut self.scan).deepest;
        if deepest > MAX_READ_DEPTH {
            if let Refusal::Answer(answer) = refusal(&self.line, deepest) {
                self.ready.extend_from_slice(answer.as_bytes());
                self.ready.push(b'\n');
            }
        } else if self.ready.is_empty() {
            // The usual case: the reader has taken everything before this line, so the line itself
            // becomes what is ready, without a second copy of it.
            std::mem::swap(&mut self.ready, &mut self.line);
            if newline {
                self.ready.push(b'\n');
            }
        } else {
            self.ready.extend_from_slice(&self.line);
            if newline {
                self.ready.push(b'\n');
            }
        }
        self.line.clear();
        release(&mut self.line);
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for DepthGuard<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        loop {
            if this.handed < this.ready.len() {
                let pending = this.ready.get(this.handed..).unwrap_or_default();
                let n = pending.len().min(buf.remaining());
                buf.put_slice(pending.get(..n).unwrap_or_default());
                this.handed += n;
                if this.handed == this.ready.len() {
                    this.ready.clear();
                    release(&mut this.ready);
                    this.handed = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.eof {
                if this.line.is_empty() {
                    return Poll::Ready(Ok(()));
                }
                this.finish(false);
                continue;
            }
            let mut chunk = [0_u8; 8 * 1024];
            let mut read = ReadBuf::new(&mut chunk);
            match Pin::new(&mut this.inner).poll_read(cx, &mut read) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => {
                    let filled = read.filled();
                    if filled.is_empty() {
                        this.eof = true;
                    } else {
                        this.take(filled);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A large shallow line passes through unchanged, and once it has been read the guard holds no
    /// more than [`RETAINED_CAPACITY`] in either buffer: a stdio server that once returned a 4 MiB
    /// screenshot does not keep 8 MiB of the guard's for the life of the connection.
    #[tokio::test]
    async fn a_large_line_passes_through_and_is_not_held_after_it_is_read() {
        use tokio::io::AsyncReadExt as _;
        let big = format!("{{\"v\":\"{}\"}}\n", "x".repeat(4 * 1024 * 1024));
        let input = format!("{big}{{\"after\":1}}\n");
        let mut guard = DepthGuard::new(input.as_bytes());
        let mut out = Vec::new();
        guard.read_to_end(&mut out).await.unwrap();
        assert_eq!(
            out,
            input.as_bytes(),
            "a shallow line is passed through byte for byte"
        );
        assert!(
            guard.line.capacity() <= RETAINED_CAPACITY,
            "the line buffer kept {} bytes",
            guard.line.capacity()
        );
        assert!(
            guard.ready.capacity() <= RETAINED_CAPACITY,
            "the ready buffer kept {} bytes",
            guard.ready.capacity()
        );
    }
    use rmcp::model::{JsonRpcMessage, ServerJsonRpcMessage};
    use tokio::io::AsyncBufReadExt as _;

    /// A `tools/call` reply whose `structuredContent` nests until the whole message is `depth`
    /// arrays and objects deep.
    fn reply(id: u64, depth: usize) -> String {
        // The message, `result` and `structuredContent` are three objects; the rest are arrays.
        let arrays = depth.saturating_sub(3);
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"content":[],"structuredContent":{{"v":{}1{}}}}}}}"#,
            "[".repeat(arrays),
            "]".repeat(arrays),
        )
    }

    /// The guard's limit is serde_json's, measured rather than assumed: every reply rmcp's own
    /// parse reads passes, and every one it cannot is refused. Moving [`MAX_READ_DEPTH`] either way
    /// fails a row.
    #[test]
    fn the_guard_refuses_exactly_what_serde_json_cannot_read() {
        for depth in 120..=136 {
            let message = reply(1, depth);
            assert_eq!(nesting_depth(message.as_bytes()), depth, "fixture depth");
            let parses = serde_json::from_str::<ServerJsonRpcMessage>(&message).is_ok();
            assert_eq!(
                refuse(message.as_bytes()).is_none(),
                parses,
                "depth {depth}: rmcp {} it",
                if parses { "reads" } else { "cannot read" }
            );
        }
    }

    #[test]
    fn brackets_inside_strings_are_not_nesting() {
        let text = br#"{"a":"[[[{{{\"]]]","b":["\\",{"c":"}}"}]}"#;
        assert_eq!(nesting_depth(text), 3);
    }

    /// A refused reply becomes an error for the same request, saying how deep it was.
    #[test]
    fn a_refused_reply_fails_its_own_request_and_says_why() {
        for id in [r#"7"#, r#""req-7""#] {
            let message = reply(0, 200).replacen(r#""id":0"#, &format!(r#""id":{id}"#), 1);
            let Some(Refusal::Answer(answer)) = refuse(message.as_bytes()) else {
                panic!("a reply with an id is answered");
            };
            let JsonRpcMessage::Error(error) =
                serde_json::from_str::<ServerJsonRpcMessage>(&answer).unwrap()
            else {
                panic!("the answer is a JSON-RPC error: {answer}");
            };
            assert_eq!(
                serde_json::to_value(&error.id).unwrap(),
                serde_json::from_str::<Value>(id).unwrap()
            );
            assert_eq!(error.error.code.0, PARSE_ERROR);
            assert_eq!(error.error.message, refusal_message(200));
            assert!(error.error.message.contains("200 levels"));
            assert!(error.error.message.contains("127 levels"));
        }
    }

    /// Far past serde_json's limit — and past any stack — the probe still finds the id.
    #[test]
    fn the_id_is_found_however_deep_the_reply_nests() {
        let message = reply(9, 200_000);
        assert!(matches!(
            refuse(message.as_bytes()),
            Some(Refusal::Answer(answer)) if answer.contains(r#""id":9"#)
        ));
    }

    #[test]
    fn a_deep_server_request_or_notification_is_dropped() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"sampling/createMessage","params":{deep}}}"#
        );
        let notification =
            format!(r#"{{"jsonrpc":"2.0","method":"notifications/message","params":{deep}}}"#);
        let no_id = format!(r#"{{"jsonrpc":"2.0","result":{deep}}}"#);
        for message in [request, notification, no_id] {
            assert_eq!(
                refuse(message.as_bytes()),
                Some(Refusal::Drop),
                "{message:.60}"
            );
        }
    }

    /// Lines arrive split across reads and several to a read; each is measured whole, the shallow
    /// ones pass through byte for byte and the deep reply is answered in its place.
    #[tokio::test]
    async fn the_reader_answers_a_deep_line_and_passes_the_rest_through() {
        let shallow = reply(1, 5);
        let deep = reply(2, 300);
        let notice = r#"{"jsonrpc":"2.0","method":"notifications/message","params":{"x":"[[["}}"#;
        let stream = format!("{shallow}\n{deep}\r\n{notice}\n{shallow}");
        let (mut tx, rx) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            for piece in stream.as_bytes().chunks(37) {
                tx.write_all(piece).await.unwrap();
            }
        });
        let mut lines = tokio::io::BufReader::new(DepthGuard::new(rx)).lines();
        let mut read = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            read.push(line);
        }
        writer.await.unwrap();

        assert_eq!(read.len(), 4, "{read:?}");
        assert_eq!(read.first(), Some(&shallow));
        let answer = read.get(1).unwrap();
        assert!(
            answer.contains(r#""id":2"#) && answer.contains("300 levels"),
            "{answer}"
        );
        assert_eq!(read.get(2).map(String::as_str), Some(notice));
        assert_eq!(
            read.get(3),
            Some(&shallow),
            "a last line without a newline still arrives"
        );
    }

    #[test]
    fn an_sse_event_is_answered_in_place_and_keeps_its_id() {
        let event = Sse::default().id("e1").data(reply(4, 150));
        let guarded = guard_sse_event(event);
        assert_eq!(guarded.id.as_deref(), Some("e1"));
        assert!(guarded.data.unwrap().contains("150 levels"));

        let shallow = reply(4, 10);
        let untouched = guard_sse_event(Sse::default().data(shallow.clone()));
        assert_eq!(untouched.data, Some(shallow));
    }

    #[test]
    fn a_json_body_is_read_answered_or_left_for_rmcp_to_accept() {
        assert!(matches!(
            read_json_body(reply(5, 10).as_bytes()),
            Some(JsonRpcMessage::Response(_))
        ));
        assert!(matches!(
            read_json_body(reply(5, 140).as_bytes()),
            Some(JsonRpcMessage::Error(_))
        ));
        assert!(read_json_body(b"not json").is_none());
    }
}
