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

use super::protocol::{CallTarget, HostReply, IsolateInit, ScriptSettled, WorkerMessage};

/// What starts a line that carries a frame.
pub(super) const FRAME_MARKER: &str = "\u{1}cyrup-codemode-sandbox\u{1}";

/// Largest frame either side accepts. A script's return value is bounded by the isolate's heap
/// (256 MiB in the tool) and JSON escaping at most doubles the characters it can hold.
pub(super) const MAX_FRAME_BYTES: usize = 512 * 1024 * 1024;

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
        args: Option<String>,
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
                args,
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
                args,
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
        return serde_json::from_slice(body.strip_suffix(b"\n").unwrap_or(body))
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

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

    #[test]
    fn a_line_that_is_not_json_behind_the_marker_is_an_error() {
        let mut pipe = FRAME_MARKER.as_bytes().to_vec();
        pipe.extend_from_slice(b"{not json\n");
        assert!(
            matches!(&frames(&pipe)[0], Err(error) if error.kind() == io::ErrorKind::InvalidData)
        );
    }
}
