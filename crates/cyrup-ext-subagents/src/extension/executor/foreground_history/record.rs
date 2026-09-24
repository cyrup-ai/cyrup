//! The foreground-history record shape, in memory and on disk, with the invariants encoded in the
//! TYPES rather than in branches a reader/writer could forget — plus the "remember" producer that
//! turns a settled [`SingleResult`] into one.
//!
//! Port of the shapes [`ForegroundResumeRun`/`ForegroundResumeChild`
//! (`shared/types.ts:1430-1479`)](../../../../../../../workspace/pi-subagents/src/shared/types.ts)
//! as persisted, and of the settle-path slice of `rememberForegroundRun`
//! (`runs/foreground/subagent-executor.ts:749-793`) this crate's ONE foreground settle path
//! (`foreground.rs::run_foreground_impl`) actually needs.

use std::path::{Path, PathBuf};

use crate::background::{RunId, RunMode};
use crate::exec::SingleResult;
use crate::extension::executor::SubagentExecutor;
use crate::fork_context::ContextMode;
use crate::identity::SessionId;

/// pi `MAX_REMEMBERED_FOREGROUND_RUNS` (`foreground-history.ts:10`) — both the in-memory bound
/// ([`SubagentExecutor::remember_foreground_run`]'s eviction sweep) and the default on-disk bound
/// (`persist`/`restore`'s callers).
pub(crate) const MAX_REMEMBERED_FOREGROUND_RUNS: usize = 50;

/// pi `MAX_INLINE_OUTPUT_BYTES` (`foreground-history.ts:12`) — 64 KiB, in BYTES.
const MAX_INLINE_OUTPUT_BYTES: usize = 64 * 1024;

/// pi `isRestorableForegroundStatus` (`foreground-history.ts:66-68`) — the ONLY four statuses
/// that reach disk. `detached` is deliberately absent: a detached run is remembered in memory and
/// never persisted, which is what makes `fleet-view.ts:408`'s detached filter a live-process-only
/// surface upstream.
pub(crate) const RESTORABLE: [&str; 4] = ["completed", "failed", "paused", "stopped"];

/// The history schema version, `1` — pi `HISTORY_VERSION` (`foreground-history.ts:11`).
///
/// A unit type, not a `u32` field, following [`crate::background::result_index::IndexVersion`]:
/// "this file is version 1" becomes a PARSE outcome instead of a check every reader must remember.
/// A future version-2 file fails to deserialize, and `persist::read_index`'s existing "any failure
/// → empty" arm already handles that — exactly as upstream's own `record.version !==
/// HISTORY_VERSION` → `{ runs: [] }` does (`:108`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct HistoryVersion;

impl HistoryVersion {
    /// The only value this type represents.
    pub(crate) const VALUE: u32 = 1;
}

impl serde::Serialize for HistoryVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for HistoryVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported foreground-history version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// pi `ForegroundResumeChild` (`shared/types.ts:1430-1468`) as persisted — restricted to the
/// fields cyrup's [`crate::tui::fleet_state::ForegroundResumeChildView`] actually renders,
/// matching that view's own field set exactly so the projection at
/// [`SubagentExecutor::foreground_runs_views`] is a straight copy.
///
/// **Scoped out, deliberately:** pi's `resumeContract` / `MAX_RESUME_CONTRACT_BYTES` /
/// `isRestorableResumeContract` / `launchContractDigest` / `extensionBindings` /
/// `capabilityCeiling` (`:70-85`). cyrup has no foreground resume-contract concept —
/// `control.rs`'s `ResumeOutcome` is async-only, and `resolve_terminal_revival` respawns from a
/// persisted transcript. Porting a validator for a field nothing produces would be dead code.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForegroundHistoryChild {
    pub(crate) agent: String,
    pub(crate) index: usize,
    /// pi's `SubagentResultStatus` string; kept as a plain string for the same reason
    /// [`crate::tui::fleet_state::ForegroundResumeChildView::status`] is — `statusGlyph` compares
    /// it against literals including `"detached"`, which is not itself ever persisted here (see
    /// [`RESTORABLE`]).
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<ContextMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) saved_output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// cyrup's [`SingleResult`] carries no output WRITE-FAILURE field yet (pi `outputSaveError`)
    /// — always `None` until a producer exists, exactly like
    /// [`crate::tui::fleet_state::ForegroundResumeChildView`]'s own same-named field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output_save_error: Option<String>,
    /// pi `transcriptError` — [`SingleResult::transcript_error`], the live transcript writer's
    /// latched failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) final_output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_count: Option<u64>,
    /// SUBA-063 — pi `ForegroundResumeChild.runtimeAcknowledgedExtensions`
    /// (`shared/types.ts:2118` @v0.68.0), copied from the settled result when present
    /// (`subagent-executor.ts:815`, `:895`). IN-MEMORY ONLY: pi's `compactChild` does not persist
    /// it (`foreground-history.ts:28-63`), so [`super::persist`] clears it before writing and a
    /// restored run carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_acknowledged_extensions:
        Option<crate::exec::run_result::RuntimeAcknowledgedChildExtensions>,
}

/// pi `ForegroundResumeRun` (`shared/types.ts:1470-1479`) as persisted.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForegroundHistoryRun {
    pub(crate) run_id: RunId,
    pub(crate) mode: RunMode,
    pub(crate) cwd: PathBuf,
    /// pi `sessionId`. **Required, and a parsed [`SessionId`]** — this single choice IS pi's
    /// `compactRun`'s `if (!run.sessionId) return undefined;` (`foreground-history.ts:89`) and its
    /// `isRestorableRun`'s `typeof run.sessionId === "string" && Boolean(run.sessionId)` (`:121`).
    /// A run with no session cannot be CONSTRUCTED, so it cannot be persisted, and a record on
    /// disk with a null/empty session fails to deserialize instead of restoring as a run that
    /// belongs to nobody.
    pub(crate) session_id: SessionId,
    pub(crate) updated_at: i64,
    /// pi `children` — never empty in a PERSISTED record (`:90`, `:127`); enforced by
    /// `persist`'s own eligibility check, not by this type (an in-memory run always has at least
    /// one child — see [`SubagentExecutor::remember_foreground_run`]'s own early return — so the
    /// type does not need to re-encode it).
    pub(crate) children: Vec<ForegroundHistoryChild>,
}

/// pi `utf8Tail(value, maxBytes).text` (`shared/utf8.ts:7-11`) — the LAST `max_bytes` bytes of
/// `value`, with any leading UTF-8 continuation bytes (`0b10xxxxxx`) dropped so the result is valid
/// UTF-8.
///
/// Bytes, not chars: a 64 KiB cap that counted `char`s would admit a 256 KiB final output of
/// 4-byte code points into a file bounded at 50 entries.
pub(crate) fn bounded_tail(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() <= MAX_INLINE_OUTPUT_BYTES {
        return value.to_string();
    }
    let mut start = bytes.len() - MAX_INLINE_OUTPUT_BYTES;
    // pi `decodeUtf8Tail` (`utf8.ts:1-5`): `(b & 0xc0) === 0x80`. `str::is_char_boundary` is the
    // same test expressed positively and is already bounds-checked.
    while start < bytes.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}

/// pi `resolveSubagentResultStatus` (`intercom/result-intercom.ts:20-41`), narrowed to the
/// terminal shapes a foreground [`SingleResult`] can actually report: `detached` when the run
/// detached, `stopped` when it was explicitly stopped, `failed` on a nonzero exit code or a
/// recorded error, else `completed`.
fn foreground_history_child_status(result: &SingleResult) -> &'static str {
    if result.detached {
        "detached"
    } else if result.stopped {
        "stopped"
    } else if result.exit_code != 0 || result.error.is_some() {
        "failed"
    } else {
        "completed"
    }
}

/// `true` when no child of `run` is `detached` — pi `trimRememberedForegroundRuns`'s own
/// eviction predicate (`subagent-executor.ts:713-722`): `!run.children.some((child) => child.status
/// === "detached")`. A DIFFERENT (looser) test than persist-eligibility (`persist`'s own
/// `is_persistable`): this one only excludes `detached`, so a run whose children are e.g. `paused`
/// is still evictable even though `paused` reaching disk depends on [`RESTORABLE`] membership too.
fn is_fully_settled(run: &ForegroundHistoryRun) -> bool {
    !run.children.iter().any(|c| c.status == "detached")
}

/// A clone of every settled run the given map currently remembers — the persist writers'
/// (`super::persist`) one input.
///
/// Takes the MAP rather than `&SubagentExecutor` because the writers do: `foreground.rs`'s
/// detached continuation holds the `Arc` to this map and deliberately not the executor (its task
/// must not be able to pin a shutdown alive), and there is exactly one snapshot rule for both
/// paths.
///
/// Cloned out of the lock rather than holding it across the merge/serialize/write that follows.
pub(crate) fn foreground_runs_snapshot_of(
    foreground_runs: &std::sync::Mutex<
        std::collections::HashMap<crate::background::RunId, ForegroundHistoryRun>,
    >,
) -> Vec<ForegroundHistoryRun> {
    foreground_runs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect()
}

impl SubagentExecutor {
    /// pi `rememberForegroundRun` (`subagent-executor.ts:749-793`), narrowed to the fields cyrup's
    /// [`ForegroundHistoryChild`] carries and to the call shape this crate's ONE settle path
    /// (`foreground.rs::run_foreground_impl`) has in hand: a flat `&[&SingleResult]` rather than
    /// pi's richer `{results, params, effectiveOutput, ...}` bundle, because cyrup's foreground run
    /// has no resume-contract/extension-binding concept to persist (see
    /// [`ForegroundHistoryChild`]'s own "scoped out" note).
    ///
    /// **Skips the run entirely when [`SubagentExecutor::current_session_id`] is `None`** — pi's
    /// own `:89` (`if (!run.sessionId) return undefined`), enforced by the TYPE rather than a
    /// branch that can be forgotten: [`ForegroundHistoryRun::session_id`] is a required
    /// [`SessionId`], so an unattributed run has no representable record.
    pub(crate) fn remember_foreground_run(
        &self,
        run_id: &RunId,
        mode: RunMode,
        cwd: &Path,
        results: &[&SingleResult],
    ) {
        let Some(session_id) = SessionId::parse_opt(self.current_session_id().as_deref()) else {
            return;
        };
        if results.is_empty() {
            return;
        }
        let now = crate::time::now_epoch_millis();
        let children: Vec<ForegroundHistoryChild> = results
            .iter()
            .enumerate()
            .map(|(index, result)| ForegroundHistoryChild {
                agent: result.agent.clone(),
                index,
                status: foreground_history_child_status(result).to_string(),
                updated_at: Some(now),
                // cyrup's `SingleResult` carries no context-mode/thinking field yet — the same
                // delta `tui::fleet_state::ForegroundChildView` already documents.
                context: None,
                model: result.model.as_ref().map(|m| m.as_str().to_string()),
                thinking: None,
                session_file: result.session_file.clone(),
                // pi `transcriptPath: result.transcriptPath` — the WRITER's path, `Some` only
                // when a live transcript was actually written (not merely named by the bundle).
                transcript_path: result.transcript_path.clone(),
                saved_output_path: result.saved_output_path.as_ref().map(PathBuf::from),
                artifact_output_path: result
                    .artifact_paths
                    .as_ref()
                    .map(|p| p.output_path.clone()),
                error: result.error.clone(),
                output_save_error: None,
                transcript_error: result.transcript_error.clone(),
                final_output: result.final_output.clone(),
                tokens: (result.usage.total_tokens > 0).then_some(result.usage.total_tokens),
                tool_count: {
                    let n = result.tool_calls.len() as u64;
                    (n > 0).then_some(n)
                },
                runtime_acknowledged_extensions: result.runtime_acknowledged_extensions.clone(),
            })
            .collect();
        let run = ForegroundHistoryRun {
            run_id: run_id.clone(),
            mode,
            cwd: cwd.to_path_buf(),
            session_id,
            updated_at: now,
            children,
        };

        let mut map = self
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(run_id.clone(), run);
        // pi `:716-722` — bound the in-memory map by evicting the OLDEST run that is not still
        // `detached`, once size exceeds the cap.
        while map.len() > MAX_REMEMBERED_FOREGROUND_RUNS {
            let oldest = map
                .values()
                .filter(|run| is_fully_settled(run))
                .min_by_key(|run| run.updated_at)
                .map(|run| run.run_id.clone());
            match oldest {
                Some(id) => {
                    map.remove(&id);
                }
                None => break,
            }
        }
    }

    /// WORKFLOW_7 §3.3 — project the in-memory record onto the FleetView's
    /// [`crate::tui::fleet_state::ForegroundResumeRunView`], the projection boundary where the
    /// typed [`SessionId`] degrades to the view's `Option<String>` (never inside the record
    /// itself). Sorted by run id for a deterministic order a `HashMap` iteration cannot give —
    /// the same discipline `foreground_fleet_entries` (`status.rs`) already follows; the caller
    /// re-sorts by `updated_at` for rendering, so this is purely a tie-break.
    pub(crate) fn foreground_runs_views(
        &self,
    ) -> Vec<crate::tui::fleet_state::ForegroundResumeRunView> {
        let mut views: Vec<crate::tui::fleet_state::ForegroundResumeRunView> = self
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|run| crate::tui::fleet_state::ForegroundResumeRunView {
                run_id: run.run_id.as_str().to_string(),
                mode: run.mode,
                cwd: run.cwd.clone(),
                session_id: Some(run.session_id.as_str().to_string()),
                updated_at: run.updated_at,
                children: run
                    .children
                    .iter()
                    .map(|child| crate::tui::fleet_state::ForegroundResumeChildView {
                        agent: child.agent.clone(),
                        index: child.index,
                        status: child.status.clone(),
                        updated_at: child.updated_at,
                        context: child.context,
                        model: child.model.clone(),
                        thinking: child.thinking.clone(),
                        session_file: child.session_file.clone(),
                        transcript_path: child.transcript_path.clone(),
                        saved_output_path: child.saved_output_path.clone(),
                        artifact_output_path: child.artifact_output_path.clone(),
                        error: child.error.clone(),
                        output_save_error: child.output_save_error.clone(),
                        transcript_error: child.transcript_error.clone(),
                        final_output: child.final_output.clone(),
                        tokens: child.tokens,
                        tool_count: child.tool_count,
                    })
                    .collect(),
            })
            .collect();
        views.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        views
    }
}

/// A minimal, fully-populated [`SingleResult`] fixture — `pub(crate)` under `#[cfg(test)]` so
/// this facade's OTHER test modules (`persist`/`restore`) can build one without each hand-rolling
/// the same 30-field struct literal (the same shared-fixture discipline
/// `background::run_history`'s own test module establishes locally; here it crosses sibling files
/// within one facade, so it lives at the module's top level instead of inside `mod tests`).
#[cfg(test)]
pub(crate) fn test_single_result(agent: &str, exit_code: i32) -> SingleResult {
    SingleResult {
        execution: None,
        native_machine: None,
        runtime_acknowledged_extensions: None,
        skills_warning: None,
        watchdog: None,
        usage_budget: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        child_run_id: None,
        agent: agent.to_string(),
        task: "do the thing".to_string(),
        exit_code,
        usage: cyrup_core::Usage::default(),
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
        detached_reason: None,
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
        tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
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
    use crate::extension::testsupport::FixedSessionIdHost;
    use std::sync::Arc;

    fn with_session(executor: &SubagentExecutor, session_id: &str) {
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session_id.to_string()),
            file: None,
        }));
    }

    /// [`bounded_tail`]: short input is untouched; long input keeps only the last
    /// `MAX_INLINE_OUTPUT_BYTES` bytes, snapped forward to a char boundary.
    #[test]
    fn bounded_tail_keeps_short_text_untouched_and_snaps_long_text_to_a_char_boundary() {
        assert_eq!(bounded_tail("short"), "short");

        // A multi-byte character (3-byte UTF-8) straddling the cut point must not be split —
        // the tail must start ON a char boundary, even if that means keeping one byte fewer.
        let filler = "a".repeat(MAX_INLINE_OUTPUT_BYTES - 1);
        let value = format!("{filler}\u{20ac}"); // \u20ac is 3 bytes in UTF-8
        let tail = bounded_tail(&value);
        assert!(tail.len() <= MAX_INLINE_OUTPUT_BYTES + 3);
        assert!(value.is_char_boundary(value.len() - tail.len()));
        assert!(tail.ends_with('\u{20ac}'));
    }

    /// With no current session, `remember_foreground_run` records NOTHING — pi's `:89`,
    /// enforced by the type rather than a branch that can be forgotten.
    #[test]
    fn remember_foreground_run_skips_a_run_with_no_session_identity() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&test_single_result("scout", 0)],
        );
        assert!(foreground_runs_snapshot_of(&executor.foreground_runs).is_empty());
    }

    /// A clean exit records `completed`; a nonzero exit records `failed`; both are remembered
    /// under the SAME run id (one foreground SINGLE run, one child at index 0).
    #[test]
    fn remember_foreground_run_derives_the_correct_status_and_carries_usage() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let run_id = RunId::new();
        let mut ok = test_single_result("scout", 0);
        ok.usage.total_tokens = 42;
        ok.tool_calls = vec![crate::exec::tool_call_summary::ToolCallSummary {
            text: "bash".to_string(),
            expanded_text: "bash -c ls".to_string(),
        }];
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&ok],
        );

        let runs = foreground_runs_snapshot_of(&executor.foreground_runs);
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.run_id, run_id);
        assert_eq!(run.session_id.as_str(), "session-a");
        assert_eq!(run.children.len(), 1);
        assert_eq!(run.children[0].status, "completed");
        assert_eq!(run.children[0].tokens, Some(42));
        assert_eq!(run.children[0].tool_count, Some(1));

        let mut failed = test_single_result("scout", 1);
        failed.error = Some("boom".to_string());
        let run_id_2 = RunId::new();
        executor.remember_foreground_run(
            &run_id_2,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&failed],
        );
        let runs = foreground_runs_snapshot_of(&executor.foreground_runs);
        let failed_run = runs
            .iter()
            .find(|r| r.run_id == run_id_2)
            .expect("second run remembered");
        assert_eq!(failed_run.children[0].status, "failed");
    }

    /// `remember_foreground_run` bounds the in-memory map at [`MAX_REMEMBERED_FOREGROUND_RUNS`] by
    /// evicting the OLDEST non-detached run, never an arbitrary one.
    #[test]
    fn remember_foreground_run_evicts_the_oldest_settled_run_once_over_the_cap() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let mut ids = Vec::new();
        for _ in 0..MAX_REMEMBERED_FOREGROUND_RUNS + 1 {
            let run_id = RunId::new();
            executor.remember_foreground_run(
                &run_id,
                RunMode::Single,
                Path::new("/tmp/project"),
                &[&test_single_result("scout", 0)],
            );
            ids.push(run_id);
            // Force a distinct `updated_at` per insert so "oldest" is unambiguous even on a fast
            // clock.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let runs = foreground_runs_snapshot_of(&executor.foreground_runs);
        assert_eq!(runs.len(), MAX_REMEMBERED_FOREGROUND_RUNS);
        let first = ids.first().expect("at least one id minted");
        assert!(
            !runs.iter().any(|r| &r.run_id == first),
            "the very first (oldest) run must have been evicted"
        );
        let last = ids.last().expect("at least one id minted");
        assert!(
            runs.iter().any(|r| &r.run_id == last),
            "the most recent run must survive"
        );
    }

    /// `foreground_runs_views` projects the record onto the FleetView shape field-for-field.
    #[test]
    fn foreground_runs_views_projects_the_record_onto_the_fleet_view_shape() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let run_id = RunId::new();
        executor.remember_foreground_run(
            &run_id,
            RunMode::Single,
            Path::new("/tmp/project"),
            &[&test_single_result("scout", 0)],
        );

        let views = executor.foreground_runs_views();
        assert_eq!(views.len(), 1);
        let view = &views[0];
        assert_eq!(view.run_id, run_id.as_str());
        assert_eq!(view.session_id.as_deref(), Some("session-a"));
        assert_eq!(view.children.len(), 1);
        assert_eq!(view.children[0].agent, "scout");
        assert_eq!(view.children[0].status, "completed");
    }
}
