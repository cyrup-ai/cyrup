//! SUBA-063 — the runtime-acknowledged-extensions protocol: how a subagent CHILD tells its parent
//! which extensions actually registered inside it, and how the parent reads that back onto the
//! child's result.
//!
//! Ports pi-subagents `runs/shared/runtime-acknowledged-extensions.ts` (the validation, projection
//! and sanitizer are identical at v0.64.0 and v0.68.0), the child-side collector
//! `registerRuntimeExtensionAcknowledgements` (`runs/shared/subagent-prompt-runtime.ts:116-140`
//! @v0.64.0, `:68-93` @v0.68.0) and the parent-side read-back (`foreground/execution.ts:1386`,
//! `background/subagent-runner.ts:1859` @v0.64.0).
//!
//! # The protocol
//!
//! 1. The parent names a file per attempt — `<attempt scratch dir>/runtime-acknowledged-extensions.json`
//!    — and hands its path to the child in [`RUNTIME_EXTENSION_ACK_PATH_ENV`] (pi `pi-args.ts:842-846`
//!    @v0.64.0, written for EVERY child).
//! 2. Inside the child, any extension emits [`RUNTIME_EXTENSION_ACK_EVENT`] on the inter-extension
//!    bus (`pi.events.emit("subagent:acknowledge-extension", { id })`); the child's prompt runtime
//!    ([`crate::prompt_runtime::SubagentPromptRuntime`]) subscribes to that topic and collects every
//!    valid id ([`RuntimeExtensionAcknowledgements::acknowledge`]).
//! 3. At the child's `agent_end` (or `session_shutdown`, whichever is first) the collector is
//!    finalized exactly once: the validated, de-duplicated, 32-capped projection is written to the
//!    file with mode `0600`, or the file is REMOVED when nothing valid was acknowledged
//!    ([`write_runtime_acknowledged_extensions`]).
//! 4. When the child process closes, the parent reads the file back through the size-bounded
//!    sanitizer ([`read_runtime_acknowledged_extensions`]) — the file is written by a child and is
//!    therefore untrusted input — and publishes the result as
//!    [`crate::exec::SingleResult::runtime_acknowledged_extensions`], from where it rides onto the
//!    background run's `StepStatus`/`RunStatus`, the terminal `ResultFile`, the remembered
//!    foreground child and the artifact `_meta.json`, as upstream publishes it.
//!
//! # [CYRUP-DELTA] transport, not semantics
//!
//! At v0.65.0 upstream moved its children in-process (`d9bc62f8`, "run child agents as in-process
//! pi sessions") and replaced steps 1 and 4 with an in-memory `sink` callback
//! (`child-hooks.ts:141-159` @v0.68.0); the env var and file were deleted with it. cyrup's children
//! are still OS processes, so the file IS the only channel back — this module keeps upstream's
//! v0.64.0 transport, with the v0.68.0 validation/projection/sanitization, which did not change.
//!
//! "Acknowledged" is best-effort observability, NOT extension health: `source: "child-runtime"`
//! records only that the child's runtime saw the acknowledgement (`shared/types.ts:1201-1207`).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exec::run_result::RuntimeAcknowledgedChildExtensions;

/// pi `RUNTIME_EXTENSION_ACK_EVENT` (`runtime-acknowledged-extensions.ts:3` @v0.68.0) — the bus
/// topic an extension inside a child emits `{ id }` on.
pub const RUNTIME_EXTENSION_ACK_EVENT: &str = "subagent:acknowledge-extension";

/// pi `RUNTIME_EXTENSION_ACK_PATH_ENV = "PI_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS"`
/// (`runtime-acknowledged-extensions.ts:6` @v0.64.0) in this crate's `CYRUP_` naming family: the
/// file the child writes its acknowledgements to.
pub const RUNTIME_EXTENSION_ACK_PATH_ENV: &str = "CYRUP_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS";

/// pi `MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_IDS` (`:4` @v0.68.0).
pub const MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_IDS: usize = 32;

/// pi `MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_ID_LENGTH` (`:5` @v0.68.0).
pub const MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_ID_LENGTH: usize = 128;

/// pi `MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_FILE_BYTES` (`:9-10` @v0.64.0): "Generous bound for a
/// valid capture (32 ids x 128 chars plus envelope); larger files are child misbehavior."
pub const MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_FILE_BYTES: u64 = 64 * 1024;

/// The file name pi joins onto the attempt's temp dir (`pi-args.ts:842-845` @v0.64.0).
pub const RUNTIME_ACKNOWLEDGED_EXTENSIONS_FILE: &str = "runtime-acknowledged-extensions.json";

/// pi `version: 1`.
const VERSION: u8 = 1;
/// pi `source: "child-runtime"`.
const SOURCE: &str = "child-runtime";

/// Where an attempt whose scratch directory is `temp_dir` tells its child to write.
#[must_use]
pub fn runtime_acknowledged_extensions_path_in(temp_dir: &Path) -> PathBuf {
    temp_dir.join(RUNTIME_ACKNOWLEDGED_EXTENSIONS_FILE)
}

/// pi `isRuntimeAcknowledgedExtensionId` (`:7-15` @v0.68.0): a non-empty string of at most 128
/// characters over `[A-Za-z0-9._:@+-]`, containing no `..`, `/` or `\`.
///
/// The character class is ASCII-only, so upstream's UTF-16 `length` and Rust's byte length agree on
/// every id that can pass the class check.
#[must_use]
pub fn is_runtime_acknowledged_extension_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_ID_LENGTH
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'@' | b'+' | b'-')
        })
        && !value.contains("..")
        && !value.contains('/')
        && !value.contains('\\')
}

/// pi `projectRuntimeAcknowledgedExtensions` (`:17-32` @v0.68.0): keep the valid ids in first-seen
/// order, de-duplicated; `None` when none is valid; otherwise the first 32 plus how many valid ids
/// did not fit.
#[must_use]
pub fn project_runtime_acknowledged_extensions<'a>(
    ids: impl IntoIterator<Item = &'a Value>,
) -> Option<RuntimeAcknowledgedChildExtensions> {
    let mut unique: Vec<String> = Vec::new();
    for id in ids {
        let Some(id) = id.as_str() else {
            continue;
        };
        if !is_runtime_acknowledged_extension_id(id) || unique.iter().any(|seen| seen == id) {
            continue;
        }
        unique.push(id.to_string());
    }
    if unique.is_empty() {
        return None;
    }
    let omitted = unique
        .len()
        .saturating_sub(MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_IDS);
    unique.truncate(MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_IDS);
    Some(RuntimeAcknowledgedChildExtensions {
        version: VERSION,
        source: SOURCE.to_string(),
        ids: unique,
        omitted: u32::try_from(omitted).unwrap_or(u32::MAX),
    })
}

/// pi `sanitizeRuntimeAcknowledgedExtensions` (`:34-45` @v0.68.0): accept only a `{version: 1,
/// source: "child-runtime", ids: [...]}` object, re-project its ids through
/// [`project_runtime_acknowledged_extensions`], and ADD any finite `omitted` it declared (floored,
/// never negative) to the projection's own overflow count.
#[must_use]
pub fn sanitize_runtime_acknowledged_extensions(
    value: &Value,
) -> Option<RuntimeAcknowledgedChildExtensions> {
    let raw = value.as_object()?;
    if raw.get("version").and_then(Value::as_f64) != Some(1.0)
        || raw.get("source").and_then(Value::as_str) != Some(SOURCE)
    {
        return None;
    }
    let ids = raw.get("ids")?.as_array()?;
    let mut projected = project_runtime_acknowledged_extensions(ids)?;
    // `typeof raw.omitted === "number" && Number.isFinite(raw.omitted) ? Math.max(0,
    // Math.floor(raw.omitted)) : 0`. serde_json never yields a non-finite f64, and `as` saturates.
    let declared = raw
        .get("omitted")
        .and_then(Value::as_f64)
        .map_or(0, |omitted| omitted.floor().max(0.0) as u32);
    projected.omitted = projected.omitted.saturating_add(declared);
    Some(projected)
}

/// pi `readRuntimeAcknowledgedExtensions` (`:47-56` @v0.64.0): the parent's read-back. `None` for
/// no path, a missing or unreadable file, a file larger than
/// [`MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_FILE_BYTES`], malformed JSON, or content the sanitizer
/// rejects. Never an error — the acknowledgement is observability and must not fail a run.
#[must_use]
pub fn read_runtime_acknowledged_extensions(
    file_path: Option<&Path>,
) -> Option<RuntimeAcknowledgedChildExtensions> {
    let path = file_path?;
    if std::fs::metadata(path).ok()?.len() > MAX_RUNTIME_ACKNOWLEDGED_EXTENSION_FILE_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    sanitize_runtime_acknowledged_extensions(&parsed)
}

/// pi `writeRuntimeAcknowledgedExtensions` (`:58-67` @v0.64.0): the child's finalize write —
/// `mkdir -p` the parent, then either write the projection with mode `0600` or, when nothing valid
/// was acknowledged, remove the file so no stale capture survives. Best-effort: every failure is
/// swallowed, as upstream's `catch {}`.
pub fn write_runtime_acknowledged_extensions<'a>(
    file_path: &Path,
    ids: impl IntoIterator<Item = &'a Value>,
) {
    let projection = project_runtime_acknowledged_extensions(ids);
    if let Some(parent) = file_path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    match projection {
        Some(projection) => {
            let Ok(bytes) = serde_json::to_vec(&projection) else {
                return;
            };
            let _ = write_owner_only(file_path, &bytes);
        }
        None => {
            let _ = std::fs::remove_file(file_path);
        }
    }
}

/// `fs.writeFileSync(filePath, …, { mode: 0o600 })`.
fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

/// The child-side collector — pi `registerRuntimeExtensionAcknowledgements`
/// (`subagent-prompt-runtime.ts:116-140` @v0.64.0): armed only when the parent named an output
/// file, it gathers every valid acknowledged id until it is finalized, then writes them ONCE.
/// Acknowledgements arriving after finalize are ignored, as upstream's `if (finalized …) return`.
#[derive(Debug)]
pub struct RuntimeExtensionAcknowledgements {
    output_path: PathBuf,
    state: std::sync::Mutex<AckState>,
}

#[derive(Debug, Default)]
struct AckState {
    ids: Vec<Value>,
    finalized: bool,
}

impl RuntimeExtensionAcknowledgements {
    /// pi `const outputPath = process.env[RUNTIME_EXTENSION_ACK_PATH_ENV]?.trim(); if (!outputPath)
    /// return;` — `None` for an unset or blank variable, so a process the parent did not ask
    /// registers nothing.
    #[must_use]
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let raw = get(RUNTIME_EXTENSION_ACK_PATH_ENV)?;
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| Self {
            output_path: PathBuf::from(trimmed),
            state: std::sync::Mutex::new(AckState::default()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AckState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The file this collector finalizes into.
    #[must_use]
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// pi `acknowledge(payload)`: a payload that is not an object, or whose `id` is not a valid
    /// extension id, is ignored; so is everything after finalize.
    pub fn acknowledge(&self, payload: &Value) {
        let mut state = self.lock();
        if state.finalized {
            return;
        }
        let Some(id) = payload.as_object().and_then(|object| object.get("id")) else {
            return;
        };
        if id
            .as_str()
            .is_some_and(is_runtime_acknowledged_extension_id)
        {
            state.ids.push(id.clone());
        }
    }

    /// pi `finalize()`: the first call writes (or removes) the file; every later call is a no-op.
    pub fn finalize(&self) {
        let ids = {
            let mut state = self.lock();
            if state.finalized {
                return;
            }
            state.finalized = true;
            std::mem::take(&mut state.ids)
        };
        write_runtime_acknowledged_extensions(&self.output_path, &ids);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use serde_json::json;

    /// pi's id rule, case by case — including the traversal shapes the character class alone
    /// would let through (`..`) and the two separators it already excludes.
    #[test]
    fn the_id_rule_is_upstreams() {
        for ok in [
            "ext",
            "a.b",
            "@scope.x",
            "pkg:ext@1.2+b_c-d",
            &"x".repeat(128),
        ] {
            assert!(is_runtime_acknowledged_extension_id(ok), "{ok}");
        }
        for bad in [
            "",
            "a..b",
            "a/b",
            "a\\b",
            "a b",
            "é",
            &"x".repeat(129),
            "a\nb",
        ] {
            assert!(!is_runtime_acknowledged_extension_id(bad), "{bad:?}");
        }
    }

    /// De-duplicated in first-seen order, invalid entries skipped, 32 kept and the rest counted.
    #[test]
    fn the_projection_dedups_caps_at_32_and_counts_the_rest() {
        let mut ids: Vec<Value> = vec![json!("b"), json!("a"), json!("b"), json!(7), json!("../x")];
        ids.extend((0..40).map(|i| json!(format!("e{i}"))));
        let projected = project_runtime_acknowledged_extensions(&ids).expect("valid ids");
        assert_eq!(projected.version, 1);
        assert_eq!(projected.source, "child-runtime");
        assert_eq!(projected.ids.len(), 32);
        assert_eq!(&projected.ids[..3], ["b", "a", "e0"]);
        // 42 unique valid ids, 32 kept.
        assert_eq!(projected.omitted, 10);
        assert_eq!(
            project_runtime_acknowledged_extensions(&[json!(1), json!("")]),
            None
        );
    }

    /// The sanitizer's envelope checks and its additive `omitted`.
    #[test]
    fn the_sanitizer_requires_the_envelope_and_adds_declared_omitted() {
        let ok = sanitize_runtime_acknowledged_extensions(
            &json!({"version": 1, "source": "child-runtime", "ids": ["a", "a", "b/c", "d"], "omitted": 2.9}),
        )
        .expect("valid");
        assert_eq!(ok.ids, ["a", "d"]);
        assert_eq!(ok.omitted, 2);
        let negative = sanitize_runtime_acknowledged_extensions(
            &json!({"version": 1, "source": "child-runtime", "ids": ["a"], "omitted": -5}),
        )
        .expect("valid");
        assert_eq!(negative.omitted, 0);
        for bad in [
            json!({"version": 2, "source": "child-runtime", "ids": ["a"]}),
            json!({"version": 1, "source": "launch-resolved", "ids": ["a"]}),
            json!({"version": 1, "source": "child-runtime", "ids": "a"}),
            json!({"version": 1, "source": "child-runtime", "ids": ["../a"]}),
            json!(["a"]),
        ] {
            assert_eq!(
                sanitize_runtime_acknowledged_extensions(&bad),
                None,
                "{bad}"
            );
        }
    }

    /// The collector writes once, with `0600`, and the parent's reader reads exactly that back;
    /// a finalize with nothing valid REMOVES a stale file; an oversized file reads as nothing.
    #[test]
    fn a_finalized_collector_round_trips_through_the_parents_reader() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = runtime_acknowledged_extensions_path_in(&dir.path().join("nested"));
        let raw = path.display().to_string();
        let collector = RuntimeExtensionAcknowledgements::from_env(&|key| {
            (key == RUNTIME_EXTENSION_ACK_PATH_ENV).then(|| format!("  {raw}  "))
        })
        .expect("armed");
        collector.acknowledge(&json!({"id": "alpha"}));
        collector.acknowledge(&json!({"id": "bad/id"}));
        collector.acknowledge(&json!("alpha"));
        collector.acknowledge(&json!({"id": "beta"}));
        collector.finalize();
        collector.acknowledge(&json!({"id": "late"}));
        collector.finalize();
        let read = read_runtime_acknowledged_extensions(Some(&path)).expect("written");
        assert_eq!(read.ids, ["alpha", "beta"]);
        assert_eq!(read.omitted, 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        write_runtime_acknowledged_extensions(&path, &[json!("../nope")]);
        assert!(!path.exists(), "nothing valid removes the stale capture");

        std::fs::write(&path, vec![b' '; 64 * 1024 + 1]).unwrap();
        assert_eq!(read_runtime_acknowledged_extensions(Some(&path)), None);
        assert_eq!(read_runtime_acknowledged_extensions(None), None);
        assert!(RuntimeExtensionAcknowledgements::from_env(&|_| Some("   ".to_string())).is_none());
    }
}
