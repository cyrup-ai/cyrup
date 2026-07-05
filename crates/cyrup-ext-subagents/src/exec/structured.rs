//! Parent-side structured-output extraction and JSON-Schema re-validation (func-SA §5.2 R-SA-030;
//! arch-SA §6.3.3/§12 item 13).
//!
//! # Scope
//!
//! This module owns exactly two algorithms, both operating strictly on already-parsed/already-
//! collected data — it spawns nothing itself and owns no subprocess lifecycle, mirroring
//! `exec/output.rs`'s own scope discipline:
//!
//! 1. [`extract_structured_output_value`] — recover the child's structured-output JSON value from
//!    its NDJSON event stream. As of this crate's build-out, there is no dedicated wire event or
//!    env-var/file-handoff channel a child uses to emit a structured-output value (arch-SA §12
//!    open question 6: "Structured-output capture mechanism ... is unspecified at the
//!    CLI-flag/env-var level"); the only channel this crate can observe today is the same
//!    `MessageEnd` text-content stream [`crate::exec::output::extract_final_output`] (R-SA-029)
//!    already folds. This function therefore reuses the exact same reverse-chronological,
//!    most-recent-message-first scan (and [`crate::exec::output::fenced_blocks`]'s shared fence
//!    scanner) to recover a fenced ` ```json `/`jsonc`/`json5` block from the winning message and
//!    parse ITS body as one JSON value — the structured-output analogue of R-SA-029's own
//!    acceptance-report-shaped-block detection, over the identical wire bytes.
//! 2. [`validate_structured_output`] — compile the task's declared JSON Schema and check the
//!    extracted value against it via the `jsonschema` crate (arch-SA §12 item 13's resolved crate
//!    choice — see the workspace `Cargo.toml`'s own comment for why `jsonschema`, not `schemars`,
//!    is correct here), returning a human-readable validation-error message on failure rather than
//!    a boolean, so [`crate::exec::run_sync`]'s caller sees exactly why the run was rejected.
//!
//! [`resolve_structured_output`] composes both steps into the single entry point `run_sync` calls,
//! implementing R-SA-030's full contract: absence when a schema is declared is a hard failure
//! (unless plain text was also produced as a fallback — the fallback exemption is `run_sync`'s own
//! concern via its already-separate "produced no output at all" check, not this module's); a
//! present-but-invalid value is also a hard failure; a present-and-valid value populates
//! `SingleResult::structured_output`. When no schema was declared at all, this module is a pure
//! no-op — `run_sync` never even calls into it in that case (mirrored by
//! [`resolve_structured_output`] returning [`StructuredOutcome::NotRequested`] defensively even if
//! it is called anyway, so a caller cannot mis-order this check ahead of the "was a schema even
//! declared" gate without still getting the correct, harmless outcome).
//!
//! This module has ZERO dependency on `cyrup-agent` — every message/content shape it inspects is
//! the same opaque `serde_json::Value` [`crate::exec::ndjson::SubagentEvent`] already exposes,
//! never a typed `AgentMessage`/`Content` re-import (arch-SA §2.1/§1.1, restated at every module
//! boundary in this crate, identical to `exec/output.rs`'s own module doc).

use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::fenced_blocks;

/// The three fenced-code-block language tags this module (and R-SA-029's acceptance-report
/// detection) both treat as "this fenced body is meant to be parsed as JSON" — mirrors
/// `output::looks_like_acceptance_report`'s own `"json" | "jsonc" | "json5"` set exactly, so a
/// child that fences its structured-output value under any of these three conventional tags is
/// recognized identically by both R-SA-029 and R-SA-030's extraction paths.
const JSON_FENCE_LANGS: &[&str] = &["json", "jsonc", "json5"];

/// R-SA-030 (extraction half): recover the child's structured-output JSON value from `events` (the
/// same chronologically ordered [`SubagentEvent::MessageEnd`] slice
/// [`crate::exec::output::extract_final_output`] scans), or `None` if no non-error assistant
/// message contains a parseable fenced JSON block.
///
/// Mirrors [`crate::exec::output::extract_final_output`]'s reverse-scan priority exactly: the most
/// recent non-error-flagged assistant `MessageEnd` wins; within that message's content parts (in
/// original order), the FIRST fenced `json`/`jsonc`/`json5` block whose body parses as valid JSON
/// is the returned value. A message containing no parseable fenced JSON block does NOT fall
/// through to an OLDER message — exactly like R-SA-029's own "message recency is the outer
/// priority level" rule, restated here: an older message's structured-output block must never be
/// preferred over a newer message that simply chose not to emit one at all (that newer message's
/// absence is what R-SA-030 classifies as "structured output absent", not a reason to keep
/// searching backward past it).
#[must_use]
pub fn extract_structured_output_value(events: &[SubagentEvent]) -> Option<serde_json::Value> {
    for event in events.iter().rev() {
        let SubagentEvent::MessageEnd { message } = event else {
            continue;
        };
        if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        if event.is_error_or_aborted_message() {
            continue;
        }

        let Some(content) = message.get("content").and_then(serde_json::Value::as_array) else {
            continue;
        };

        for part in content {
            let is_text = part.get("type").and_then(serde_json::Value::as_str) == Some("text");
            if !is_text {
                continue;
            }
            let Some(text) = part.get("text").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if let Some(value) = first_parseable_json_fence(text) {
                return Some(value);
            }
        }

        // This (most recent, non-error) message had text content but no parseable fenced JSON
        // block — per this function's own doc comment, this is "absent", not a cue to keep
        // scanning further back for an older message's block.
        return None;
    }
    None
}

/// Scan `text`'s fenced blocks (via [`fenced_blocks`]) in original order and return the first one
/// tagged `json`/`jsonc`/`json5` whose body parses as valid JSON. A fenced block with a matching
/// language tag but an unparseable body is skipped (not an error) — this function's contract is
/// "find A block that actually is JSON", matching pi-subagents' own tolerant read-back (a malformed
/// body degrades to "missing", never a panic/hard-error at the extraction step; validation-shaped
/// errors are [`validate_structured_output`]'s job, not this scan's).
fn first_parseable_json_fence(text: &str) -> Option<serde_json::Value> {
    fenced_blocks(text).into_iter().find_map(|block| {
        let lang = block.lang.to_ascii_lowercase();
        if !JSON_FENCE_LANGS.contains(&lang.as_str()) {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(block.body).ok()
    })
}

// ============================================================================================
// R-SA-030 (validation half): compiled JSON-Schema check via the `jsonschema` crate
// ============================================================================================

/// R-SA-030 (validation half): compile `schema` and check `value` against it via `jsonschema`
/// (arch-SA §12 item 13). Returns `Ok(())` on success; on failure, returns a human-readable message
/// combining every violation (bounded to a small number so one hugely-nested schema cannot produce
/// an unbounded error string), each prefixed with its JSON-Pointer-style instance path — mirroring
/// pi-subagents' own `validateStructuredOutputValue`
/// (`pi-subagents/src/runs/shared/structured-output.ts:38-53`) in spirit: "root" for the top-level
/// instance, `a.b.c`-style dotted paths for nested violations, multiple violations joined with
/// `"; "`.
///
/// A malformed `schema` itself (fails to compile as a JSON Schema) is also reported through this
/// same `Err(String)` return — never a panic, never a silently-ignored no-op validation — since an
/// orchestrator-side schema authoring mistake must fail the run exactly as loudly as a genuine
/// child-side structured-output mismatch (both are "this run cannot be trusted to have produced
/// the declared shape").
///
/// # Errors
///
/// Returns `Err(message)` if `schema` fails to compile, or if `value` fails to validate against
/// it.
pub fn validate_structured_output(
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<(), String> {
    let validator = jsonschema::Validator::new(schema)
        .map_err(|err| format!("invalid structured-output schema: {err}"))?;

    if validator.is_valid(value) {
        return Ok(());
    }

    const MAX_REPORTED_ERRORS: usize = 8;
    let messages: Vec<String> = validator
        .iter_errors(value)
        .take(MAX_REPORTED_ERRORS)
        .map(|err| {
            let path = err.instance_path().to_string();
            let path = path.trim_start_matches('/').replace('/', ".");
            let path = if path.is_empty() {
                "root".to_string()
            } else {
                path
            };
            format!("{path}: {err}")
        })
        .collect();

    let joined = if messages.is_empty() {
        "schema validation failed".to_string()
    } else {
        messages.join("; ")
    };
    Err(format!("structured output validation failed: {joined}"))
}

// ============================================================================================
// resolve_structured_output: the single R-SA-030 entry point run_sync calls
// ============================================================================================

/// The outcome of [`resolve_structured_output`] — deliberately not a bare `Result` since "no
/// schema was declared at all" is a distinct, non-error case from both success and failure, and
/// `run_sync` needs to branch on all three (R-SA-030's own text: absence is a hard failure ONLY
/// "if the task declares a structured-output schema").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredOutcome {
    /// No `structured_output_schema` was declared for this run — R-SA-030 does not apply at all;
    /// `run_sync` must leave `SingleResult::structured_output` as `None` without treating that as
    /// any kind of failure.
    NotRequested,
    /// A schema was declared, a value was extracted, and it validated successfully — carries the
    /// validated value verbatim (never a re-serialized/normalized copy), for direct assignment to
    /// `SingleResult::structured_output`.
    Valid(serde_json::Value),
    /// A schema was declared but no structured-output value could be extracted from the child's
    /// event stream at all (R-SA-030: "MUST treat its absence as a hard run failure unless plain
    /// text was also produced as a fallback" — the plain-text-fallback exemption is `run_sync`'s
    /// own separate concern, since it depends on `extract_final_output`'s result, which this
    /// module does not have visibility into; this variant always signals "no structured value was
    /// present", leaving the fallback decision to the caller).
    Missing,
    /// A schema was declared, a value was extracted, but it failed schema validation — carries the
    /// human-readable validation-error message R-SA-030 requires the run to fail with.
    Invalid(String),
}

/// R-SA-030's single composed entry point: given the task's declared `schema` (if any) and the
/// winning attempt's chronologically ordered `MessageEnd` events, extract and validate the child's
/// structured-output value.
///
/// `schema.is_none()` short-circuits to [`StructuredOutcome::NotRequested`] before touching
/// `events` at all — R-SA-030's whole contract is conditioned on "if the task declares a
/// structured-output schema"; a task with no such declaration has nothing for this function to
/// enforce, regardless of what the child's transcript happens to contain.
#[must_use]
pub fn resolve_structured_output(
    schema: Option<&serde_json::Value>,
    events: &[SubagentEvent],
) -> StructuredOutcome {
    let Some(schema) = schema else {
        return StructuredOutcome::NotRequested;
    };

    let Some(value) = extract_structured_output_value(events) else {
        return StructuredOutcome::Missing;
    };

    match validate_structured_output(schema, &value) {
        Ok(()) => StructuredOutcome::Valid(value),
        Err(message) => StructuredOutcome::Invalid(message),
    }
}

// ============================================================================================
// File-based `structured_output` tool contract (pi `structured-output.ts:1-77`)
//
// pi's authoritative structured-output mechanism is NOT event-scraping: a schema-declared step
// creates a private capture file, injects an instruction, and the child completes by CALLING the
// `structured_output` tool, which writes its value to that file. `readStructuredOutput` then reads
// it back — and its defining property (structured-output.ts:56-58) is that a MISSING capture file
// (the child never called the tool) is a HARD failure EVEN WHEN the child produced prose. The
// child-side tool registration + env capture is a child-process concern
// (`subagent-prompt-runtime.ts`, outer-layer, out of this crate's scope); this module owns the
// parent side: runtime creation, the injected instruction wording, the env-var contract, and the
// read-back — plus [`STRUCTURED_OUTPUT_MISSING_ERROR`], which `run_sync` surfaces for a declared
// schema whose value never arrived, prose notwithstanding.
// ============================================================================================

/// Env var carrying the JSON Schema file path handed to the child (pi
/// `PI_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA`).
pub const STRUCTURED_OUTPUT_SCHEMA_ENV: &str = "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA";

/// Env var carrying the capture file path the child's `structured_output` tool writes to (pi
/// `PI_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE`).
pub const STRUCTURED_OUTPUT_CAPTURE_ENV: &str = "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE";

/// The exact hard-failure message a declared `outputSchema` with no captured `structured_output`
/// call produces (pi `readStructuredOutput`, `structured-output.ts:57`) — surfaced EVEN WHEN the
/// child produced prose. pi runs its structured-output check on every clean exit
/// (`execution.ts:791`) and fails on a missing capture file unconditionally; prose is never an
/// exemption. This is the observable divergence C12's structured-output note calls out.
pub const STRUCTURED_OUTPUT_MISSING_ERROR: &str =
    "Missing structured_output call; this step has outputSchema and must finish by calling structured_output.";

/// The child-facing instruction injected when a schema is declared: the run MUST finish by calling
/// the `structured_output` tool. Kept here as the one canonical wording the spawn/task-text
/// assembly (or a future child-side prompt runtime) injects, mirroring pi's boundary instruction.
#[must_use]
pub fn structured_output_instruction() -> &'static str {
    "This step has a declared output schema. You MUST finish by calling the `structured_output` \
     tool exactly once with a value conforming to the schema; prose alone is not accepted as the \
     structured result for this step."
}

/// The parent-side runtime for one structured-output capture (pi `StructuredOutputRuntime`).
#[derive(Debug, Clone)]
pub struct StructuredOutputRuntime {
    pub schema: serde_json::Value,
    pub schema_path: std::path::PathBuf,
    pub output_path: std::path::PathBuf,
}

/// pi `createStructuredOutputRuntime` (`structured-output.ts:27-36`): create a private temp dir
/// under `base_dir`, write the schema to `schema.json`, and define the `output.json` capture path
/// the child's `structured_output` tool will write to. Uses std-only directory creation (this crate
/// has no non-test `tempfile` dependency) with a pid+counter+timestamp-unique name.
///
/// # Errors
///
/// Filesystem errors creating the directory or writing the schema.
pub fn create_structured_output_runtime(
    schema: &serde_json::Value,
    base_dir: &std::path::Path,
) -> std::io::Result<StructuredOutputRuntime> {
    std::fs::create_dir_all(base_dir)?;
    let dir = base_dir.join(format!(
        "cyrup-subagent-structured-{}-{}",
        std::process::id(),
        next_runtime_counter()
    ));
    std::fs::create_dir(&dir)?;
    let schema_path = dir.join("schema.json");
    let output_path = dir.join("output.json");
    let bytes = serde_json::to_vec(schema)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    std::fs::write(&schema_path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&schema_path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(StructuredOutputRuntime {
        schema: schema.clone(),
        schema_path,
        output_path,
    })
}

fn next_runtime_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    seed ^ COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// pi `readStructuredOutput` (`structured-output.ts:55-68`): read the child's captured structured
/// output. A MISSING capture file (the child never called `structured_output`) is a hard failure —
/// EVEN WHEN prose was produced — with [`STRUCTURED_OUTPUT_MISSING_ERROR`]. A present-but-unparseable
/// or schema-invalid value is also a hard failure.
///
/// # Errors
///
/// [`STRUCTURED_OUTPUT_MISSING_ERROR`] when the capture file is absent; a read/parse message when it
/// is unreadable; a validation message when the captured value fails the declared schema.
pub fn read_structured_output(
    runtime: &StructuredOutputRuntime,
) -> Result<serde_json::Value, String> {
    if !runtime.output_path.exists() {
        return Err(STRUCTURED_OUTPUT_MISSING_ERROR.to_string());
    }
    let bytes = std::fs::read(&runtime.output_path)
        .map_err(|err| format!("Failed to read structured output: {err}"))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Failed to read structured output: {err}"))?;
    validate_structured_output(&runtime.schema, &value)?;
    Ok(value)
}

/// pi `cleanupStructuredOutputRuntime` (`structured-output.ts:70-77`): best-effort removal of the
/// runtime's private temp dir.
pub fn cleanup_structured_output_runtime(runtime: &StructuredOutputRuntime) {
    if let Some(dir) = runtime.schema_path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn message_end(role: &str, texts: &[&str]) -> SubagentEvent {
        let content: Vec<serde_json::Value> = texts
            .iter()
            .map(|t| serde_json::json!({"type": "text", "text": t}))
            .collect();
        SubagentEvent::MessageEnd {
            message: serde_json::json!({"role": role, "content": content}),
        }
    }

    fn message_end_error(texts: &[&str]) -> SubagentEvent {
        let content: Vec<serde_json::Value> = texts
            .iter()
            .map(|t| serde_json::json!({"type": "text", "text": t}))
            .collect();
        SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": content,
                "stopReason": "error",
                "errorMessage": "boom",
            }),
        }
    }

    // ---- extract_structured_output_value ----

    #[test]
    fn extracts_a_fenced_json_block_from_the_most_recent_assistant_message() {
        let events = vec![message_end(
            "assistant",
            &["Here is my result:\n```json\n{\"ok\": true, \"count\": 3}\n```\n"],
        )];
        let value = extract_structured_output_value(&events).expect("must extract");
        assert_eq!(value, serde_json::json!({"ok": true, "count": 3}));
    }

    #[test]
    fn recognizes_jsonc_and_json5_fence_tags_too() {
        for lang in ["jsonc", "json5"] {
            let text = format!("```{lang}\n{{\"a\": 1}}\n```");
            let events = vec![message_end("assistant", &[text.as_str()])];
            let value = extract_structured_output_value(&events).expect("must extract");
            assert_eq!(value, serde_json::json!({"a": 1}));
        }
    }

    #[test]
    fn most_recent_message_wins_even_without_a_json_block_of_its_own() {
        // The newest message has plain text only (no fenced json block); an OLDER message does
        // have one. Per this function's own doc contract, the newest message's absence must NOT
        // fall through to the older message's block.
        let events = vec![
            message_end("assistant", &["```json\n{\"old\": true}\n```"]),
            message_end("assistant", &["just a plain final answer"]),
        ];
        assert_eq!(extract_structured_output_value(&events), None);
    }

    #[test]
    fn skips_error_flagged_messages_and_falls_back_to_the_last_good_one() {
        let events = vec![
            message_end("assistant", &["```json\n{\"good\": 1}\n```"]),
            message_end_error(&["```json\n{\"never\": true}\n```"]),
        ];
        let value = extract_structured_output_value(&events).expect("must extract");
        assert_eq!(value, serde_json::json!({"good": 1}));
    }

    #[test]
    fn ignores_non_assistant_and_non_message_end_events() {
        let events = vec![
            SubagentEvent::AgentStart,
            message_end("user", &["```json\n{\"user\": true}\n```"]),
            message_end("assistant", &["```json\n{\"real\": true}\n```"]),
        ];
        let value = extract_structured_output_value(&events).expect("must extract");
        assert_eq!(value, serde_json::json!({"real": true}));
    }

    #[test]
    fn a_fenced_block_with_unparseable_json_body_is_treated_as_no_block() {
        let events = vec![message_end(
            "assistant",
            &["```json\nthis is not valid json\n```"],
        )];
        assert_eq!(extract_structured_output_value(&events), None);
    }

    #[test]
    fn a_non_json_fenced_language_is_ignored() {
        let events = vec![message_end(
            "assistant",
            &["```rust\nfn main() {}\n```\nplain trailer"],
        )];
        assert_eq!(extract_structured_output_value(&events), None);
    }

    #[test]
    fn no_events_at_all_returns_none() {
        assert_eq!(extract_structured_output_value(&[]), None);
    }

    #[test]
    fn prefers_the_first_json_fence_within_one_message_when_multiple_are_present() {
        let events = vec![message_end(
            "assistant",
            &["```json\n{\"first\": 1}\n```\nsome text\n```json\n{\"second\": 2}\n```"],
        )];
        let value = extract_structured_output_value(&events).expect("must extract");
        assert_eq!(value, serde_json::json!({"first": 1}));
    }

    // ---- validate_structured_output ----

    #[test]
    fn valid_value_against_schema_succeeds() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "age": {"type": "integer"}},
            "required": ["name", "age"]
        });
        let value = serde_json::json!({"name": "ada", "age": 30});
        assert!(validate_structured_output(&schema, &value).is_ok());
    }

    #[test]
    fn invalid_value_against_schema_fails_with_a_clear_message() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}, "age": {"type": "integer"}},
            "required": ["name", "age"]
        });
        let value = serde_json::json!({"name": "ada"}); // missing required "age"
        let err = validate_structured_output(&schema, &value).expect_err("must fail");
        assert!(
            err.contains("structured output validation failed"),
            "got: {err}"
        );
    }

    #[test]
    fn type_mismatch_reports_the_offending_field_path() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"count": {"type": "integer"}},
            "required": ["count"]
        });
        let value = serde_json::json!({"count": "not a number"});
        let err = validate_structured_output(&schema, &value).expect_err("must fail");
        assert!(
            err.contains("count"),
            "expected field path in message, got: {err}"
        );
    }

    #[test]
    fn malformed_schema_itself_fails_closed_with_a_message_rather_than_panicking() {
        // `"type"` must be a string or array of strings — this is a structurally invalid schema.
        let schema = serde_json::json!({"type": 123});
        let value = serde_json::json!({"anything": true});
        let err = validate_structured_output(&schema, &value).expect_err("must fail");
        assert!(
            err.contains("invalid structured-output schema"),
            "got: {err}"
        );
    }

    #[test]
    fn array_and_nested_object_schemas_validate_correctly() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["items"]
        });
        assert!(
            validate_structured_output(&schema, &serde_json::json!({"items": ["a", "b", "c"]}))
                .is_ok()
        );
        assert!(
            validate_structured_output(&schema, &serde_json::json!({"items": ["a", 2, "c"]}))
                .is_err()
        );
    }

    // ---- resolve_structured_output: the composed entry point ----

    #[test]
    fn no_schema_declared_short_circuits_to_not_requested_without_inspecting_events() {
        let events = vec![message_end("assistant", &["```json\n{\"x\": 1}\n```"])];
        assert_eq!(
            resolve_structured_output(None, &events),
            StructuredOutcome::NotRequested
        );
    }

    #[test]
    fn schema_declared_and_value_present_and_valid_yields_valid_outcome() {
        let schema = serde_json::json!({"type": "object", "required": ["x"]});
        let events = vec![message_end("assistant", &["```json\n{\"x\": 1}\n```"])];
        assert_eq!(
            resolve_structured_output(Some(&schema), &events),
            StructuredOutcome::Valid(serde_json::json!({"x": 1}))
        );
    }

    #[test]
    fn schema_declared_but_no_value_present_yields_missing_outcome() {
        let schema = serde_json::json!({"type": "object"});
        let events = vec![message_end(
            "assistant",
            &["just plain text, no json block"],
        )];
        assert_eq!(
            resolve_structured_output(Some(&schema), &events),
            StructuredOutcome::Missing
        );
    }

    #[test]
    fn schema_declared_and_value_present_but_invalid_yields_invalid_outcome_with_message() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"count": {"type": "integer"}},
            "required": ["count"]
        });
        let events = vec![message_end(
            "assistant",
            &["```json\n{\"count\": \"nope\"}\n```"],
        )];
        let outcome = resolve_structured_output(Some(&schema), &events);
        assert!(
            matches!(&outcome, StructuredOutcome::Invalid(message) if message.contains("count")),
            "expected Invalid outcome mentioning the offending field, got {outcome:?}"
        );
    }

    // ---- file-based structured_output tool contract (pi structured-output.ts:55-68) ----

    fn sample_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"summary": {"type": "string"}, "count": {"type": "integer"}},
            "required": ["summary", "count"]
        })
    }

    #[test]
    fn create_runtime_writes_the_schema_and_defines_a_capture_path() {
        let base = tempfile::tempdir().expect("tempdir");
        let schema = sample_schema();
        let runtime = create_structured_output_runtime(&schema, base.path()).expect("runtime");
        assert!(runtime.schema_path.exists(), "schema.json must be written");
        assert!(!runtime.output_path.exists(), "capture file must not exist until the child writes it");
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime.schema_path).unwrap()).unwrap();
        assert_eq!(written, schema);
        cleanup_structured_output_runtime(&runtime);
        assert!(!runtime.schema_path.exists(), "cleanup removes the runtime dir");
    }

    #[test]
    fn read_structured_output_missing_capture_is_a_hard_failure_even_with_prose() {
        // The defining pi property: NO structured_output call (capture file absent) is a hard
        // failure regardless of any prose the child produced.
        let base = tempfile::tempdir().expect("tempdir");
        let runtime = create_structured_output_runtime(&sample_schema(), base.path()).expect("runtime");
        let err = read_structured_output(&runtime).expect_err("missing capture must fail");
        assert_eq!(err, STRUCTURED_OUTPUT_MISSING_ERROR);
        assert!(err.contains("must finish by calling structured_output"));
    }

    #[test]
    fn read_structured_output_present_and_valid_returns_the_value() {
        let base = tempfile::tempdir().expect("tempdir");
        let runtime = create_structured_output_runtime(&sample_schema(), base.path()).expect("runtime");
        std::fs::write(&runtime.output_path, br#"{"summary":"ok","count":3}"#).unwrap();
        let value = read_structured_output(&runtime).expect("valid capture");
        assert_eq!(value, serde_json::json!({"summary": "ok", "count": 3}));
    }

    #[test]
    fn read_structured_output_present_but_schema_invalid_is_a_hard_failure() {
        let base = tempfile::tempdir().expect("tempdir");
        let runtime = create_structured_output_runtime(&sample_schema(), base.path()).expect("runtime");
        std::fs::write(&runtime.output_path, br#"{"summary":"ok","count":"three"}"#).unwrap();
        let err = read_structured_output(&runtime).expect_err("invalid capture must fail");
        assert!(err.contains("validation failed") && err.contains("count"), "got: {err}");
    }

    #[test]
    fn structured_output_instruction_and_env_constants_are_stable() {
        assert!(structured_output_instruction().contains("structured_output"));
        assert_eq!(STRUCTURED_OUTPUT_SCHEMA_ENV, "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA");
        assert_eq!(STRUCTURED_OUTPUT_CAPTURE_ENV, "CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE");
    }
}
