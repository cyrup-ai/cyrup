//! What crosses between an execution's supervisor (on the caller's runtime) and its isolate thread
//! (pi `runtime/protocol.ts` @v1.0.1).
//!
//! Upstream passes everything as JSON strings so the worker never builds structured values; so
//! does this. The functions here are the pure half of that boundary: they turn the strings the
//! isolate reports into [`CodemodeResult`] parts, and the host's values into the strings it
//! reads, with no engine and no I/O.

use cyrup_codemode::identifier::to_codemode_identifier;
use cyrup_codemode::types::OutputItem;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::types::{CodemodeError, CodemodeStoreWrites, CodemodeTool, ErrorKind};

/// Which table a call from the script is looked up in (`protocol.ts:31` `target`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallTarget {
    /// `tools.<name>(args)`; recorded in the result's `calls`.
    Tool,
    /// A configured global; not recorded.
    Global,
}

/// Isolate thread to supervisor (`protocol.ts` `WorkerToHostMessage`).
#[derive(Debug)]
pub(super) enum WorkerMessage {
    Call {
        id: u32,
        target: CallTarget,
        name: String,
        /// JSON text; `None` is `undefined`.
        args: Option<String>,
    },
    Output(OutputItem),
    /// The script settled. Sent at most once.
    Done(ScriptSettled),
    /// The engine failed outside the script's control (`protocol.ts` `crash`).
    Crash(String),
}

/// How the script ended, as the isolate reports it.
#[derive(Debug)]
pub(super) enum ScriptSettled {
    Returned {
        /// JSON text of the return value; `None` is `undefined`.
        value: Option<String>,
        /// JSON array of `[key, json]` for `store()` and `[key]` for deletions.
        writes: String,
    },
    /// JSON `{ name?, message, stack? }`.
    Threw(String),
}

/// Supervisor to isolate thread (`protocol.ts` `HostToWorkerMessage`): a tool call's outcome.
#[derive(Debug)]
pub(super) struct HostReply {
    pub id: u32,
    pub payload: ReplyPayload,
}

#[derive(Debug)]
pub(super) enum ReplyPayload {
    /// JSON text of the result; `None` is `undefined`.
    Value(Option<String>),
    /// The message of the `Error` the script's call rejects with.
    Failure(String),
}

/// Everything one isolate needs to run one script (`protocol.ts` `WorkerData`).
#[derive(Debug)]
pub(super) struct IsolateInit {
    pub code: String,
    pub tools_json: String,
    pub globals_json: String,
    pub store_json: String,
    pub memory_limit: Option<usize>,
}

#[derive(Serialize)]
struct ToolEntry<'a> {
    name: &'a str,
    #[serde(rename = "jsName")]
    js_name: String,
    description: &'a str,
}

#[derive(Serialize)]
struct GlobalEntry<'a> {
    name: &'a str,
    spread: bool,
}

/// The smallest heap limit an isolate can be given. V8 bootstraps the engine's own objects inside
/// the limit (measured: between 6 and 8 MiB), and a limit below that is not an error V8 reports
/// but a fatal out-of-memory that aborts the whole process. A smaller `memory_limit_bytes` is
/// therefore raised to this.
pub const MIN_MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// The limit an isolate runs under for the `memory_limit_bytes` the sandbox was given.
pub(super) fn effective_memory_limit(bytes: u64) -> usize {
    usize::try_from(bytes)
        .unwrap_or(usize::MAX)
        .max(MIN_MEMORY_LIMIT_BYTES)
}

/// `[{ name, jsName, description }]` for the prelude (`host.ts:139-143`).
pub(super) fn tools_json(tools: &[CodemodeTool]) -> String {
    let entries: Vec<ToolEntry<'_>> = tools
        .iter()
        .map(|tool| ToolEntry {
            name: &tool.declaration.name,
            js_name: to_codemode_identifier(&tool.declaration.name).into_string(),
            description: tool.declaration.description.as_deref().unwrap_or(""),
        })
        .collect();
    json_text(&entries)
}

/// `[{ name, spread }]` for the prelude (`host.ts:144-147`).
pub(super) fn globals_json(globals: &[CodemodeTool]) -> String {
    let entries: Vec<GlobalEntry<'_>> = globals
        .iter()
        .map(|global| GlobalEntry {
            name: &global.declaration.name,
            spread: global.declaration.spread,
        })
        .collect();
    json_text(&entries)
}

/// The snapshot `load()` reads: key to JSON text (`host.ts:40-47` `serializeStore`). A JSON value
/// is never `undefined`, so every key is kept.
pub(super) fn store_json(store: &Map<String, Value>) -> String {
    let serialized: Map<String, Value> = store
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(json_text(value))))
        .collect();
    json_text(&serialized)
}

fn json_text<T: Serialize + ?Sized>(value: &T) -> String {
    // A `Value`, a map of strings and these entry structs always serialize.
    serde_json::to_string(value).unwrap_or_else(|_| String::from("null"))
}

/// The error of a script that threw (`host.ts:201-206` `handleDone`). `name` and `stack` are kept
/// only when they are strings, as `describeError` writes them.
pub(super) fn script_error(error_json: &str) -> CodemodeError {
    let parsed: Option<Map<String, Value>> = serde_json::from_str(error_json).ok();
    let field = |key: &str| {
        parsed
            .as_ref()
            .and_then(|map| map.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    CodemodeError {
        kind: ErrorKind::Script,
        name: field("name"),
        message: field("message").unwrap_or_default(),
        stack: field("stack"),
    }
}

/// The error report of a script that ran out of its memory budget. Upstream's engine throws
/// `InternalError: out of memory` into the script, which is uncaught here and so reaches the
/// prelude's `describeError`: `name`, `message`, and a stack that is the head alone because the
/// engine unwinds the script without running any of it.
pub(super) fn out_of_memory_json() -> String {
    json_text(&serde_json::json!({
        "name": "InternalError",
        "message": "out of memory",
        "stack": "InternalError: out of memory",
    }))
}

/// Why a store-writes report could not be read; the prelude writes it, so this is an engine fault.
#[derive(Debug, thiserror::Error)]
#[error("The script's store writes could not be read: {0}")]
pub(super) struct StoreWritesError(String);

/// `host.ts:49-56` `parseStoreWrites`.
pub(super) fn store_writes(json: &str) -> Result<CodemodeStoreWrites, StoreWritesError> {
    let entries: Vec<Vec<String>> =
        serde_json::from_str(json).map_err(|error| StoreWritesError(error.to_string()))?;
    let mut writes = CodemodeStoreWrites::default();
    for entry in entries {
        let mut parts = entry.into_iter();
        let Some(key) = parts.next() else { continue };
        match parts.next() {
            None => writes.delete.push(key),
            Some(text) => {
                let value = serde_json::from_str(&text)
                    .map_err(|error| StoreWritesError(error.to_string()))?;
                writes.set.insert(key, value);
            }
        }
    }
    Ok(writes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn script_error_reads_the_prelude_shape() {
        let error = script_error(
            r#"{"name":"TypeError","message":"boom 1","stack":"TypeError: boom 1\n    at codemode.js:2:7"}"#,
        );
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("TypeError"));
        assert_eq!(error.message, "boom 1");
        assert!(error.stack.unwrap().contains("codemode.js:2"));
        let bare = script_error(r#"{"message":"{\"code\":7}"}"#);
        assert_eq!((bare.name, bare.stack), (None, None));
        assert_eq!(bare.message, r#"{"code":7}"#);
    }

    #[test]
    fn store_writes_separate_sets_from_deletes() {
        let writes = store_writes(r#"[["a","1"],["gone"],["b","{\"x\":[null]}"]]"#).unwrap();
        assert_eq!(writes.delete, vec!["gone".to_owned()]);
        assert_eq!(writes.set.get("a"), Some(&serde_json::json!(1)));
        assert_eq!(writes.set.get("b"), Some(&serde_json::json!({"x": [null]})));
    }

    #[test]
    fn store_snapshot_is_key_to_json_text() {
        let mut store = Map::new();
        store.insert("counter".into(), serde_json::json!(41));
        store.insert("old".into(), serde_json::json!("x"));
        assert_eq!(store_json(&store), r#"{"counter":"41","old":"\"x\""}"#);
    }
}
