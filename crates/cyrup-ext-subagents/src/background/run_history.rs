//! Run history (pi `runs/shared/run-history.ts`): the append-only `run-history.jsonl` record of
//! finished runs — one pi-shaped `RunEntry` line per result.
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::{Path, PathBuf};

use crate::exec::SingleResult;

use super::temp_root_dir;

/// One line of `run-history.jsonl` (pi `RunEntry`, `run-history.ts:5-12`): the agent, its (200-char-
/// capped) task, a **seconds** epoch timestamp, an `"ok"`/`"error"` status, the run duration in
/// milliseconds, and — only when nonzero — the failing exit code. Field names match pi's exact
/// on-disk keys (`agent`/`task`/`ts`/`status`/`duration`/`exit`) so a reader of either runtime's
/// history file sees the identical shape.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunHistoryEntry {
    /// The agent this entry records.
    pub agent: String,
    /// The task text, truncated to pi's 200-character cap.
    pub task: String,
    /// Epoch **seconds** (pi's `Math.floor(Date.now() / 1000)`).
    pub ts: i64,
    /// `"ok"` for a clean exit, `"error"` otherwise (pi's `exitCode === 0 ? "ok" : "error"`).
    pub status: String,
    /// The run's duration in milliseconds.
    pub duration: i64,
    /// The failing exit code, present only when nonzero (pi's `...(exitCode !== 0 ? { exit } : {})`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
}

/// `getHistoryPath()` (`runs/shared/run-history.ts:23-25` @v0.43.0):
/// `path.join(getAgentDir(), "run-history.jsonl")`.
///
/// Run history is DELIBERATELY not under [`temp_root_dir`]: it is the one thing this module writes
/// that is meant to outlive a reboot (it is what `--force`/staleness checks and the cost report
/// read back), so it belongs in the agent dir with the rest of the user's durable state. The
/// previous `<home>/.cyrup/subagents/run-history.jsonl` was neither — it sat in the run-scratch
/// tree that this module now correctly treats as disposable.
#[must_use]
pub fn run_history_path() -> PathBuf {
    crate::paths::agent_dir().join("run-history.jsonl")
}

/// The run-history file for a run whose per-run directories hang off `async_root`.
///
/// For every run that actually lives in this user's canonical scratch root ([`temp_root_dir`]) —
/// which is every production run, since `resolve_background_storage_roots` derives its roots from
/// [`run_artifact_roots`](crate::background::run_artifact_roots) or from an inherited nested route that is itself under that root — this
/// is exactly pi's unconditional [`run_history_path`].
///
/// [CYRUP-DELTA] a run whose roots were REDIRECTED somewhere else records its history beside those
/// roots instead of in the real user's agent dir. This is the same principle C7 established for the
/// results dir: **the runner honours the absolute roots it was handed and never re-derives a path
/// the orchestrator did not choose.** History was the one write still ignoring that, and the cost
/// was measurable — a full workspace gate put 136 lines of synthetic test history (`researcher` /
/// `"do the thing"` / `scout`) into a developer's real `~/.cyrup/agent/run-history.jsonl`, because
/// in-process `run()` callers hand it a `TempDir` for every path EXCEPT this one.
#[must_use]
pub fn run_history_path_for(async_root: &Path) -> PathBuf {
    if async_root.starts_with(temp_root_dir()) {
        return run_history_path();
    }
    async_root
        .parent()
        .unwrap_or(async_root)
        .join("run-history.jsonl")
}

/// Append one [`RunHistoryEntry`] per `result` to `run-history.jsonl` (pi's `recordRun`,
/// `run-history.ts:132-153`) — best-effort: a missing directory is created, and every I/O or
/// serialization failure is silently swallowed so history recording can never fail a run (pi wraps
/// the whole thing in a `try {} catch {}` for exactly this reason). `run_started_at` is the run's
/// epoch-millis start, used to derive each entry's `duration`.
pub async fn record_run_history(async_root: &Path, run_started_at: i64, results: &[SingleResult]) {
    record_run_history_at(&run_history_path_for(async_root), run_started_at, results).await;
}

/// The path-explicit core of [`record_run_history`], so tests can target a private temp path
/// without mutating process-global `CYRUP_HOME`/`HOME` (the lib crate is `#![forbid(unsafe_code)]`,
/// which blocks the `unsafe { set_var }` an env override would otherwise require in a `src/` test).
async fn record_run_history_at(path: &Path, run_started_at: i64, results: &[SingleResult]) {
    if results.is_empty() {
        return;
    }
    let now = crate::time::now_epoch_millis();
    let duration = (now - run_started_at).max(0);
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = tokio::fs::create_dir_all(parent).await;

    let mut buf = String::new();
    for result in results {
        let entry = RunHistoryEntry {
            agent: result.agent.clone(),
            task: result.task.chars().take(200).collect(),
            ts: now / 1000,
            status: if result.exit_code == 0 { "ok" } else { "error" }.to_string(),
            duration,
            exit: (result.exit_code != 0).then_some(result.exit_code),
        };
        if let Ok(line) = serde_json::to_string(&entry) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    if buf.is_empty() {
        return;
    }

    use tokio::io::AsyncWriteExt;
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        // `tokio::fs::File` buffers writes internally and does NOT flush on drop, so a bare
        // `write_all` can leave the bytes sitting in tokio's buffer (with the backing write still
        // dispatched to the blocking pool) when the handle is dropped — the write then never lands
        // and a reader sees an empty file. Flush explicitly so the entries are durable before the
        // handle drops; still best-effort, so a flush error is swallowed like every other I/O error.
        if file.write_all(buf.as_bytes()).await.is_ok() {
            let _ = file.flush().await;
        }
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

    use cyrup_core::Usage;

    use crate::background::run_artifact_roots;

    use super::*;

    #[tokio::test]
    async fn record_run_history_appends_one_ok_and_one_error_line() {
        // Hermetic: write into a private temp path via the path-explicit core, so this never
        // touches the real `~/.cyrup` and needs no `unsafe { set_var }` (blocked by the lib's
        // `#![forbid(unsafe_code)]`).
        let home = tempfile::tempdir().expect("real tempdir");
        let history_path = home.path().join("run-history.jsonl");

        let ok = SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            child_run_id: None,
            agent: "researcher".to_string(),
            task: "look into the thing".to_string(),
            exit_code: 0,
            usage: Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: Some("done".to_string()),
            structured_output: None,
            session_file: None,
            output_state: Default::default(),
            structured_output_path: None,
            artifact_paths: None,
            transcript_path: None,
            transcript_error: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
            // Test fixture: no child was planned, so there is no surface to report.
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        };
        let mut bad = ok.clone();
        bad.agent = "writer".to_string();
        bad.exit_code = 7;

        record_run_history_at(
            &history_path,
            crate::time::now_epoch_millis() - 1234,
            &[ok, bad],
        )
        .await;

        let contents = std::fs::read_to_string(&history_path).expect("history file exists");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "one line per result: {contents:?}");

        let first: RunHistoryEntry = serde_json::from_str(lines[0]).expect("parse first entry");
        assert_eq!(first.agent, "researcher");
        assert_eq!(first.status, "ok");
        assert!(first.exit.is_none(), "a clean exit omits `exit`");
        assert!(first.duration >= 0);

        let second: RunHistoryEntry = serde_json::from_str(lines[1]).expect("parse second entry");
        assert_eq!(second.agent, "writer");
        assert_eq!(second.status, "error");
        assert_eq!(second.exit, Some(7), "a nonzero exit records `exit`");
    }

    #[test]
    fn run_history_path_is_the_agent_dir_not_the_scratch_root() {
        let path = run_history_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("run-history.jsonl")
        );
        assert_eq!(path.parent(), Some(crate::paths::agent_dir().as_path()));
        assert!(
            !path.starts_with(temp_root_dir()),
            "durable run history must not live inside the disposable scratch root: {path:?}"
        );
    }

    /// [`run_history_path_for`]: a run in the canonical scratch root records to the agent dir (pi's
    /// unconditional `getHistoryPath()`); a run whose roots were REDIRECTED records beside those
    /// roots instead.
    ///
    /// The second half is the regression: in-process `run()` callers hand the runner a `TempDir`
    /// for every path except this one, so re-deriving it wrote 136 lines of synthetic test history
    /// (`researcher`, `"do the thing"`, `scout`) into a real `~/.cyrup/agent/run-history.jsonl` on
    /// one full workspace gate.
    #[test]
    fn run_history_path_for_follows_a_redirected_run_and_never_the_real_agent_dir() {
        // Canonical root -> pi's unconditional agent-dir path, unchanged.
        let canonical = run_artifact_roots(Path::new("/some/project")).async_root;
        assert_eq!(
            run_history_path_for(&canonical),
            run_history_path(),
            "a run under the canonical scratch root keeps pi's `getAgentDir()/run-history.jsonl`"
        );

        // Redirected root (what every in-process `run()` test hands over) -> beside those roots.
        let redirected = Path::new("/some/tempdir/async");
        assert_eq!(
            run_history_path_for(redirected),
            PathBuf::from("/some/tempdir/run-history.jsonl")
        );
        assert_ne!(
            run_history_path_for(redirected),
            run_history_path(),
            "a redirected run must NEVER append to the real user's durable run history"
        );

        // A root with no parent degrades to the root itself rather than panicking.
        assert_eq!(
            run_history_path_for(Path::new("/")),
            PathBuf::from("/run-history.jsonl")
        );
    }
}
