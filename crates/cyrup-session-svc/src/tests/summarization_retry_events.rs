//! DRIFT-006 — the `summarization_retry_*` and `bash_execution_update` event surface.
//!
//! `bb301b6` landed the retry MECHANISM around summarization (DRIFT-005: `retryAssistantCall`,
//! `cacheRetention: "none"`, a fresh per-request session id) but wired the observer seam to `None`
//! at both production call sites, so a compaction that retried did so SILENTLY — the front-end saw
//! `compaction_start`, then nothing at all for `baseDelayMs * 2^(n-1)`, then `compaction_end`. Pi
//! emits four union members from `_summarizationRetryCallbacks` (`agent-session.ts:166-179`,
//! `:2641-2670`) plus `bash_execution_update` (`:181`, `:2786`).
//!
//! Everything asserted here is a FAILURE path: a stream that drops (`terminated`, which Pi's
//! `retry.ts:63` classifies retryable) and then succeeds. The happy-path control asserts the same
//! events are ABSENT, because `onRetryFinished` fires only when `lastRetry` is set
//! (`retry.ts:176/183/188`).
//!
//! The load-bearing assertion is `a_retried_compaction_appends_exactly_one_compaction_entry`: the
//! session JSONL is append-only, so a retry that re-ran the append would be unrecoverable
//! corruption.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{AgentSessionEvent, BashOptions, NavigateTreeOptions, SessionBuilder, SessionConfig};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use futures::StreamExt;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Force even a small session to compact, and make the retry policy `enabled: true, maxRetries: 3,
/// baseDelayMs: 0` — Pi's own defaults are `3` / `2000` (`settings-manager.ts:821-822`); only the
/// delay is zeroed so the test does not actually sleep 2s+4s+8s.
fn retryable_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 0}),
    )
    .unwrap();
    cli
}

/// A step that fails the way a dropped socket does — Pi classifies `terminated` as retryable
/// (`retry.ts:63`). Byte-identical to `cyrup-session/tests/compaction.rs`'s helper.
fn transient_failure_step() -> FauxResponseStep {
    FauxResponseStep::factory(|_ctx, _opts, _state, _model| {
        let mut msg = faux_assistant_message(vec![], StopReason::Error);
        msg.error_message = Some("terminated".to_string());
        msg
    })
}

fn ok_step(body: &'static str) -> FauxResponseStep {
    faux_assistant_message(vec![faux_text(body)], StopReason::Stop).into()
}

/// Drain a persistent subscription into a shared buffer on a background task, so the fan-out's
/// per-subscriber backpressure can never stall the operation under test.
fn collect(session: &crate::AgentSession) -> Arc<Mutex<Vec<AgentSessionEvent>>> {
    let seen: Arc<Mutex<Vec<AgentSessionEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let mut sub = session.subscribe();
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Some(ev) = sub.next().await {
            sink.lock().unwrap().push(ev);
        }
    });
    seen
}

/// Let the collector task drain whatever is already queued, detected by OBSERVING the buffer go
/// quiescent instead of by sleeping a fixed 50 ms (DRIFT-036).
///
/// Every caller reaches here after the operation under test has already returned, so the only work
/// left is in-process hops (fan-out → subscription → the collector task) with no timer anywhere in
/// the chain. `yield_now` lets every ready task run, so "the length stopped changing across
/// [`QUIESCENT_YIELDS`] consecutive yields" is an observation of the state the assertions read, not
/// a guess about how long it takes. The outer bound turns a stuck pipeline into a named failure
/// rather than a hang, and is never itself the assertion.
async fn settle(seen: &Arc<Mutex<Vec<AgentSessionEvent>>>) {
    /// Consecutive no-growth yields that count as drained.
    const QUIESCENT_YIELDS: u32 = 64;
    let mut last = usize::MAX;
    let mut stable = 0u32;
    for _ in 0..20_000 {
        let now = seen.lock().unwrap().len();
        if now == last {
            stable += 1;
            if stable >= QUIESCENT_YIELDS {
                return;
            }
        } else {
            last = now;
            stable = 0;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "the event collector never went quiescent: {:?}",
        kinds(seen)
    );
}

fn kinds(seen: &Arc<Mutex<Vec<AgentSessionEvent>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|e| e.kind().to_string())
        .collect()
}

/// Build a two-turn session whose compaction summarization drops once and then succeeds.
/// `set_response_steps` is consumed in order across the WHOLE session, so the two prompt turns come
/// first and the summarization steps follow.
async fn session_with_a_dropping_summarization(fx: &Fixture) -> crate::AgentSession {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        ok_step("first answer"),
        ok_step("second answer"),
        // The summarization: drop, then succeed.
        transient_failure_step(),
        ok_step("CONTEXT SUMMARY"),
        // Spare completions in case the transcript compacts as a split turn.
        ok_step("TURN PREFIX SUMMARY"),
        ok_step("SPARE"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(fx))
        .cli_settings(retryable_compaction_settings())
        .build()
        .await
        .expect("build");
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    session
}

// ===================================================================== the append-idempotency bar ==

/// HAZARD: the session JSONL is append-only. A retried summarization that appended TWICE would be
/// unrecoverable corruption. `retryAssistantCall` deliberately wraps ONLY the provider call
/// (`compaction.ts:580`) — the append happens once, after the loop settles — so a retry re-issues
/// the request and nothing else. Proven both in-memory and by re-reading the persisted session.
#[tokio::test]
async fn a_retried_compaction_appends_exactly_one_compaction_entry() {
    let fx = fixture();
    let session = session_with_a_dropping_summarization(&fx).await;
    let seen = collect(&session);

    let before = session.entries_json().await.len();
    let result = session
        .compact(None)
        .await
        .expect("the drop is retried and the compaction lands");
    settle(&seen).await;

    // The SUCCEEDING attempt's text is what got stored — not the failed attempt's empty body.
    assert!(
        result.summary.contains("CONTEXT SUMMARY"),
        "the retried (succeeding) summarization is what lands: {:?}",
        result.summary
    );

    // A retry actually happened — otherwise this test proves nothing about idempotency.
    let k = kinds(&seen);
    assert!(
        k.iter().any(|s| s == "summarization_retry_scheduled"),
        "the transient drop must have been retried; saw {k:?}"
    );

    let after = session.entries_json().await.len();
    assert_eq!(
        after,
        before + 1,
        "a retried compaction appends EXACTLY ONE entry (append-only JSONL); before={before} \
         after={after}"
    );
    assert_eq!(
        k.iter().filter(|s| *s == "compaction_end").count(),
        1,
        "exactly one terminal compaction_end, never one per attempt: {k:?}"
    );
    assert_eq!(
        k.iter()
            .filter(|s| *s == "summarization_retry_finished")
            .count(),
        1,
        "onRetryFinished fires exactly once per summarization loop (retry.ts:176/183/188): {k:?}"
    );

    // Re-read the persisted JSONL: the on-disk truth must agree with the in-memory tree.
    let path = session
        .session_file()
        .await
        .expect("a persisted session file");
    let compaction_lines = std::fs::read_to_string(&path)
        .expect("session file")
        .lines()
        .filter(|l| {
            serde_json::from_str::<serde_json::Value>(l)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
                .as_deref()
                == Some("compaction")
        })
        .count();
    assert_eq!(
        compaction_lines,
        1,
        "exactly one `compaction` record on disk at {}",
        path.display()
    );
}

// ============================================================================= the event surface ==

/// The full Pi ordering: `compaction_start` … `summarization_retry_scheduled` (BEFORE the backoff,
/// `retry.ts:193`) → `summarization_retry_attempt_start` (AFTER it, `:204`) →
/// `summarization_retry_finished` (once at loop end, `:183`) … `compaction_end`.
#[tokio::test]
async fn a_dropped_summarization_emits_the_retry_events_in_pi_order() {
    let fx = fixture();
    let session = session_with_a_dropping_summarization(&fx).await;
    let seen = collect(&session);

    session
        .compact(None)
        .await
        .expect("compaction lands after the retry");
    settle(&seen).await;

    let k = kinds(&seen);
    let idx = |name: &str| {
        k.iter()
            .position(|s| s == name)
            .unwrap_or_else(|| panic!("{name} missing from {k:?}"))
    };
    let (start, sched, attempt, fin, end) = (
        idx("compaction_start"),
        idx("summarization_retry_scheduled"),
        idx("summarization_retry_attempt_start"),
        idx("summarization_retry_finished"),
        idx("compaction_end"),
    );
    assert!(
        start < sched && sched < attempt && attempt < fin && fin < end,
        "Pi order compaction_start < scheduled < attempt_start < finished < compaction_end; got {k:?}"
    );
}

/// The payloads, on the wire. Pi's `summarization_retry_scheduled` carries the retry budget and the
/// transient error verbatim (`agent-session.ts:166-172`); `summarization_retry_attempt_start`
/// carries the discriminated `source` (`:173-178`); `summarization_retry_finished` carries NOTHING
/// (`:179`) because `_summarizationRetryCallbacks` discards `onRetryFinished`'s three arguments
/// (`:2664-2667`).
#[tokio::test]
async fn the_retry_events_serialize_to_pi_s_exact_shapes() {
    let fx = fixture();
    let session = session_with_a_dropping_summarization(&fx).await;
    let seen = collect(&session);

    session
        .compact(None)
        .await
        .expect("compaction lands after the retry");
    settle(&seen).await;

    let json: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let of = |t: &str| {
        json.iter()
            .find(|v| v["type"] == t)
            .unwrap_or_else(|| panic!("{t} missing from {:?}", kinds(&seen)))
            .clone()
    };

    let sched = of("summarization_retry_scheduled");
    assert_eq!(sched["attempt"], serde_json::json!(1), "1-indexed attempt");
    assert_eq!(
        sched["maxAttempts"],
        serde_json::json!(3),
        "the settings budget (`retry.maxRetries`), NOT an invented bound"
    );
    assert_eq!(
        sched["delayMs"],
        serde_json::json!(0),
        "baseDelayMs * 2^(attempt-1) = 0 * 1"
    );
    assert_eq!(
        sched["errorMessage"], "terminated",
        "the transient provider error verbatim, so the UI can show WHY it is retrying"
    );

    let attempt = of("summarization_retry_attempt_start");
    assert_eq!(
        attempt["source"], "compaction",
        "`source` is a SIBLING of `type` (Pi's discriminated union member), not a nested object"
    );
    assert_eq!(
        attempt["reason"], "manual",
        "a manual /compact carries reason:\"manual\""
    );

    let fin = of("summarization_retry_finished");
    assert_eq!(
        fin.as_object().map(serde_json::Map::len),
        Some(1),
        "payload-free — `type` only, matching agent-session.ts:179 / :2666: {fin}"
    );
}

/// The control: with the SAME session, transcript and settings but no dropped stream, none of the
/// three events is emitted. Pi guards every callback on `lastRetry` being set
/// (`retry.ts:176/183/188`), so a first-try success is silent — without this control the test above
/// could pass on an implementation that emitted unconditionally.
#[tokio::test]
async fn a_first_try_success_emits_no_retry_events_at_all() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        ok_step("first answer"),
        ok_step("second answer"),
        ok_step("CONTEXT SUMMARY"),
        ok_step("TURN PREFIX SUMMARY"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(retryable_compaction_settings())
        .build()
        .await
        .expect("build");
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let seen = collect(&session);
    session.compact(None).await.expect("a clean compaction");
    settle(&seen).await;

    let k = kinds(&seen);
    assert!(
        !k.iter().any(|s| s.starts_with("summarization_retry")),
        "a summarization that succeeds on its first attempt emits NO retry events: {k:?}"
    );
    assert!(
        k.iter().any(|s| s == "compaction_end"),
        "…but the compaction still settled: {k:?}"
    );
}

/// The retry is BOUNDED and the bound is observable: with a provider that never recovers, the
/// budget is spent exactly `retry.maxRetries` times and the compaction then FAILS rather than
/// looping forever. `maxAttempts = policy.enabled ? policy.maxRetries : 0` (`retry.ts:159`).
#[tokio::test]
async fn an_unrecoverable_drop_stops_after_exactly_max_retries() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // Eight queued drops against a budget of three: the bound is what stops the loop, and there is
    // deliberate slack so an UNBOUNDED implementation would keep going (and blow the count assert)
    // rather than stopping early because the faux ran dry.
    let mut steps = vec![ok_step("first answer"), ok_step("second answer")];
    steps.extend((0..8).map(|_| transient_failure_step()));
    faux.set_response_steps(steps);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(retryable_compaction_settings())
        .build()
        .await
        .expect("build");
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let seen = collect(&session);
    let before = session.entries_json().await.len();
    let err = tokio::time::timeout(Duration::from_secs(20), session.compact(None))
        .await
        .expect("a bounded retry MUST terminate — an unbounded one would wedge the session here")
        .expect_err("an unrecoverable summarization fails the compaction");
    settle(&seen).await;

    assert!(
        err.to_string().contains("terminated"),
        "the final provider error surfaces: {err}"
    );
    let k = kinds(&seen);
    assert_eq!(
        k.iter()
            .filter(|s| *s == "summarization_retry_scheduled")
            .count(),
        3,
        "exactly `retry.maxRetries` (3) retries scheduled, matching Pi's bound — not more: {k:?}"
    );
    assert_eq!(
        k.iter()
            .filter(|s| *s == "summarization_retry_finished")
            .count(),
        1,
        "one terminal event for the whole exhausted loop: {k:?}"
    );
    assert_eq!(
        session.entries_json().await.len(),
        before,
        "a failed compaction appends NOTHING (least of all one entry per attempt)"
    );
}

/// HAZARD: a retry loop needs a CANCELLATION path, not just a bound — compaction sits on the user's
/// critical path, and with Pi's real `baseDelayMs: 2000` an exhausting retry sleeps 2s+4s+8s. Esc
/// during `/compact` must cut through the BACKOFF SLEEP itself, not merely between attempts. Pi
/// passes the abort signal into `sleep(delayMs, signal)` and normalizes the rejection to an aborted
/// `AssistantMessage` (`retry.ts:195-203`); cyrup's port sleeps under
/// `cancel.run_until_cancelled`. Here the first backoff is 30s, so an implementation that ignored
/// the token would blow the 10s timeout.
#[tokio::test]
async fn aborting_during_the_retry_backoff_returns_promptly() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let mut steps = vec![ok_step("first answer"), ok_step("second answer")];
    steps.extend((0..8).map(|_| transient_failure_step()));
    faux.set_response_steps(steps);

    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    // A 30s first backoff: only real cancellation can get us out of it inside the timeout.
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 30000}),
    )
    .unwrap();

    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let seen = collect(&session);
    let before = session.entries_json().await.len();
    let compacting = tokio::spawn({
        let s = Arc::clone(&session);
        async move { s.compact(None).await.map(|_| ()) }
    });
    // Wait until the retry is provably armed (scheduled fires BEFORE the sleep), then abort.
    for _ in 0..200 {
        if kinds(&seen)
            .iter()
            .any(|s| s == "summarization_retry_scheduled")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        kinds(&seen)
            .iter()
            .any(|s| s == "summarization_retry_scheduled"),
        "the retry must be armed before we test cancelling it: {:?}",
        kinds(&seen)
    );
    session.abort_compaction();

    let outcome = tokio::time::timeout(Duration::from_secs(10), compacting)
        .await
        .expect("abort must cut through the 30s backoff sleep, not wait it out")
        .expect("join");
    assert!(
        outcome.is_err(),
        "an aborted compaction is a refusal, not a result"
    );
    settle(&seen).await;
    assert_eq!(
        session.entries_json().await.len(),
        before,
        "an aborted compaction appends nothing"
    );
}

// ======================================================================= branch summarization ====

/// Branch summarization is Pi's THIRD summarization call site
/// (`agent-session.ts:2996-2998` — `retry: getRetrySettings()`, `callbacks:
/// _summarizationRetryCallbacks({ source: "branchSummary" })`), and cyrup routes it through a
/// DIFFERENT method (`generate_branch_summary_with_instructions`) than compaction, so wiring one
/// says nothing about the other. Same failure: the branch summarizer's stream drops once.
///
/// The `source` discriminant matters behaviorally, not cosmetically: the TUI recreates the
/// underlying indicator from it after the retry (`interactive-mode.ts:3233-3238`), so a
/// branch-summary retry that reported `source: "compaction"` would leave the user staring at
/// "Compacting context…" during a `/tree` navigation.
#[tokio::test]
async fn a_dropped_branch_summarization_emits_retry_events_tagged_branch_summary() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        ok_step("a1"),
        ok_step("a2"),
        // The branch summarizer: drop, then succeed.
        transient_failure_step(),
        ok_step("BRANCH-BODY"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(retryable_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("first").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.expect("prompt 2");
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u1 = anchors[0].entry_id.clone();

    let seen = collect(&session);
    let before = session.entries_json().await.len();
    let outcome = session
        .navigate_tree(
            u1,
            NavigateTreeOptions {
                summarize: true,
                custom_instructions: Some("focus".to_string()),
                replace_instructions: false,
                label: None,
            },
        )
        .await
        .expect("the drop is retried and the branch summary still lands");
    settle(&seen).await;

    let entry = outcome
        .summary_entry
        .expect("a branch summary entry was appended");
    assert!(
        entry.summary.contains("BRANCH-BODY"),
        "the retried (succeeding) call is what lands: {}",
        entry.summary
    );

    let json: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let attempt = json
        .iter()
        .find(|v| v["type"] == "summarization_retry_attempt_start")
        .unwrap_or_else(|| panic!("no branch-summary retry observed: {:?}", kinds(&seen)));
    assert_eq!(
        attempt["source"], "branchSummary",
        "Pi tags the branch path `branchSummary` (agent-session.ts:173/2998): {attempt}"
    );
    assert!(
        attempt.get("reason").is_none(),
        "`reason` belongs to the compaction arm ONLY (agent-session.ts:173 vs :174-178): {attempt}"
    );
    assert_eq!(
        json.iter()
            .filter(|v| v["type"] == "summarization_retry_finished")
            .count(),
        1,
        "one terminal event for the branch summarization loop"
    );

    // Same append-idempotency bar as compaction: exactly one branch_summary entry, not one per
    // attempt. `navigate_tree` also re-roots the leaf, so count the branch_summary records only.
    let jsonl = session
        .export_to_jsonl(None)
        .await
        .expect("export")
        .expect("jsonl");
    assert_eq!(
        jsonl.matches("branch_summary").count(),
        1,
        "a retried branch summarization persists EXACTLY ONE branch_summary record"
    );
    assert!(
        session.entries_json().await.len() > before,
        "sanity: the summary really was appended"
    );
}

/// The `branch_with_summary` entry point (cyrup's other branch-summary caller, `session.rs`) must
/// be wired too — it constructs its own `DynSummarizer` and would otherwise stay silent.
#[tokio::test]
async fn branch_with_summary_also_emits_the_retry_events() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        ok_step("a1"),
        ok_step("a2"),
        transient_failure_step(),
        ok_step("BRANCH-BODY"),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(retryable_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("first").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.expect("prompt 2");
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u1 = anchors[0].entry_id.clone();

    let seen = collect(&session);
    let summary = session
        .branch_with_summary(u1, true)
        .await
        .expect("the drop is retried and the branch summary still lands");
    settle(&seen).await;

    assert!(
        summary.is_some_and(|s| s.contains("BRANCH-BODY")),
        "the retried call's body is what lands"
    );
    let json: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let attempt = json
        .iter()
        .find(|v| v["type"] == "summarization_retry_attempt_start")
        .unwrap_or_else(|| panic!("no branch-summary retry observed: {:?}", kinds(&seen)));
    assert_eq!(attempt["source"], "branchSummary");
}

// ========================================================================= bash_execution_update ==

/// Pi wraps the caller's `onChunk` and emits `bash_execution_update` for every delta REGARDLESS of
/// whether a sink was supplied (`agent-session.ts:2784-2787`), so an event-only front-end still
/// renders live output. Cyrup previously emitted nothing at all: the sink was the only outlet, and
/// the RPC caller passes `None`.
#[tokio::test]
async fn execute_bash_emits_bash_execution_update_even_with_no_chunk_sink() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");
    let seen = collect(&session);

    let result = session
        .execute_bash(
            "printf 'alpha\\n'",
            BashOptions {
                exclude_from_context: false,
                id: Some("rpc-7".to_string()),
                operations: None,
            },
            // No sink — exactly what `rpc.rs`'s `SessionCommand::Bash` arm passes.
            None,
        )
        .await
        .expect("bash runs");
    settle(&seen).await;
    assert!(
        result.output.contains("alpha"),
        "sanity: the command produced output"
    );

    let updates: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.kind() == "bash_execution_update")
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    assert!(
        !updates.is_empty(),
        "at least one delta was emitted: {:?}",
        kinds(&seen)
    );
    assert!(
        updates
            .iter()
            .any(|v| v["delta"].as_str().is_some_and(|d| d.contains("alpha"))),
        "the deltas carry the real output: {updates:?}"
    );
    assert!(
        updates.iter().all(|v| v["id"] == "rpc-7"),
        "every delta carries `options.id` (Pi rpc-mode.ts:574 threads the request id): {updates:?}"
    );
}

/// `id` is `id?: string` in Pi (`agent-session.ts:181`) — emitted as `options?.id`, so it must be
/// ABSENT from the JSON when the caller supplied none, never `null`.
#[tokio::test]
async fn bash_execution_update_omits_id_when_the_caller_supplied_none() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");
    let seen = collect(&session);

    session
        .execute_bash(
            "printf 'beta\\n'",
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: None,
            },
            None,
        )
        .await
        .expect("bash runs");
    settle(&seen).await;

    let updates: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.kind() == "bash_execution_update")
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    assert!(!updates.is_empty(), "at least one delta was emitted");
    for v in &updates {
        assert!(
            v.get("id").is_none(),
            "`id` must be omitted, not serialized as null (Pi `id?: string`): {v}"
        );
    }
}

/// TOOL-031 / PARITY-GAPS PB-5, the IMMEDIATE-BASH half.
///
/// pi sets the agent-identity markers on `process.env` in `cli.ts` before `main()` runs
/// (`PI_CODING_AGENT = "true"`, `cli.ts:13` @v0.83.0; `AI_AGENT = "pi"`, `:14` @v0.84.1, mirrored
/// in `rpc-entry.ts:7-8`), so EVERY child inherits them via `getShellEnv()`'s `{...process.env}`
/// (`utils/shell.ts:130-133`) — including this seam, which reaches `getShellEnv()` by the same
/// fall-through as the `bash` tool (`core/tools/bash.ts:100`).
///
/// RED before this pass: cyrup's bin declines the process-global `set_var`, so each spawn site has
/// to push the pair itself. The `bash` TOOL did; `crate::bash::run_bash` did not — so `!!cmd` in
/// the TUI and the RPC `executeBash` handed the child a DIFFERENT environment from the identical
/// command run as a tool. Both `${PI_CODING_AGENT-}` expansions below rendered empty.
#[tokio::test]
async fn immediate_bash_carries_the_agent_identity_markers() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");

    let result = session
        .execute_bash(
            r#"printf '[%s][%s]\n' "${PI_CODING_AGENT-}" "${AI_AGENT-}""#,
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: None,
            },
            None,
        )
        .await
        .expect("bash runs");

    assert!(
        result.output.contains("[true][cyrup]"),
        "the immediate-bash child must see the same identity markers the `bash` tool's child sees \
         (pi `cli.ts:13-14`); got: {:?}",
        result.output
    );
}

/// CFG-069 — `AI_AGENT` is a FORWARD-PORT: the key does not exist at the ported tag at all
/// (`git -C pi grep -n AI_AGENT v0.83.0 -- packages/` → 0; `cli.ts:13` @v0.83.0 sets only
/// `PI_CODING_AGENT`), it arrives at `cli.ts:14` @v0.84.1. The delta annotation therefore has to
/// name the KEY and the TAG, not only the value — otherwise a later v0.84.1 uplift reads the site
/// as already-done-at-tag and never records that cyrup ran ahead of the baseline.
///
/// Presence before absence: `PI_CODING_AGENT`, which IS at the ported tag, must still be pushed
/// beside it — this test must not be satisfiable by deleting the forward-ported marker.
#[test]
fn the_forward_ported_ai_agent_marker_names_its_key_and_its_tag() {
    let src = include_str!("../bash.rs");

    assert!(
        src.contains(r#"env.push(("PI_CODING_AGENT".to_string(), "true".to_string()));"#),
        "the at-tag marker `PI_CODING_AGENT` (cli.ts:13 @v0.83.0) must still be pushed"
    );

    let push = r#"env.push(("AI_AGENT".to_string(), "cyrup".to_string()));"#;
    let at = src
        .find(push)
        .expect("`AI_AGENT` is pushed into the immediate-bash child env");
    // The annotation is the comment block immediately above the push.
    let annotation = &src[..at];
    let annotation = &annotation[annotation
        .rfind("[CYRUP-DELTA")
        .expect("a delta annotation")..];

    assert!(
        annotation.contains("@v0.84.1"),
        "the delta line must state the TAG the key comes from; got: {annotation}"
    );
    assert!(
        annotation.contains("AI_AGENT"),
        "the delta line must name the KEY, not only its value; got: {annotation}"
    );
    assert!(
        annotation.contains("v0.83.0"),
        "the delta line must state that the key is ABSENT at the ported tag; got: {annotation}"
    );
}
