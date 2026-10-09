//! EXT-078 — `turn_end` as pi's boundary and `agent_before_settle` (pi v0.87.0;
//! `core/agent-session.ts:846-1036`, `:1832-1915` and `core/extensions/runner.ts:1029-1080` @v1.1.0).
//!
//! The ledger's Verify: a `turn_end` handler that asks for one more provider request on a tool-less
//! final turn gets exactly one, and a `custom` draft lands in the session file before the next
//! turn's entries. Around it: the event names the entries the turn was persisted as; the preview a
//! later extension is handed includes an earlier one's drafts; a draft list that does not apply is
//! discarded with its continuation and reported; a continuation the context cannot honour is
//! refused and reported; a compaction committed mid-run is the context the next turn is sent; and
//! `agent_before_settle` continues a run once, or not at all when the user aborts during it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use futures::{FutureExt, StreamExt};
use serde_json::{Value, json};

use super::tool_transcript::{Fixture, Reply, Requests, config, fixture, lines, prompt, script};
use crate::{AgentSession, AgentSessionEvent, SessionBuilder, SessionTarget};

/// What one handler call was handed.
#[derive(Clone, Debug)]
struct Handed {
    kind: EventKind,
    message_entry_id: String,
    tool_result_entry_ids: Vec<String>,
    entries: Value,
    continue_: bool,
    context: Value,
    outcome: String,
}

type Log = Arc<Mutex<Vec<Handed>>>;
type Answer =
    fn(&Handed, usize, &Option<Arc<AgentSession>>) -> Option<(Option<Value>, Option<bool>)>;

/// A boundary extension: records every boundary it is handed and answers with `answer(handed,
/// call_number, session)`.
struct Bound {
    id: &'static str,
    kinds: Vec<EventKind>,
    log: Log,
    answer: Answer,
    calls: AtomicUsize,
    session: Arc<OnceLock<Weak<AgentSession>>>,
}

impl Bound {
    fn new(id: &'static str, kinds: &[EventKind], log: &Log, answer: Answer) -> Self {
        Self {
            id,
            kinds: kinds.to_vec(),
            log: Arc::clone(log),
            answer,
            calls: AtomicUsize::new(0),
            session: Arc::default(),
        }
    }
}

#[async_trait::async_trait]
impl NativeExtension for Bound {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&self.kinds);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let handed = match ev {
            HostEvent::TurnEnd {
                message_entry_id,
                tool_result_entry_ids,
                boundary,
                ..
            } => Handed {
                kind: EventKind::TurnEnd,
                message_entry_id: message_entry_id.clone(),
                tool_result_entry_ids: tool_result_entry_ids.clone(),
                entries: boundary.entries.clone(),
                continue_: boundary.continue_,
                context: boundary.context.clone(),
                outcome: boundary.outcome.clone(),
            },
            HostEvent::AgentBeforeSettle { boundary } => Handed {
                kind: EventKind::AgentBeforeSettle,
                message_entry_id: String::new(),
                tool_result_entry_ids: Vec::new(),
                entries: boundary.entries.clone(),
                continue_: boundary.continue_,
                context: boundary.context.clone(),
                outcome: boundary.outcome.clone(),
            },
            _ => return HookOutcome::Noop,
        };
        self.log.lock().unwrap().push(handed.clone());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let session = self.session.get().and_then(Weak::upgrade);
        match (self.answer)(&handed, call, &session) {
            Some((entries, continue_)) => {
                HookOutcome::Mutate(EventPatch::Boundary { entries, continue_ })
            }
            None => HookOutcome::Noop,
        }
    }
}

async fn open(
    requests: &Requests,
    replies: Vec<Reply>,
    extensions: Vec<Bound>,
) -> (Fixture, Arc<AgentSession>) {
    let fx = fixture();
    let slots: Vec<_> = extensions.iter().map(|e| Arc::clone(&e.session)).collect();
    let mut builder = SessionBuilder::new(
        script(requests, replies) as Arc<dyn Provider>,
        config(&fx, SessionTarget::New),
    )
    .with_native_extension(Arc::new(super::tool_transcript::ToolsExt(vec![
        super::tool_transcript::scenario_probe("early"),
    ])));
    for ext in extensions {
        builder = builder.with_native_extension(Arc::new(ext));
    }
    let session = builder.build().await.unwrap().into_shared();
    for slot in slots {
        let _ = slot.set(Arc::downgrade(&session));
    }
    (fx, session)
}

/// Every extension error reported, as `(extension, event, error)`.
fn errors(session: &AgentSession) -> Arc<Mutex<Vec<(String, String, String)>>> {
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
    let sink = Arc::clone(&seen);
    session.ext_host().add_error_listener(Arc::new(move |e| {
        sink.lock().unwrap().push((
            e.extension.to_string(),
            e.event.to_string(),
            e.error.clone(),
        ));
    }));
    seen
}

fn custom_message(text: &str) -> Value {
    json!({"type": "custom_message", "customType": "note", "content": text, "display": true})
}

/// The `type`s of the entries in the session file after the header, in order, with the message
/// role for message entries.
fn kinds_of(file: &std::path::Path) -> Vec<String> {
    lines(file)
        .iter()
        .skip(1)
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            match v["type"].as_str().unwrap() {
                "message" => format!("message:{}", v["message"]["role"].as_str().unwrap()),
                "custom" => format!("custom:{}", v["customType"].as_str().unwrap()),
                "custom_message" => format!("custom_message:{}", v["customType"].as_str().unwrap()),
                other => other.to_string(),
            }
        })
        .collect()
}

/// The Verify: on a tool-less final turn, a `turn_end` handler that appends a context message and
/// asks to continue gets EXACTLY one more provider request, and the drafts land in the session
/// file in order, after the turn they answer and before the next turn's entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_end_continuation_runs_one_more_request_and_its_drafts_precede_the_next_turn() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![
            Reply::Text("first"),
            Reply::Text("second"),
            Reply::Text("never"),
        ],
        vec![Bound::new(
            "continues",
            &[EventKind::TurnEnd],
            &log,
            |_, call, _| {
                (call == 0).then(|| {
                    (
                        Some(json!([
                            {"type": "custom", "customType": "mark", "data": {"n": 1}},
                            custom_message("keep going"),
                        ])),
                        Some(true),
                    )
                })
            },
        )],
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 2, "exactly one more request");
    assert!(
        seen[1].messages.contains("keep going"),
        "{}",
        seen[1].messages
    );
    drop(seen);
    let kinds = kinds_of(&file);
    let first = kinds.iter().position(|k| k == "message:assistant").unwrap();
    assert_eq!(
        &kinds[first + 1..],
        ["custom:mark", "custom_message:note", "message:assistant"],
        "{kinds:?}"
    );
    let handed = log.lock().unwrap().clone();
    assert_eq!(handed.len(), 2, "one turn_end per turn");
    assert_eq!(handed[0].outcome, "completed");
    assert!(!handed[1].continue_, "every chain starts without a request");
}

/// The event names the entries the turn was persisted as (pi `messageEntryId`,
/// `toolResultEntryIds`), and starts from no drafts and pi's preview of the context as it stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_end_names_the_persisted_entries_and_previews_the_context() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Call("early"), Reply::Text("done")],
        vec![Bound::new(
            "reads",
            &[EventKind::TurnEnd],
            &log,
            |_, _, _| None,
        )],
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let ids: Vec<(String, String)> = lines(&file)
        .iter()
        .skip(1)
        .filter_map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            (v["type"] == "message").then(|| {
                (
                    v["message"]["role"].as_str().unwrap().to_string(),
                    v["id"].as_str().unwrap().to_string(),
                )
            })
        })
        .collect();
    let id_of = |role: &str, nth: usize| {
        ids.iter()
            .filter(|(r, _)| r == role)
            .nth(nth)
            .map(|(_, id)| id.clone())
            .unwrap()
    };
    let handed = log.lock().unwrap().clone();
    assert_eq!(handed.len(), 2);
    assert_eq!(handed[0].message_entry_id, id_of("assistant", 0));
    assert_eq!(handed[0].tool_result_entry_ids, [id_of("toolResult", 0)]);
    assert_eq!(handed[1].message_entry_id, id_of("assistant", 1));
    assert!(handed[1].tool_result_entry_ids.is_empty());
    assert_eq!(handed[0].entries, json!([]));
    let context = &handed[0].context;
    assert_eq!(
        context["canContinue"],
        json!(true),
        "a tool result ends the context: {context}"
    );
    assert_eq!(
        handed[1].context["canContinue"],
        json!(false),
        "an assistant message ends it"
    );
    let last = context["llmMessages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "toolResult");
    assert!(
        context["contextEntries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["sourceEntry"]["id"] == json!(id_of("toolResult", 0))),
        "{context}"
    );
}

/// The preview a later extension is handed includes the drafts an earlier one left (pi rebuilds
/// it after every handler), and the earlier one's continuation is what the later one sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_extension_previews_an_earlier_ones_drafts() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Text("only")],
        vec![
            Bound::new("drafts", &[EventKind::TurnEnd], &log, |_, _, _| {
                Some((Some(json!([custom_message("PREVIEWED")])), None))
            }),
            Bound::new("reads", &[EventKind::TurnEnd], &log, |_, _, _| None),
        ],
    )
    .await;
    prompt(&session, "hi").await;

    let handed = log.lock().unwrap().clone();
    assert_eq!(handed[1].entries, json!([custom_message("PREVIEWED")]));
    assert!(
        handed[1].context["contextMessages"]
            .to_string()
            .contains("PREVIEWED"),
        "{}",
        handed[1].context
    );
    assert_eq!(
        handed[1].context["canContinue"],
        json!(true),
        "a custom message after the assistant can be continued from"
    );
    assert!(!handed[0].context.to_string().contains("PREVIEWED"));
}

/// A draft list that does not apply is reported with pi's text and discarded, continuation and all
/// (`emitBoundary` returns `{entries: [], continue: false}` when the last preview failed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drafts_that_do_not_apply_are_reported_and_discarded_with_their_continuation() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Text("only"), Reply::Text("never")],
        vec![Bound::new(
            "bad",
            &[EventKind::TurnEnd],
            &log,
            |_, call, _| {
                (call == 0).then(|| {
                    (
                        Some(json!([
                            custom_message("would continue"),
                            {"type": "context_edit", "targetId": "nope", "replacement": null},
                        ])),
                        Some(true),
                    )
                })
            },
        )],
    )
    .await;
    let errors = errors(&session);
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    assert_eq!(requests.lock().unwrap().len(), 1, "no continuation");
    assert!(
        !kinds_of(&file)
            .iter()
            .any(|k| k.starts_with("custom_message")),
        "nothing committed"
    );
    assert_eq!(
        *errors.lock().unwrap(),
        [(
            "bad".to_string(),
            "turn_end".to_string(),
            "Invalid boundary entries: Entry nope not found".to_string()
        )]
    );
}

/// A continuation the context cannot be continued from — the turn ended on an assistant message
/// and nothing was appended or queued — is refused and reported (pi
/// `_reportInvalidBoundaryContinuation`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_continuation_without_runnable_context_is_refused_and_reported() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Text("only"), Reply::Text("never")],
        vec![Bound::new(
            "wants",
            &[EventKind::TurnEnd],
            &log,
            |_, _, _| Some((None, Some(true))),
        )],
    )
    .await;
    let errors = errors(&session);
    prompt(&session, "hi").await;

    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        *errors.lock().unwrap(),
        [(
            "<boundary>".to_string(),
            "turn_end".to_string(),
            "turn_end requested continuation without runnable model context".to_string()
        )]
    );
}

/// A self-retaining compaction committed at a turn boundary mid-run is the context the next turn
/// is sent: the earlier turns are gone from the request, the summary is in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_committed_mid_run_is_the_next_turns_context() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Call("early"), Reply::Text("done")],
        vec![Bound::new(
            "compacts",
            &[EventKind::TurnEnd],
            &log,
            |_, call, _| {
                (call == 0).then(|| {
                    (
                        Some(json!([{
                            "type": "compaction",
                            "summary": "BOUNDARY SUMMARY",
                            "firstKeptEntryId": null,
                        }])),
                        None,
                    )
                })
            },
        )],
    )
    .await;
    let mut events = session.subscribe();
    let file = session.session_file().await.unwrap();
    prompt(&session, "the original prompt").await;

    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].messages.contains("the original prompt"));
    assert!(
        !seen[1].messages.contains("the original prompt"),
        "{}",
        seen[1].messages
    );
    assert!(
        seen[1].messages.contains("BOUNDARY SUMMARY"),
        "{}",
        seen[1].messages
    );
    drop(seen);
    assert!(kinds_of(&file).contains(&"compaction".to_string()));
    let mut appended = Vec::new();
    while let Some(Some(ev)) = events.next().now_or_never() {
        if let AgentSessionEvent::EntryAppended { entry } = ev {
            appended.push(entry["type"].as_str().unwrap_or_default().to_string());
        }
    }
    assert_eq!(
        appended,
        ["compaction"],
        "each committed entry is announced"
    );
}

/// `agent_before_settle` runs once nothing else continues the run: a handler that appends a
/// context message and asks to continue gets one more provider request, then the run settles
/// (its second call does not ask again).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_before_settle_continues_a_run_once() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![
            Reply::Text("first"),
            Reply::Text("second"),
            Reply::Text("never"),
        ],
        vec![Bound::new(
            "settles",
            &[EventKind::AgentBeforeSettle],
            &log,
            |_, call, _| {
                (call == 0).then(|| (Some(json!([custom_message("one more")])), Some(true)))
            },
        )],
    )
    .await;
    prompt(&session, "hi").await;

    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].messages.contains("one more"));
    drop(seen);
    let handed = log.lock().unwrap().clone();
    assert_eq!(handed.len(), 2, "before each settlement attempt");
    assert!(
        handed
            .iter()
            .all(|h| h.kind == EventKind::AgentBeforeSettle)
    );
    assert_eq!(handed[0].outcome, "completed");
    assert_eq!(handed[0].context["canContinue"], json!(false));
}

/// An abort while the `agent_before_settle` chain runs stops the continuation it asked for (pi
/// `_abortDuringBeforeSettle`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abort_during_agent_before_settle_stops_its_continuation() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Text("first"), Reply::Text("never")],
        vec![Bound::new(
            "aborts",
            &[EventKind::AgentBeforeSettle],
            &log,
            |_, _, session| {
                session.as_ref().expect("bound").abort();
                Some((Some(json!([custom_message("one more")])), Some(true)))
            },
        )],
    )
    .await;
    prompt(&session, "hi").await;

    assert_eq!(requests.lock().unwrap().len(), 1, "no continuation");
    assert_eq!(log.lock().unwrap().len(), 1);
}

/// The boundary's `outcome` is the run's (pi `AgentActivityOutcome`): `"error"` for a turn that
/// ended in an error, at `turn_end` and at the `agent_before_settle` that follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_turn_reports_the_error_outcome_at_both_boundaries() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Fail("the model fell over")],
        vec![Bound::new(
            "reads",
            &[EventKind::TurnEnd, EventKind::AgentBeforeSettle],
            &log,
            |_, _, _| None,
        )],
    )
    .await;
    prompt(&session, "hi").await;

    let handed = log.lock().unwrap().clone();
    let outcomes: Vec<(EventKind, String)> =
        handed.iter().map(|h| (h.kind, h.outcome.clone())).collect();
    assert_eq!(
        outcomes,
        [
            (EventKind::TurnEnd, "error".to_string()),
            (EventKind::AgentBeforeSettle, "error".to_string()),
        ]
    );
}

/// The abort latch pi takes during `agent_before_settle` ends the boundary BEFORE its continuation
/// is judged (`if (this._abortDuringBeforeSettle) return false;`, `agent-session.ts:1905`
/// @v1.1.0): a continuation the context could not honour is then not reported either — the user
/// stopped the run, nothing was wrong with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abort_during_agent_before_settle_is_not_reported_as_an_invalid_continuation() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_fx, session) = open(
        &requests,
        vec![Reply::Text("first"), Reply::Text("never")],
        vec![Bound::new(
            "aborts",
            &[EventKind::AgentBeforeSettle],
            &log,
            |_, _, session| {
                session.as_ref().expect("bound").abort();
                Some((None, Some(true)))
            },
        )],
    )
    .await;
    let errors = errors(&session);
    prompt(&session, "hi").await;

    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
}
