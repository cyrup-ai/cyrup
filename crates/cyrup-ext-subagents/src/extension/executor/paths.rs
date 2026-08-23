//! Path/artifact derivation and the free helpers the run paths share: async and results roots,
//! tilde expansion, output-artifact writing and the synchronous foreground drive.

use std::path::{Path, PathBuf};

use cyrup_core::ToolUpdateSink;

use crate::background::{RunId, RunPaths};
use crate::error::SubagentError;
use crate::exec::{AgentConfig, RunOptions, SingleResult};
use crate::fork_context::ContextMode;

// C7: both roots come from the ONE shared derivation in `background/mod.rs`
// ([`crate::background::run_artifact_roots`]) so the orchestrator and the detached runner can never
// derive divergent results dirs again. These stay as thin, named wrappers because
// `default_async_root`/`default_results_dir` are already the vocabulary every executor call site
// reads in terms of — `resume_tracking` in `executor::status`, `run_doctor` in `executor::reports`,
// and the depth-guard tests in `executor::{chain, foreground, paths}`.
pub(crate) fn default_async_root(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).async_root
}

pub(crate) fn default_results_dir(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).results_dir
}

/// The directory a just-spawned background run owns — the SAME arithmetic
/// [`crate::extension::SubagentExecutor::spawn_background`] used to create it
/// ([`resolve_background_storage_roots`] + [`RunPaths::for_run`]), re-derived at the tool's own
/// call site so an async launch can report it as `details.asyncDir`.
///
/// pi carries `asyncDir` on EVERY async launch's `details`:
/// `{ mode, runId, results: [], asyncId: id, asyncDir, … }`
/// (`runs/background/async-execution.ts:1191` for the chain/parallel path and `:1563` for the
/// single path @v0.43.0). cyrup emitted `asyncId` but not `asyncDir`, and that one missing key is
/// what silently disabled the whole background half of the mission subsystem:
///
/// * `attachMissionToLaunchResult` writes the `mission.json` binding into the async dir only
///   `if (input.result.details.asyncDir)` (`missions/lifecycle.ts:212-213`), so no tool-launched
///   background run ever got one — and without it `syncMissionFromAsyncCompletion`'s
///   `readMissionBinding(event.asyncDir)` (`:290`) returns `undefined` and every completed
///   background run's mission stayed unreconciled;
/// * `runStatusForResult` returns `"active"` for a run with an `asyncDir` (`:98`); without one the
///   `results: []` payload fell through to `"completed"`, so a background mission was marked DONE
///   the instant it was launched;
/// * `artifactsForResult`'s `status.json` + `events.jsonl` pair (`:129-134`) is gated on the same
///   key, so a background run recorded no artifacts at all.
///
/// Pure path arithmetic over `cwd` plus this process's own env, evaluated in the same process that
/// just spawned the run, so it resolves to the identical directory (including the nested-route
/// subtree when this process inherited one). `None` only when the roots cannot be resolved at all,
/// in which case the key is omitted rather than guessed at.
fn async_dir_for_run(cwd: &Path, run_id: &RunId) -> Option<PathBuf> {
    let inherited = crate::spawn::nested_events::resolve_inherited_nested_route_from_env(|key| {
        std::env::var(key).ok()
    });
    let (async_root, results_dir) =
        resolve_background_storage_roots(cwd, inherited.as_ref()).ok()?;
    Some(RunPaths::for_run(&async_root, &results_dir, run_id).run_dir)
}

/// pi's `details` for a confirmed async launch — `{ mode, runId, results: [], asyncId, asyncDir }`
/// (`runs/background/async-execution.ts:1191` and `:1563` @v0.43.0), shared by all three async
/// arms so the `asyncDir` key can never again be present on one and missing on another.
pub(crate) fn async_launch_details(mode: &str, run_id: &RunId, cwd: &Path) -> serde_json::Value {
    let mut details = serde_json::Map::new();
    details.insert("mode".to_string(), serde_json::Value::String(mode.to_string()));
    details.insert("runId".to_string(), serde_json::Value::String(run_id.as_str().to_string()));
    details.insert("results".to_string(), serde_json::Value::Array(Vec::new()));
    details.insert("asyncId".to_string(), serde_json::Value::String(run_id.as_str().to_string()));
    if let Some(dir) = async_dir_for_run(cwd, run_id) {
        details.insert(
            "asyncDir".to_string(),
            serde_json::Value::String(dir.to_string_lossy().into_owned()),
        );
    }
    serde_json::Value::Object(details)
}

/// The `(async_root, results_dir)` pair a background run's storage should use (pi
/// `executeAsyncChain`/`executeAsyncSingle`'s `asyncDir`/`resultPath` ternaries,
/// `async-execution.ts:631-634,701,890-893,966` @v0.34.0): the nested subtree keyed under `nested_route`'s
/// root when this process inherited one from its own parent's env, else the ordinary per-`cwd`
/// C7 shared roots. Pure path arithmetic — `nested_route` is already-resolved (never re-reads env
/// itself), so this is directly unit-testable without touching real process environment state.
///
/// # Errors
///
/// Returns [`SubagentError`] if `nested_route`'s `root_run_id` is unsafe (defense in depth — an
/// already-validated inherited route should never fail this).
pub(crate) fn resolve_background_storage_roots(
    cwd: &Path,
    nested_route: Option<&crate::spawn::nested_events::NestedRoute>,
) -> Result<(PathBuf, PathBuf), SubagentError> {
    match nested_route {
        Some(route) => Ok((
            crate::spawn::nested_events::nested_async_root(&route.root_run_id)?,
            crate::spawn::nested_events::nested_results_dir(&route.root_run_id)?,
        )),
        None => {
            let crate::background::RunArtifactRoots { async_root, results_dir } =
                crate::background::run_artifact_roots(cwd);
            Ok((async_root, results_dir))
        }
    }
}

pub(crate) fn dirs_home() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// pi `expandTilde` (`extension/index.ts:233-234`): a leading `~/` expands against the user's home
/// directory; any other value (including a bare `~` with no trailing slash) passes through
/// unchanged.
pub(crate) fn expand_tilde(value: &str) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => dirs_home().join(rest),
        None => PathBuf::from(value),
    }
}

/// pi `path.resolve(...)` applied to an already-tilde-expanded value (doctor.ts:111,114): a
/// relative path resolves against the REAL process working directory, never the doctor call's own
/// `requestCwd` — Node's single-argument `path.resolve(p)` is exactly `path.resolve(process.cwd(),
/// p)`. Surfaces `std::env::current_dir()`'s own error (e.g. the process cwd has been deleted)
/// rather than silently falling back to a placeholder, matching pi's `lineFromCheck` "let a throw
/// here render as a failed line" contract.
pub(crate) fn resolve_against_process_cwd(expanded: &Path) -> std::io::Result<PathBuf> {
    if expanded.is_absolute() {
        Ok(expanded.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

/// pi `formatConfiguredSessionDir` (doctor.ts:108-116), wrapped in `lineFromCheck` (doctor.ts:65-71, applied at :122):
/// an explicit per-call `sessionDir` wins, else the extension's own configured
/// `default_session_dir`, else the literal `"not configured"`. A resolution failure renders `failed
/// — <err>`, which [`format_session_lines`](crate::registration::doctor) then prefixes with `-
/// configured session dir: ` exactly as pi's whole-line `lineFromCheck` replacement does.
pub(crate) fn format_configured_session_dir(
    requested_session_dir: Option<&str>,
    default_session_dir: Option<&Path>,
) -> String {
    let raw: Option<String> = requested_session_dir
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            default_session_dir
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| path.display().to_string())
        });
    match raw {
        Some(raw) => match resolve_against_process_cwd(&expand_tilde(&raw)) {
            Ok(resolved) => resolved.display().to_string(),
            Err(err) => format!("failed — {err}"),
        },
        None => "not configured".to_string(),
    }
}


/// Write a completed foreground run's output/metadata/event-stream artifacts (T6, the after-run half
/// of pi `runs/foreground/execution.ts:1047-1069`). The `_input.md` is written by the caller BEFORE
/// the run (crash-safety, matching pi); this writes the remaining three files gated on `cfg`. All
/// writes are best-effort — a failed artifact write must never change the run's observable result.
pub(crate) fn write_foreground_output_artifacts(
    paths: &crate::artifacts::ArtifactPaths,
    cfg: &crate::artifacts::ArtifactConfig,
    run_id: &str,
    result: &SingleResult,
) {
    if !cfg.enabled {
        return;
    }
    if cfg.include_output {
        let _ = crate::artifacts::write_artifact(
            &paths.output_path,
            result.final_output.as_deref().unwrap_or(""),
        );
    }
    if cfg.include_metadata {
        let _ = crate::artifacts::write_metadata(
            &paths.metadata_path,
            &crate::artifacts::run_artifact_metadata(run_id, result),
        );
    }
    if cfg.include_jsonl {
        for line in crate::artifacts::run_artifact_jsonl_lines(result) {
            let _ = crate::artifacts::append_jsonl(&paths.jsonl_path, &line);
        }
    }
}

/// Drive one foreground [`crate::exec::run_sync`], optionally streaming live progress through
/// `on_update` (C19 — the crate-side of pi's `onUpdate`/`fireUpdate`,
/// `runs/foreground/execution.ts:805-826`). When `on_update` is `None` this is a plain awaited
/// `run_sync` — the original, silent-until-completion behavior; every non-streaming caller (the
/// `/run` slash command, tests) is unchanged.
///
/// When `on_update` is `Some`, a [`crate::exec::LiveEventSink`] is installed on
/// [`RunOptions::live_events`] that folds each raw child NDJSON line into a shared
/// [`crate::tui::events::LiveProgressFold`] and, on every progress-relevant event, pushes a
/// [`crate::tui::events::SubagentUpdatePayload`] onto an unbounded channel. `run_sync` and a drain
/// of that channel are then raced on the SAME task via `tokio::select!` — no extra task is spawned,
/// and the `Fn`-only sink (which cannot itself touch the `FnMut` `on_update`) bridges to it purely
/// through the channel — so live updates are delivered as the child streams, and a final settle
/// update carries the terminal [`SingleResult`] on the same channel (pi's settle-time snapshot).
pub(crate) async fn drive_foreground_run_sync(
    agent_config: &AgentConfig,
    task: &str,
    mut run_options: RunOptions,
    agent_name: &str,
    resolved_context: ContextMode,
    on_update: Option<ToolUpdateSink>,
) -> SingleResult {
    use crate::tui::events::{LiveProgressFold, LiveProgressStatus, SubagentUpdatePayload};

    let Some(mut on_update) = on_update else {
        // No sink installed (the `/run` slash command, tests): the original awaited run — identical
        // to the pre-C19 behavior, no channel, no select, no live_events.
        return crate::exec::run_sync(agent_config, task, &run_options).await;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<cyrup_core::ToolUpdate>();
    // The fold is shared with the `Fn + Send + Sync` sink; `run_sync` calls that sink synchronously
    // from its single stdout-read loop, so the `Mutex` is uncontended in practice — it exists only
    // to satisfy the `Sync` bound the sink requires. A poisoned lock (impossible without a panic in
    // the sink) recovers the inner value rather than propagating, so a live-progress hiccup never
    // fails the run itself.
    let fold = std::sync::Arc::new(std::sync::Mutex::new(LiveProgressFold::new(Some(
        agent_name.to_string(),
    ))));
    let sink = {
        // Both closures below need their own handles; take the note sink's copies FIRST, since the
        // line sink's `move` consumes the ones named after it.
        let note_fold = std::sync::Arc::clone(&fold);
        let note_tx = tx.clone();
        let fold = std::sync::Arc::clone(&fold);
        crate::exec::LiveEventSink::new(move |raw: &str| {
            let mut guard = match fold.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            // Emit an update exactly when a progress-relevant event fired (pi's `fireUpdate`
            // cadence), never once per raw line.
            if guard.record_line(raw) {
                let snapshot = guard.snapshot(LiveProgressStatus::Running);
                let payload = SubagentUpdatePayload::single_live(resolved_context, snapshot);
                let text = payload.content_text();
                // A closed receiver (the caller already returned) is a benign no-op.
                let _ = tx.send(payload.into_tool_update(text));
            }
        })
        // Parent-side attempt notes (model-fallback / startup-retry) fold into the SAME ring and
        // fire an update immediately: the note explains a relaunch that is happening right now, so
        // it is worthless if it waits for the next child event — and it cannot wait for settle,
        // where `compact_completed` empties `recent_output` outright.
        .with_note_sink(move |note: &str| {
            let mut guard = match note_fold.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.record_attempt_note(note);
            let snapshot = guard.snapshot(LiveProgressStatus::Running);
            let payload = SubagentUpdatePayload::single_live(resolved_context, snapshot);
            let text = payload.content_text();
            let _ = note_tx.send(payload.into_tool_update(text));
        })
    };
    run_options.live_events = Some(sink);

    let run = crate::exec::run_sync(agent_config, task, &run_options);
    tokio::pin!(run);
    let result = loop {
        tokio::select! {
            settled = &mut run => break settled,
            Some(update) = rx.recv() => on_update(update),
        }
    };
    // Deliver any updates buffered between the last poll and the child settling (the child's stdout
    // is fully drained by the time `run_sync` returns, so no further sends can arrive).
    while let Ok(update) = rx.try_recv() {
        on_update(update);
    }

    // Final settle update (pi emits a terminal snapshot on the same channel): flip the status to
    // the run's terminal outcome and carry the full `SingleResult` in `results` so the inline
    // surface can render the completed row from the same `details` shape the live updates used.
    let final_status = if result.exit_code == 0 && !result.timed_out {
        LiveProgressStatus::Complete
    } else {
        LiveProgressStatus::Failed
    };
    let final_snapshot = match fold.lock() {
        Ok(guard) => guard.snapshot(final_status),
        Err(poisoned) => poisoned.into_inner().snapshot(final_status),
    };
    let final_payload =
        SubagentUpdatePayload::single_final(resolved_context, result.clone(), final_snapshot);
    let text = result
        .final_output
        .clone()
        .unwrap_or_else(|| final_payload.content_text());
    on_update(final_payload.into_tool_update(text));

    result
}

/// Render a foreground `/run` result as a completion summary (T8 slash-live-state, partial): the
/// single transcript entry `execute_command` returns, shaped to read as pi's live-state placeholder
/// RESOLVED to completion (`slash/slash-live-state.ts` -> `renderSubagentResult`). A status line
/// (done/failed/paused/timed-out + agent + tool-call and token stats) precedes the delivered output
/// — the same header/stats/body composition pi's settled placeholder renders, minus the mid-run
/// in-place updating that requires a host transcript-update channel (documented at the `/run`
/// dispatch site as the remaining outer-layer step).
pub(crate) fn format_slash_run_completion(result: &SingleResult) -> String {
    let tokens = result.usage.input.saturating_add(result.usage.output);
    let tool_count = result.tool_calls.len();
    let status = if result.interrupted {
        "paused (interrupted)".to_string()
    } else if result.timed_out {
        "timed out".to_string()
    } else if result.exit_code == 0 {
        "done".to_string()
    } else {
        format!("failed (exit {})", result.exit_code)
    };
    let plural = if tool_count == 1 { "" } else { "s" };
    let header =
        format!("subagent {} · {status} · {tool_count} tool call{plural} · {tokens} tokens", result.agent);
    let body = result.final_output.clone().unwrap_or_default();
    let body = if body.trim().is_empty() {
        result
            .error
            .clone()
            .unwrap_or_else(|| "(no output)".to_string())
    } else {
        body
    };
    format!("{header}\n\n{body}")
}

/// Enumerate every installed package across both [`cyrup_resources::InstallScope`]s by loading the
/// persisted `packages.json` install registries `cyrup-resources` itself writes — Global under
/// `<global_dir>/packages.json`, Project under `<project_root>/.cyrup/packages.json` (the exact
/// paths [`cyrup_resources::PackageStore::registry_path`] resolves) — and concatenating them in the
/// fixed project-then-global order [`crate::discovery::scan_package_agents`] re-sorts into anyway.
///
/// A missing registry file is an empty registry (never an error — the common "no packages installed"
/// case), mirroring `cyrup_resources::package::lock::load`'s own missing-file contract; a malformed
/// registry is likewise treated as "no packages from that scope" rather than aborting all of
/// discovery, since a package-registry read failure is not one of R-SA-009's three surfaced-error
/// cases (which cover malformed agent frontmatter, chain files, and `subagents.*` settings only).
/// This is the read-only enumeration half of the package tier; the on-disk package roots are
/// resolved later, per-package, by `scan_package_agents`/`scan_package_chain_scopes` via
/// `installed_dir` from these same records.
pub(crate) fn enumerate_installed_packages(
    global_dir: &Path,
    project_root: Option<&Path>,
) -> cyrup_resources::InstalledPackages {
    use cyrup_resources::InstallScope;

    let store = cyrup_resources::PackageStore::new(
        global_dir.to_path_buf(),
        project_root.map(Path::to_path_buf),
    );
    let mut installed = cyrup_resources::InstalledPackages::default();
    for scope in [InstallScope::Project, InstallScope::Global] {
        let Some(registry_path) = store.registry_path(scope) else {
            continue;
        };
        if let Ok(registry) = cyrup_resources::package::lock::load(&registry_path) {
            installed.packages.extend(registry.packages);
        }
    }
    installed
}

/// The 8 bundled builtin agent personas' resource root (R-SA-132/134: "the extension MUST expose
/// its bundled agent personas... as bundled resources loaded through the `cyrup-resources`
/// discovery pipeline"), mirroring `scout`/`delegate`/`context-builder`/`planner`/`researcher`/
/// `reviewer`/`worker`/`oracle` (func-SA §5.1 R-SA-132's exact target list).
///
/// Points at `crates/cyrup-ext-subagents/resources/` — the parent of the conventional `agents/`
/// child directory (`resources/agents/*.md`) — so [`cyrup_resources::resolve_manifest`]'s
/// auto-discovery fallback (no `cyrup.toml` needed here) recognizes it exactly the same way it
/// recognizes any other package's `agents = ["./agents"]` manifest declaration (R-SA-020), which
/// `scan_builtin_agents` (`discovery/mod.rs`) then expands via the ordinary
/// [`walk_agent_dir`](crate::discovery::walk_agent_dir) pipeline.
///
/// [`BUILTIN_AGENTS_DIR_ENV_VAR`] allows a caller to override this path for a packaged/installed
/// binary that does not ship with an intact `CARGO_MANIFEST_DIR`-relative source tree (e.g. a
/// release artifact that instead vendors the bundled personas into a fixed install-time location)
/// — this crate takes no position on that packaging strategy itself, it just leaves the seam open
/// via the same closure-injectable-env-lookup convention `resolve_extra_agent_dirs`
/// (`discovery/mod.rs`) already establishes for `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`. The default,
/// used by every real `cyrup` binary invocation and this crate's own tests today, resolves against
/// this crate's own `CARGO_MANIFEST_DIR` (baked in at compile time), which is correct for every
/// from-source build of this workspace.
const BUILTIN_AGENTS_DIR_ENV_VAR: &str = "CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR";

pub(crate) fn builtin_agents_dir() -> PathBuf {
    std::env::var_os(BUILTIN_AGENTS_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// Structurally unreachable per [`crate::extension::SubagentExecutor::fork_resolver`]'s own documented reasoning
/// (`SessionManager::in_memory` with a `None` id never fails); retained as an explicit, named,
/// never-called total function rather than a bare `unreachable!()`/`panic!()` — this crate forbids
/// both outside tests — so the type system still sees a total `SessionManager` value at every call
/// site without this crate ever actually executing a panic path in practice. If this function is
/// ever reached, it constructs the same in-memory session a third time; per `in_memory`'s own
/// contract this cannot fail, so the loop is guaranteed to terminate above it in practice.
pub(crate) fn unreachable_session_manager() -> cyrup_session::SessionManager {
    // Retry indefinitely rather than panic — matches this crate's crate-wide `#![deny(panic)]`
    // policy. In practice this is never entered (see this function's own doc).
    loop {
        if let Ok(m) = cyrup_session::SessionManager::in_memory(
            Path::new("."),
            cyrup_session::NewSessionOpts::default(),
        ) {
            return m;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::background::RunMode;
    use crate::background::RunState;
    use crate::background::atomic::write_atomic_json;
    use crate::discovery::AgentDiscoveryConfig;
    use crate::discovery::discover_agents;
    use crate::discovery::types::AgentReadScope;
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::executor::requests::BackgroundSingleRequest;
    use crate::extension::executor::requests::BackgroundStepsSpec;
    use crate::extension::testsupport::FixedSessionHost;
    use crate::extension::testsupport::dispatch_tool;
    use crate::extension::testsupport::scoped_mission_config;
    use crate::extension::testsupport::scoped_tool;
    use crate::extension::testsupport::seed_orphaned_run;
    use crate::extension::testsupport::tool_text;
    use crate::spawn::chain_graph::RunnerStep;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// SUBA-N03: `outputMode: "file-only"` with no resolvable output path is refused BEFORE the
    /// detached hop-1 process is spawned, not surfaced later as a hop-2 step failure the caller
    /// never sees synchronously.
    ///
    /// pi runs this check inside `executeAsyncSingle` itself, in the PARENT process, before
    /// `spawnRunner` (`validateFileOnlyOutputMode(outputMode, outputPath, \`Async single run
    /// (${agent})\`)`, `runs/background/async-execution.ts:414-559` via `single-output.ts:140-145`).
    /// The filesystem assertion is the load-bearing half: no run directory may exist afterwards.
    #[tokio::test]
    async fn a_background_single_run_refuses_file_only_output_mode_with_no_path_before_spawning() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");

        let err = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "worker",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                // No `output`, and the builtin `worker` persona declares none of its own.
                output: None,
                output_mode: Some("file-only".to_string()),
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect_err("file-only with no output path must be refused");
        assert!(
            matches!(err, SubagentError::OutputPathRequired),
            "expected R-SA-025's OutputPathRequired, got: {err:?}"
        );
        assert!(
            !default_async_root(dir.path()).exists(),
            "the refusal must land BEFORE any run directory is created"
        );
    }

    /// The background (`bg: true`) shape's own independent entry point must enforce the identical
    /// R-SA-055 ordering: depth guard before discovery, fork-context resolution, run-directory
    /// creation, or the detached hop-1 process spawn. Proven the same way as the foreground test
    /// above — an unresolvable agent name combined with an exhausted depth ceiling must surface
    /// `DepthExceeded`, not `AgentNotFound`, AND no run directory may exist afterward (the
    /// filesystem-level proof that `spawn_background` never reached its own `create_dir_all`/
    /// detached-spawn steps, which live strictly after the depth check in program order).
    #[tokio::test]
    async fn spawn_background_rejects_on_depth_before_discovery_or_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .spawn_background(BackgroundSingleRequest {
                // SUBA-021: unbudgeted on this path (see the field doc).
                usage_budget: None,
                turn_budget: None,
                structured_output_schema: None,
                tool_budget: None,
                cwd: dir.path(),
                agent_name: "ghost",
                task: "do something",
                context: Some(ContextMode::Fresh),
                model_override: None,
                agent_scope: AgentReadScope::Both,
                acceptance: None,
                control: None,
                include_progress: None,
                output: None,
                output_mode: None,
                skills: None,
                share: None,
                session_dir: None,
                artifacts: None,
                timeout_ms: None,
            })
            .await
            .expect_err("a blocked depth ceiling must reject before discovery or any spawn setup");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of discovery's own AgentNotFound, got: {err:?}"
        );
        // The load-bearing proof that NOTHING was set up: neither the async-run root nor the
        // results directory `spawn_background` would otherwise create via `create_dir_all` (both
        // strictly after the depth check in program order) may exist.
        assert!(
            !default_async_root(dir.path()).exists(),
            "the async-run root must never be created for a depth-blocked background dispatch"
        );
        assert!(
            !default_results_dir(dir.path()).exists(),
            "the results directory must never be created for a depth-blocked background dispatch"
        );
    }

    /// Regression (pi `restoreActiveJobs`, `async-job-tracker.ts:490-511`): resuming tracking from
    /// disk must (a) skip any run whose RECONCILED state is already terminal (`complete`/`failed`/
    /// `paused`) — pi's own `listAsyncRuns({ states: ["queued", "running"] })` filter — and (b) seed
    /// each restored run's `events.jsonl` byte cursor at the file's CURRENT size (pi's
    /// `restoredControlEventCursor`), not `0`. Pre-fix, `resume_tracking` re-tracked EVERY
    /// subdirectory unconditionally (including an already-`Complete` run) and always seeded the
    /// cursor at `0`, which would cause a restored job's entire historical `events.jsonl` to be
    /// re-tailed the next poll tick.
    #[tokio::test]
    async fn resume_tracking_skips_terminal_runs_and_seeds_the_events_cursor_at_eof() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());

        // A still-running run: real live pid (this test process itself) so reconciliation leaves
        // it Running, plus a non-empty events.jsonl whose EXISTING bytes must never be re-tailed.
        let running_run_id = RunId::from_token("run-running");
        let running_paths = RunPaths::for_run(&async_root, &results_dir, &running_run_id);
        tokio::fs::create_dir_all(&running_paths.run_dir)
            .await
            .expect("mkdir running run_dir");
        let mut running_status = crate::background::RunStatus::queued(
            running_run_id.clone(),
            RunMode::Single,
            Some(std::process::id()),
        );
        running_status.advance_state(RunState::Running).expect("Queued -> Running");
        write_atomic_json(&running_paths.status, &running_status)
            .await
            .expect("write running status fixture");
        let events_content = b"{\"kind\":\"a\"}\n{\"kind\":\"b\"}\n";
        tokio::fs::write(&running_paths.events, events_content)
            .await
            .expect("seed events.jsonl for the running run");

        // A run that already finished before this process started: must NOT be re-tracked at all.
        let complete_run_id = RunId::from_token("run-complete");
        let complete_paths = RunPaths::for_run(&async_root, &results_dir, &complete_run_id);
        tokio::fs::create_dir_all(&complete_paths.run_dir)
            .await
            .expect("mkdir complete run_dir");
        let mut complete_status = crate::background::RunStatus::queued(
            complete_run_id.clone(),
            RunMode::Single,
            Some(1),
        );
        complete_status.state = RunState::Complete;
        write_atomic_json(&complete_paths.status, &complete_status)
            .await
            .expect("write complete status fixture");

        executor.resume_tracking(dir.path()).await;

        assert_eq!(
            executor.tracker().tracked_count(),
            1,
            "only the queued/running run may be restored — a terminal run must be skipped entirely"
        );
        assert!(
            executor.tracker().get(&complete_run_id).is_none(),
            "an already-terminal run must never be re-tracked by resume_tracking"
        );
        let restored = executor
            .tracker()
            .get(&running_run_id)
            .expect("the still-running run must be restored");
        assert_eq!(
            restored.events_cursor,
            events_content.len() as u64,
            "the restored job's events cursor must be seeded at the file's CURRENT size (EOF), \
             never 0, so historical control events are never re-tailed"
        );
    }

    /// pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:1198-1565`
    /// / `890-893,966,989-993` @v0.34.0): a background run started from WITHIN an already-nested run
    /// reroutes its storage under the inherited root's `nested-subagent-runs`/`nested` subtree,
    /// instead of the ordinary per-`cwd` shared async/results roots — otherwise it is
    /// indistinguishable from a top-level run and invisible to the root's own nested registry.
    /// Before this fix, `spawn_background_steps` had no nested-route awareness at all and
    /// unconditionally called `run_artifact_roots(cwd)` (this test's `resolve_background_storage_roots`
    /// callee did not exist pre-fix, so a nested route could never reroute anything).
    #[test]
    fn resolve_background_storage_roots_reroutes_under_the_inherited_nested_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let route = crate::spawn::nested_events::create_nested_route("root-parity-test-async-exec")
            .expect("create_nested_route should succeed");

        let (nested_async, nested_results) =
            resolve_background_storage_roots(dir.path(), Some(&route))
                .expect("nested rerouting must succeed for a valid route");
        assert!(
            nested_async.ends_with("root-parity-test-async-exec"),
            "the async root for a nested run must be keyed under the inherited route's own root \
             run id, got: {nested_async:?}"
        );
        assert!(
            nested_async.to_string_lossy().contains("nested-subagent-runs"),
            "a nested run's async root must live under the nested-subagent-runs subtree, got: \
             {nested_async:?}"
        );
        assert!(
            nested_results.to_string_lossy().contains("nested"),
            "a nested run's results dir must live under the nested results subtree, got: \
             {nested_results:?}"
        );

        let (default_async, default_results) = resolve_background_storage_roots(dir.path(), None)
            .expect("the non-nested default derivation must still succeed");
        assert_eq!(default_async, default_async_root(dir.path()));
        assert_eq!(default_results, default_results_dir(dir.path()));
        assert_ne!(
            nested_async, default_async,
            "a nested run must never land in the same shared per-cwd async root as a top-level run"
        );

        // Best-effort cleanup of the route directory this test created under the real temp root.
        if let Some(parent) = route.event_sink.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    /// [`SubagentExecutor::spawn_background_steps`] (the general multi-step background dispatch
    /// [`SubagentExecutor::spawn_background`] itself wraps, and `/chain`/`/parallel`'s `--bg` shape
    /// calls directly) must reject a blocked depth ceiling before creating the async-run root,
    /// results directory, or run directory — the filesystem-level proof mirrors this test's own
    /// `spawn_background`-level sibling above, applied to this lower-level entry point directly
    /// rather than through the single-task wrapper.
    #[tokio::test]
    async fn spawn_background_steps_rejects_on_depth_before_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let step = RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: "worker".to_string(),
            task: "do something".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        });

        let err = executor
            .spawn_background_steps(
                dir.path(),
                BackgroundStepsSpec {
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    steps: vec![step],
                    mode: RunMode::Single,
                    session_file: None,
                    resolved_agents: BTreeMap::new(),
                    original_task: String::new(),
                    chain_dir: None,
                    control: None,
                    include_progress: None,
                    run_id: RunId::new(),
                    timeout_ms: None,
                    share: None,
                    artifacts_dir: None,
                    artifact_config: crate::artifacts::ArtifactConfig::default(),
                },
            )
            .await
            .expect_err("a blocked depth ceiling must reject before any directory creation");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
        assert!(!default_async_root(dir.path()).exists());
        assert!(!default_results_dir(dir.path()).exists());
    }

    // NOTE: `teardown_session_stops_the_tracker_and_clears_the_parent_session_anchor` (and its
    // `FixedSessionHost` double) moved to `tests/cyrup_home_env_sandboxed_tests.rs` — see that
    // file's module doc; it needs the `CYRUP_HOME` env-var sandbox that requires `unsafe`, which
    // this crate's `#![forbid(unsafe_code)]` `src/lib.rs` disallows in-crate.

    // ---------------------------------------------------------------------------------------
    // `run_doctor` parity regressions (pi `buildDoctorReport`/`formatConfiguredSessionDir`,
    // doctor.ts:108-128; caller `subagent-executor.ts:2801-2840`)
    // ---------------------------------------------------------------------------------------

    /// pi `formatConfiguredSessionDir` (doctor.ts:108-116): a per-call `sessionDir` wins over the
    /// configured `default_session_dir`, which wins over the literal `"not configured"`. Pre-fix,
    /// `run_doctor` always rendered the always-on computed `<home>/.cyrup/sessions/<cwd_key>`
    /// directory here regardless of either input, and `"not configured"` was unreachable — this
    /// test fails against that behavior on all three branches.
    #[test]
    fn format_configured_session_dir_prefers_requested_then_default_then_not_configured() {
        assert_eq!(
            format_configured_session_dir(Some("/abs/requested"), Some(Path::new("/abs/default"))),
            "/abs/requested",
            "an explicit per-call sessionDir must win over the configured default"
        );
        assert_eq!(
            format_configured_session_dir(None, Some(Path::new("/abs/default"))),
            "/abs/default",
            "with no per-call override, the configured default_session_dir must be used"
        );
        assert_eq!(
            format_configured_session_dir(None, None),
            "not configured",
            "with neither a per-call override nor a configured default, pi's literal \
             \"not configured\" must be reachable"
        );
        // An empty-string override is JS-falsy in pi (`if (input.requestedSessionDir)`) and must
        // fall through exactly like an absent one.
        assert_eq!(
            format_configured_session_dir(Some(""), Some(Path::new("/abs/default"))),
            "/abs/default"
        );
    }

    /// pi `expandTilde` (`extension/index.ts:233-234`) composed with `path.resolve`: a leading `~/`
    /// expands against the home directory before being resolved to an absolute path.
    #[test]
    fn format_configured_session_dir_expands_a_leading_tilde() {
        let rendered = format_configured_session_dir(Some("~/my-sessions"), None);
        let expected = dirs_home().join("my-sessions");
        assert_eq!(rendered, expected.display().to_string());
    }

    /// The item's own Verify, end to end through the USER-REACHABLE surface: a reload-orphaned run
    /// is listed as live, `{action:"dismiss", id}` clears it, and it is gone from `{action:"status"}`.
    ///
    /// **Pre-fix this test goes red twice over**: `dismiss` was absent from `SUBAGENT_ACTIONS` and
    /// from every dispatch arm, so the tool answered `unknown subagent action 'dismiss'` (the
    /// did-you-mean message) instead of a `ToolResult`; and with no producer for
    /// `display_dismissed_at` the run stayed in the active listing for good.
    #[tokio::test]
    async fn dismiss_clears_a_reload_orphaned_run_from_the_fleet_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = scoped_tool(dir.path()).await;
        tool.executor().set_host_services(Arc::new(FixedSessionHost("session-a")));
        seed_orphaned_run(dir.path(), "run0orphan00", Some("session-a"), None);

        // The defect itself: before the dismissal the orphan is reported as live.
        let before = tool
            .executor()
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list");
        assert!(before.contains("run0orphan00"), "the orphan must start out listed: {before}");

        let result = dispatch_tool(&tool, serde_json::json!({ "action": "dismiss", "id": "run0orphan00" }))
            .await
            .expect("dismiss dispatches through the tool");
        assert_eq!(
            tool_text(&result),
            "Dismissed recovered workflow run0orphan00 from the display. No running work was \
             terminated.",
            "pi `async-dismiss-action.ts:82`, byte for byte"
        );

        let after = tool
            .executor()
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list");
        assert_eq!(
            after, "No active async runs.",
            "a dismissed run must vanish from the active listing every fleet surface renders from"
        );

        // Display-only: nothing was terminated, and the record still says what it said.
        let paths = RunPaths::for_run(
            &default_async_root(dir.path()),
            &default_results_dir(dir.path()),
            &RunId::from_token("run0orphan00".to_string()),
        );
        let persisted: crate::background::RunStatus =
            serde_json::from_slice(&std::fs::read(&paths.status).expect("read status"))
                .expect("parse status");
        assert!(persisted.display_dismissed_at.is_some(), "the marker must be persisted");
        assert_eq!(
            persisted.state,
            RunState::Running,
            "dismissal is display-only — it must NOT advance the run's state"
        );
        assert!(!paths.result.exists(), "dismissal must not fabricate a terminal result file");

        // An id-addressed lookup still answers honestly, with pi's display-dismissed report.
        let single = tool
            .executor()
            .control_status(dir.path(), Some("run0orphan00"), None, false)
            .await
            .expect("single-run status");
        assert!(single.contains("State: display-dismissed"), "{single}");
    }

    /// The remaining three refusals plus the missing-selector guard, each with pi's exact sentence
    /// (`async-dismiss-action.ts:18-48`, `subagent-executor.ts:5874`).
    ///
    /// Pre-fix every one of these was `unknown subagent action 'dismiss'`.
    #[tokio::test]
    async fn dismiss_refusals_are_pis_exact_sentences() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionHost("session-a")));

        // pi `subagent-executor.ts:5874` — no selector at all.
        assert_eq!(
            executor.control_dismiss(dir.path(), None).await.expect_err("id required"),
            "action='dismiss' requires id."
        );

        // pi `:18-22` — the selector resolves to no async run on disk.
        assert_eq!(
            executor
                .control_dismiss(dir.path(), Some("run0missing0"))
                .await
                .expect_err("nothing on disk"),
            "Recovered workflow 'run0missing0' has no disk status to dismiss."
        );

        // pi `:24-30` — a run directory exists but carries no readable `status.json`.
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let bare = RunPaths::for_run(&async_root, &results_dir, &RunId::from_token("run0bare0000".to_string()));
        std::fs::create_dir_all(&bare.run_dir).expect("mkdir bare run dir");
        assert_eq!(
            executor
                .control_dismiss(dir.path(), Some("run0bare0000"))
                .await
                .expect_err("no status.json"),
            "Run 'run0bare0000' is not a recovered workflow."
        );

        // pi `:31-36` — the run belongs to another session.
        seed_orphaned_run(dir.path(), "run0othersess", Some("session-b"), None);
        assert_eq!(
            executor
                .control_dismiss(dir.path(), Some("run0othersess"))
                .await
                .expect_err("wrong session"),
            "Recovered workflow 'run0othersess' was not found in the active session."
        );

        // pi `:43-48` — the run is not `running`, and the state word is pi's own.
        let paused = seed_orphaned_run(dir.path(), "run0paused00", Some("session-a"), None);
        let mut status: crate::background::RunStatus =
            serde_json::from_slice(&std::fs::read(&paused.status).expect("read")).expect("parse");
        status.state = RunState::Paused;
        std::fs::write(&paused.status, serde_json::to_string(&status).expect("serialize"))
            .expect("rewrite paused status");
        assert_eq!(
            executor
                .control_dismiss(dir.path(), Some("run0paused00"))
                .await
                .expect_err("not running"),
            "Recovered workflow 'run0paused00' is paused, not running."
        );
    }

    /// A confirmed async launch reports the run directory the spawn actually created, so the
    /// mission binding chain has something to bind to (`async-execution.ts:1191,1563`).
    #[test]
    fn an_async_launch_reports_the_run_directory_it_spawned_into() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = crate::background::RunId::from_token("bgrun000042");
        let details = async_launch_details("single", &run_id, dir.path());
        assert_eq!(details.get("mode").and_then(|v| v.as_str()), Some("single"));
        assert_eq!(details.get("runId").and_then(|v| v.as_str()), Some("bgrun000042"));
        assert_eq!(details.get("asyncId").and_then(|v| v.as_str()), Some("bgrun000042"));
        assert_eq!(details.get("results").and_then(|v| v.as_array()).map(Vec::len), Some(0));

        // Absent a nested route in this process's env, the dir is the ordinary per-cwd C7 slot —
        // exactly what `spawn_background`'s `RunPaths::for_run` created.
        if crate::spawn::nested_events::resolve_inherited_nested_route_from_env(|key| {
            std::env::var(key).ok()
        })
        .is_none()
        {
            let expected = RunPaths::for_run(
                &default_async_root(dir.path()),
                &default_results_dir(dir.path()),
                &run_id,
            )
            .run_dir;
            assert_eq!(
                details.get("asyncDir").and_then(|v| v.as_str()),
                Some(expected.to_string_lossy().as_ref())
            );
        }
    }

    /// The chain that one missing key severed, end to end: an async launch's `details` must carry
    /// the run ACTIVE (not completed), write the `mission.json` binding into the async dir, and
    /// record the run's `status.json`/`events.jsonl` artifacts.
    ///
    /// Pre-fix `details` carried `asyncId` but no `asyncDir`, so `runStatusForResult`
    /// (`missions/lifecycle.ts:98`) fell through `results: []` to `"completed"` — a background
    /// mission was marked DONE at launch — no binding file was written, and
    /// `MissionSyncCompletionObserver` could never find one to reconcile against.
    #[test]
    fn an_async_launch_binds_its_mission_marks_it_active_and_records_its_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = scoped_mission_config(dir.path());
        let binding = crate::missions::prepare_mission_launch(
            &crate::missions::MissionLaunchParams {
                task: Some("run it in the background".to_string()),
                ..Default::default()
            },
            dir.path(),
            Some(&config),
            Some("sess"),
        )
        .expect("prepare")
        .expect("a task-bearing launch binds a mission");

        let run_id = crate::background::RunId::from_token("bgrun000043");
        let details = async_launch_details("single", &run_id, dir.path());
        let async_dir = PathBuf::from(
            details.get("asyncDir").and_then(|v| v.as_str()).expect("asyncDir is reported"),
        );
        // The spawn creates this directory; the binding file lands in it.
        std::fs::create_dir_all(&async_dir).expect("mkdir");

        crate::missions::attach_mission_to_launch_result(
            &binding,
            crate::missions::LaunchOutcome {
                content: vec![cyrup_core::Content::text("started")],
                details: Some(details),
                is_error: false,
            },
        )
        .expect("attach");

        let record =
            crate::missions::read_mission(&binding.location, &binding.mission_id).expect("read");
        assert_eq!(record.status, crate::missions::MissionStatus::Active);
        assert_eq!(record.runs.len(), 1);
        assert_eq!(
            record.runs[0].status.as_deref(),
            Some("active"),
            "an async launch is ACTIVE, not completed"
        );
        assert_eq!(record.runs[0].async_dir.as_deref(), Some(async_dir.to_string_lossy().as_ref()));
        assert!(record.runs[0].completed_at.is_none());

        let artifacts: Vec<&str> = record.artifacts.iter().map(|a| a.path.as_str()).collect();
        assert!(
            artifacts.contains(&async_dir.join("status.json").to_string_lossy().as_ref()),
            "{artifacts:?}"
        );
        assert!(
            artifacts.contains(&async_dir.join("events.jsonl").to_string_lossy().as_ref()),
            "{artifacts:?}"
        );

        // The binding file the completion observer later reads back.
        let read_back = crate::missions::read_mission_binding(&async_dir)
            .expect("binding parses")
            .expect("a binding file was written into the async dir");
        assert_eq!(read_back.mission_id, binding.mission_id);
    }

    /// pi's `SteerRunning` delivered-follow-up confirmation (`subagent-executor.ts:846-871`): the
    /// header sent over the broker MUST include the resolved agent name (`Follow-up for async run
    /// ${runId} (${agent}):`), not just the run id. Proven with a REAL on-disk running-run fixture
    /// (not mocked) and a fake, always-delivers `SteerChannel` that records exactly what it received.
    #[tokio::test]
    async fn control_resume_steer_running_follow_up_header_includes_the_agent_name() {
        struct RecordingSteerChannel {
            received: std::sync::Mutex<Vec<(String, String)>>,
        }
        impl crate::tui::intercom::SteerChannel for RecordingSteerChannel {
            fn steer(
                &self,
                target: String,
                text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                self.received.lock().expect("lock").push((target, text));
                Box::pin(async { Ok(true) })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run00042");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // A genuinely live run always carries a real runner pid (`RunStatus::pid`'s own doc: "in
        // practice this is `Some` from the very first write") — the SteerRunning arm's own
        // interrupt-precondition check (pi `interruptLiveAsyncResumeTarget`,
        // `background/async-resume.ts:53-56`) now requires exactly that before it will even
        // attempt to interrupt, so this fixture must supply one to exercise the delivery path
        // rather than the "no interrupt-capable runner pid was found" abort.
        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write running status fixture");

        let steer = Arc::new(RecordingSteerChannel {
            received: std::sync::Mutex::new(Vec::new()),
        });
        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            steer.clone(),
        );

        let confirmation = executor
            .control_resume(dir.path(), Some("run00042"), Some("carry on"), None, None)
            .await
            .expect("a running child with a delivering steer channel resumes via live steer");
        assert!(
            confirmation.starts_with("Interrupted live async child, then delivered follow-up."),
            "got: {confirmation}"
        );

        let received = steer.received.lock().expect("lock");
        assert_eq!(received.len(), 1, "the follow-up must be delivered exactly once");
        assert!(
            received[0].1.starts_with("Follow-up for async run run00042 (researcher):\n\n"),
            "the follow-up header must include the resolved agent name, got: {:?}",
            received[0].1
        );
    }

    /// pi's `deliverSubagentIntercomMessageEvent` bounds EVERY caller — including this live-child
    /// follow-up steer (`subagent-executor.ts:860`) — to a 500ms default timeout race
    /// (`result-intercom.ts:325-358`): the caller's own turn is never blocked longer than that
    /// waiting on a delivery ack. Proven with a `SteerChannel` whose `steer` never resolves at all
    /// (the real-world shape of "no receiver ever answers"): pre-fix, `control_resume` awaited the
    /// raw `SteerChannel::steer` future directly with no outer race, so this would hang forever;
    /// post-fix it must resolve to the "not registered" fallback within a small bounded multiple of
    /// [`crate::tui::intercom::DEFAULT_STEER_TIMEOUT`] (500ms).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn control_resume_steer_running_degrades_within_the_bounded_timeout_when_steer_never_resolves() {
        struct HangingSteerChannel;
        impl crate::tui::intercom::SteerChannel for HangingSteerChannel {
            fn steer(
                &self,
                _target: String,
                _text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                Box::pin(std::future::pending())
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run00099");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // A genuinely live run always carries a real runner pid (`RunStatus::pid`'s own doc: "in
        // practice this is `Some` from the very first write") — the SteerRunning arm's own
        // interrupt-precondition check (pi `interruptLiveAsyncResumeTarget`,
        // `background/async-resume.ts:53-56`) now requires exactly that before it will even
        // attempt to interrupt, so this fixture must supply one to exercise the delivery path
        // rather than the "no interrupt-capable runner pid was found" abort.
        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write running status fixture");

        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            Arc::new(HangingSteerChannel),
        );

        let started = std::time::Instant::now();
        // Wrapped in an explicit, generous outer bound so a regression back to the pre-fix
        // unbounded-await behavior fails this test with a clear message instead of hanging the
        // whole suite indefinitely.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.control_resume(dir.path(), Some("run00099"), Some("carry on"), None, None),
        )
        .await
        .expect(
            "control_resume must resolve well within 5s even when the steer channel never \
             resolves — pre-fix this awaited the raw SteerChannel future with no outer race and \
             would hang forever",
        );
        let elapsed = started.elapsed();

        // A steer that never resolves must degrade to the documented "not registered" fallback —
        // never hang the caller's own turn indefinitely.
        let err = outcome.expect_err("an undelivered steer must degrade to the not-registered fallback");
        assert!(
            err.starts_with("Async child appears live but its intercom target is not registered."),
            "got: {err}"
        );
        assert!(
            elapsed < crate::tui::intercom::DEFAULT_STEER_TIMEOUT * 5,
            "must not block the caller's turn far past the documented 500ms steer timeout bound, \
             got: {elapsed:?}"
        );
    }

    /// pi `interruptLiveAsyncResumeTarget` (`background/async-resume.ts:53-56`): before EVER
    /// attempting to interrupt (or delivering any follow-up), `resume`'s live-steer arm must
    /// re-reconcile and REQUIRE `status.state === "running"` with a numeric pid — a run whose
    /// overall state claims `Running` but carries no known runner pid must abort the WHOLE resume
    /// with pi's exact diagnostic, never silently fall through to "steering" a child that was never
    /// confirmed interruptible. Pre-fix, `control_resume`'s `SteerRunning` arm discarded
    /// `control::interrupt`'s own `Ok(NotRunning)`-shaped outcomes (and, indirectly, a pid-less
    /// status) and proceeded straight to an intercom-delivery attempt regardless.
    #[tokio::test]
    async fn control_resume_steer_running_requires_a_running_status_with_a_known_pid() {
        struct RecordingSteerChannel {
            received: std::sync::Mutex<Vec<(String, String)>>,
        }
        impl crate::tui::intercom::SteerChannel for RecordingSteerChannel {
            fn steer(
                &self,
                target: String,
                text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                self.received.lock().expect("lock").push((target, text));
                Box::pin(async { Ok(true) })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run0nopid");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // `Running` overall state, but NO known runner pid (`pid: None`) — pi's guard treats this
        // identically to "no interrupt-capable runner pid was found", never as a steerable child.
        let mut status = crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, None);
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write pid-less running status fixture");

        let steer = Arc::new(RecordingSteerChannel {
            received: std::sync::Mutex::new(Vec::new()),
        });
        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            steer.clone(),
        );

        let err = executor
            .control_resume(dir.path(), Some("run0nopid"), Some("carry on"), None, None)
            .await
            .expect_err("a Running status with no known pid must abort the resume outright");
        assert_eq!(
            err,
            "Async run run0nopid is live but no interrupt-capable runner pid was found."
        );
        assert!(
            steer.received.lock().expect("lock").is_empty(),
            "no follow-up may ever be delivered when the interrupt precondition itself was never \
             satisfied"
        );
    }

    /// pi `target.cwd ?? requestCwd` (`subagent-executor.ts:890`, fed by `status.cwd ?? result.cwd`
    /// at `background/async-resume.ts:373`): a terminal-revival `resume` must resolve the revived
    /// child's persona against the ORIGINAL run's own cwd (persisted onto `status.json` by
    /// `finish_run`), not whatever cwd happens to be current at resume time. Proven with a custom
    /// agent defined ONLY under the original run's cwd: pre-fix, `revive_from_transcript` always
    /// discovered against the REQUEST cwd, so this agent would never be found and the call would
    /// fail with `agent not found: orig-only-agent` before ever reaching the (deliberately, via
    /// `max_subagent_depth = 0`) blocked spawn step; post-fix it must resolve the persona
    /// successfully and fail one step later, at the depth ceiling instead — proving the ORIGINAL
    /// cwd was searched. The depth block keeps this test from ever reaching a real detached process
    /// spawn.
    #[tokio::test]
    async fn control_resume_revive_prefers_the_original_runs_cwd_over_the_request_cwd() {
        let orig_dir = tempfile::tempdir().expect("orig tempdir");
        let request_dir = tempfile::tempdir().expect("request tempdir");

        let agents_dir = orig_dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir orig agents dir");
        std::fs::write(
            agents_dir.join("orig-only-agent.md"),
            "---\nname: orig-only-agent\ndescription: Only discoverable under orig_dir\n---\nBody.\n",
        )
        .expect("write orig-only-agent fixture");

        let session_file = orig_dir.path().join("session.jsonl");
        std::fs::write(&session_file, "").expect("write dummy session file");

        // The source run's storage lives under `request_dir`'s own async root (resume looks it up
        // via the REQUEST cwd, matching pi's fixed-but-here-cwd-scoped async/results roots) — only
        // the run's OWN recorded `cwd` field (set by `finish_run`) points back at `orig_dir`.
        let async_root = default_async_root(request_dir.path());
        let results_dir = default_results_dir(request_dir.path());
        let run_id = RunId::from_token("run0revive");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");

        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("orig-only-agent");
        step.status = crate::background::StepState::Complete;
        step.session_file = Some(session_file.clone());
        status.steps = vec![step];
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        status.cwd = Some(orig_dir.path().to_path_buf());
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write terminal status fixture");

        let executor = SubagentExecutor::new();
        {
            // Block the spawn AFTER persona resolution succeeds, so a correct fix observably fails
            // at the depth ceiling instead of ever launching a real detached subprocess.
            let mut cfg = executor.config_cell().lock().await;
            cfg.max_subagent_depth = 0;
        }

        let err = executor
            .control_resume(
                request_dir.path(),
                Some("run0revive"),
                Some("please continue"),
                None,
                None,
            )
            .await
            .expect_err("the blocked depth ceiling must still reject this revive");

        assert!(
            err.contains("depth limit exceeded"),
            "the revived persona must resolve against the ORIGINAL run's cwd (orig_dir), reaching \
             the depth ceiling, not fail with an agent-not-found error from searching the request \
             cwd instead; got: {err}"
        );
        assert!(
            !err.contains("agent not found"),
            "pre-fix regression: revive_from_transcript searched the REQUEST cwd (which has no \
             'orig-only-agent') instead of the original run's own cwd; got: {err}"
        );
    }

    // =====================================================================================
    // Tier-2 (c): package-tier enumeration -> a package agent is discovered at Package scope.
    // =====================================================================================

    /// (c) A package that declares an `agents` dir (here via manifest auto-discovery of a Path-source
    /// package's conventional `agents/`) has its persona discovered at
    /// [`crate::discovery::types::AgentSource::Package`] once the installed-packages registry is
    /// enumerated into the discovery config (the wire-up [`enumerate_installed_packages`] +
    /// `discovery_config` perform for real).
    #[test]
    fn a_package_provided_agent_is_discovered_at_package_scope() {
        let home = tempfile::tempdir().expect("tempdir");
        let global_dir = home.path().join(".cyrup");
        // A real on-disk package tree with a conventional agents/ dir holding one persona.
        let pkg_root = home.path().join("code-analysis-pkg");
        let agents_dir = pkg_root.join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir package agents dir");
        std::fs::write(
            agents_dir.join("analyzer.md"),
            "---\nname: analyzer\ndescription: A package-provided analyzer agent\n---\nYou analyze code.\n",
        )
        .expect("write package agent file");

        // Persist a Global-scope, Path-source install record in the global packages.json registry —
        // exactly what `enumerate_installed_packages` loads.
        let installed = cyrup_resources::InstalledPackages {
            packages: vec![cyrup_resources::InstalledPackage {
                id: cyrup_core::PackageId::from("path:code-analysis".to_string()),
                source: cyrup_resources::PackageSource::Path {
                    path: pkg_root.clone(),
                },
                scope: cyrup_resources::InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            }],
        };
        let store = cyrup_resources::PackageStore::new(global_dir.clone(), None);
        let registry_path = store
            .registry_path(cyrup_resources::InstallScope::Global)
            .expect("global registry path");
        cyrup_resources::package::lock::save(&registry_path, &installed)
            .expect("persist packages.json");

        // The wire-up under test: enumerate the registry, then discover.
        let enumerated = enumerate_installed_packages(&global_dir, None);
        assert_eq!(
            enumerated.packages.len(),
            1,
            "the global packages.json registry must enumerate its one installed package"
        );

        let cfg = AgentDiscoveryConfig {
            builtin_agents_dir: None,
            installed_packages: enumerated,
            global_dir,
            project_root: None,
            trusted_project: false,
            ..AgentDiscoveryConfig::default()
        };
        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let analyzer = result
            .agents
            .iter()
            .find(|a| a.name == "analyzer")
            .expect("the package-provided analyzer agent must be discovered");
        assert_eq!(
            analyzer.source,
            crate::discovery::types::AgentSource::Package,
            "a package-provided agent must be discovered at Package scope"
        );
    }

}
