//! The pipe between an execution's supervisor and the sandbox process that holds its isolate.
//!
//! The same messages [`super::protocol`] defines for the isolate thread cross a pipe instead of a
//! channel: one frame per line, a JSON document behind [`FRAME_MARKER`]. Lines without the marker
//! are not frames and are skipped, so whatever else reaches the pipe (a test harness's banner,
//! a stray print) cannot be taken for a message. The reader bounds a frame at
//! [`MAX_FRAME_BYTES`]: the process on the other end runs a model's script, and a length the
//! reader trusted would be a way to make the supervisor allocate without limit.

use std::io::{self, BufRead, Read, Write};

use cyrup_codemode::types::OutputItem;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::protocol::{CallTarget, HostReply, IsolateInit, ScriptSettled, WorkerMessage};

/// What starts a line that carries a frame.
pub(super) const FRAME_MARKER: &str = "\u{1}cyrup-codemode-sandbox\u{1}";

/// Largest frame either side accepts. A script's return value is bounded by the isolate's heap
/// (256 MiB in the tool) and JSON escaping at most doubles the characters it can hold.
pub(super) const MAX_FRAME_BYTES: usize = 512 * 1024 * 1024;

/// The most a frame buffer keeps between frames; one that grew past it is released after its frame.
const KEPT_SCRATCH_BYTES: usize = 1024 * 1024;

/// Supervisor to sandbox process.
#[derive(Debug, Serialize, Deserialize)]
pub(super) enum ToProcess {
    /// The first frame: what the isolate runs.
    Start(IsolateInit),
    /// A tool call's outcome.
    Reply(HostReply),
}

/// Sandbox process to supervisor: [`WorkerMessage`], with the output item spelled out, plus the
/// one thing only a dying process can say.
#[derive(Debug, Serialize, Deserialize)]
pub(super) enum FromProcess {
    Call {
        id: u32,
        target: CallTarget,
        name: String,
        args: Option<CallArgs>,
    },
    Text(String),
    /// One `console.*` line (`OutputItem::Console`, pi `eb326d265` @v1.1.0).
    Console(String),
    Image {
        data: String,
        mime_type: String,
    },
    Done(ScriptSettled),
    Crash(String),
    /// The engine reported a fatal out-of-memory, which no script and no host code can survive;
    /// the process is about to abort.
    OutOfMemory,
    /// The script used up its active-time limit.
    ActiveLimit,
}

/// A call's arguments on the pipe: the JSON text the prelude wrote, carried as JSON inside the frame
/// rather than as a string of it (CODE-053). A string would be escaped on one side and unescaped on
/// the other, which the reader does in a buffer of its own before it copies the result out: three
/// copies of the arguments at once in the host (the frame, that buffer and the text), where JSON
/// inside the frame is copied out once, as it is. `Text` carries what is not one JSON value, which
/// the prelude never writes; the host then refuses the call as it always did.
#[derive(Debug, Serialize, Deserialize)]
pub(super) enum CallArgs {
    Json(Box<RawValue>),
    Text(String),
}

impl CallArgs {
    fn new(text: String) -> Self {
        if serde_json::from_str::<&RawValue>(&text).is_ok() {
            match RawValue::from_string(text) {
                Ok(json) => Self::Json(json),
                // Not reached: the same text was just read as one JSON value.
                Err(error) => Self::Text(error.to_string()),
            }
        } else {
            Self::Text(text)
        }
    }

    fn into_text(self) -> String {
        match self {
            Self::Json(json) => Box::<str>::from(json).into_string(),
            Self::Text(text) => text,
        }
    }
}

impl From<WorkerMessage> for FromProcess {
    fn from(message: WorkerMessage) -> Self {
        match message {
            WorkerMessage::Call {
                id,
                target,
                name,
                args,
            } => Self::Call {
                id,
                target,
                name,
                args: args.map(CallArgs::new),
            },
            WorkerMessage::Output(OutputItem::Text(text)) => Self::Text(text),
            WorkerMessage::Output(OutputItem::Console(text)) => Self::Console(text),
            WorkerMessage::Output(OutputItem::Image { data, mime_type }) => {
                Self::Image { data, mime_type }
            }
            WorkerMessage::Done(settled) => Self::Done(settled),
            WorkerMessage::Crash(message) => Self::Crash(message),
            WorkerMessage::OutOfMemory => Self::OutOfMemory,
            WorkerMessage::ActiveLimit => Self::ActiveLimit,
        }
    }
}

impl From<FromProcess> for WorkerMessage {
    fn from(frame: FromProcess) -> Self {
        match frame {
            FromProcess::Call {
                id,
                target,
                name,
                args,
            } => Self::Call {
                id,
                target,
                name,
                args: args.map(CallArgs::into_text),
            },
            FromProcess::Text(text) => Self::Output(OutputItem::Text(text)),
            FromProcess::Console(text) => Self::Output(OutputItem::Console(text)),
            FromProcess::Image { data, mime_type } => {
                Self::Output(OutputItem::Image { data, mime_type })
            }
            FromProcess::Done(settled) => Self::Done(settled),
            FromProcess::Crash(message) => Self::Crash(message),
            FromProcess::OutOfMemory => Self::OutOfMemory,
            FromProcess::ActiveLimit => Self::ActiveLimit,
        }
    }
}

/// Writes one frame and flushes it.
pub(super) fn write_frame<T: Serialize>(out: &mut impl Write, frame: &T) -> io::Result<()> {
    let mut line = Vec::with_capacity(256);
    line.extend_from_slice(FRAME_MARKER.as_bytes());
    serde_json::to_writer(&mut line, frame).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// Reads the next frame; `None` at the end of the pipe. `scratch` is reused between calls.
pub(super) fn read_frame<T: DeserializeOwned>(
    input: &mut impl BufRead,
    scratch: &mut Vec<u8>,
) -> io::Result<Option<T>> {
    read_frame_bounded(input, scratch, MAX_FRAME_BYTES)
}

fn read_frame_bounded<T: DeserializeOwned>(
    input: &mut impl BufRead,
    scratch: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<Option<T>> {
    loop {
        scratch.clear();
        // One byte past the bound, so a line that fills it is told from one that ends there.
        let limit = u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX);
        let read = input.by_ref().take(limit).read_until(b'\n', scratch)?;
        if read == 0 {
            return Ok(None);
        }
        if scratch.len() > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a frame is longer than {max_bytes} bytes"),
            ));
        }
        let Some(body) = scratch.strip_prefix(FRAME_MARKER.as_bytes()) else {
            continue;
        };
        let frame = serde_json::from_slice(body.strip_suffix(b"\n").unwrap_or(body))
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        // A buffer grown by one large frame (a call's arguments, a reply) would otherwise keep its
        // size for as long as the pipe is read: a copy of that frame for the rest of the execution
        // (CODE-053).
        if scratch.capacity() > KEPT_SCRATCH_BYTES {
            *scratch = Vec::new();
        }
        return frame;
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn frames(bytes: &[u8]) -> Vec<io::Result<Option<FromProcess>>> {
        let mut input = bytes;
        let mut scratch = Vec::new();
        let mut read = Vec::new();
        loop {
            let next = read_frame::<FromProcess>(&mut input, &mut scratch);
            let end = !matches!(next, Ok(Some(_)));
            read.push(next);
            if end {
                return read;
            }
        }
    }

    #[test]
    fn a_frame_survives_the_pipe_and_other_lines_are_skipped() {
        let mut pipe = b"running 1 test\n".to_vec();
        write_frame(&mut pipe, &FromProcess::Text("a\nb \u{1}".into())).unwrap();
        pipe.extend_from_slice(b"noise without a marker\n");
        write_frame(&mut pipe, &FromProcess::OutOfMemory).unwrap();
        let read = frames(&pipe);
        assert!(matches!(&read[0], Ok(Some(FromProcess::Text(text))) if text == "a\nb \u{1}"));
        assert!(matches!(&read[1], Ok(Some(FromProcess::OutOfMemory))));
        assert!(matches!(&read[2], Ok(None)));
    }

    #[test]
    fn a_frame_longer_than_the_bound_is_refused_without_being_read_to_its_end() {
        let mut pipe = Vec::new();
        write_frame(&mut pipe, &FromProcess::Text("x".repeat(1000))).unwrap();
        let mut input = pipe.as_slice();
        let mut scratch = Vec::new();
        let error = read_frame_bounded::<FromProcess>(&mut input, &mut scratch, 100).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("longer than 100 bytes"),
            "{error}"
        );
        // Nothing past the bound was buffered.
        assert!(scratch.len() <= 101, "{}", scratch.len());
        // The bound is on the line, marker and newline included.
        let mut pipe = Vec::new();
        write_frame(&mut pipe, &FromProcess::OutOfMemory).unwrap();
        let exact = pipe.len();
        assert!(
            read_frame_bounded::<FromProcess>(&mut pipe.as_slice(), &mut scratch, exact).is_ok()
        );
        assert!(
            read_frame_bounded::<FromProcess>(&mut pipe.as_slice(), &mut scratch, exact - 1)
                .is_err()
        );
    }

    /// `CODE-053`: a call's arguments cross as JSON inside the frame, not as a JSON string of their
    /// text. As a string they were escaped by the process and unescaped by the host into a buffer
    /// of the reader's own before being copied out, three copies at once in the host; as JSON they
    /// are copied out once. Text that is not one JSON value still crosses, as text.
    #[test]
    fn a_calls_arguments_cross_as_json_and_arrive_as_their_text() {
        for (text, carried) in [
            (
                r#"{"a":"x\"y","b":[1,2.5]}"#,
                r#""args":{"Json":{"a":"x\"y","b":[1,2.5]}}"#,
            ),
            ("{not json", r#""args":{"Text":"{not json"}"#),
        ] {
            let mut pipe = Vec::new();
            let call = WorkerMessage::Call {
                id: 3,
                target: CallTarget::Tool,
                name: String::from("t"),
                args: Some(text.to_owned()),
            };
            write_frame(&mut pipe, &FromProcess::from(call)).unwrap();
            let line = String::from_utf8(pipe.clone()).unwrap();
            assert!(line.contains(carried), "{line}");
            let mut scratch = Vec::new();
            let read = read_frame::<FromProcess>(&mut pipe.as_slice(), &mut scratch).unwrap();
            let Some(WorkerMessage::Call { args, .. }) = read.map(WorkerMessage::from) else {
                panic!("not a call: {line}");
            };
            assert_eq!(args.as_deref(), Some(text));
        }
    }

    /// `CODE-053`: the reader's buffer does not keep the size one large frame gave it; it held a
    /// copy of a call's arguments for the rest of the execution.
    #[test]
    fn a_buffer_grown_by_a_large_frame_is_released_after_it() {
        let mut pipe = Vec::new();
        write_frame(
            &mut pipe,
            &FromProcess::Text("x".repeat(4 * KEPT_SCRATCH_BYTES)),
        )
        .unwrap();
        write_frame(&mut pipe, &FromProcess::OutOfMemory).unwrap();
        let mut input = pipe.as_slice();
        let mut scratch = Vec::new();
        assert!(matches!(
            read_frame::<FromProcess>(&mut input, &mut scratch),
            Ok(Some(FromProcess::Text(_)))
        ));
        assert!(
            scratch.capacity() <= KEPT_SCRATCH_BYTES,
            "the buffer kept {} bytes",
            scratch.capacity()
        );
        // A small frame keeps its buffer, which is reused.
        assert!(matches!(
            read_frame::<FromProcess>(&mut input, &mut scratch),
            Ok(Some(FromProcess::OutOfMemory))
        ));
        assert!(scratch.capacity() > 0);
    }

    #[test]
    fn a_line_that_is_not_json_behind_the_marker_is_an_error() {
        let mut pipe = FRAME_MARKER.as_bytes().to_vec();
        pipe.extend_from_slice(b"{not json\n");
        assert!(
            matches!(&frames(&pipe)[0], Err(error) if error.kind() == io::ErrorKind::InvalidData)
        );
    }
}
