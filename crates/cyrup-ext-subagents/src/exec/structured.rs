//! Parent-side structured-output capture and JSON-Schema re-validation (func-SA §5.2 R-SA-030;
//! arch-SA §6.3.3/§12 item 13).
//!
//! # Scope
//!
//! This module owns the PARENT half of pi's file-based structured-output contract
//! (`runs/shared/structured-output.ts` @`pi-subagents` v0.43.0) — runtime creation, the injected
//! instruction wording, the env-var contract, the read-back, and the temp-dir cleanup — plus the
//! JSON-Schema validator both the parent read-back and the child-side `structured_output` tool
//! ([`crate::prompt_runtime`]) share. It spawns nothing itself and owns no subprocess lifecycle,
//! mirroring `exec/output.rs`'s own scope discipline:
//!
//! 1. [`create_structured_output_runtime`] / [`read_structured_output`] /
//!    [`cleanup_structured_output_runtime`] — pi's `createStructuredOutputRuntime` (`:127-136`),
//!    `readStructuredOutput` (`:156-173`) and `cleanupStructuredOutputRuntime` (`:175-182`). The
//!    child's value arrives through a private CAPTURE FILE, never through the transcript.
//! 2. [`validate_structured_output`] — compile the task's declared JSON Schema and check the
//!    captured value against it via the `jsonschema` crate (arch-SA §12 item 13's resolved crate
//!    choice — see the workspace `Cargo.toml`'s own comment for why `jsonschema`, not `schemars`,
//!    is correct here), returning a human-readable validation-error message on failure rather than
//!    a boolean, so [`crate::exec::run_sync`]'s caller sees exactly why the run was rejected.
//!
//! [`StructuredOutcome`] is the three-way branch `run_sync` needs on top of that: R-SA-030's
//! contract is conditioned on "if the task declares a structured-output schema", so "no schema was
//! declared at all" is a distinct, non-error case from both success and failure.
//!
//! # SUBA-S01 — what this module deliberately no longer has
//!
//! Until SUBA-S01's residual pass, this module also exported `extract_structured_output_value` /
//! `resolve_structured_output`: a reverse-chronological scan of the child's assistant messages that
//! returned the first parseable fenced ` ```json ` block as "the structured output". That heuristic
//! has NO pi counterpart at any tag — upstream's defining property (`structured-output.ts:157-159`)
//! is that a missing capture file is a HARD failure "EVEN WHEN prose was produced", and a fenced
//! block IS prose. It was worse than merely lenient: a coincidental fence in the child's prose
//! could validate against the caller's schema and become the run's structured result, silently
//! feeding a wrong answer into a chain's output bindings. Both functions and their tests are gone;
//! the capture file is now the ONLY channel, and a declared schema whose runtime could not even be
//! created is [`STRUCTURED_OUTPUT_MISSING_ERROR`], not a transcript scan.
//!
//! This module has ZERO dependency on `cyrup-agent` — every message/content shape it inspects is
//! the same opaque `serde_json::Value` [`crate::exec::ndjson::SubagentEvent`] already exposes,
//! never a typed `AgentMessage`/`Content` re-import (arch-SA §2.1/§1.1, restated at every module
//! boundary in this crate, identical to `exec/output.rs`'s own module doc).

// ============================================================================================
// R-SA-030 (validation half): compiled JSON-Schema check via the `jsonschema` crate
// ============================================================================================

/// R-SA-030 (validation half): compile `schema` and check `value` against it via `jsonschema`
/// (arch-SA §12 item 13). Returns `Ok(())` on success; on failure, returns a human-readable message
/// combining every violation (bounded to a small number so one hugely-nested schema cannot produce
/// an unbounded error string), each prefixed with its JSON-Pointer-style instance path — mirroring
/// pi-subagents' own `validateStructuredOutputValue`
/// (`pi-subagents/src/runs/shared/structured-output.ts:138-154`) in spirit: "root" for the top-level
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
// StructuredOutcome: the three-way branch `run_sync` takes on the read-back
// ============================================================================================

/// The outcome `run_sync` branches on after [`read_structured_output`] — deliberately not a bare
/// `Result` since "no schema was declared at all" is a distinct, non-error case from both success
/// and failure, and `run_sync` needs to branch on all three (R-SA-030's own text: absence is a hard
/// failure ONLY "if the task declares a structured-output schema").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredOutcome {
    /// No `structured_output_schema` was declared for this run — R-SA-030 does not apply at all;
    /// `run_sync` must leave `SingleResult::structured_output` as `None` without treating that as
    /// any kind of failure.
    NotRequested,
    /// A schema was declared, the capture file was read back, and it validated successfully —
    /// carries the validated value verbatim (never a re-serialized/normalized copy), for direct
    /// assignment to `SingleResult::structured_output`.
    Valid(serde_json::Value),
    /// A schema was declared but the child never wrote a captured value —
    /// [`STRUCTURED_OUTPUT_MISSING_ERROR`]. pi's rule (`structured-output.ts:157-159`) is that this
    /// is a hard failure EVEN WHEN prose was produced, so prose is never an exemption here. This
    /// variant is also what a declared schema whose capture runtime could not be created at all
    /// resolves to: there is no file, therefore there is no value — never a transcript scan.
    Missing,
    /// A schema was declared, a value was captured, but it failed schema validation — carries the
    /// human-readable validation-error message R-SA-030 requires the run to fail with.
    Invalid(String),
}

// ============================================================================================
// File-based `structured_output` tool contract (pi `structured-output.ts:1-77`)
//
// pi's authoritative structured-output mechanism is NOT event-scraping: a schema-declared step
// creates a private capture file, injects an instruction, and the child completes by CALLING the
// `structured_output` tool, which writes its value to that file. `readStructuredOutput` then reads
// it back — and its defining property (structured-output.ts:157-159) is that a MISSING capture file
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
/// call produces (pi `readStructuredOutput`, `structured-output.ts:158`) — surfaced EVEN WHEN the
/// child produced prose. pi runs its structured-output check on every clean exit
/// (`execution.ts:791`) and fails on a missing capture file unconditionally; prose is never an
/// exemption. This is the observable divergence C12's structured-output note calls out.
pub const STRUCTURED_OUTPUT_MISSING_ERROR: &str =
    "Missing structured_output call; this step has outputSchema and must finish by calling structured_output.";

/// The child-facing instruction injected when a schema is declared: the run MUST finish by calling
/// the `structured_output` tool. Kept here as the one canonical wording the spawn/task-text
/// assembly (or a future child-side prompt runtime) injects, mirroring pi's boundary instruction.
pub const STRUCTURED_OUTPUT_INSTRUCTION: &str =
    "This step has a declared output schema. You MUST finish by calling the `structured_output` \
     tool exactly once with a value conforming to the schema; prose alone is not accepted as the \
     structured result for this step.";

#[must_use]
pub fn structured_output_instruction() -> &'static str {
    STRUCTURED_OUTPUT_INSTRUCTION
}

/// The parent-side runtime for one structured-output capture (pi `StructuredOutputRuntime`).
#[derive(Debug, Clone)]
pub struct StructuredOutputRuntime {
    pub schema: serde_json::Value,
    pub schema_path: std::path::PathBuf,
    pub output_path: std::path::PathBuf,
}

/// pi `createStructuredOutputRuntime` (`structured-output.ts:127-136`): create a private temp dir
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

/// pi `readStructuredOutput` (`structured-output.ts:156-173`): read the child's captured structured
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

/// pi `cleanupStructuredOutputRuntime` (`structured-output.ts:175-182`): best-effort removal of the
/// runtime's private temp dir.
pub fn cleanup_structured_output_runtime(runtime: &StructuredOutputRuntime) {
    if let Some(dir) = runtime.schema_path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// SUBA-S01 residual — the RAII port of pi's `finally { if (!r?.detached)
/// cleanupStructuredOutputRuntime(structuredRuntime); }`
/// (`runs/foreground/subagent-executor.ts:3780-3787` @`pi-subagents` v0.43.0).
///
/// Two things about that `finally` do not survive a naive statement-at-the-end translation, and
/// both were wrong before this guard existed:
///
/// 1. **`finally` always runs; a Rust statement does not.** A JS `async function` ALWAYS settles,
///    so upstream's `finally` fires on the throw path too. A Rust future can be dropped at ANY
///    `.await` — and `run_sync` awaits the entire fallback ladder between creating the runtime and
///    cleaning it up. A host that cancels the `subagent` tool call by dropping its future therefore
///    left the private directory — containing a 0600 `schema.json` that carries whatever the
///    caller's schema describes — behind PERMANENTLY, one per cancelled run. `Drop` is the only
///    construct with `finally`'s guarantee, so the cleanup lives here.
/// 2. **The `!r?.detached` half is not an optimization, it is correctness.** A detached run's child
///    is STILL ALIVE (R-SA-037) and its `structured_output` tool has not written yet; the capture
///    file lives IN this directory. Deleting it on the detach receipt destroys the directory the
///    live child must still write into. Upstream says so in its own words at `:3782-3784` — "A
///    successful detached receipt transfers both to onDetachedExit while the authoritative
///    completion remains live" — and hands the same cleanup to `onDetachedExit`'s inner `finally`
///    (`:3757-3761`) instead. [`Self::disarm`] is that transfer.
#[derive(Debug)]
pub struct StructuredOutputCleanupGuard {
    runtime: StructuredOutputRuntime,
    armed: bool,
}

impl StructuredOutputCleanupGuard {
    /// Take ownership of `runtime`'s private directory. Armed: dropping this value removes the
    /// directory unless [`disarm`](Self::disarm) was called first.
    #[must_use]
    pub fn new(runtime: StructuredOutputRuntime) -> Self {
        Self {
            runtime,
            armed: true,
        }
    }

    /// The paths/schema to hand to the child and to read back — borrowing, never transferring
    /// ownership of the directory's lifetime.
    #[must_use]
    pub fn runtime(&self) -> &StructuredOutputRuntime {
        &self.runtime
    }

    /// pi's `if (!r?.detached)`: give up ownership of the directory because the still-live detached
    /// child now owns it. Idempotent.
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    /// Whether dropping this guard would still remove the directory. Test-facing; the production
    /// path only ever arms (at construction) or disarms (on a detach receipt).
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

impl Drop for StructuredOutputCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            cleanup_structured_output_runtime(&self.runtime);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

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

    // ---- StructuredOutputCleanupGuard: pi's `finally { if (!r?.detached) cleanup(...) }` ----

    /// The `finally` half. A JS `async function` always settles, so upstream's `finally` fires even
    /// when `runSync` throws; a Rust future can be dropped at any `.await`, and `run_sync` awaits
    /// the whole fallback ladder between creating this runtime and cleaning it up. Only `Drop`
    /// reproduces the guarantee — a plain end-of-function statement leaks the 0600 schema dir on
    /// every cancelled run.
    #[test]
    fn dropping_an_armed_guard_removes_the_runtime_directory() {
        let base = tempfile::tempdir().expect("tempdir");
        let runtime =
            create_structured_output_runtime(&sample_schema(), base.path()).expect("runtime");
        let dir = runtime
            .schema_path
            .parent()
            .expect("runtime dir")
            .to_path_buf();
        // Guard against a vacuous pass: the directory must exist BEFORE we assert it is gone.
        assert!(dir.exists(), "precondition: the runtime dir exists");

        {
            let guard = StructuredOutputCleanupGuard::new(runtime);
            assert!(guard.is_armed());
            assert!(dir.exists(), "the guard must not clean up while it is alive");
        }

        assert!(!dir.exists(), "dropping an armed guard removes the dir");
    }

    /// The `!r?.detached` half (pi `subagent-executor.ts:3780-3787` @v0.43.0). A detached run's
    /// child is STILL ALIVE and its `structured_output` tool has not written yet — the capture file
    /// lives in this very directory. Cleaning up on the detach receipt would destroy the directory
    /// the live child must still write into, so ownership transfers instead.
    #[test]
    fn a_disarmed_guard_leaves_the_directory_for_the_still_live_detached_child() {
        let base = tempfile::tempdir().expect("tempdir");
        let runtime =
            create_structured_output_runtime(&sample_schema(), base.path()).expect("runtime");
        let dir = runtime
            .schema_path
            .parent()
            .expect("runtime dir")
            .to_path_buf();
        let output_path = runtime.output_path.clone();

        {
            let mut guard = StructuredOutputCleanupGuard::new(runtime);
            guard.disarm();
            assert!(!guard.is_armed());
        }

        assert!(dir.exists(), "a detach receipt must not delete the live child's capture dir");
        // The whole point: the child can still write its captured value afterwards.
        std::fs::write(&output_path, br#"{"summary":"late","count":1}"#)
            .expect("the still-live child must be able to write its capture file");
        cleanup_structured_output_runtime(&StructuredOutputRuntime {
            schema: sample_schema(),
            schema_path: dir.join("schema.json"),
            output_path,
        });
        assert!(!dir.exists(), "the detached-exit path still cleans up eventually");
    }
}
