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
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxConfig, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionEvent, SessionBuilder, SessionConfig};
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
    Fixture { _tmp: tmp, cwd, agent_dir }
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
            HostEvent::SessionBeforeCompact { .. } => {
                HookOutcome::Block { reason: Some("not now".to_string()) }
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
