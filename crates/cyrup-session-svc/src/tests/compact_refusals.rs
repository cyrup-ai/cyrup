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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{AssistantMessage, ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxConfig, FauxMessageOptions,
    FauxProvider,
};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{AgentSessionEvent, SessionBuilder};
use futures::StreamExt;

fn kinds(events: &[AgentSessionEvent]) -> Vec<&'static str> {
    events.iter().map(AgentSessionEvent::kind).collect()
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
            HostEvent::SessionBeforeCompact { .. } => {
                HookOutcome::Block { reason: Some("not now".to_string()), terminate: false }
            }
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
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build");

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

    let first = session.compact(None).await.expect("the first compaction succeeds");
    assert!(!first.summary.is_empty(), "the first compaction produced a summary");

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
    assert_eq!(err.to_string(), "Compaction cancelled", "verbatim Pi message");
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
        if let AgentSessionEvent::CompactionEnd { aborted, error_message, .. } = &ev {
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
    // and the post-run auto-compaction fires (`compaction_tokens_after::real_run_threshold_compaction_emits_threshold_end`
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
        faux_assistant_message(vec![faux_text("a real answer worth some tokens")], StopReason::Stop),
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
    assert!(started_auto, "the post-run threshold auto-compaction must start");
    // Let the summarization stream actually open before cancelling it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    session.abort_compaction();

    let mut end: Option<(bool, Option<String>)> = None;
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        if let AgentSessionEvent::CompactionEnd { aborted, error_message, .. } = &ev {
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
    assert!(started_overflow, "the overflow compaction must start: {:?}", kinds(&events));
    // …and really FAILED: no result on the end event.
    let failed = events.iter().any(|e| {
        matches!(e, AgentSessionEvent::CompactionEnd { result, .. } if result.is_none())
    });
    assert!(failed, "the summarization error must fail the compaction: {:?}", kinds(&events));

    let transcript = session.agent_messages().await;
    assert!(
        !matches!(transcript.last(), Some(cyrup_agent::AgentMessage::Assistant(_))),
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

/// Facade parity vs Pi `agent-session.ts` / `sdk.ts`: the `CompactionResult` flow — a session with nothing to compact reports the
/// error through the result rather than failing the call, and still emits the start/end pair.
#[tokio::test]
async fn compact_on_small_session_errors_nothing_to_compact_and_emits_events() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();
    let mut sub = session.subscribe();

    // Nothing to compact on a fresh/tiny session: an ERROR carrying Pi's reason (Pi throws
    // "Nothing to compact (session too small)", agent-session.ts:1806), no panic, events still flow.
    let err = session.compact(None).await.expect_err("small session has nothing to compact");
    assert_eq!(err.to_string(), "Nothing to compact (session too small)");

    let mut saw_start = false;
    let mut saw_end = false;
    for _ in 0..6 {
        match tokio::time::timeout(std::time::Duration::from_millis(200), {
            use futures::StreamExt;
            sub.next()
        })
        .await
        {
            Ok(Some(ev)) => match ev.kind() {
                "compaction_start" => saw_start = true,
                "compaction_end" => saw_end = true,
                _ => {}
            },
            _ => break,
        }
    }
    assert!(saw_start && saw_end, "compaction_start + compaction_end must be emitted");
}

// ==================================== the OVERRIDE arm of the same `session_before_compact` ====

/// A native extension subscribed to `session_before_compact` that READS the typed
/// `CompactionPreparation` off the event and returns a custom-summary override (Pi
/// `SessionBeforeCompactResult.compaction`, agent-session.ts:1672-1693). Records the preparation it
/// observed so the test can assert the typed payload actually crossed the seam.
struct CompactionOverrider {
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}
#[async_trait::async_trait]
impl NativeExtension for CompactionOverrider {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("compaction-overrider")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionBeforeCompact]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionBeforeCompact { preparation, reason, .. } => {
                self.seen.lock().unwrap().push(preparation.clone());
                // Derive the override summary from the REAL preparation so the assertion proves the
                // typed payload was read, not fabricated.
                let first_kept = preparation
                    .get("firstKeptEntryId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                HookOutcome::Mutate(EventPatch::CompactionOverride(serde_json::json!({
                    "summary": format!("ext-summary[{reason}|firstKept={first_kept}]"),
                })))
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// L4 gap #5: an ASSEMBLED manual compaction where a native guest reads the typed
/// `CompactionPreparation` and returns a custom-summary override — the override lands in the appended
/// compaction entry (`fromExtension`) and flows out as the `CompactionResult.summary`, replacing the
/// default model summarization (no summarizer call needed).
#[tokio::test]
async fn compaction_before_compact_override_lands_in_entry() {
    let fx = fixture();
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    // Only the two turn responses — the override skips the model summarizer entirely.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .with_native_extension(Arc::new(CompactionOverrider { seen: seen.clone() }))
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;

    let cr = session
        .compact(None)
        .await
        .expect("an aggressive-keep compaction over two turns produces a result");

    // The extension read a REAL preparation: it carries the Pi `CompactionPreparation` fields.
    let observed = seen.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "the before_compact hook fired exactly once");
    let prep = &observed[0];
    assert!(prep.get("firstKeptEntryId").is_some(), "typed preparation carries firstKeptEntryId: {prep}");
    assert!(prep.get("messagesToSummarize").is_some(), "typed preparation carries messagesToSummarize: {prep}");
    assert!(prep.get("tokensBefore").is_some(), "typed preparation carries tokensBefore: {prep}");

    // The override summary landed in the resulting compaction entry (fromExtension), replacing the
    // default model summary.
    assert!(
        cr.summary.starts_with("ext-summary[manual|firstKept="),
        "the extension override summary lands in the compaction result: {}",
        cr.summary
    );

    // And it is durable in the exported JSONL as a compaction entry.
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(jsonl.contains("ext-summary[manual"), "the override summary is persisted: {jsonl}");
    assert!(jsonl.contains("\"type\":\"compaction\""), "a compaction entry was appended");
}
