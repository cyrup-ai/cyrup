//! VL-S11b — `/subagents-detach`: the producer that makes cyrup's already-built detached-run
//! READER half reachable, proved against a REAL OS child process.
//!
//! The frozen contract both halves are written against is
//! `crates/cyrup-ext-subagents/src/extension/executor/detach.rs`. Every sentence asserted below is
//! quoted from it (and from pi `slash/slash-commands.ts:978-1001` @v0.68.0) as a LITERAL rather
//! than through the constant, because those items are `pub(crate)`: an integration test that
//! imported them could not fail on a change to the bytes, which is the only thing these
//! assertions are for.
//!
//! # Why this file exists at all, and what is unique to it
//!
//! `detach.rs`'s own unit tests already prove the handshake (accept, refuse, the
//! settle-after-accept rule) and the message builders. None of that can prove the one property
//! the feature exists for:
//!
//! > **cyrup's foreground child is a real OS process owned by the `drive_foreground_run_sync`
//! > future** (`detach.rs:16-20`). Dropping that future kills the child. An implementation that
//! > "detaches" by cancelling the run passes every in-crate test and KILLS the thing the feature
//! > exists to preserve.
//!
//! So the load-bearing assertion in this file — and the most valuable one in its batch — is the
//! `/proc`-backed PID liveness check taken IMMEDIATELY after the command returns, in
//! [`a_live_detach_returns_the_receipt_and_leaves_its_child_running`]. It is an OS-level probe,
//! never this crate's own bookkeeping, and it is not a sleep-and-hope: the child is read out of
//! `/proc` by the unique `--fixture-script` path this test alone passes.
//!
//! # Bounded waits
//!
//! Every wait here has a deadline whose failure message says which of the two things happened:
//! "the thing never happened" versus "the thing happened late". A bare hang in CI teaches
//! nothing. The one deadline that is load-bearing rather than defensive is
//! [`RECEIPT_BUDGET`], which is deliberately far SHORTER than the scripted child's own sleep
//! ([`CHILD_SLEEP_MS`]) — so a receipt that only arrives after the child exits (i.e. an
//! implementation that never split the run and simply awaited it) is a TIMEOUT here, not a pass.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, Tool, ToolCallId};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::background::{RunId, run_artifact_roots_in};
use cyrup_ext_subagents::discovery::types::AgentReadScope;
use cyrup_ext_subagents::exec::SingleResult;
use cyrup_ext_subagents::extension::{
    ForegroundRunRequest, SingleRunOverrides, SubagentsExtension, WaitTool,
};
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;

/// How long the scripted child sleeps before emitting its terminal turn. Long enough that the
/// detach, the `status` probe and the start of the `bg_wait` all happen while it is genuinely
/// still running, and long enough that [`RECEIPT_BUDGET`] cannot be satisfied by waiting for it.
const CHILD_SLEEP_MS: u64 = 10_000;

/// How long the detach receipt may take to come back. Deliberately a small fraction of
/// [`CHILD_SLEEP_MS`]: the whole point of the detach split is that the caller is answered while
/// the child keeps running, so a receipt that arrives only at child-exit time is a FAILURE here
/// and the timeout message says so.
const RECEIPT_BUDGET: Duration = Duration::from_secs(4);

/// The ceiling on every "eventually" poll below. Comfortably longer than [`CHILD_SLEEP_MS`] so a
/// timeout means the event never happened, not that it was slow.
const SETTLE_BUDGET: Duration = Duration::from_secs(45);

// =================================================================================================
// OS-level liveness — lifted verbatim from `background_spawn_detached_integration.rs:80-128`
// =================================================================================================

/// Report whether the OS still considers `pid` a genuinely *running* process.
///
/// Verbatim (rationale included) from `background_spawn_detached_integration.rs`, which is the
/// other file in this suite whose whole subject is "did a process survive": `kill -0` succeeds for
/// a ZOMBIE, and under this suite's container pid 1 does not reap orphans, so a bare `kill -0`
/// would claim a cleanly-exited child is alive forever. `/proc/<pid>/stat` field 3 is the process
/// state character; `Z` is not alive, every other state is.
fn pid_is_alive(pid: u32) -> bool {
    match proc_state_char(pid) {
        Some(state) => state != 'Z',
        None => kill_zero_succeeds(pid),
    }
}

/// Field 2 (`comm`) is parenthesized and may itself contain spaces and `)`, so the fields after it
/// are located from the LAST `)` in the line rather than by splitting from the start.
fn proc_state_char(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

fn kill_zero_succeeds(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn kill_pid_for_cleanup(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// Every live pid whose `/proc/<pid>/cmdline` names `script_path`.
///
/// This is how the test gets the CHILD's pid without reaching into the crate's private control
/// registry: the fixture is spawned as `<fixture-binary> --fixture-script <path>` and `path` is a
/// per-test tempdir file, so a cmdline carrying it belongs to this test's child and nothing else.
/// `cmdline` is NUL-separated.
fn fixture_pids(script_path: &Path) -> Vec<u32> {
    let needle = script_path.display().to_string();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
        if cmdline.contains(&needle) {
            out.push(pid);
        }
    }
    out
}

// =================================================================================================
// Fixtures
// =================================================================================================

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// A child that BLOCKS for `sleep_ms` and then finishes cleanly. `sleep_ms` is the fixture's only
/// blocking primitive (`bin/cyrup_subagent_fixture.rs`'s `ScriptStep::SleepMs`), and blocking is
/// the precondition for every assertion here: a detach of an already-settled run is a refusal, not
/// a detach.
fn blocking_child_script(dir: &Path, name: &str, sleep_ms: u64) -> PathBuf {
    let script = serde_json::json!({
        "steps": [
            { "kind": "sleep_ms", "ms": sleep_ms },
            { "kind": "emit", "line": message_end_line("DETACHED_CHILD_FINISHED") }
        ],
        "exit_code": 0
    });
    let path = dir.join(name);
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

/// The persona every run below uses, discovered through the REAL project-scope pipeline
/// (`<cwd>/.cyrup/agents`), pinned at the fixture model so no provider traffic is possible.
fn write_worker_persona(cwd: &Path) {
    let agents = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents.join("worker.md"),
        "---\nname: worker\ndescription: a blocking fixture persona for the VL-S11b detach \
         proofs\nmodel: fixture/model\n---\n\nYou are a trivial test persona.\n",
    )
    .expect("write worker persona");
}

/// The session every remembered run here is attributed to.
///
/// NOT optional. `remember_foreground_run` refuses a run with no `current_session_id` **by type**
/// (`ForegroundHistoryRun::session_id` is a required `SessionId`), and `run_foreground_impl`'s
/// detach arm treats that refusal as a failure to publish the receipt: it closes the gate with
/// `DetachRefusal::LifecycleFinished`, leaves the run ATTACHED, and the command renders
/// *"Foreground run … is not currently detachable."* instead of the success sentence. So a
/// detach IT that binds no session is not testing the detach — it is testing that arm.
const SESSION: &str = "detach-session";

struct FixedSessionHost;

impl cyrup_ext::host::HostServices for FixedSessionHost {
    fn session_id(&self) -> Option<String> {
        Some(SESSION.to_string())
    }
}

fn extension(home: &Path, cwd: &Path, script: &Path) -> Arc<SubagentsExtension> {
    let ext = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            // A foreground run is the path `spawn_command` reaches (a detached hop-2 runner would
            // re-resolve its binary from the environment and the injection would be inert).
            async_by_default: false,
            spawn_command: Some(SpawnCommand {
                binary: crate::support::bins::subagent_fixture(),
                base_args: vec!["--fixture-script".to_string(), script.display().to_string()],
            }),
            roots: Roots::sandboxed(home),
            ..SubagentExtensionConfig::default()
        },
        cwd.to_path_buf(),
    ));
    ext.executor().set_host_services(Arc::new(FixedSessionHost));
    ext
}

fn command_ctx(cwd: &Path) -> HostCtx {
    HostCtx::command(ExtMode::Tui, true, cwd.to_path_buf())
}

/// Run `/subagents-detach <args>` through the REAL `NativeExtension::execute_command` dispatch.
async fn detach(ext: &SubagentsExtension, args: &str, ctx: &HostCtx) -> String {
    ext.execute_command("subagents-detach", args, ctx)
        .await
        .expect(
            "`/subagents-detach` must be a DISPATCHED command: an ExtError here means no \
             `SlashCommandName::SubagentsDetach` arm exists in `dispatch_slash`",
        )
        .expect("the `/subagents-detach` handler renders text")
}

/// Launch one foreground single run on the blocking script, detached from this task so the test
/// can act while it is still in flight. The join handle yields the run's own `SingleResult`.
fn launch_blocking_run(
    ext: &Arc<SubagentsExtension>,
    cwd: &Path,
    task: &str,
) -> tokio::task::JoinHandle<SingleResult> {
    let executor = Arc::clone(ext.executor());
    let cwd = cwd.to_path_buf();
    let task = task.to_string();
    tokio::spawn(async move {
        executor
            .run_foreground(&cwd, "worker", &task, None, None, None)
            .await
            .expect("the foreground run resolves its persona and spawns its child")
    })
}

/// Every LIVE foreground run id this executor is driving, newest-sorted by the projection itself.
/// `fleet_state` is the only public window onto `foreground_controls` (`status.rs:200`).
async fn live_foreground_ids(ext: &SubagentsExtension, cwd: &Path) -> Vec<String> {
    ext.executor()
        .fleet_state(cwd, false, false)
        .await
        .foreground_controls
        .into_iter()
        .map(|control| control.run_id)
        .collect()
}

/// Poll `probe` until it answers `Some`, or fail with `never` after `budget`.
async fn eventually<T, F, Fut>(budget: Duration, never: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + budget;
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{never} (nothing observed within {budget:?} — the event NEVER happened; a slow-but-\
             eventual event would have been observed by a later poll inside this same budget)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `<results_dir>/foreground-history.json`, the file pi's `foreground-history.ts:67-69,89-100`
/// deliberately never writes a detached run into (ported at
/// `foreground_history/persist.rs:19-21,249-253`).
fn history_path(home: &Path, cwd: &Path) -> PathBuf {
    run_artifact_roots_in(&Roots::sandboxed(home), cwd)
        .results_dir
        .join("foreground-history.json")
}

/// Whether the on-disk foreground history names `run_id` at all. A missing file is "no".
fn history_names(path: &Path, run_id: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text.contains(run_id))
        .unwrap_or(false)
}

// =================================================================================================
// §I.10 — three sentences from ONE resolver
// =================================================================================================

/// pi `selectForegroundDetachControl` (`slash-commands.ts:237-250`) has THREE outcomes at two
/// severities, and `detach.rs`'s `DetachTarget` names all three. The existing
/// `SubagentExecutor::resolve_live_foreground_run` (`nested_control.rs:211`) returns
/// `Option<String>` and so collapses AMBIGUOUS into NOT-FOUND — its own doc says an ambiguous
/// prefix "is deliberately NOT treated as a foreground match".
///
/// Gutted: reuse that collapsing resolver and the third assertion fires, because an ambiguous
/// prefix comes back as `No active foreground run found for '<prefix>'.` instead of the ambiguity
/// sentence. The first two assertions still PASS in that world, which is exactly why all three
/// live in one test against one resolver.
///
/// # Why this test launches several runs
///
/// A `RunId` is 128 bits of uuid entropy rendered as 32 lowercase hex characters
/// (`background/run_id.rs:38-41`), so two runs share a prefix only by chance. The loop below
/// launches runs until two ids share their first hex character. That is not flaky: there are 16
/// possible first characters, so by the pigeonhole principle a collision is GUARANTEED at 17
/// distinct ids, and the loop's cap is 17. In practice it stops at four or five.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_three_detach_resolver_sentences_all_come_from_one_resolver() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    write_worker_persona(cwd.path());
    let script = blocking_child_script(cwd.path(), "blocking.json", 120_000);
    let ext = extension(home.path(), cwd.path(), &script);
    let ctx = command_ctx(cwd.path());

    // (1) NOTHING LIVE — pi `:987`'s no-id branch, an `info`.
    let none_live = detach(&ext, "", &ctx).await;
    assert!(
        none_live.contains("No active foreground single-subagent run to detach."),
        "with no live foreground run at all, the no-id form is upstream's own sentence; \
         got: {none_live}"
    );

    // Launch until two ids share a first hex character. See this test's doc for why 17 is a
    // guarantee rather than a hope.
    let mut handles = Vec::new();
    let mut shared: Option<(char, Vec<String>)> = None;
    for n in 1..=17usize {
        handles.push(launch_blocking_run(&ext, cwd.path(), &format!("block {n}")));
        let ids = eventually(
            SETTLE_BUDGET,
            "the foreground control registry never grew",
            || {
                let ext = Arc::clone(&ext);
                let cwd = cwd.path().to_path_buf();
                async move {
                    let ids = live_foreground_ids(&ext, &cwd).await;
                    (ids.len() == n).then_some(ids)
                }
            },
        )
        .await;

        shared = "0123456789abcdef".chars().find_map(|c| {
            let matched: Vec<String> = ids.iter().filter(|id| id.starts_with(c)).cloned().collect();
            (matched.len() >= 2).then_some((c, matched))
        });
        if shared.is_some() {
            break;
        }
    }
    let (prefix_char, mut matched) = shared.expect(
        "17 distinct 32-hex-char run ids cannot all differ in their first character (16 values, \
         pigeonhole) — reaching here means the ids are not what `RunId::new` mints",
    );
    let prefix = prefix_char.to_string();

    // (2) AN ID THAT MATCHES NOTHING — pi `:987`'s id branch. `zzz` is not a hex prefix, so it
    // matches none of the live ids no matter how many there are.
    let no_match = detach(&ext, "zzz", &ctx).await;
    assert!(
        no_match.contains("No active foreground run found for 'zzz'."),
        "an id naming nothing gets upstream's id-shaped sentence, NOT the no-id one; \
         got: {no_match}"
    );

    // (3) AN AMBIGUOUS PREFIX — pi `:241`'s throw, rendered by the handler's catch as an `error`.
    // This is the outcome a two-way `Option` resolver cannot express.
    let ambiguous = detach(&ext, &prefix, &ctx).await;
    matched.sort();
    assert!(
        ambiguous.contains(&format!(
            "Ambiguous foreground run id prefix '{prefix}' matched: "
        )),
        "a prefix matching {} live runs must be AMBIGUOUS, not not-found and not an arbitrary \
         pick — {:?} all start with '{prefix}'; got: {ambiguous}",
        matched.len(),
        matched
    );
    assert!(
        ambiguous.contains(". Provide a longer id."),
        "the ambiguity sentence ends with upstream's own remedy; got: {ambiguous}"
    );
    for id in &matched {
        assert!(
            ambiguous.contains(id.as_str()),
            "the ambiguity sentence lists EVERY match, so the human can pick one; {id} is \
             missing from: {ambiguous}"
        );
    }

    for handle in handles {
        handle.abort();
    }
    for pid in fixture_pids(&script) {
        kill_pid_for_cleanup(pid);
    }
}

// =================================================================================================
// §I.11 + §I.12 + §I.13 — the live detach, end to end
// =================================================================================================

/// The whole feature, in one arc, against one real child:
///
/// * **§I.11** the receipt comes back naming both recovery verbs, the child is STILL ALIVE, and
///   the `SingleResult` carries upstream's `detached`/`exit_code`/`detached_reason` triple;
/// * **§I.12** the detached run stays addressable — `subagent({action:"status", id})` renders it
///   and `bg_wait({id})` BLOCKS — and reconciles when the child finally exits;
/// * **§I.13** it never reaches disk while detached, and DOES once it settles.
///
/// They are one test because they are one run: splitting them would mean launching, detaching and
/// settling three separate children to assert three facts about the same handover, and the §I.12
/// assertions are only meaningful against the exact run §I.11 detached.
///
/// Gutted, assertion by assertion:
///
/// * an implementation that CANCELS instead of detaching kills the child → the liveness assertion
///   fires;
/// * one that returns the sentence without minting the receipt → the `SingleResult` assertions
///   fire;
/// * one that never returns early (drops nothing, just awaits the child) → the receipt TIMES OUT
///   inside [`RECEIPT_BUDGET`], which is far shorter than the child's own sleep;
/// * without `status.rs:584`'s second arm over the remembered runs → the status probe gets
///   `Async run not found. Provide id or dir.`;
/// * without `active_detached_foreground_runs` in `background/wait.rs` → `bg_wait` returns
///   immediately instead of blocking, and the elapsed-time assertion fires;
/// * without the continuation task → the remembered child status stays `"detached"` forever, the
///   `bg_wait` times out, and the history file never gains the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_detach_returns_the_receipt_and_leaves_its_child_running() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    write_worker_persona(cwd.path());
    let script = blocking_child_script(cwd.path(), "blocking.json", CHILD_SLEEP_MS);
    let ext = extension(home.path(), cwd.path(), &script);
    let ctx = command_ctx(cwd.path());
    let history = history_path(home.path(), cwd.path());

    let handle = launch_blocking_run(&ext, cwd.path(), "block for a long while");

    // The run is registered and its child is a real, running OS process.
    let run_id = eventually(
        SETTLE_BUDGET,
        "the foreground run never registered a live control",
        || {
            let ext = Arc::clone(&ext);
            let cwd = cwd.path().to_path_buf();
            async move { live_foreground_ids(&ext, &cwd).await.into_iter().next() }
        },
    )
    .await;
    let child_pid = eventually(
        SETTLE_BUDGET,
        "the scripted child never appeared in /proc",
        || {
            let script = script.clone();
            async move { fixture_pids(&script).into_iter().next() }
        },
    )
    .await;
    assert!(
        pid_is_alive(child_pid),
        "precondition: the child must be running BEFORE the detach, or the detach is a no-op on \
         an already-settled run"
    );
    assert!(
        !history_names(&history, &run_id),
        "precondition: a still-running foreground run is not in {} yet",
        history.display()
    );

    // ---- (a) the receipt sentence ----
    let receipt = detach(&ext, "", &ctx).await;
    assert!(
        receipt.contains(&format!(
            "Detached foreground run {run_id} without terminating its child."
        )),
        "the success sentence must name THIS run id (pi `:999`); got: {receipt}"
    );
    assert!(
        receipt.contains(&format!(
            "subagent({{ action: \"status\", id: \"{run_id}\" }})"
        )),
        "the receipt quotes the `status` recovery verb with the id filled in — a model told its \
         run detached and given no way to recover it is the bug this sentence closes; \
         got: {receipt}"
    );
    assert!(
        receipt.contains(&format!("bg_wait({{ id: \"{run_id}\" }})")),
        "and the REGISTERED wait-tool name (VL-S8's `bg_wait`, not `wait`/`subagent_wait`); \
         got: {receipt}"
    );

    // ---- (b) THE ASSERTION THIS FILE EXISTS FOR ----
    // Immediately, with no sleep: the child the detach was supposed to preserve is still running.
    assert!(
        pid_is_alive(child_pid),
        "THE POINT OF THE FEATURE: /subagents-detach must not terminate its child. pid \
         {child_pid} is gone (or a zombie) right after the command returned, which is what an \
         implementation that CANCELS the run — or that drops the driving future and with it the \
         `SpawnedChild` — produces. Receipt was: {receipt}"
    );

    // ---- (b2) AND STILL ALIVE A MOMENT LATER ----
    //
    // The probe above is kept exactly as it was and this one is ADDED, because the batch's own
    // verify pass ran the mutation (b) names — `spawn_detached_foreground_continuation` drops the
    // drive future instead of moving it into a task that owns it — and **(b) passed**.
    //
    // It passed for a reason worth writing down rather than tuning around.
    // `SpawnedChild::drop` (`spawn/mod.rs:1086-1099`) does not terminate anything synchronously:
    // it SIGKILLs the child's process GROUP, and delivery plus the transition to `Z` is the
    // kernel's business, not the dropping thread's. A probe taken with no sleep at all therefore
    // reads the state the child had a few microseconds ago — which is `R`/`S`, i.e. alive — and
    // (b) is satisfied by a child that is already condemned.
    //
    // So (b) proves "not killed SYNCHRONOUSLY by the detach", which is worth proving and is not
    // what the file's doc claims for it. This probe proves the rest: after a delay that is a small
    // fraction of [`CHILD_SLEEP_MS`], a child a cancel-based implementation killed is gone or a
    // zombie, and a child a real detach handed over is still doing its scripted sleep. It is not
    // a sleep-and-hope — it is a sleep-and-DISPROVE, and the thing it disproves is the one
    // implementation that passes every in-crate test.
    tokio::time::sleep(Duration::from_millis(750)).await;
    assert!(
        pid_is_alive(child_pid),
        "the child survived the instant of the detach but was gone 750ms later, with ~{}ms of \
         its scripted sleep still to run. That is an asynchronous kill — a dropped drive future \
         SIGKILLing the process group from `SpawnedChild::drop` — not a handover. Receipt was: \
         {receipt}",
        CHILD_SLEEP_MS - 750
    );

    // ---- (c) the receipt the run itself returns ----
    let result = tokio::time::timeout(RECEIPT_BUDGET, handle)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the foreground run did not return a receipt within {RECEIPT_BUDGET:?} while its \
                 child still has ~{CHILD_SLEEP_MS}ms of scripted sleep left. That is not a slow \
                 receipt — it is NO receipt: the run is still being awaited to completion, so the \
                 detach never split `run_foreground_impl` in two."
            )
        })
        .expect("the foreground run task did not panic");

    assert!(
        result.detached,
        "pi `execution.ts:646-648` sets `detached = true` on the receipt; got: {result:#?}"
    );
    assert_eq!(
        result.exit_code, -2,
        "pi `execution.ts:618`'s `exitCode = -2`, as distinct from any real process exit"
    );
    assert_eq!(
        result.detached_reason.as_deref(),
        Some("user request"),
        "pi `subagent-executor.ts:3978` — `/subagents-detach` stamps the USER-REQUEST reason, not \
         the intercom one; got: {:?}",
        result.detached_reason
    );
    assert_eq!(
        result.final_output.as_deref(),
        Some("Detached at user request before task completion."),
        "pi `execution.ts:621-625`'s reason-keyed `finalOutput`"
    );

    // ---- §I.13, first half: never on disk while detached ----
    assert!(
        !history_names(&history, &run_id),
        "a DETACHED run is deliberately never persisted (pi `foreground-history.ts:67-69`, \
         ported at `persist.rs:249-253`); {} names {run_id}",
        history.display()
    );

    // ---- §I.12, first: the run is still addressable by id ----
    let status = subagent_status(&ext, &run_id).await;
    assert!(
        !status.contains("Async run not found. Provide id or dir."),
        "a detached foreground run is by construction NOT in `foreground_controls`, so without \
         `control_status`'s SECOND arm over the remembered runs (pi `run-status.ts:421`) the id \
         falls through to the async resolver's not-found notice; got: {status}"
    );
    // Tightened after the run went green. `contains(&run_id)` alone would also pass on the
    // not-found notice if it ever quoted the id, and on any renderer that merely echoed it. These
    // two literals are `format_remembered_foreground_status`' own first two lines
    // (`extension/executor/status.rs:1022-1023`), so they can only come from the SECOND arm —
    // which is the thing this assertion exists to prove is wired.
    assert!(
        status.contains(&format!("Run: {run_id}")),
        "the status report must render THIS run through the remembered-foreground renderer; \
         got: {status}"
    );
    assert!(
        status.contains("State: remembered foreground"),
        "and it must be THAT renderer — `State: remembered foreground` is the line only \
         `format_remembered_foreground_status` emits; got: {status}"
    );

    // ---- §I.12, second: `bg_wait({ id })` BLOCKS, then returns the terminal result ----
    let started = Instant::now();
    let wait_text = bg_wait(&ext, cwd.path(), &run_id).await;
    let blocked_for = started.elapsed();

    assert!(
        blocked_for >= Duration::from_secs(2),
        "`bg_wait({{ id }})` returned after only {blocked_for:?} on a run whose child still had \
         seconds of scripted sleep left — i.e. it did NOT block. The candidate set was async runs \
         ALONE, so the detached FOREGROUND run was invisible to it. Check that \
         `WaitTool::execute` (`extension/wait_tool.rs:192-220`) actually calls \
         `WaitDeps::with_detached_foreground(Some(…))` with a live \
         `DetachedForegroundRunsSource` over the executor's `foreground_runs` map: \
         `active_detached_foreground_runs` (`background/wait.rs:616-622`) returns EMPTY when that \
         hook is `None`, whatever else is wired. Returned: {wait_text}"
    );
    assert!(
        blocked_for < SETTLE_BUDGET,
        "and it must have RETURNED, not timed out at its own budget; took {blocked_for:?}"
    );
    // Tightened after the run went green, against the bytes in
    // `background/wait.rs:1254-1258` rather than against a quote of them. `contains(&run_id)`
    // was the loose form this was written with, because the settled sentence did not exist yet;
    // it would pass on ANY string that mentioned the run, including the attention sentence, the
    // "no remembered detached foreground run remained" refusal, or a progress line.
    //
    // Both halves matter and they fail differently:
    //
    // * `; done. Outcome: 1 completed.` is the SETTLED arm (`:1254-1255`) and its
    //   `summarize_foreground_children` count (`:657-673`), which skips any child still
    //   `"detached"`. A wait that returned before the child landed cannot produce it.
    // * the recovery clause is upstream's own `subagent({ action: "status", id })` pointer
    //   (`:1256-1257`), which is the ONLY thing that tells a caller where the recovered output
    //   went — a renderer that dropped it would still "resolve", and silently.
    assert!(
        wait_text.contains(&format!(
            "for remembered detached foreground run \"{run_id}\"; done. Outcome: 1 completed."
        )),
        "the wait must resolve through the SETTLED arm for THIS run, with its child counted as \
         completed rather than still detached (`background/wait.rs:1254-1255`); got: {wait_text}"
    );
    assert!(
        wait_text.contains(&format!(
            "Completion event observed; inspect with subagent({{ action: \"status\", id: \
             \"{run_id}\" }}) for recovered output."
        )),
        "and it must carry the recovery clause that names where the recovered output is \
         (`background/wait.rs:1256-1257`); got: {wait_text}"
    );

    // ---- §I.12, third: the remembered run's child status flips off "detached" ----
    let statuses = eventually(
        SETTLE_BUDGET,
        "the remembered foreground run's child never left the \"detached\" status",
        || {
            let ext = Arc::clone(&ext);
            let cwd = cwd.path().to_path_buf();
            let run_id = run_id.clone();
            async move {
                let state = ext.executor().fleet_state(&cwd, true, false).await;
                let run = state
                    .foreground_runs
                    .into_iter()
                    .find(|run| run.run_id == run_id)?;
                let statuses: Vec<String> = run.children.iter().map(|c| c.status.clone()).collect();
                (!statuses.is_empty() && statuses.iter().all(|s| s != "detached"))
                    .then_some(statuses)
            }
        },
    )
    .await;
    assert!(
        !statuses.is_empty(),
        "the reconciled run still has its child rows: {statuses:?}"
    );

    // ---- §I.13, second half: it DOES reach disk once it settles ----
    //
    // **This assertion was inverted by R-VLS11b-02.** It used to require that the run was still
    // ABSENT here, and to trigger the write with a SECOND ordinary foreground run — because
    // `spawn_detached_foreground_continuation` carried a `[CYRUP-DELTA]` saying it could not
    // persist (it holds only the `Arc` to the map, not the executor), so the reconciled run
    // reached disk only *"at the session's next foreground settle"*. R-VLS11b-02 added the
    // `&self`-free `persist_foreground_run_history_in`, and R-VLS11b-01 wired the continuation to
    // it, so the continuation now writes the run itself, with nothing else happening in the
    // session. The old shape is not merely stale — it would PASS against an implementation that
    // still could not persist, which is exactly the gap that was closed.
    //
    // It is still the same rule being proved. `persist.rs`'s `is_persistable` refuses a run any
    // of whose children is `"detached"` (`RESTORABLE` is the four terminal statuses), so this
    // file can only name the run if the continuation RECONCILED it first and then wrote it: an
    // implementation that persisted before reconciling writes nothing, and one that reconciles
    // but never persists never writes at all. Both time this poll out.
    eventually(
        SETTLE_BUDGET,
        "the settled detached run never reached the foreground history on disk — the \
         continuation reconciled it (the status poll above passed) but did not persist it, which \
         is the R-VLS11b-02 gap this call site closed",
        || {
            let history = history.clone();
            let run_id = run_id.clone();
            async move { history_names(&history, &run_id).then_some(()) }
        },
    )
    .await;

    for pid in fixture_pids(&script) {
        kill_pid_for_cleanup(pid);
    }
}

/// `subagent({ action: "status", id })` through the REAL registered tool, with the not-found
/// notice (which the tool reports as an `Err`) folded into the same string so the caller asserts
/// on ONE text regardless of which arm it came back on.
async fn subagent_status(ext: &SubagentsExtension, run_id: &str) -> String {
    let outcome = ext
        .subagent_tool()
        .execute(
            ToolCallId::from("detach-status"),
            serde_json::json!({ "action": "status", "id": run_id }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;
    match outcome {
        Ok(result) => result
            .content
            .iter()
            .filter_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(err) => err.message,
    }
}

/// `bg_wait({ id, timeoutMs })` through the REAL registered tool object
/// (`extension::WaitTool`, the same type `RegistrationMode::Full` installs on a live session),
/// so whatever candidate set the production tool assembles is the one under test.
async fn bg_wait(ext: &SubagentsExtension, cwd: &Path, run_id: &str) -> String {
    let tool = WaitTool::new(Arc::clone(ext.executor()), cwd.to_path_buf(), true);
    assert_eq!(
        tool.name(),
        "bg_wait",
        "sanity: this is the tool the model is told to call (VL-S8)"
    );
    let outcome = tool
        .execute(
            ToolCallId::from("detach-wait"),
            serde_json::json!({ "id": run_id, "timeoutMs": 40_000 }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;
    match outcome {
        Ok(result) => result
            .content
            .iter()
            .filter_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(err) => err.message,
    }
}

// =================================================================================================
// R-VLS11b-01 — the WORKFLOW-CHILD arm of the detach, against a real OS child
// =================================================================================================

/// Launch one foreground run shaped exactly as `WorkflowRunHost::launch` shapes its children —
/// `parent_workflow_run_id: Some(..)` — detached from this task so the test can act while it is
/// in flight. The join handle yields what the WORKFLOW STEP would receive.
///
/// `workflow_steer` stays `None`. Production sets it whenever `parent_workflow_run_id` is `Some`
/// (`requests.rs`'s "`Some` exactly when" rule), but it only decides where steer requests are
/// written and read; the detach fork reads `parent_workflow_run_id` alone, and building a real
/// steer handle would require a workflow run directory this test has no other use for.
fn launch_workflow_child(
    ext: &Arc<SubagentsExtension>,
    cwd: &Path,
    parent_workflow_run_id: RunId,
) -> tokio::task::JoinHandle<SingleResult> {
    let executor = Arc::clone(ext.executor());
    let cwd = cwd.to_path_buf();
    tokio::spawn(async move {
        executor
            .run_foreground_streaming(
                ForegroundRunRequest {
                    overrides: SingleRunOverrides::default(),
                    cwd: &cwd,
                    agent_name: "worker",
                    task: "block for a long while, as a workflow child",
                    agent_scope: AgentReadScope::Both,
                    context: None,
                    model_override: None,
                    timeout_ms: None,
                    cancel: CancelToken::new(),
                    // THE ONE FIELD UNDER TEST.
                    parent_workflow_run_id: Some(parent_workflow_run_id),
                    workflow_key: None,
                    workflow_steer: None,
                },
                // `run_foreground_streaming` takes the sink by value, not as an `Option`; this
                // test reads the RESULT, not the progress stream, so it discards every update.
                Box::new(|_: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect("the workflow child resolves its persona and spawns its child")
            .0
    })
}

/// R-VLS11b-01, end to end against a real OS child: `/subagents-detach` aimed at a WORKFLOW child
/// is ACCEPTED (upstream allows it — `subagent-executor.ts:6938` stamps `"single"` for a workflow
/// child and carries `parentWorkflowRunId` separately at `:7201`), the child keeps running, and
/// the launching call is **not** answered with the receipt: it keeps awaiting and returns the
/// child's real terminal result, which is upstream's `workflowAwaitDetached` promise (`:4009`,
/// resolved at `:4078`, awaited at `:4123`).
///
/// # Why this cannot be a unit test
///
/// `foreground.rs`'s `detach_producer_tests` call `hand_off_detached_foreground_run` directly with
/// a synthetic drive future, so they prove the FORK but not the plumbing into it. A mutation that
/// passes `parent_workflow_run_id: None` at the one call site inside `run_foreground_impl` — or
/// that reads `workflow_key` instead — leaves every one of those tests green while every workflow
/// child silently goes back to being answered with a `-2` receipt. Only a run that goes through
/// the real `run_foreground_impl`, the real `DetachGate` race and a real child process can catch
/// that, and this is it.
///
/// Gutted, assertion by assertion:
///
/// * detach refused for a workflow child (the struck "stamp the real mode" shortcut) → the
///   success-sentence assertion fires with *"is not a single-subagent run"*;
/// * the fork not wired to `parent_workflow_run_id` → the step's result is the receipt
///   (`detached: true`, `exit_code == -2`, the detach sentence) and (c)'s three asserts fire;
/// * the fork wired but the child dropped instead of awaited → the liveness probe fires, and the
///   step never stops awaiting inside [`SETTLE_BUDGET`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_detached_workflow_child_settles_back_into_the_step_that_launched_it() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    write_worker_persona(cwd.path());
    // [`CHILD_SLEEP_MS`], for its own stated reason: every probe below — the detach, the liveness
    // check and the "the step has NOT returned yet" check — is only meaningful while the child is
    // genuinely still running, and a shorter sleep makes that a race against the scheduler rather
    // than a fact. A 4s sleep was tried and lost that race under a loaded box, where the child was
    // torn down mid-run and the step got `exit_code: 1` / *"Subagent produced no output"*; this
    // suite's existing constant is sized so it cannot.
    let script = blocking_child_script(cwd.path(), "wf-blocking.json", CHILD_SLEEP_MS);
    let ext = extension(home.path(), cwd.path(), &script);
    let ctx = command_ctx(cwd.path());

    let parent = RunId::new();
    let step = launch_workflow_child(&ext, cwd.path(), parent.clone());

    let run_id = eventually(
        SETTLE_BUDGET,
        "the workflow child never registered a live control",
        || {
            let ext = Arc::clone(&ext);
            let cwd = cwd.path().to_path_buf();
            async move { live_foreground_ids(&ext, &cwd).await.into_iter().next() }
        },
    )
    .await;
    let child_pid = eventually(
        SETTLE_BUDGET,
        "the scripted child never appeared in /proc",
        || {
            let script = script.clone();
            async move { fixture_pids(&script).into_iter().next() }
        },
    )
    .await;

    // ---- (a) the detach is ACCEPTED for a workflow child ----
    let receipt = tokio::time::timeout(RECEIPT_BUDGET, detach(&ext, &run_id, &ctx))
        .await
        .expect("the detach command answers the human promptly even for a workflow child");
    assert!(
        receipt.contains(&format!(
            "Detached foreground run {run_id} without terminating its child."
        )),
        "a workflow child IS detachable — upstream stamps `\"single\"` for it and allows the \
         detach, which is why `resolveDetachedWorkflowChild` exists. A refusal here is the struck \
         shortcut, not a fix; got: {receipt}"
    );

    // ---- (b) the child is still running, and the STEP has not been answered ----
    assert!(
        pid_is_alive(child_pid),
        "the detach must not terminate a workflow child's process either; pid {child_pid} is \
         gone right after the command returned"
    );
    assert!(
        !step.is_finished(),
        "THE ASSERTION THIS TEST EXISTS FOR: the launching step must still be AWAITING. A step \
         that has already returned was answered with the detach receipt — a fabricated \
         `exit_code == -2` failure carrying the detach sentence as the child's output — which is \
         exactly the divergence R-VLS11b-01 closed."
    );

    // ---- (c) ...and when the child really exits, the STEP gets its real result ----
    let settled = tokio::time::timeout(SETTLE_BUDGET, step)
        .await
        .expect("the step must be answered once its detached child exits")
        .expect("the launching task completes");
    assert!(
        !settled.detached,
        "the step receives a TERMINAL result, not a receipt. A `detached: true` here is what \
         `workflow.rs`'s detach hook records as a `DetachedWorkflowChild`, parking the workflow \
         at `paused` against a child that has in fact finished: {settled:?}"
    );
    // `!= -2`, deliberately, and NOT `== 0`. The claim under test is "the step was handed a
    // TERMINAL result rather than the detach receipt", and the receipt's exit code is the closed
    // constant `DETACHED_EXIT_CODE` (`detach.rs`, pi `execution.ts:618`) — so this assertion is
    // exactly the claim, with nothing added.
    //
    // `== 0` would additionally assert that the FIXTURE CHILD succeeded, which is a different
    // claim and a load-sensitive one: under the full `-p cyrup-it` suite (nine binaries, several
    // of them v8/wasm-heavy) this child was twice observed to exit zero having written nothing,
    // giving the step `exit_code: 1` with `"Subagent produced no output …"` — a starved child on a
    // saturated box, not a receipt, and not something the routing under test can cause. Asserting
    // it here would make this test red for a reason it is not about.
    assert_ne!(
        settled.exit_code, -2,
        "the receipt's exit code must never reach the step: {settled:?}"
    );
    assert!(
        settled
            .final_output
            .as_deref()
            .is_none_or(|out| !out.contains("Detached at user request")),
        "the detach sentence is the RECEIPT's output, never the child's: {settled:?}"
    );

    for pid in fixture_pids(&script) {
        kill_pid_for_cleanup(pid);
    }
}
