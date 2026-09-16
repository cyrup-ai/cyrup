//! Run-id PREFIX resolution over the async/results dirs (pi `run-id-resolver.ts` async slice +
//! `async-resume.ts::findAsyncRunPrefixMatches`)
//!
//! pi's control ops accept a run-id PREFIX and resolve it against the on-disk async/results dirs,
//! erroring on ambiguity and returning the resolved location (`resolveSubagentRunId`'s async branch,
//! `run-id-resolver.ts:54-83`; `findAsyncRunPrefixMatches`). This is the background/async slice of
//! that resolver — the only namespace this crate's background subsystem owns (the foreground-control
//! and nested-async namespaces pi also merges are separate subsystems). Both an EXACT id and a unique
//! PREFIX resolve; an ambiguous prefix is a hard error naming every match, exactly like pi.
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::{Path, PathBuf};

use super::RunId;

/// The on-disk location a resolved background run id maps to (pi `AsyncRunLocation`,
/// `async-resume.ts`). At least one of `async_dir`/`result_path` is always `Some` (a run is
/// resolvable iff its run dir OR its terminal result file exists).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsyncRunLocation {
    /// The run's own directory (`<async_root>/<id>`), when it exists on disk.
    pub async_dir: Option<PathBuf>,
    /// The run's terminal result file (`<results_dir>/<id>.json`), when it exists on disk.
    pub result_path: Option<PathBuf>,
    /// The fully-resolved (non-prefix) run id.
    pub resolved_id: RunId,
}

/// Why a run-id (prefix) failed to resolve (pi throws with these exact ambiguity/safety messages).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveRunIdError {
    /// The id token was empty or contained a path separator / `..` (pi `assertSafeNestedId`).
    #[error("'{0}' is not a safe id token")]
    UnsafeToken(String),
    /// The prefix matched more than one run (pi's "Ambiguous subagent run id prefix" throw).
    #[error(
        "Ambiguous subagent run id prefix '{prefix}' matched: {}. Provide a longer id.",
        matches.join(", ")
    )]
    Ambiguous {
        /// The ambiguous prefix as supplied.
        prefix: String,
        /// Every matched `async:<id>` label, in sorted order.
        matches: Vec<String>,
    },
}

/// A safe run-id token (pi `assertSafeNestedId`, `nested-events.ts`): non-empty, no path separator,
/// no `..`.
fn is_safe_run_id_token(token: &str) -> bool {
    !token.is_empty() && !token.contains('/') && !token.contains('\\') && !token.contains("..")
}

/// The exact-id location for `id`, if either its run dir or its terminal result file exists (pi
/// `exactAsyncLocation`, `run-id-resolver.ts:19-28`). Pure filesystem existence checks only.
fn exact_async_location(
    id: &str,
    async_root: &Path,
    results_dir: &Path,
) -> Option<AsyncRunLocation> {
    // pi `resolveTargetedAsyncRun` (`async-status.ts:235`) rejects the reserved index directories
    // before any filesystem probe: `.terminal-runs` sits inside the async root, so
    // `async_dir.exists()` below is TRUE for it and would otherwise mint a phantom
    // [`AsyncRunLocation`]. Guarded here so both the exact route and the prefix enumeration
    // (which funnels every candidate through this function) are covered at once.
    if crate::background::terminal_run_index::is_reserved_async_root_entry(id) {
        return None;
    }
    let async_dir = async_root.join(id);
    // The legacy root path: addressable by run id alone, and where builds predating the owned
    // partition published. A payload this build promoted is NOT here — it is resolved by
    // `resolve_async_run_id` below, which can consult the run index. Existence of the run's own
    // directory is the dominant signal either way.
    let result_path = results_dir.join(format!("{id}.json"));
    let async_exists = async_dir.exists();
    // Either address counts as evidence that a terminal result exists: the legacy root file, or
    // the run's index entry (the only run-id-addressable record of a payload that was promoted
    // into its owning session's partition).
    let result_exists = result_path.exists()
        || crate::background::result_index::indexed_result_exists(
            results_dir,
            &RunId::from_token(id),
        );
    if !async_exists && !result_exists {
        return None;
    }
    Some(AsyncRunLocation {
        async_dir: async_exists.then_some(async_dir),
        result_path: result_exists.then_some(result_path),
        resolved_id: RunId::from_token(id),
    })
}

/// Every background run whose id starts with `prefix` **and belongs to `current_session`**,
/// gathered from BOTH the async run-dir tree and the results-dir (`<id>.json`) tree
/// (pi `findAsyncRunPrefixMatches`, and the session filter pi applies at
/// `run-id-resolver.ts:92`/`:102`). Returns a de-duplicated, id-sorted list of locations.
///
/// # The session filter is not optional
///
/// Both roots are per-cwd (`background/artifact_roots.rs:281-284`), so this enumeration sees every
/// concurrent cyrup instance's runs. Resolving a prefix across that set would let a short id
/// silently name another instance's run — and every caller of this function goes on to *act* on
/// what it resolves.
///
/// **This function currently has no production callers** (only `background/mod.rs`'s re-export and
/// this module's own tests). That is exactly why the parameter is mandatory rather than an
/// `Option` bolted on later: the next caller to appear will be forced to answer the question, and
/// cannot inherit an unscoped default. `None` means "no session identity" and is PERMISSIVE, the
/// same class as the control operations ([`crate::background::delivery::SessionGate::Permissive`]).
#[must_use]
pub fn find_async_run_prefix_matches(
    prefix: &str,
    async_root: &Path,
    results_dir: &Path,
    current_session: Option<&crate::identity::SessionId>,
) -> Vec<AsyncRunLocation> {
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(async_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix) {
                ids.insert(name);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(results_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".json")
                && stem.starts_with(prefix)
            {
                ids.insert(stem.to_string());
            }
        }
    }
    ids.into_iter()
        .filter_map(|id| exact_async_location(&id, async_root, results_dir))
        .filter(|location| location_belongs_to(location, async_root, current_session))
        .collect()
}

/// Whether a resolved location's run belongs to `current_session`.
///
/// Reads the run's own `status.json` — the only record of who launched it. A run whose status
/// cannot be read is treated as unattributed and, per
/// [`crate::background::delivery::SessionGate`], is refused whenever a current session exists:
/// an unreadable run is not one this instance can claim.
fn location_belongs_to(
    location: &AsyncRunLocation,
    async_root: &Path,
    current_session: Option<&crate::identity::SessionId>,
) -> bool {
    if current_session.is_none() {
        return true; // PERMISSIVE: a host with no identity filters nothing.
    }
    let status_path = crate::background::RunPaths::for_run(
        async_root,
        async_root, // results dir is irrelevant for the status path
        &location.resolved_id,
    )
    .status;
    let recorded = std::fs::read(&status_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::background::RunStatus>(&bytes).ok())
        .and_then(|status| status.session_id);
    crate::background::delivery::SessionGate::Permissive.admits(current_session, recorded.as_ref())
}

/// Resolve `id` (an EXACT id or a unique PREFIX) to its on-disk [`AsyncRunLocation`] over
/// `async_root`/`results_dir` — the background/async slice of pi's `resolveSubagentRunId`
/// (`run-id-resolver.ts:54-83`). An exact match wins outright; otherwise a prefix that matches
/// exactly one run resolves, a prefix matching several is [`ResolveRunIdError::Ambiguous`], and a
/// prefix matching none is `Ok(None)`.
///
/// # Errors
///
/// [`ResolveRunIdError::UnsafeToken`] for an unsafe id token, or [`ResolveRunIdError::Ambiguous`]
/// when a prefix matches more than one run.
pub fn resolve_async_run_id(
    id: &str,
    async_root: &Path,
    results_dir: &Path,
    current_session: Option<&crate::identity::SessionId>,
) -> Result<Option<AsyncRunLocation>, ResolveRunIdError> {
    if !is_safe_run_id_token(id) {
        return Err(ResolveRunIdError::UnsafeToken(id.to_string()));
    }
    if let Some(exact) = exact_async_location(id, async_root, results_dir)
        && location_belongs_to(&exact, async_root, current_session)
    {
        return Ok(Some(exact));
    }
    let mut matches = find_async_run_prefix_matches(id, async_root, results_dir, current_session);
    if matches.len() > 1 {
        let labels = matches
            .iter()
            .map(|m| format!("async:{}", m.resolved_id.as_str()))
            .collect::<Vec<_>>();
        return Err(ResolveRunIdError::Ambiguous {
            prefix: id.to_string(),
            matches: labels,
        });
    }
    Ok(matches.pop())
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

    #[test]
    fn resolve_async_run_id_resolves_exact_and_unique_prefix_and_errors_on_ambiguous() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        std::fs::create_dir_all(&async_root).expect("mkdir async_root");
        std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");

        // One run present as a run DIRECTORY, another present only as a terminal RESULT file.
        std::fs::create_dir_all(async_root.join("deadbeef0001")).expect("mkdir run dir");
        std::fs::write(results_dir.join("cafef00d0002.json"), b"{}").expect("write result file");

        // An EXACT id (the dir-backed run) resolves.
        let exact = resolve_async_run_id("deadbeef0001", &async_root, &results_dir, None)
            .expect("no error")
            .expect("exact id resolves");
        assert_eq!(exact.resolved_id.as_str(), "deadbeef0001");
        assert!(exact.async_dir.is_some());

        // An EXACT id backed only by its terminal result file resolves via that file.
        let exact_result = resolve_async_run_id("cafef00d0002", &async_root, &results_dir, None)
            .expect("no error")
            .expect("result-backed id resolves");
        assert_eq!(exact_result.resolved_id.as_str(), "cafef00d0002");
        assert!(exact_result.result_path.is_some());

        // A unique PREFIX resolves to the single matching run (the load-bearing behavior this task
        // calls for: control ops accept a run-id prefix, not only an exact id).
        let by_prefix = resolve_async_run_id("deadbeef", &async_root, &results_dir, None)
            .expect("no error")
            .expect("unique prefix resolves");
        assert_eq!(by_prefix.resolved_id.as_str(), "deadbeef0001");

        // A prefix matching zero runs resolves to `None`, not an error.
        let miss = resolve_async_run_id("zzzz", &async_root, &results_dir, None).expect("no error");
        assert!(miss.is_none());

        // A second run sharing the `deadbeef` prefix makes that prefix AMBIGUOUS — a hard error.
        std::fs::create_dir_all(async_root.join("deadbeef9999")).expect("mkdir second run dir");
        let ambiguous = resolve_async_run_id("deadbeef", &async_root, &results_dir, None);
        assert!(
            matches!(ambiguous, Err(ResolveRunIdError::Ambiguous { .. })),
            "a prefix matching >1 run must be a hard Ambiguous error: {ambiguous:?}"
        );
    }

    #[test]
    fn prefix_resolution_does_not_cross_sessions() {
        // S9 — both roots are per-cwd, so an unscoped prefix search can name another instance's
        // run, and every caller of this function goes on to ACT on what it resolves. The function
        // is currently unwired; this pins the behaviour before a caller arrives.
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");

        for (id, session) in [
            ("deadbeef0001", "session-MINE"),
            ("deadbeef0002", "session-THEIRS"),
        ] {
            let run_id = RunId::from_token(id);
            let paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
            std::fs::create_dir_all(&paths.run_dir).expect("mkdir");
            let mut status = crate::background::RunStatus::queued(
                run_id,
                crate::background::RunMode::Single,
                Some(1),
            );
            status.session_id = crate::identity::SessionId::parse(session);
            std::fs::write(
                &paths.status,
                serde_json::to_vec(&status).expect("serialize"),
            )
            .expect("write status");
        }

        let mine = crate::identity::SessionId::parse("session-MINE");

        // The shared prefix matches BOTH runs on disk...
        let unscoped = find_async_run_prefix_matches("deadbeef", &async_root, &results_dir, None);
        assert_eq!(unscoped.len(), 2, "both runs exist under the shared root");

        // ...but scoped to my session it matches exactly one, so it resolves unambiguously.
        let scoped =
            find_async_run_prefix_matches("deadbeef", &async_root, &results_dir, mine.as_ref());
        assert_eq!(scoped.len(), 1, "only my run may match");
        assert_eq!(scoped[0].resolved_id.as_str(), "deadbeef0001");

        // And an EXACT id belonging to another session does not resolve at all.
        let foreign =
            resolve_async_run_id("deadbeef0002", &async_root, &results_dir, mine.as_ref())
                .expect("no error");
        assert!(foreign.is_none(), "an exact foreign id must not resolve");

        // A host with no session identity still resolves everything (PERMISSIVE).
        let headless = resolve_async_run_id("deadbeef0002", &async_root, &results_dir, None)
            .expect("no error");
        assert!(headless.is_some(), "a headless host is not filtered");
    }

    /// SCOPE_13 — a run tombstone (`.deleting-run-*`,
    /// [`crate::background::async_retention`]) is a DIRECTORY inside the async root, so without
    /// the prefix arm on
    /// [`crate::background::terminal_run_index::is_reserved_async_root_entry`] this resolver
    /// would mint a phantom [`AsyncRunLocation`] for it — the failure mode the guard at the top
    /// of `resolve_async_dir` names verbatim — and would let it poison an otherwise unique
    /// prefix into an `Ambiguous` error.
    #[test]
    fn a_run_tombstone_never_resolves_and_never_makes_a_prefix_ambiguous() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        std::fs::create_dir_all(&async_root).expect("mkdir async_root");
        std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");
        std::fs::create_dir_all(async_root.join("deadbeef0001")).expect("mkdir run dir");
        // Two maintenance entries the reaper creates in this same root.
        std::fs::create_dir_all(async_root.join(".deleting-run-deadbeef0001-1"))
            .expect("mkdir tombstone");
        std::fs::create_dir_all(async_root.join(".async-retention")).expect("mkdir maintenance");

        // Addressed directly, the tombstone is not a run.
        let direct = resolve_async_run_id(
            ".deleting-run-deadbeef0001-1",
            &async_root,
            &results_dir,
            None,
        );
        assert!(
            matches!(direct, Ok(None)),
            "a tombstone must never mint an AsyncRunLocation: {direct:?}"
        );
        let maintenance = resolve_async_run_id(".async-retention", &async_root, &results_dir, None);
        assert!(matches!(maintenance, Ok(None)), "{maintenance:?}");

        // And it does not count toward a prefix match, so the real run still resolves uniquely.
        let by_prefix = resolve_async_run_id("deadbeef", &async_root, &results_dir, None)
            .expect("the tombstone must not make this prefix ambiguous")
            .expect("the real run still resolves");
        assert_eq!(by_prefix.resolved_id.as_str(), "deadbeef0001");
        assert_eq!(
            find_async_run_prefix_matches(".deleting", &async_root, &results_dir, None).len(),
            0,
            "and no prefix ever enumerates a tombstone"
        );
    }
}
