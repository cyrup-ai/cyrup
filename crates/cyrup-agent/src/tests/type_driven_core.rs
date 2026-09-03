//! The core-loop type-driven refactor (CLTR_1..8): the guarantees the new types carry, exercised
//! through the public `Agent` API and the crate-internal parsed values.
//!
//! - `AppRole` is a closed enum with one fallible door (`parse`) and a byte-stable wire tag.
//! - `ResumePoint::check` is the ONE home of the "may this transcript resume?" rule, and
//!   `RunEntry` derives `skip_initial_steering_poll` from its own shape.
//! - `QueueMode: FromStr` is strict; leniency is the settings boundary's, not the parser's.
//! - A modelless agent is a STATE: it builds, accepts edits, and refuses to run with
//!   `AgentError::NoModelSelected` — without ever holding the run latch.
//! - `edit_transcript` / `pop_trailing_assistant_if` are atomic and refused mid-run.
//! - `snapshot().is_streaming` is the run latch, not a second flag.

use std::sync::Arc;
use std::time::Duration;

use crate::agent::{PromptSource, ResumePoint, RunEntry};
use crate::{
    Agent, AgentBuilder, AgentError, AgentMessage, AppRole, BusyEntry, ContinueSurface, QueueMode,
    StreamFn,
};
use cyrup_core::{EventStream, ModelRef, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
use futures::StreamExt;
use serde_json::{json, Map, Value};

use super::support::{faux_stream_fn, model_ref};

// ---------------------------------------------------------------------------
// AppRole
// ---------------------------------------------------------------------------

/// `parse` is the inverse of `as_str` on every variant, and rejects everything else — including
/// the four typed roles, which are NOT app roles.
#[test]
fn app_role_parse_round_trips_and_rejects_the_rest() {
    for role in AppRole::ALL {
        assert_eq!(AppRole::parse(role.as_str()), Some(role), "{role:?}");
    }
    for tag in ["user", "assistant", "toolResult", "custom", "", "BashExecution", "bash_execution"] {
        assert_eq!(AppRole::parse(tag), None, "{tag:?} must not parse");
    }
    assert_eq!(AppRole::BashExecution.as_str(), "bashExecution");
    assert_eq!(AppRole::BranchSummary.as_str(), "branchSummary");
    assert_eq!(AppRole::CompactionSummary.as_str(), "compactionSummary");
}

/// The wire shape of an `App` message is byte-stable: the payload IS the wire object (every
/// constructor stamps `role` with the enum's tag into it, as the bash path and the resume bridge
/// do), the tag appears exactly once, and the message deserializes back to the same role — the
/// serde gate goes through `AppRole::parse`, the one fallible door.
#[test]
fn app_message_wire_role_is_the_enum_tag_and_round_trips() {
    let mut payload = Map::new();
    payload.insert("role".to_string(), Value::from(AppRole::BashExecution.as_str()));
    payload.insert("command".to_string(), json!("ls"));
    let msg = AgentMessage::App { role: AppRole::BashExecution, payload };
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(value["role"], json!("bashExecution"));
    assert_eq!(value["command"], json!("ls"));
    let text = serde_json::to_string(&msg).unwrap();
    assert_eq!(text.matches("\"role\"").count(), 1, "the role key appears exactly once: {text}");

    let back: AgentMessage = serde_json::from_value(value).unwrap();
    match back {
        AgentMessage::App { role, payload } => {
            assert_eq!(role, AppRole::BashExecution);
            assert_eq!(payload.get("command"), Some(&json!("ls")));
        }
        other => panic!("expected App, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ResumePoint / RunEntry
// ---------------------------------------------------------------------------

fn assistant(text: &str) -> AgentMessage {
    AgentMessage::Assistant(Arc::new(faux_assistant_message(vec![faux_text(text)], StopReason::Stop)))
}

/// The one resume rule: an empty transcript is `NoMessages` (carrying the caller's surface, so
/// pi's per-surface text survives), a trailing assistant is `ContinueFromAssistant`, anything
/// else resumes.
#[test]
fn resume_point_check_is_the_one_resume_rule() {
    let empty: Vec<AgentMessage> = Vec::new();
    assert!(matches!(
        ResumePoint::check(&empty, ContinueSurface::Loop),
        Err(AgentError::NoMessages(ContinueSurface::Loop))
    ));
    assert!(matches!(
        ResumePoint::check(&empty, ContinueSurface::Agent),
        Err(AgentError::NoMessages(ContinueSurface::Agent))
    ));
    let trailing_assistant = vec![AgentMessage::user_text("hi"), assistant("reply")];
    assert!(matches!(
        ResumePoint::check(&trailing_assistant, ContinueSurface::Agent),
        Err(AgentError::ContinueFromAssistant)
    ));
    let trailing_user = vec![assistant("reply"), AgentMessage::user_text("more")];
    assert!(ResumePoint::check(&trailing_user, ContinueSurface::Agent).is_ok());
}

/// `skip_initial_steering_poll` is a property of the entry, not a separate flag: only a steering
/// drain skips the first poll (pi `skipInitialSteeringPoll`).
#[test]
fn run_entry_derives_the_steering_poll_skip() {
    let steering =
        RunEntry::Prompt { messages: vec![AgentMessage::user_text("s")], source: PromptSource::SteeringDrain };
    let fresh = RunEntry::Prompt { messages: vec![AgentMessage::user_text("f")], source: PromptSource::Fresh };
    let follow =
        RunEntry::Prompt { messages: vec![AgentMessage::user_text("u")], source: PromptSource::FollowUpDrain };
    let proof = ResumePoint::check(&[AgentMessage::user_text("x")], ContinueSurface::Agent).unwrap();
    let cont = RunEntry::Continue(proof);
    assert!(steering.skip_initial_steering_poll());
    assert!(!fresh.skip_initial_steering_poll());
    assert!(!follow.skip_initial_steering_poll());
    assert!(!cont.skip_initial_steering_poll());
}

// ---------------------------------------------------------------------------
// QueueMode: FromStr
// ---------------------------------------------------------------------------

/// The parser is strict: the two pi strings and nothing else. (The settings boundary in the
/// session service adds pi's lenient fallback ON TOP of this, with a warning.)
#[test]
fn queue_mode_from_str_is_strict() {
    assert_eq!("all".parse::<QueueMode>(), Ok(QueueMode::All));
    assert_eq!("one-at-a-time".parse::<QueueMode>(), Ok(QueueMode::OneAtATime));
    for bad in ["ALL", "one_at_a_time", "", "steer"] {
        let err = bad.parse::<QueueMode>().expect_err(bad);
        assert!(err.contains(bad), "the error names the rejected value: {err}");
    }
}

// ---------------------------------------------------------------------------
// Modelless agent
// ---------------------------------------------------------------------------

/// A modelless agent is a state, not a sentinel: it builds with `model: None`, `prompt` and
/// `continue_run` refuse with `NoModelSelected`, and the refusal never leaves the run latch held
/// — `is_running()` is false and `wait_for_idle` resolves at once. Selecting a model afterwards
/// makes the same agent run.
#[tokio::test]
async fn modelless_agent_refuses_to_run_without_holding_the_latch() {
    let (_faux, sf) = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let agent = AgentBuilder::new(sf).build();
    assert!(agent.snapshot().await.model.is_none());

    let err = agent.prompt("hi").await.err().expect("modelless prompt must be refused");
    assert!(matches!(err, AgentError::NoModelSelected), "{err}");
    assert_eq!(err.to_string(), "No model selected. Select a model before starting a run.");
    assert!(!agent.is_running(), "a refused run must not hold the latch");
    tokio::time::timeout(Duration::from_secs(1), agent.wait_for_idle())
        .await
        .expect("wait_for_idle must resolve immediately after a refusal");
    assert!(!agent.snapshot().await.is_streaming);

    // `continue_run` goes through the same claim: a resumable transcript still needs a model.
    agent.set_messages(vec![AgentMessage::user_text("resume me")]).await;
    assert!(matches!(agent.continue_run().await, Err(AgentError::NoModelSelected)));
    assert!(!agent.is_running());

    agent.set_model(Some(model_ref())).await;
    assert_eq!(agent.snapshot().await.model, Some(model_ref()));
    let handle = agent.prompt("now").await.expect("a model makes the same agent run");
    handle.finished().await;
    agent.wait_for_idle().await;
    assert!(!agent.is_running());
}

/// `Agent::builder(model, sf)` is `AgentBuilder::new(sf).model(model)`: the two doors build the
/// same agent.
#[tokio::test]
async fn builder_with_model_and_explicit_model_setter_agree() {
    let (_f1, sf1) = faux_stream_fn(Vec::new());
    let (_f2, sf2) = faux_stream_fn(Vec::new());
    let a = Agent::builder(model_ref(), sf1).build();
    let b = AgentBuilder::new(sf2).model(model_ref()).build();
    assert_eq!(a.snapshot().await.model, b.snapshot().await.model);
    assert_eq!(a.snapshot().await.model, Some(model_ref()));
}

// ---------------------------------------------------------------------------
// edit_transcript / pop_trailing_assistant_if
// ---------------------------------------------------------------------------

/// A stream fn that never delivers its terminal until the run is aborted — the agent stays
/// genuinely mid-run for as long as the test needs.
struct BlockingStreamFn;

impl StreamFn for BlockingStreamFn {
    fn stream(&self, _model: &ModelRef, _ctx: &Context, _opts: &StreamOptions) -> EventStream<StreamEvent> {
        let tail = futures::stream::once(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            StreamEvent::terminal(faux_assistant_message(vec![faux_text("late")], StopReason::Stop))
        });
        let start = StreamEvent::Start {
            partial: Arc::new(faux_assistant_message(Vec::new(), StopReason::Pending)),
        };
        Box::pin(futures::stream::iter(vec![start]).chain(tail))
    }
}

/// Idle: the edit runs under the state lock and its return value comes back; the pop helper
/// pops the trailing assistant only when the predicate holds, and never pops anything else.
#[tokio::test]
async fn transcript_edits_are_atomic_and_predicate_driven_when_idle() {
    let (_faux, sf) = faux_stream_fn(Vec::new());
    let agent = Agent::builder(model_ref(), sf).build();

    let len = agent.edit_transcript(|m| {
        m.push(AgentMessage::user_text("one"));
        m.len()
    });
    assert_eq!(len.ok(), Some(1));
    assert_eq!(agent.snapshot().await.messages.len(), 1);

    agent.set_messages(vec![AgentMessage::user_text("q"), assistant("a")]).await;
    // Predicate false: nothing is popped.
    let none = agent.pop_trailing_assistant_if(|_| false).unwrap();
    assert!(none.is_none());
    assert_eq!(agent.snapshot().await.messages.len(), 2);
    // Predicate true: the assistant comes back and is gone from the transcript.
    let popped = agent.pop_trailing_assistant_if(|a| a.stop_reason == StopReason::Stop).unwrap();
    assert!(popped.is_some());
    assert_eq!(agent.snapshot().await.messages.len(), 1);
    // A trailing non-assistant is never popped, whatever the predicate says.
    assert!(agent.pop_trailing_assistant_if(|_| true).unwrap().is_none());
    assert_eq!(agent.snapshot().await.messages.len(), 1);
}

/// Mid-run the edit is refused on the agent's own latch with the `Edit` busy text, and the
/// refusal is a clean `Err` — no state write, no latch change. After the run settles it succeeds.
#[tokio::test]
async fn transcript_edits_are_refused_while_a_run_is_in_flight() {
    let sf: Arc<dyn StreamFn> = Arc::new(BlockingStreamFn);
    let agent = Agent::builder(model_ref(), sf).build();
    agent.prompt("hi").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(agent.is_running());

    let err = agent.edit_transcript(|m| m.push(AgentMessage::user_text("sneak"))).err().unwrap();
    assert!(matches!(err, AgentError::RunActive(BusyEntry::Edit)));
    assert_eq!(
        err.to_string(),
        "Agent is already processing. Wait for completion before editing the transcript."
    );
    assert!(matches!(
        agent.pop_trailing_assistant_if(|_| true),
        Err(AgentError::RunActive(BusyEntry::Edit))
    ));

    agent.abort();
    agent.wait_for_idle().await;
    assert!(!agent.is_running());
    assert!(agent.edit_transcript(|m| m.push(AgentMessage::user_text("now"))).is_ok());
}

// ---------------------------------------------------------------------------
// is_streaming is the latch
// ---------------------------------------------------------------------------

/// `snapshot().is_streaming` and `is_running()` are one fact: false before a run, true while the
/// provider is parked mid-stream, false again once the run has settled.
#[tokio::test]
async fn snapshot_is_streaming_reads_the_run_latch() {
    let sf: Arc<dyn StreamFn> = Arc::new(BlockingStreamFn);
    let agent = Agent::builder(model_ref(), sf).build();
    assert!(!agent.snapshot().await.is_streaming);
    assert!(!agent.is_running());

    agent.prompt("hi").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(agent.is_running());
    assert!(agent.snapshot().await.is_streaming, "the snapshot reads the latch");

    agent.abort();
    agent.wait_for_idle().await;
    assert!(!agent.is_running());
    assert!(!agent.snapshot().await.is_streaming);
    // Reset keeps them in agreement too: it refuses while running and clears nothing but state.
    agent.reset().await.unwrap();
    assert!(!agent.snapshot().await.is_streaming);
    let _: Value = json!(null);
}
