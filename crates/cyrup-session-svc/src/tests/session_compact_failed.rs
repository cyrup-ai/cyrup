//! SESS-050 — `session_compact_failed` (pi `SessionCompactFailedEvent`,
//! `core/extensions/types.ts:795-807` @v1.1.0), and `agent_settled.aborted` (pi
//! `AgentSettledEvent.aborted`, `:1005-1009`).
//!
//! pi emits `session_compact_failed` to the extensions right after every failing `compaction_end`
//! (`_emitSessionCompactFailed`, `core/agent-session.ts:1053-1057`), from three sites: the manual
//! `compact()` catch (`:2889-2905`), the overflow-recovery-already-attempted refusal
//! (`:3022-3037`), and `_runAutoCompaction`'s catch, only `if (started)` (`:3218-3236`). An
//! automatic compaction with nothing to prepare returns BEFORE it starts (`:3110-3119`), so it emits
//! nothing at all.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{AgentSessionEvent, CompactionReason, SessionBuilder, SessionConfig};
use cyrup_core::{AssistantMessage, ExtensionId, StopReason, TerminateHint, Usage};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxMessageOptions, FauxProvider, faux_assistant_message, faux_assistant_message_with,
    faux_text,
};
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};
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

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Compaction settings that force even a small session to compact.
fn aggressive() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// What `session_compact_failed` delivered, in pi's own key spelling.
fn payload(ev: &HostEvent) -> Option<Value> {
    match ev {
        HostEvent::SessionCompactFailed {
            reason,
            error_message,
            aborted,
            will_retry,
            from_extension,
        } => Some(json!({
            "reason": reason,
            "errorMessage": error_message,
            "aborted": aborted,
            "willRetry": will_retry,
            "fromExtension": from_extension,
        })),
        _ => None,
    }
}

/// Records `session_compact_failed` and `agent_settled`, and optionally vetoes or overrides
/// `session_before_compact`.
#[derive(Default)]
struct Recorder {
    seen: Arc<Mutex<Vec<Value>>>,
    settled: Arc<Mutex<Vec<bool>>>,
    before_compact: Option<HookOutcome>,
    /// When set, `session_before_compact` also cancels the compaction it is answering — the user
    /// pressing Escape while the handler runs.
    cancel_from: Option<Arc<std::sync::OnceLock<std::sync::Weak<crate::AgentSession>>>>,
}

#[async_trait::async_trait]
impl NativeExtension for Recorder {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("compact-failed-recorder")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[
            EventKind::SessionCompactFailed,
            EventKind::SessionBeforeCompact,
            EventKind::AgentSettled,
        ]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let Some(p) = payload(ev) {
            self.seen.lock().unwrap().push(p);
        }
        match ev {
            HostEvent::AgentSettled { aborted } => {
                self.settled.lock().unwrap().push(*aborted);
                HookOutcome::Noop
            }
            HostEvent::SessionBeforeCompact { .. } => {
                if let Some(session) = self
                    .cancel_from
                    .as_ref()
                    .and_then(|slot| slot.get())
                    .and_then(std::sync::Weak::upgrade)
                {
                    session.abort_compaction();
                }
                self.before_compact.clone().unwrap_or(HookOutcome::Noop)
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// A manual compaction that has nothing to compact: the throw's text, not aborted, not from an
/// extension (pi `:2889-2905`).
#[tokio::test]
async fn a_manual_compaction_that_finds_nothing_to_compact_reports_the_failure() {
    let fx = fixture();
    let rec = Recorder::default();
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(
        Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
        config(&fx),
    )
    .with_native_extension(Arc::new(rec))
    .build()
    .await
    .unwrap();

    let _ = session.compact(None).await.expect_err("nothing to compact");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({
            "reason": "manual",
            "errorMessage": "Compaction failed: Nothing to compact (session too small)",
            "aborted": false,
            "willRetry": false,
            "fromExtension": false,
        })]
    );
}

/// An extension's `{cancel: true}` is an ABORT: `aborted: true` and no error text (pi `:2890-2891`,
/// `cancelledByExtension`).
#[tokio::test]
async fn a_compaction_an_extension_cancels_reports_an_abort_without_error_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let rec = Recorder {
        before_compact: Some(HookOutcome::Block {
            reason: Some("not now".into()),
            terminate: TerminateHint::Unspecified,
        }),
        ..Recorder::default()
    };
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(aggressive())
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap();
    for p in ["one", "two"] {
        let _ = session.prompt(p).await.unwrap();
        session.wait_for_idle().await;
    }

    let _ = session.compact(None).await.expect_err("cancelled");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({
            "reason": "manual",
            "errorMessage": null,
            "aborted": true,
            "willRetry": false,
            "fromExtension": false,
        })]
    );
}

/// The compaction content came from a `session_before_compact` handler, then the compaction was
/// cancelled: `fromExtension: true` (pi sets it the moment a handler supplies `result.compaction`,
/// `:2809-2812`, and its `if (signal.aborted) throw new Error("Compaction cancelled")` runs after,
/// `:2843-2845`), `aborted: true`, no error text.
#[tokio::test]
async fn a_cancelled_extension_supplied_compaction_says_it_came_from_the_extension() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let slot = Arc::new(std::sync::OnceLock::new());
    let rec = Recorder {
        before_compact: Some(HookOutcome::Mutate(
            cyrup_ext::EventPatch::CompactionOverride(json!({
                "summary": "an extension's summary",
                "firstKeptEntryId": "kept",
                "tokensBefore": 10,
            })),
        )),
        cancel_from: Some(Arc::clone(&slot)),
        ..Recorder::default()
    };
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .cli_settings(aggressive())
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));
    for p in ["one", "two"] {
        let _ = session.prompt(p).await.unwrap();
        session.wait_for_idle().await;
    }

    let result = session.compact(None).await;
    assert!(result.is_err(), "cancelled: {result:?}");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({
            "reason": "manual",
            "errorMessage": null,
            "aborted": true,
            "willRetry": false,
            "fromExtension": true,
        })]
    );
}

/// Overflow recovery whose summarization fails: `reason: "overflow"` and pi's
/// `Context overflow recovery failed: …` text; `willRetry` is false on the failure even though the
/// compaction was started to retry (`:3218-3236`).
#[tokio::test]
async fn a_failed_overflow_recovery_reports_its_reason_and_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let rec = Recorder::default();
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, config(&fx))
        .cli_settings(aggressive())
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap()
        .into_shared();
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("first answer worth some tokens")],
        StopReason::Stop,
    )]);
    let _ = session.prompt("one").await.unwrap();
    session.wait_for_idle().await;

    let model = session.model().unwrap();
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
    let stream = session.prompt("two").await.unwrap();
    session.wait_for_idle().await;
    let _: Vec<AgentSessionEvent> = stream.collect().await;

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["reason"], "overflow");
    assert_eq!(seen[0]["aborted"], false);
    assert_eq!(seen[0]["willRetry"], false);
    let text = seen[0]["errorMessage"].as_str().unwrap();
    assert!(
        text.starts_with("Context overflow recovery failed: "),
        "{text}"
    );
}

/// An automatic compaction with nothing to prepare never STARTS: no `compaction_start`, no
/// `compaction_end`, no `session_compact_failed` (pi returns before `started = true`, `:3110-3119`).
/// cyrup used to emit the start, then close it with an empty end.
#[tokio::test]
async fn an_automatic_compaction_with_nothing_to_compact_emits_nothing() {
    let fx = fixture();
    let rec = Recorder::default();
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(
        Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
        config(&fx),
    )
    .cli_settings(aggressive())
    .with_native_extension(Arc::new(rec))
    .build()
    .await
    .unwrap();
    let mut events = session.subscribe();

    // A completed answer whose usage overflows the window: an overflow compaction with no retry,
    // on a session that has nothing to compact.
    let model = session.model().unwrap();
    let mut answer = faux_assistant_message(vec![faux_text("hi")], StopReason::Stop);
    answer.provider = model.provider.clone();
    answer.model = model.model.to_string();
    answer.usage = Usage {
        input: 10_000_000,
        ..Usage::default()
    };
    let ran = session.check_compaction(&answer, true).await.unwrap();
    assert!(!ran);

    // Everything this compaction emitted is already buffered: `check_compaction` emits in-line and
    // returned above. Drain without waiting for more.
    let mut kinds = Vec::new();
    while let Some(Some(ev)) = events.next().now_or_never() {
        kinds.push(ev.kind());
    }
    assert!(
        !kinds.contains(&"compaction_start") && !kinds.contains(&"compaction_end"),
        "{kinds:?}"
    );
    assert!(seen.lock().unwrap().is_empty());
    let _ = CompactionReason::Overflow;
}

/// `agent_settled.aborted`: false for a run that finished, true for one the user aborted — the same
/// value to the extensions and to the session subscribers (pi reads the latch once,
/// `_emitAgentSettled`, `:1078-1086`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_settled_reports_whether_the_run_was_aborted() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            tokens_per_second: Some(5.0),
            ..Default::default()
        },
    ));
    let rec = Recorder::default();
    let settled = Arc::clone(&rec.settled);
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, config(&fx))
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap()
        .into_shared();

    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let finished: Vec<AgentSessionEvent> = session.prompt("one").await.unwrap().collect().await;
    session.wait_for_idle().await;

    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text(
            "a long and slowly streamed answer that the user will not wait for",
        )],
        StopReason::Stop,
    )]);
    let stream = session.prompt("two").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    session.abort();
    let aborted: Vec<AgentSessionEvent> = stream.collect().await;
    session.wait_for_idle().await;

    let settled_of = |events: &[AgentSessionEvent]| {
        events.iter().find_map(|e| match e {
            AgentSessionEvent::AgentSettled { aborted } => Some(*aborted),
            _ => None,
        })
    };
    assert_eq!(settled_of(&finished), Some(false));
    assert_eq!(settled_of(&aborted), Some(true));
    assert_eq!(*settled.lock().unwrap(), vec![false, true]);
    let wire = serde_json::to_value(AgentSessionEvent::AgentSettled { aborted: true }).unwrap();
    assert_eq!(wire, json!({"type": "agent_settled", "aborted": true}));
}

/// An UNBOUND session (no post-run driver: the subscriber settles the run at `agent_end`) reports
/// the same latch: an aborted run settles with `aborted: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unbound_session_reports_an_aborted_run_too() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            tokens_per_second: Some(5.0),
            ..Default::default()
        },
    ));
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text(
            "a long and slowly streamed answer that the user will not wait for",
        )],
        StopReason::Stop,
    )]);
    let rec = Recorder::default();
    let settled = Arc::clone(&rec.settled);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, config(&fx))
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap();
    let stream = session.prompt("go").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    session.abort();
    let events: Vec<AgentSessionEvent> = stream.collect().await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentSessionEvent::AgentSettled { aborted: true })),
        "{:?}",
        events
            .iter()
            .map(AgentSessionEvent::kind)
            .collect::<Vec<_>>()
    );
    assert_eq!(*settled.lock().unwrap(), vec![true]);
}

/// The overflow-recovery-already-attempted refusal is pi's second emit site (`:3022-3037`): no
/// `compaction_start`, then a failing `compaction_end` and `session_compact_failed` with pi's text.
#[tokio::test]
async fn a_second_overflow_after_recovery_reports_the_refusal() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let rec = Recorder::default();
    let seen = Arc::clone(&rec.seen);
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, config(&fx))
        .cli_settings(aggressive())
        .with_native_extension(Arc::new(rec))
        .build()
        .await
        .unwrap()
        .into_shared();
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("first answer worth some tokens")],
        StopReason::Stop,
    )]);
    let _ = session.prompt("one").await.unwrap();
    session.wait_for_idle().await;

    // Overflow, a successful recovery summary, then the retried request overflows again. The
    // retried overflow is stamped AFTER the recovery's compaction, as a real response would be:
    // a scripted reply's `0` predates it and the stale-boundary guard rightly skips it.
    let model = session.model().unwrap();
    let overflow = || {
        let mut m = AssistantMessage::errored(
            model.provider.clone(),
            model.model.as_str(),
            None,
            StopReason::Error,
            "context_length_exceeded",
        );
        m.timestamp = i64::MAX / 2;
        m
    };
    faux.set_responses(vec![
        overflow(),
        faux_assistant_message(vec![faux_text("## Goal\nsummary")], StopReason::Stop),
        overflow(),
    ]);
    let _: Vec<AgentSessionEvent> = session.prompt("two").await.unwrap().collect().await;
    session.wait_for_idle().await;

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.last(),
        Some(&json!({
            "reason": "overflow",
            "errorMessage": "Context overflow recovery failed after one compact-and-retry attempt. \
                             Try reducing context or switching to a larger-context model.",
            "aborted": false,
            "willRetry": false,
            "fromExtension": false,
        })),
        "{seen:?}"
    );
}
