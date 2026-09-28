//! SEAM-007 — a compaction REFUSAL must be a distinguishable error, not `success` with a null body.
//!
//! Pi's `AgentSession.compact` is typed `Promise<CompactionResult>` and never resolves to
//! `undefined` (pi/packages/coding-agent/src/core/agent-session.ts:1783). It `throw`s instead:
//!
//! * `agent-session.ts:1801-1807` — `prepareCompaction` produced nothing. If the last branch entry is
//!   already a `compaction` it throws `"Already compacted"`, otherwise
//!   `"Nothing to compact (session too small)"`.
//! * `agent-session.ts:1823-1825` — a `session_before_compact` handler returned `{cancel:true}` ⇒
//!   `"Compaction cancelled"` (the same string it throws at :1869 for a post-summarization abort, and
//!   the literal its own catch compares against at :1911 to classify the abort).
//!
//! Those throws reach the RPC dispatcher's catch and become `{success:false, error:"…"}`
//! (rpc-mode.ts:530-532 + the surrounding handler). Pre-fix cyrup returned `Ok(None)` for all three,
//! which the RPC adapter serialized as `{"success":true,"data":null}` — three different refusals
//! collapsed into one indistinguishable "success".
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_core::{AssistantMessage, ExtensionId, StopReason, TerminateHint};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxConfig, FauxMessageOptions, FauxProvider, faux_assistant_message,
    faux_assistant_message_with, faux_text,
};
use futures::StreamExt;
use tempfile::TempDir;

fn kinds(events: &[AgentSessionEvent]) -> Vec<&'static str> {
    events.iter().map(AgentSessionEvent::kind).collect()
}

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

/// Compaction settings that force even a small session to compact (keep nothing, reserve nothing).
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// A guest that vetoes every `session_before_compact` (Pi `{cancel:true}`).
struct CompactionVetoer;

#[async_trait::async_trait]
impl NativeExtension for CompactionVetoer {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("compaction-vetoer")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionBeforeCompact]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionBeforeCompact { .. } => HookOutcome::Block {
                reason: Some("not now".to_string()),
                terminate: TerminateHint::Unspecified,
            },
            _ => HookOutcome::Noop,
        }
    }
}

/// A fresh session has nothing to summarize: Pi throws
/// `"Nothing to compact (session too small)"` (agent-session.ts:1806).
#[tokio::test]
async fn compact_on_a_tiny_session_errors_nothing_to_compact() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build");

    let err = session
        .compact(None)
        .await
        .expect_err("a refusal must be an Err, not Ok(_) — Pi throws (agent-session.ts:1801-1807)");
    assert_eq!(
        err.to_string(),
        "Nothing to compact (session too small)",
        "verbatim Pi message (agent-session.ts:1806)"
    );
}

/// Compact twice: the second call finds the branch already ending in a `compaction` entry, which Pi
/// reports as `"Already compacted"` — a DIFFERENT message from the too-small case
/// (agent-session.ts:1803-1806).
#[tokio::test]
async fn compact_twice_errors_already_compacted() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        // Ample summary completions so summarization never starves.
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let first = session
        .compact(None)
        .await
        .expect("the first compaction succeeds");
    assert!(
        !first.summary.is_empty(),
        "the first compaction produced a summary"
    );

    let err = session.compact(None).await.expect_err(
        "compacting an already-compacted branch must be an Err (Pi agent-session.ts:1803-1805)",
    );
    assert_eq!(err.to_string(), "Already compacted", "verbatim Pi message");
}

/// A `session_before_compact` veto: Pi throws `"Compaction cancelled"`
/// (agent-session.ts:1823-1825) — never a resolved value.
#[tokio::test]
async fn compact_vetoed_by_an_extension_errors_compaction_cancelled() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .with_native_extension(Arc::new(CompactionVetoer))
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let err = session
        .compact(None)
        .await
        .expect_err("an extension veto must be an Err (Pi agent-session.ts:1824)");
    assert_eq!(
        err.to_string(),
        "Compaction cancelled",
        "verbatim Pi message"
    );
}

/// Aborting an IN-FLIGHT compaction (Esc during `/compact` ⇒ `abort_compaction`) is the same refusal
/// family as the veto above, and Pi raises it with the SAME bare string:
/// `agent-session.ts:1868-1870` — `if (this._compactionAbortController.signal.aborted) { throw new
/// Error("Compaction cancelled"); }` — which `rpc-mode.ts:789-795` propagates verbatim as
/// `error(command.id, command.type, commandError.message)`. Pi's own catch also classifies the abort
/// by comparing `message === "Compaction cancelled"` (agent-session.ts:1911), so the exact string is
/// load-bearing, not cosmetic.
///
/// Pre-fix cyrup let `CompactionError::Aborted` fall through `Err(e.into())` into
/// `SessionServiceError::Compaction`, whose `#[error("compaction: {0}")]` wrapped
/// `#[error("compaction cancelled")]` — an RPC client saw `"compaction: compaction cancelled"`.
#[tokio::test]
async fn compact_aborted_in_flight_errors_with_pi_s_bare_compaction_cancelled() {
    let fx = fixture();
    // Pace the faux stream slowly enough that the summarization is unambiguously still in flight
    // when the abort lands (`tokensPerSecond`, Pi faux.ts:300-306).
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("b")], StopReason::Stop),
        // The summarization completion: long enough that at 1 token/s it streams for minutes.
        faux_assistant_message(
            vec![faux_text(
                "this summary is deliberately long so that the compaction summarization is still \
                 streaming when the user presses escape and abort_compaction fires the cancel token",
            )],
            StopReason::Stop,
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let mut stream = session.subscribe();
    let compacting = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.compact(None).await.map(|_| ()) }
    });

    // Wait for `compaction_start` so the compaction cancel token is definitely installed, then abort.
    let mut started = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
        if matches!(ev, AgentSessionEvent::CompactionStart { .. }) {
            started = true;
            break;
        }
    }
    assert!(started, "compaction_start must fire before the abort");
    // Let the summarization stream actually open before cancelling it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.abort_compaction();

    let err = tokio::time::timeout(Duration::from_secs(10), compacting)
        .await
        .expect("the aborted compaction must return promptly")
        .expect("compaction task must not panic")
        .expect_err("an in-flight abort must be an Err (Pi agent-session.ts:1869)");
    assert_eq!(
        err.to_string(),
        "Compaction cancelled",
        "Pi throws the BARE string, not a wrapped `compaction: compaction cancelled` \
         (agent-session.ts:1869, propagated by rpc-mode.ts:789-795)"
    );

    // The event payload for this path is Pi's abort shape: `aborted:true`, no `errorMessage`
    // (agent-session.ts:1909-1916).
    let mut end: Option<(bool, Option<String>)> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
        if let AgentSessionEvent::CompactionEnd {
            aborted,
            error_message,
            ..
        } = &ev
        {
            end = Some((*aborted, error_message.clone()));
            break;
        }
    }
    assert_eq!(
        end,
        Some((true, None)),
        "compaction_end must carry aborted:true with no errorMessage"
    );

    // SESS-040/042 — the DURABLE half of the cancel, and the one the live repro caught cyrup
    // getting wrong: "a `compaction` entry with a full summary was appended to the session file"
    // for the run in which Escape was pressed. pi re-tests the abort signal immediately before
    // `appendCompaction` (`agent-session.ts:1868-1870`), so an aborted compaction leaves the
    // session file byte-identical. cyrup's equivalent guard is
    // `cyrup-session/src/compaction/mod.rs:297`; without a caller for `abort_compaction` it could
    // never fire, which is exactly why it must be asserted from the abort path and not in
    // isolation.
    let compaction_entries = session
        .entries_json()
        .await
        .into_iter()
        .filter(|e| e.get("type").and_then(serde_json::Value::as_str) == Some("compaction"))
        .count();
    assert_eq!(
        compaction_entries, 0,
        "an aborted compaction must append NOTHING — the user said stop and the session file is \
         durable state (agent-session.ts:1868-1870)"
    );

    // …and the token slot must be empty again, so the next prompt is not diverted into the
    // compaction queue by a stale `is_compacting()`.
    assert!(
        !session.is_compacting(),
        "the cancel token must be released once the aborted compaction settles"
    );
}

/// SESS-040 — a `compact()` future DROPPED mid-flight must not wedge `is_compacting()` at true.
///
/// This is the JS→Rust guarantee gap, not a hypothetical: pi's `compact` is an `async fn` and an
/// `async fn` ALWAYS settles, so its clear of `this._compactionAbortController` cannot be skipped.
/// cyrup's `AgentSession::compact` is a public API whose body is one 10-20 s provider call, and two
/// shipped callers can drop it at an `.await` — `run_rpc`'s `select!` drops the whole driver when
/// the write pump reports a broken pipe (`cyrup-modes/src/rpc.rs:668-676`), and any embedder that
/// wraps the `cyrup-sdk` handle (`cyrup-sdk/src/handle.rs:285`) in a `tokio::time::timeout` does the
/// same. The hand-written clears at each `return` cannot run then.
///
/// What the user sees when it leaks: `is_compacting()` answers true forever, and the TUI's Submit
/// arm consults it before anything else, so every prompt typed afterwards is diverted into the
/// compaction queue and drained by a `compaction_end` that can never arrive. The session accepts
/// input and silently sends none of it. The fix is `CompactionCancelGuard`'s `Drop`.
#[tokio::test]
async fn a_dropped_compaction_future_releases_the_cancel_token() {
    let fx = fixture();
    // 1 token/s, so the summarization is unambiguously still in flight when the future is dropped.
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("b")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_text(
                "this summary is deliberately long so that the compaction summarization is still \
                 streaming when the caller's future is dropped out from under it",
            )],
            StopReason::Stop,
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    {
        // The literal shape of `run_rpc`'s race: the compaction is one arm, something else wins,
        // and the arm's future is dropped where it stands.
        let mut compacting = std::pin::pin!(session.compact(None));
        tokio::select! {
            _ = &mut compacting => panic!(
                "the summarization streams at 1 token/s — it cannot have settled inside 800 ms, \
                 so this test would no longer be dropping an IN-FLIGHT future"
            ),
            () = tokio::time::sleep(Duration::from_millis(800)) => {}
        }
        assert!(
            session.is_compacting(),
            "precondition: the compaction must be in flight with its token installed"
        );
    } // `compacting` dropped here, at whatever `.await` it was parked on.

    assert!(
        !session.is_compacting(),
        "a DROPPED compact() future must release the cancel token — leaving it installed makes \
         `is_compacting()` true forever, and the TUI then swallows every later prompt into the \
         compaction queue with no `compaction_end` ever coming to drain it"
    );
}

/// SESS-041 — `abortCompaction` must cancel the **auto** compaction too.
///
/// pi's whole body is two aborts (`agent-session.ts:1930-1933` @v0.83.0):
///
/// ```ts
/// abortCompaction(): void {
///     this._compactionAbortController?.abort();
///     this._autoCompactionAbortController?.abort();
/// }
/// ```
///
/// cyrup had only the first line. `run_auto_compaction` installs its own child token in
/// `auto_compaction_cancel` and never touches `compaction_cancel`, so pressing Escape during the
/// post-run auto-compaction cancelled a `None` and the 10-18 s summarization ran to completion —
/// the one compaction the user did NOT ask for was the one they could not escape. `is_compacting`
/// already read BOTH fields, which is exactly what hid the asymmetry.
///
/// RED before the fix: `compaction_end` arrives with `aborted:false` (the summarization finishes
/// normally) instead of `aborted:true`.
#[tokio::test]
async fn abort_compaction_also_cancels_an_auto_compaction() {
    let fx = fixture();
    // `reserveTokens` just under the window, so the real run's own usage trips the THRESHOLD arm
    // and the post-run auto-compaction fires (round9's `real_run_threshold_compaction_emits_threshold_end`
    // uses the same lever).
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 127999}),
    )
    .unwrap();

    // 1 token/s, so the summarization is unambiguously still streaming when the abort lands.
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_text("a real answer worth some tokens")],
            StopReason::Stop,
        ),
        faux_assistant_message(
            vec![faux_text(
                "this auto-compaction summary is deliberately long so that it is still streaming \
                 when the user presses escape and abort_compaction fires the auto cancel token",
            )],
            StopReason::Stop,
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });

    // Wait for the AUTO compaction to start (reason `threshold`, not the manual `manual`), so the
    // auto cancel token is definitely installed, then abort.
    let mut started_auto = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        if let AgentSessionEvent::CompactionStart { reason } = &ev {
            assert_eq!(
                *reason,
                crate::CompactionReason::Threshold,
                "this test must exercise the AUTO path; a manual reason means the lever moved"
            );
            started_auto = true;
            break;
        }
    }
    assert!(
        started_auto,
        "the post-run threshold auto-compaction must start"
    );
    // Let the summarization stream actually open before cancelling it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.abort_compaction();

    let mut end: Option<(bool, Option<String>)> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        if let AgentSessionEvent::CompactionEnd {
            aborted,
            error_message,
            ..
        } = &ev
        {
            end = Some((*aborted, error_message.clone()));
            break;
        }
    }
    assert_eq!(
        end,
        Some((true, None)),
        "the auto compaction must report pi's abort shape — `aborted:true`, no errorMessage \
         (agent-session.ts:2142-2151); `aborted:false` means `abort_compaction` never reached \
         `auto_compaction_cancel`"
    );

    let _ = tokio::time::timeout(Duration::from_secs(20), running).await;
}

/// SEAM-112 — a compaction that FAILS must leave `agent.state.messages` exactly as it found it.
///
/// pi orders the re-seed strictly on the success path: `appendCompaction(...)` then
/// `this.agent.state.messages = sessionContext.messages;` (`agent-session.ts:2275-2280` auto,
/// `:1952-1955` manual), both AFTER the `signal.aborted` early-return and both inside the `try`,
/// so a cancelled, declined or throwing compaction never touches the agent transcript.
///
/// cyrup ran the re-seed unconditionally, before `match result`. That is only observable where the
/// agent transcript and the session file legitimately DISAGREE, and the overflow path is exactly
/// that place: `check_compaction` (`session.rs:4851`) calls `drop_trailing_assistant` to strip the
/// overflow response from the agent transcript before compacting, but that response was already
/// persisted on `message_end`, so re-seeding from `build_context_raw()` pulled it straight back.
///
/// **RED before the fix:** the transcript ends with the `stop_reason: Error` overflow assistant
/// again — the precise state `Agent::continue_run` refuses with `ContinueFromAssistant`
/// (`cyrup-agent/src/agent.rs:2004-2029`), i.e. a failed overflow compaction poisoned the session
/// for the retry that follows it.
#[tokio::test]
async fn a_failed_overflow_compaction_leaves_the_agent_transcript_untouched() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    // Turn 1: an ordinary answer, so the branch has something to compact.
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("first answer worth some tokens")],
        StopReason::Stop,
    )]);
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;

    // Turn 2: a context-overflow error attributed to the SAME model the session runs (pi's
    // `_checkCompaction` same-model guard), so the post-run loop drops the trailing assistant and
    // enters `run_auto_compaction(Overflow, will_retry: true)`. The NEXT scripted response is the
    // summarization call — an error, so the compaction fails.
    let model = session.model().expect("session must have a resolved model");
    faux.set_responses(vec![
        AssistantMessage::errored(
            model.provider.clone(),
            model.model.as_str(),
            None,
            StopReason::Error,
            "context_length_exceeded",
        ),
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions {
                error_message: Some("summarizer exploded".into()),
                ..Default::default()
            },
        ),
    ]);

    let stream = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    let events: Vec<AgentSessionEvent> = stream.collect().await;

    // The compaction really STARTED — i.e. `prepare` produced a preparation and the re-seed site
    // was reached. Without this the test could pass on a path that never re-seeds at all.
    let started_overflow = events.iter().any(|e| {
        matches!(
            e,
            AgentSessionEvent::CompactionStart { reason } if *reason == crate::CompactionReason::Overflow
        )
    });
    assert!(
        started_overflow,
        "the overflow compaction must start: {:?}",
        kinds(&events)
    );
    // …and really FAILED: no result on the end event.
    let failed = events
        .iter()
        .any(|e| matches!(e, AgentSessionEvent::CompactionEnd { result, .. } if result.is_none()));
    assert!(
        failed,
        "the summarization error must fail the compaction: {:?}",
        kinds(&events)
    );

    let transcript = session.agent_messages().await;
    assert!(
        !matches!(
            transcript.last(),
            Some(cyrup_agent::AgentMessage::Assistant(_))
        ),
        "a failed compaction must not resurrect the overflow response `drop_trailing_assistant` \
         had just removed (agent-session.ts:2275-2280 puts the re-seed on the success path only); \
         transcript = {transcript:?}"
    );
}

/// The point of compaction: it must shrink what the NEXT request sends to the provider.
///
/// pi does this explicitly — `agent-session.ts:1874-1876` (manual `compact`) and `:2155-2157`
/// (`_runAutoCompaction`) both assign `this.agent.state.messages = sessionContext.messages`
/// straight after `appendCompaction`, because `appendCompaction` alone only writes a JSONL entry.
///
/// cyrup built that same context solely to COUNT it for the result payload and then dropped it, so
/// `/compact` reported success, the TUI re-rendered a compacted transcript from the session, and the
/// very next turn still shipped the entire pre-compaction history. The session view and the agent
/// view silently disagreed — which is why this asserts on `agent_messages()` (the agent's own
/// in-memory transcript, the thing actually sent) and NOT on `raw_context_messages()`, which reads
/// the manager and was correct all along.
#[tokio::test]
async fn compaction_rebuilds_the_agents_in_memory_transcript() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let before = session.agent_messages().await.len();
    let result = session.compact(None).await.expect("compaction succeeds");
    assert!(!result.summary.is_empty(), "a summary was produced");

    let after = session.agent_messages().await;
    assert!(
        after.len() < before,
        "compaction must SHRINK the agent's transcript, not just write a JSONL entry \
         (before={before}, after={})",
        after.len()
    );

    // ...and it must equal the session's own rebuilt context: the two views agreeing is the whole
    // property. A shrink to some other length would mean the agent was re-seeded from the wrong
    // thing.
    let rebuilt = session.raw_context_messages().await.len();
    assert_eq!(
        after.len(),
        rebuilt,
        "the agent transcript must BE the compacted session context"
    );
}

/// SEAM-124 / TUI-104 — `navigate_tree` refuses while a compaction is running, with pi's message
/// (`agent-session.ts:3588-3592` @v0.87.1), and leaves the leaf where it was. Without the guard the
/// leaf moved first and the compaction's entry then landed on the new branch carrying a
/// `first_kept_entry_id` from the abandoned one.
#[tokio::test]
async fn navigate_tree_refuses_during_a_compaction() {
    let fx = fixture();
    // 1 token/s keeps the summarization in flight long enough to observe.
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("b")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_text(
                "a deliberately long summary so the compaction is still streaming while the \
                 navigation is attempted",
            )],
            StopReason::Stop,
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    let first_user = session.user_messages_for_forking().await[0]
        .entry_id
        .clone();
    // Read before the compaction starts: the parked `compact()` future below can hold the manager
    // lock at its await point, so a `leaf_id()` while it is pinned-but-unpolled would wait forever.
    let leaf = session.leaf_id().await;

    {
        let mut compacting = std::pin::pin!(session.compact(None));
        tokio::select! {
            _ = &mut compacting => panic!("the summarization cannot settle inside 800 ms at 1 token/s"),
            () = tokio::time::sleep(Duration::from_millis(800)) => {}
        }
        assert!(
            session.is_compacting(),
            "precondition: compaction in flight"
        );
        match session
            .navigate_tree(first_user, crate::NavigateTreeOptions::default())
            .await
        {
            Err(e @ crate::SessionServiceError::NavigateTreeWhileCompacting) => assert_eq!(
                e.to_string(),
                "Wait for the current compaction or tree navigation to finish before navigating the \
             session tree."
            ),
            other => panic!("expected the compaction refusal, got {other:?}"),
        }
    } // the in-flight compaction is dropped (and its cancel guard released) here
    assert_eq!(
        session.leaf_id().await,
        leaf,
        "the refused navigation did not move the leaf"
    );
}

/// SEAM-124 — the same call during a live run is refused with pi's streaming message
/// (`agent-session.ts:3585-3587`), which the extension control op and the command API reached
/// unguarded.
#[tokio::test]
async fn navigate_tree_refuses_during_a_run() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a")], StopReason::Stop),
        faux_assistant_message(
            vec![faux_text(
                "a deliberately long reply so the run is still live",
            )],
            StopReason::Stop,
        ),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();
    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let first_user = session.user_messages_for_forking().await[0]
        .entry_id
        .clone();

    let running = session.clone();
    let run = tokio::spawn(async move { running.prompt("tell me two").await });
    for _ in 0..100 {
        if session.is_run_active() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(session.is_run_active(), "precondition: the run is live");
    let leaf = session.leaf_id().await;
    match session
        .navigate_tree(first_user, crate::NavigateTreeOptions::default())
        .await
    {
        Err(e @ crate::SessionServiceError::NavigateTreeWhileStreaming) => assert_eq!(
            e.to_string(),
            "Wait for the current response to finish before navigating the session tree."
        ),
        other => panic!("expected the streaming refusal, got {other:?}"),
    }
    assert_eq!(session.leaf_id().await, leaf, "the leaf did not move");
    session.abort_and_settle().await;
    run.abort();
}

// ------------------------------------------- SESS-062 abort() reaches the whole run -------------

/// Settings that make the real run's own usage trip the post-run THRESHOLD compaction — the same
/// lever `abort_compaction_also_cancels_an_auto_compaction` uses.
fn sess062_threshold_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 127999}),
    )
    .unwrap();
    cli
}

/// `abort()` must cancel an in-flight post-run AUTO-compaction AND stop the continuation, not just
/// the agent run — Pi `abort()` (`agent-session.ts:2075-2085` @v0.87.1) calls `abortCompaction()`
/// and `abortBranchSummary()` between `abortRetry()` and `agent.abort()`.
///
/// cyrup's `abort()` was `abort_retry(); agent.abort();` — `abort_compaction` existed but was
/// reachable only from the TUI Escape path, so ACP cancel and SIGINT let the 10-18 s auto
/// summarization run to completion and then continued the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sess062_abort_cancels_an_in_flight_auto_compaction_and_stops_the_run() {
    let fx = fixture();
    // 1 token/s, so the summarization is unambiguously still streaming when the abort lands.
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_text("a real answer worth some tokens")],
            StopReason::Stop,
        ),
        faux_assistant_message(
            vec![faux_text(
                "this auto-compaction summary is deliberately long so that it is still streaming \
                 when abort() lands and the auto cancel token is fired",
            )],
            StopReason::Stop,
        ),
        // A THIRD response, so a wrong continuation has something to answer with and the test fails
        // on the extra `agent_start` rather than on a starved faux provider.
        faux_assistant_message(
            vec![faux_text("a continuation that must never run")],
            StopReason::Stop,
        ),
    ]);

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess062_threshold_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });

    // Every event of the whole run, so `agent_start` can be COUNTED (the first run emits one).
    let mut ks: Vec<&'static str> = Vec::new();
    // Wait for the AUTO compaction to start so the auto cancel token is definitely installed.
    let mut started_auto = false;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        ks.push(ev.kind());
        if let AgentSessionEvent::CompactionStart { reason } = &ev {
            assert_eq!(
                *reason,
                crate::CompactionReason::Threshold,
                "this test must exercise the AUTO path; a manual reason means the lever moved"
            );
            started_auto = true;
            break;
        }
    }
    assert!(
        started_auto,
        "the post-run threshold auto-compaction must start"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The ONE call under test: the generic abort, not `abort_compaction`.
    session.abort();

    let mut aborted_end: Option<bool> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        if let AgentSessionEvent::CompactionEnd { aborted, .. } = &ev {
            aborted_end = Some(*aborted);
        }
        let k = ev.kind();
        ks.push(k);
        if k == "agent_settled" {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(20), running).await;

    // (a) the auto compaction reports pi's abort shape
    assert_eq!(
        aborted_end,
        Some(true),
        "abort() must reach abort_compaction and cancel the in-flight auto summarization: {ks:?}"
    );
    // (b) no second agent run follows
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_start").count(),
        1,
        "exactly the ONE original run — no continuation may start after an abort: {ks:?}"
    );
    // (c) the run settles exactly once
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_settled").count(),
        1,
        "exactly one agent_settled: {ks:?}"
    );
}

/// The gap Pi's `_agentRunAbortRequested` latch exists to close, and the one `abortCompaction()`
/// alone CANNOT: an abort that lands AFTER `agent_end` and BEFORE the post-run step.
///
/// In that window `agent.abort()` is a no-op (the agent run already finished, so its per-run token
/// cancels nothing) and `abort_compaction()` is a no-op too (no compaction token is installed yet —
/// `check_compaction` has not run). Only the latch stops the post-run step, which is why Pi reads it
/// at the head of `_handlePostAgentRun` (`agent-session.ts:1497-1500` @v0.87.1).
///
/// The window is hit deterministically with a native extension whose `agent_end` handler calls
/// `abort()`: the whole ordered subscriber dispatch for `agent_end` returns before `drive_run`
/// reaches `handle_post_agent_run`.
///
/// Without the latch the post-run threshold compaction runs to completion and `continue_run` starts
/// a SECOND agent run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sess062_abort_between_agent_end_and_the_post_run_step_stops_the_continuation() {
    struct AbortAtAgentEnd {
        session: std::sync::Mutex<Option<std::sync::Weak<crate::AgentSession>>>,
        fired: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl NativeExtension for AbortAtAgentEnd {
        fn id(&self) -> ExtensionId {
            ExtensionId::from("sess062-abort-at-agent-end")
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe(&[EventKind::AgentEnd]);
            Ok(())
        }
        async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            if matches!(ev, HostEvent::AgentEnd { .. })
                && self.fired.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
                && let Some(s) = self
                    .session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
            {
                s.abort();
            }
            HookOutcome::Noop
        }
    }

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_text("a real answer worth some tokens")],
            StopReason::Stop,
        ),
        // Only reachable if a wrong continuation starts.
        faux_assistant_message(
            vec![faux_text("a continuation that must never run")],
            StopReason::Stop,
        ),
    ]);

    let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ext = Arc::new(AbortAtAgentEnd {
        session: std::sync::Mutex::new(None),
        fired: Arc::clone(&fired),
    });
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess062_threshold_settings())
        .with_native_extension(Arc::clone(&ext) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    *ext.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&session));

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });

    let mut ks: Vec<&'static str> = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        let k = ev.kind();
        ks.push(k);
        if k == "agent_settled" {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(20), running).await;

    assert_eq!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the abort must have been fired from exactly one agent_end handler: {ks:?}"
    );
    // SESS-062's invariant is about the POST-`agent_end` window: the abort latch must stop the
    // post-run step before it can start a compaction, where `abort_compaction()` cannot help because
    // no token is installed yet. Scoped to that window explicitly since SEAM-126 landed pi's
    // `_compactBeforeNextAssistantResponse` (agent-session.ts:693-694 @v0.87.1) on the turn
    // boundary: this session's threshold settings are already over budget, so a legitimate
    // turn-boundary compaction now runs BEFORE `agent_end` — upstream does exactly the same, since
    // pi runs `prepareNextTurnWithContext` after a terminating turn too. A flat count of zero would
    // now be asserting the absence of the compaction this row added, not the latch.
    let agent_end_at = ks
        .iter()
        .position(|k| *k == "agent_end")
        .unwrap_or(ks.len());
    assert_eq!(
        ks.iter()
            .skip(agent_end_at)
            .filter(|k| **k == "compaction_start")
            .count(),
        0,
        "the latch must stop the post-run step BEFORE the compaction starts — abort_compaction() \
         cannot help here, no token is installed yet: {ks:?}"
    );
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_start").count(),
        1,
        "exactly the ONE original run — no continuation may start after an abort in the \
         post-agent_end window: {ks:?}"
    );
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_settled").count(),
        1,
        "exactly one agent_settled: {ks:?}"
    );
}

/// Auto-retry with a near-instant backoff and auto-compaction OFF, so the only post-run work is the
/// retry continuation under test.
fn sess062_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "retry",
        serde_json::json!({"enabled": true, "maxRetries": 3, "baseDelayMs": 1}),
    )
    .unwrap();
    cli.set_field("compaction", serde_json::json!({"enabled": false}))
        .unwrap();
    cli
}

/// pi's FIFTH latch site — the one in `_runAgentPrompt`'s `finally`:
/// `if (this._agentRunAbortRequested) this._finishCancelledRetry();` (`agent-session.ts:1484`
/// @v0.87.1) — and the only one reachable when a retry is open and the loop exits on its OWN
/// condition.
///
/// The path, all of it inside the post-run loop: a retryable assistant error → `prepare_retry` sets
/// `retry_attempt = 1` and returns true → `handle_post_agent_run` returns true → the loop awaits
/// `continue_run()` → the abort lands during that continuation → the loop's head
/// (`while !abort_requested() && handle_post_agent_run()`, pi's `while (!this._agentRunAbortRequested)`
/// at `:1473`) exits WITHOUT re-entering `handle_post_agent_run`, so none of its four
/// `finish_cancelled_retry()` calls run. `settle_run` is the only site left.
///
/// The window is hit deterministically with a native extension that aborts on the SECOND
/// `agent_end` — the end of the retry continuation — because the whole ordered `agent_end` dispatch
/// returns before the driver re-reads the loop condition. The continuation is scripted as a second
/// retryable error precisely so `on_assistant_message_end` does NOT reset the counter first: pi
/// resets it there only for `stopReason !== "error"` (`agent-session.ts:953-960`), and cyrup is
/// faithful to that, so an error-terminated continuation leaves the retry genuinely open.
///
/// What leaks without this site is permanent, not cosmetic: NEITHER side resets the retry counter at
/// run START (pi zeroes `_retryAttempt` at :960, :1519 and :3366 only; cyrup in `prepare_retry`,
/// `on_assistant_message_end`, `handle_post_agent_run` and `finish_cancelled_retry`), so the stranded
/// `attempt = 1` survives into every later run until `retry_attempt() >= retry_max_retries` refuses
/// auto-retry for the rest of the session — and no `auto_retry_end` ever closes the "retrying" state
/// a consumer opened on `auto_retry_start` / `agent_end`'s `willRetry`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sess062_an_abort_during_a_retry_continuation_closes_the_retry_sequence() {
    /// Aborts on the SECOND `agent_end` — the end of the retry continuation.
    struct AbortAtRetryContinuationEnd {
        session: std::sync::Mutex<Option<std::sync::Weak<crate::AgentSession>>>,
        ends: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl NativeExtension for AbortAtRetryContinuationEnd {
        fn id(&self) -> ExtensionId {
            ExtensionId::from("sess062-abort-at-retry-continuation-end")
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe(&[EventKind::AgentEnd]);
            Ok(())
        }
        async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            if matches!(ev, HostEvent::AgentEnd { .. })
                && self.ends.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
                && let Some(s) = self
                    .session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .and_then(std::sync::Weak::upgrade)
            {
                s.abort();
            }
            HookOutcome::Noop
        }
    }

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let transient = || {
        faux_assistant_message_with(
            Vec::new(),
            StopReason::Error,
            FauxMessageOptions {
                error_message: Some("overloaded".into()),
                ..Default::default()
            },
        )
    };
    faux.set_responses(vec![
        // Turn 1: a retryable transient error → prepare_retry → attempt 1 → continue.
        transient(),
        // Turn 2 (the retry continuation): retryable again, so the counter is still 1 when the
        // abort latches on this turn's `agent_end`.
        transient(),
        // Only reachable if a wrong further continuation starts.
        faux_assistant_message(
            vec![faux_text("a continuation that must never run")],
            StopReason::Stop,
        ),
    ]);

    let ends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ext = Arc::new(AbortAtRetryContinuationEnd {
        session: std::sync::Mutex::new(None),
        ends: Arc::clone(&ends),
    });
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess062_retry_settings())
        .with_native_extension(Arc::clone(&ext) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    *ext.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&session));

    let mut stream = session.subscribe();
    let running = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            let _ = session.prompt("go").await;
            session.wait_for_idle().await;
        }
    });

    let mut ks: Vec<&'static str> = Vec::new();
    let mut retry_end: Option<(bool, u32, Option<String>)> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
        if let AgentSessionEvent::AutoRetryEnd {
            success,
            attempt,
            final_error,
        } = &ev
        {
            retry_end = Some((*success, *attempt, final_error.clone()));
        }
        let k = ev.kind();
        ks.push(k);
        if k == "agent_settled" {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(20), running).await;

    // Preconditions: the retry really ran, and the abort really fired from the continuation's end.
    assert!(
        ks.contains(&"auto_retry_start"),
        "precondition: the transient error must start an auto-retry: {ks:?}"
    );
    assert_eq!(
        ks.iter().filter(|k| **k == "agent_start").count(),
        2,
        "precondition: exactly the original run plus the retry continuation: {ks:?}"
    );
    assert_eq!(
        ends.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "precondition: the abort fired from the SECOND agent_end: {ks:?}"
    );

    // (a) the retry sequence is CLOSED, with pi's exact `_finishCancelledRetry` payload
    //     (`agent-session.ts:3363-3374` @v0.87.1).
    assert_eq!(
        retry_end,
        Some((false, 1, Some("Retry cancelled".to_string()))),
        "the aborted retry must emit auto_retry_end{{success:false, attempt:1, \
         finalError:\"Retry cancelled\"}}: {ks:?}"
    );
    // (b) pi's `finally` runs it BEFORE `_emitAgentSettled()` (`:1484` vs `:1488`), so a consumer
    //     that tears its run state down on `agent_settled` still sees the close.
    let settled_at = ks
        .iter()
        .position(|k| *k == "agent_settled")
        .expect("the run must settle");
    let retry_end_at = ks
        .iter()
        .rposition(|k| *k == "auto_retry_end")
        .expect("auto_retry_end must be in the stream");
    assert!(
        retry_end_at < settled_at,
        "auto_retry_end must precede agent_settled: {ks:?}"
    );
    // (c) the counter is not leaked into every later run.
    assert_eq!(
        session.retry_attempt(),
        0,
        "the cancelled retry must reset the attempt counter; a leak permanently refuses auto-retry"
    );
}

/// pi's `_willRetryAfterAgentEnd` opens with `if (this._agentRunAbortRequested) return false;`
/// (`agent-session.ts:977` @v0.87.1), ahead of the settings and budget reads.
///
/// The field it feeds is `agent_end.willRetry` (`subscriber.rs:215`) — a PROMISE that a continuation
/// is coming, which every front-end reads to keep a "retrying…" affordance up instead of settling the
/// turn. Once the latch is set `handle_post_agent_run` will refuse the retry, so answering `true`
/// strands that affordance on a continuation that never arrives.
#[tokio::test]
async fn sess062_will_retry_after_agent_end_is_false_once_the_abort_latch_is_set() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("hi")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess062_retry_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let transient = AssistantMessage::errored(
        "faux".into(),
        "faux-1",
        None,
        StopReason::Error,
        "overloaded: please retry",
    );
    let retryable = [Arc::new(cyrup_agent::AgentMessage::Assistant(Arc::new(
        transient,
    )))];

    // Baseline: with no abort latched, this IS a retryable error inside budget.
    assert!(
        session.will_retry_after_agent_end(&retryable),
        "precondition: a transient error with a fresh budget promises a retry"
    );

    session.set_abort_requested(true);
    assert!(
        !session.will_retry_after_agent_end(&retryable),
        "once abort() has latched, agent_end must not promise a retry that \
         handle_post_agent_run is already going to refuse"
    );

    // And the latch is the only thing that changed: clearing it restores the promise, so this pins
    // the guard rather than some incidental state the assertion above disturbed.
    session.set_abort_requested(false);
    assert!(session.will_retry_after_agent_end(&retryable));
}

/// `abort()` must cancel an in-flight BRANCH SUMMARIZATION too — pi calls `abortBranchSummary()`
/// immediately after `abortCompaction()` (`agent-session.ts:2081` @v0.87.1). That is the second half
/// of this row's headline claim, and `abort_compaction()` cannot cover it: a branch summary installs
/// its token in `branch_summary_cancel` (`forking.rs:307`), a slot `abort_compaction` never reads.
///
/// `/tree`'s own summarization is the reachable path: no agent run is active, so `agent.abort()` is a
/// no-op and only `abort_branch_summary()` can reach the 1-token/s summary call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sess062_abort_cancels_an_in_flight_branch_summarization() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(FauxConfig {
        tokens_per_second: Some(1.0),
        ..FauxConfig::default()
    }));
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("two")], StopReason::Stop),
        // The branch summary: long enough at 1 token/s that finishing it takes far longer than the
        // bound below, so an uncancelled summarization cannot pass this test.
        faux_assistant_message(
            vec![faux_text(
                "this branch summary is deliberately long so that it is unambiguously still \
                 streaming when abort() lands and the branch-summary cancel token is fired, and so \
                 that running it to completion cannot fit inside this test's bound",
            )],
            StopReason::Stop,
        ),
    ]);

    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess062_retry_settings())
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    let first_user = session.user_messages_for_forking().await[0]
        .entry_id
        .clone();

    let nav = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .navigate_tree(
                    first_user,
                    crate::NavigateTreeOptions {
                        summarize: true,
                        ..Default::default()
                    },
                )
                .await
        }
    });

    // Wait for the branch summarization to be genuinely in flight (its token installed).
    let mut spun = 0;
    while !session.is_compacting() && spun < 200 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        spun += 1;
    }
    assert!(
        session.is_compacting(),
        "precondition: the branch summarization must be in flight"
    );

    // The ONE call under test: the generic abort, not `abort_branch_summary`.
    session.abort();

    let outcome = tokio::time::timeout(Duration::from_secs(15), nav)
        .await
        .expect("abort() must cut the branch summarization short, not wait it out")
        .expect("the navigate_tree task must not panic")
        .expect("navigate_tree must report the abort, not fail");
    assert!(
        outcome.aborted,
        "the cancelled branch summarization reports pi's abort shape: {outcome:?}"
    );
    assert!(
        outcome.cancelled,
        "an aborted summarization cancels the navigation: {outcome:?}"
    );
    assert!(
        outcome.summary_entry.is_none(),
        "no branch_summary entry is appended for a cancelled summarization: {outcome:?}"
    );
}

// ------------------- SESS-052 a context_edit invalidates the trigger's usage anchor -------------

/// Auto-compaction OFF — the phase that builds the history must not compact it.
fn sess052_no_autocompaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("compaction", serde_json::json!({"enabled": false}))
        .unwrap();
    cli.set_field("retry", serde_json::json!({"enabled": false}))
        .unwrap();
    cli
}

/// Auto-compaction ON with the threshold at 2000 tokens (the faux model's window is 128000, so
/// `reserveTokens: 126000` puts it there). The seeded history is ~10000 tokens of user text, so the
/// last assistant's own provider `usage` — which counts that text as INPUT — is well over the
/// threshold, while the projection with that text OMITTED is a handful of tokens. Whichever number
/// the trigger reads decides whether a compaction starts at all.
fn sess052_split_threshold_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 126000}),
    )
    .unwrap();
    cli.set_field("retry", serde_json::json!({"enabled": false}))
        .unwrap();
    cli
}

/// Seed a fresh fixture with ~10000 tokens of user text and one small assistant reply, compaction
/// OFF, and return the persisted session file plus the fixture that owns its temp dir.
async fn sess052_seed_a_big_history() -> (Fixture, std::path::PathBuf) {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .cli_settings(sess052_no_autocompaction_settings())
        .build()
        .await
        .expect("build")
        // BOUND: an unbound session has no post-run driver, so the assistant turn — and with it the
        // `usage` reading the whole separation depends on — never lands in the file.
        .into_shared();
    let file = session
        .session_file()
        .await
        .expect("a persisted session to resume from");
    let big = "context ".repeat(5_000); // 40000 chars ⇒ ~10000 tokens at chars/4
    let _ = session.prompt(big.as_str()).await.expect("the big prompt");
    session.wait_for_idle().await;
    drop(session);
    (fx, file)
}

/// Append a pi-shaped OMITTING `context_edit` (`replacement: null`) to a persisted session file,
/// targeting its first `user` message — the big one — and parented on the file's last entry so it
/// lands on the branch.
///
/// Written by hand because cyrup has no `append_context_edit` yet (that arrives with `EXT-078`), and
/// because a pi ≥v0.87.0 file resumed in cyrup is exactly the reachable scenario `SESS-052` is about.
/// The shape is `ContextEditEntry` (`session-manager.ts:174-180` @v0.87.1); a `null` replacement is
/// what pi's own `_omitRecoveryAttempt` writes (`agent-session.ts:1015-1031` @v0.87.1).
fn sess052_append_omitting_context_edit(file: &std::path::Path) -> String {
    let text = std::fs::read_to_string(file).expect("the session file must be readable");
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let entry_id = |l: &str| -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(l).ok()?;
        Some(v.get("id")?.as_str()?.to_string())
    };
    let parent = lines
        .last()
        .and_then(|l| entry_id(l))
        .expect("the session file must have entries");
    let target = lines
        .iter()
        .find_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            if v.get("type").and_then(serde_json::Value::as_str) != Some("message") {
                return None;
            }
            if v.pointer("/message/role")
                .and_then(serde_json::Value::as_str)
                != Some("user")
            {
                return None;
            }
            Some(v.get("id")?.as_str()?.to_string())
        })
        .expect("the seeded history must contain a user message to omit");
    let line = serde_json::json!({
        "type": "context_edit",
        "id": "sess052-edit-1",
        "parentId": parent,
        "timestamp": "2026-09-01T00:00:00Z",
        "targetId": target,
        "replacement": serde_json::Value::Null,
    });
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .expect("append to the session file");
    use std::io::Write;
    writeln!(f, "{line}").expect("write the context_edit entry");
    target
}

/// Resume `file` with the split threshold, run one turn, and return the run's event kinds.
async fn sess052_resume_and_run_one_turn(
    fx: &Fixture,
    file: std::path::PathBuf,
) -> Vec<&'static str> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("SESS052 SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("SESS052 SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    let mut cfg = base_config(fx);
    cfg.target = crate::SessionTarget::Resume(file);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .cli_settings(sess052_split_threshold_settings())
        .build()
        .await
        .expect("resume")
        .into_shared();
    let stream = session.prompt("carry on").await.expect("resumed prompt");
    session.wait_for_idle().await;
    kinds(&stream.collect::<Vec<_>>().await)
}

/// SESS-052, the ESTIMATE half at the service layer. pi tests for an admitted `context_edit` FIRST,
/// ahead of both the direct-usage read and the error/zero fallback
/// (`agent-session.ts:2703-2712` @v0.87.1), and its turn-boundary trigger reads
/// `estimateProjectedContextTokens(projection, branch)` unconditionally (`:596`).
///
/// Why the precedence matters: `assistantMessage.usage` is a reading of the context the PROVIDER was
/// sent, INPUT tokens included. Once a `context_edit` later in the branch has omitted part of that
/// context, the reading describes text the model will never see again — so upstream throws the anchor
/// away and re-estimates the projection. cyrup's two service-layer triggers called the bare
/// `estimate_context_tokens_raw`, which keeps the anchor, so an omitted 10000-token turn went on being
/// counted and auto-compaction fired on a session with nothing left to compact.
///
/// Phase 1 is the LIVENESS guard: the same seeded history with NO `context_edit` MUST compact, so the
/// fixture is demonstrably over the threshold and phase 2's silence means the edit was honoured rather
/// than that the trigger was never armed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sess052_a_resumed_context_edit_makes_the_trigger_re_estimate_instead_of_trusting_usage() {
    // ---- phase 1: no edit ⇒ the provider-usage anchor stands ⇒ over threshold ⇒ compaction.
    let (control_fx, control_file) = sess052_seed_a_big_history().await;
    let control_ks = sess052_resume_and_run_one_turn(&control_fx, control_file).await;
    assert!(
        sess052_compacted_before_the_run(&control_ks),
        "LIVENESS: with no context_edit the ~10000-token usage anchor stands and the turn-boundary \
         trigger must fire, else phase 2 proves nothing: {control_ks:?}"
    );

    // ---- phase 2: a pi-written OMITTING edit on a SEPARATE seed of the same history.
    //      (Separate, because phase 1's own compaction entry would invalidate the anchor too and
    //       phase 2 would then pass for the wrong reason.)
    let (edited_fx, edited_file) = sess052_seed_a_big_history().await;
    let target = sess052_append_omitting_context_edit(&edited_file);
    assert!(
        !target.is_empty(),
        "the edit must target a real entry, else the projection has nothing to invalidate"
    );
    let edited_ks = sess052_resume_and_run_one_turn(&edited_fx, edited_file).await;
    assert!(
        !sess052_compacted_before_the_run(&edited_ks),
        "an admitted context_edit must make the turn-boundary trigger DISCARD the now-meaningless \
         provider-usage anchor and re-estimate the EDITED projection (a handful of tokens, far under \
         the 2000-token threshold): {edited_ks:?}"
    );
}

/// Whether a compaction started at the TURN BOUNDARY — before the run's first `agent_start` — which
/// is the site this test pins (`compact_before_next_assistant_response`, pi
/// `estimateProjectedContextTokens` at `agent-session.ts:596` @v0.87.1).
///
/// Deliberately NOT `ks.contains("compaction_start")`. A POST-run compaction still fires in phase 2,
/// and for a reason that is a DIFFERENT, unported piece rather than a failure of this change: the
/// anchor rule correctly KEEPS the usage of the assistant produced by the resumed turn, because that
/// assistant post-dates the edit — and that usage is large because cyrup still sends the UNEDITED
/// context to the provider. pi installs the canonical, edit-applied projection on the request
/// (`_installAgentRequestProjection` through `prepareRequest`), which area 03's own `SESS-052` body
/// records as unported with no row of its own ("the session-side canonical-context projection that pi
/// installs through it, `_installAgentRequestProjection`, is unported and has no row"). Until that
/// lands, the provider reading is honestly large and trusting it is the correct rule, not a bug here.
fn sess052_compacted_before_the_run(ks: &[&'static str]) -> bool {
    let first_start = ks.iter().position(|k| *k == "agent_start").unwrap_or(0);
    ks.iter()
        .take(first_start)
        .any(|k| *k == "compaction_start")
}
