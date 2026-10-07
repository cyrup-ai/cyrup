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

/// A payload the prelude sent that the host cannot decode (`BridgeError`, `host.ts:49-64` @v1.0.4).
/// The prelude serializes in the script's own isolate, so a script that patched a built-in, for
/// example `Array.prototype.toJSON`, could make it send malformed data; the prelude now freezes the
/// built-ins first, and the execution still ends as a `sandbox` error if one gets through.
#[derive(Debug, thiserror::Error)]
#[error(
    "Sandbox bridge broken: {0}. The script may have modified built-ins such as a prototype's toJSON."
)]
pub(super) struct BridgeError(String);

impl BridgeError {
    fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// `parseBridgeJson` (`host.ts:55-61` @v1.0.4).
fn parse_bridge_json(json: &str, what: &str) -> Result<Value, BridgeError> {
    serde_json::from_str(json).map_err(|_| BridgeError::new(format!("{what} is not valid JSON")))
}

/// The error of a script that threw (`parseScriptError`, `host.ts:81-95` @v1.0.4). `message` must be
/// a string and `name` and `stack` strings when present, as `describeError` writes them.
pub(super) fn script_error(error_json: &str) -> Result<CodemodeError, BridgeError> {
    let parsed = parse_bridge_json(error_json, "script error")?;
    let Value::Object(map) = parsed else {
        return Err(BridgeError::new("script error is not an object"));
    };
    let malformed = || BridgeError::new("script error is malformed");
    let text = |key: &str| -> Result<Option<String>, BridgeError> {
        match map.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(malformed()),
        }
    };
    let Some(message) = text("message")? else {
        return Err(malformed());
    };
    Ok(CodemodeError {
        kind: ErrorKind::Script,
        name: text("name")?,
        message,
        stack: text("stack")?,
    })
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

/// `host.ts:66-79` `parseStoreWrites` @v1.0.4: an array of `[key]` (a delete) and `[key, json]` (a
/// set) entries, every member a string, every `json` valid JSON.
pub(super) fn store_writes(json: &str) -> Result<CodemodeStoreWrites, BridgeError> {
    let Value::Array(entries) = parse_bridge_json(json, "store writes")? else {
        return Err(BridgeError::new("store writes are not an array"));
    };
    let mut writes = CodemodeStoreWrites::default();
    for entry in entries {
        let malformed = || BridgeError::new("store writes contain a malformed entry");
        let Value::Array(parts) = entry else {
            return Err(malformed());
        };
        match parts.as_slice() {
            [Value::String(key)] => writes.delete.push(key.clone()),
            [Value::String(key), Value::String(text)] => {
                let what = format!("store value for {}", json_text(key));
                writes
                    .set
                    .insert(key.clone(), parse_bridge_json(text, &what)?);
            }
            _ => return Err(malformed()),
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
        )
        .unwrap();
        assert_eq!(error.kind, ErrorKind::Script);
        assert_eq!(error.name.as_deref(), Some("TypeError"));
        assert_eq!(error.message, "boom 1");
        assert!(error.stack.unwrap().contains("codemode.js:2"));
        let bare = script_error(r#"{"message":"{\"code\":7}"}"#).unwrap();
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

    /// pi's `reports a broken bridge as a sandbox error` table (`sandbox.test.ts` @v1.0.4), the cases a
    /// typed op can still carry: the reason each malformed payload is refused for.
    #[test]
    fn a_malformed_payload_is_refused_with_the_reason_upstream_names() {
        let reason = |result: Result<_, BridgeError>| -> String {
            match result {
                Ok(_) => String::from("accepted"),
                Err(error) => error.to_string(),
            }
        };
        let broken = |what: &str| {
            format!(
                "Sandbox bridge broken: {what}. The script may have modified built-ins such as a prototype's toJSON."
            )
        };
        assert_eq!(
            reason(store_writes("null").map(drop)),
            broken("store writes are not an array")
        );
        for entries in [
            "[1]",
            "[[]]",
            r#"[["a","1","extra"]]"#,
            r#"[[1,"1"]]"#,
            r#"[["a",1]]"#,
        ] {
            assert_eq!(
                reason(store_writes(entries).map(drop)),
                broken("store writes contain a malformed entry"),
                "{entries}"
            );
        }
        assert_eq!(
            reason(store_writes(r#"[["k","{"]]"#).map(drop)),
            broken("store value for \"k\" is not valid JSON")
        );
        assert_eq!(
            reason(script_error("5").map(drop)),
            broken("script error is not an object")
        );
        assert_eq!(
            reason(script_error("{").map(drop)),
            broken("script error is not valid JSON")
        );
        for malformed in [
            "{}",
            r#"{"message":5}"#,
            r#"{"message":"m","name":null}"#,
            r#"{"message":"m","stack":1}"#,
        ] {
            assert_eq!(
                reason(script_error(malformed).map(drop)),
                broken("script error is malformed"),
                "{malformed}"
            );
        }
    }

    #[test]
    fn store_snapshot_is_key_to_json_text() {
        let mut store = Map::new();
        store.insert("counter".into(), serde_json::json!(41));
        store.insert("old".into(), serde_json::json!("x"));
        assert_eq!(store_json(&store), r#"{"counter":"41","old":"\"x\""}"#);
    }
}
