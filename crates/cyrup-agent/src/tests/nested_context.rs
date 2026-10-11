//! What a nested tool call is shown of the agent. pi hands every nested call the live
//! `agent.state.messages` array by reference; the copy a call is given here is shared between the
//! calls made against the same transcript, so a script that makes thousands of calls does not
//! copy the transcript and the system prompt thousands of times.
#![allow(clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use cyrup_core::Content;

use super::support::model_ref;
use crate::{
    Agent, AgentMessage, Context, EventStream, ModelRef, StreamEvent, StreamFn, StreamOptions,
};

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: vec![Content::text(text)],
        timestamp: None,
    }
}

struct NoStream;

impl StreamFn for NoStream {
    fn stream(
        &self,
        _model: &ModelRef,
        _ctx: &Context,
        _opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        panic!("these tests never run a turn")
    }
}

async fn agent() -> Agent {
    let stream: Arc<dyn StreamFn> = Arc::new(NoStream);
    let agent = Agent::builder(model_ref(), stream).build();
    agent.set_system_prompt("be brief".to_owned()).await;
    agent.set_messages(vec![user("one"), user("two")]).await;
    agent
}

#[tokio::test]
async fn calls_made_against_an_unchanged_transcript_share_one_copy() {
    let agent = agent().await;
    let first = agent.nested_context().await;
    let second = agent.nested_context().await;

    // The same allocations, not equal copies: nothing was copied for the second call.
    assert!(Arc::ptr_eq(&first.messages, &second.messages));
    assert!(Arc::ptr_eq(&first.system_prompt, &second.system_prompt));
    for (a, b) in first.messages.iter().zip(second.messages.iter()) {
        assert!(Arc::ptr_eq(a, b));
    }
}

#[tokio::test]
async fn the_shared_copy_is_what_a_snapshot_would_have_shown() {
    let agent = agent().await;
    let context = agent.nested_context().await;
    let snapshot = agent.snapshot().await;

    assert_eq!(&*context.system_prompt, snapshot.system_prompt.as_str());
    assert_eq!(
        context
            .messages
            .iter()
            .map(|message| format!("{message:?}"))
            .collect::<Vec<_>>(),
        snapshot
            .messages
            .iter()
            .map(|message| format!("{message:?}"))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_change_to_the_transcript_is_seen_by_the_next_call_and_never_by_a_held_one() {
    let agent = agent().await;
    let held = agent.nested_context().await;

    agent.append_finalized_message(user("three"));
    let after_append = agent.nested_context().await;
    assert_eq!(
        held.messages.len(),
        2,
        "a context already handed out is a snapshot"
    );
    assert_eq!(after_append.messages.len(), 3);
    assert!(!Arc::ptr_eq(&held.messages, &after_append.messages));
    // The new copy is shared in turn.
    assert!(Arc::ptr_eq(
        &after_append.messages,
        &agent.nested_context().await.messages
    ));

    // Every other way to change the transcript drops the copy too.
    agent
        .edit_transcript(|messages| messages.truncate(1))
        .unwrap();
    let after_edit = agent.nested_context().await;
    assert_eq!(after_edit.messages.len(), 1);

    agent
        .set_messages(vec![user("a"), user("b"), user("c"), user("d")])
        .await;
    let after_set = agent.nested_context().await;
    assert_eq!(after_set.messages.len(), 4);

    agent.reset().await.unwrap();
    assert!(agent.nested_context().await.messages.is_empty());
}

#[tokio::test]
async fn a_new_system_prompt_is_seen_and_the_transcript_stays_shared() {
    let agent = agent().await;
    let before = agent.nested_context().await;
    agent.set_system_prompt("be verbose".to_owned()).await;
    let after = agent.nested_context().await;

    assert_eq!(&*before.system_prompt, "be brief");
    assert_eq!(&*after.system_prompt, "be verbose");
    assert!(
        Arc::ptr_eq(&before.messages, &after.messages),
        "the prompt changed, the transcript did not"
    );
}
