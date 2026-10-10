//! PROMPT-001 — the system prompt the transcript replays to reaches every provider request.
//!
//! Since `CODE-014` a session builds its agent with NO prompt of its own: the prompt is the
//! transcript's system rows (`sections` diffs), the way pi's loop sends `normalizeContext({
//! messages })`. cyrup's adapters read `Context::system_prompt` and ignore system messages, so the
//! loop has to resolve the replay into that field. These tests capture the `Context` the stream
//! function is handed.

use std::sync::{Arc, Mutex};

use crate::{Agent, AgentMessage, HookError, Hooks};
use cyrup_core::{CancelToken, Content, Message, Sections, StopReason, SystemMessage};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};
use cyrup_provider::{Context, get_current_system_prompt};
use serde_json::json;

use super::support::{EchoTool, model_ref, recording_stream_fn};

fn sections(entries: &[(&str, Option<&str>)]) -> Sections {
    entries
        .iter()
        .map(|(k, v)| (*k, v.map(str::to_string)))
        .collect()
}

fn row(entries: &[(&str, Option<&str>)], timestamp: i64) -> AgentMessage {
    AgentMessage::System(SystemMessage {
        sections: Some(sections(entries)),
        timestamp,
        ..SystemMessage::default()
    })
}

/// The base prompt a session writes: tagged sections, empty `content`.
fn base_row() -> AgentMessage {
    row(
        &[
            ("preamble", Some("You are the test agent.")),
            ("tools", Some("<tools>\n- echo: echoes</tools>")),
            ("rules", Some("<rules>\n- be brief</rules>")),
        ],
        1,
    )
}

fn replay(messages: &[AgentMessage]) -> String {
    let rows: Vec<Message> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::System(s) => Some(Message::System(s.clone())),
            _ => None,
        })
        .collect();
    get_current_system_prompt(&rows)
}

fn captured(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn crate::StreamFn>, Arc<Mutex<Vec<Context>>>) {
    recording_stream_fn(responses, |_, ctx, _| ctx.clone())
}

fn done() -> cyrup_core::AssistantMessage {
    faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
}

async fn run(agent: &Agent, text: &str) {
    agent.prompt(text).await.unwrap().finished().await;
    agent.wait_for_idle().await;
}

fn no_system_rows(ctx: &Context) -> bool {
    ctx.messages
        .iter()
        .all(|m| !matches!(m, Message::System(_)))
}

/// Both requests of a run — the first, and the one after a tool call — carry the replayed prompt,
/// with its `<tools>` and `<rules>` sections, and no system row is left in the message list for an
/// adapter to render a second time.
#[tokio::test]
async fn every_request_of_a_run_carries_the_replayed_prompt() {
    let (sf, seen) = captured(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        done(),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::named("echo")])
        .messages(vec![base_row()])
        .build();
    run(&agent, "go").await;

    let transcript = agent.snapshot().await.messages;
    let expected = replay(&transcript);
    assert!(
        expected.contains("<tools>") && expected.contains("<rules>"),
        "{expected}"
    );

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "a tool call makes a second request");
    for (n, ctx) in requests.iter().enumerate() {
        assert_eq!(
            ctx.system_prompt.as_deref(),
            Some(expected.as_str()),
            "request {n}"
        );
        assert!(no_system_rows(ctx), "request {n}: {:?}", ctx.messages);
    }
}

/// A later diff row changes the NEXT request's prompt: the patched section is current, the removed
/// one is gone, and the text is still sent once.
#[tokio::test]
async fn a_later_diff_row_updates_the_next_request() {
    let (sf, seen) = captured(vec![done(), done()]);
    let agent = Agent::builder(model_ref(), sf)
        .messages(vec![base_row()])
        .build();
    run(&agent, "one").await;

    let mut transcript = agent.snapshot().await.messages;
    transcript.push(row(
        &[
            ("rules", Some("<rules>\n- be verbose</rules>")),
            ("tools", None),
        ],
        9,
    ));
    agent.set_messages(transcript).await;
    run(&agent, "two").await;

    let transcript = agent.snapshot().await.messages;
    let requests = seen.lock().unwrap().clone();
    let (first, second) = (
        requests[0].system_prompt.clone().unwrap(),
        requests[1].system_prompt.clone().unwrap(),
    );
    assert!(
        first.contains("- be brief") && first.contains("<tools>"),
        "{first}"
    );
    assert_eq!(second, replay(&transcript));
    assert!(second.contains("- be verbose"), "{second}");
    assert!(
        !second.contains("- be brief"),
        "the patched text is gone: {second}"
    );
    assert!(
        !second.contains("<tools>"),
        "a null removes the section: {second}"
    );
    assert_eq!(
        second.matches("You are the test agent.").count(),
        1,
        "{second}"
    );
}

/// A prompt handed to the `Agent` directly leads the transcript's, as `normalizeContext` folds it,
/// and is not sent twice.
#[tokio::test]
async fn an_agent_prompt_leads_the_transcripts_and_is_sent_once() {
    let (sf, seen) = captured(vec![done()]);
    let agent = Agent::builder(model_ref(), sf)
        .system_prompt("SHORTHAND")
        .messages(vec![base_row()])
        .build();
    run(&agent, "go").await;

    let requests = seen.lock().unwrap().clone();
    let prompt = requests[0].system_prompt.clone().unwrap();
    assert!(
        prompt.starts_with("SHORTHAND\n\nYou are the test agent."),
        "{prompt}"
    );
    assert_eq!(prompt.matches("SHORTHAND").count(), 1, "{prompt}");
}

/// With no prompt anywhere the request has none: `None`, not an empty system message.
#[tokio::test]
async fn no_prompt_anywhere_sends_none() {
    let (sf, seen) = captured(vec![done()]);
    let agent = Agent::builder(model_ref(), sf).build();
    run(&agent, "go").await;
    assert_eq!(seen.lock().unwrap()[0].system_prompt, None);
}

/// pi's `forceSystemPrompt` shape (`_installAgentForcedPromptProjection`): a `transform_context`
/// that collapses the transcript's system rows into ONE row holding the replacement. The request
/// carries exactly that text; the transcript keeps its own rows (it is projected, not persisted).
struct ForcePrompt(&'static str);

#[async_trait::async_trait]
impl Hooks for ForcePrompt {
    async fn transform_context(
        &self,
        msgs: Vec<Arc<AgentMessage>>,
        _cancel: CancelToken,
    ) -> Result<Vec<Arc<AgentMessage>>, HookError> {
        let head = AgentMessage::System(SystemMessage {
            content: vec![Content::text(self.0)],
            timestamp: 1,
            ..SystemMessage::default()
        });
        Ok(std::iter::once(Arc::new(head))
            .chain(
                msgs.into_iter()
                    .filter(|m| !matches!(m.as_ref(), AgentMessage::System(_))),
            )
            .collect())
    }
}

#[tokio::test]
async fn a_forced_prompt_is_the_whole_request_prompt_and_is_not_persisted() {
    let (sf, seen) = captured(vec![done()]);
    let agent = Agent::builder(model_ref(), sf)
        .messages(vec![base_row()])
        .hooks(Arc::new(ForcePrompt("FORCED REPLACEMENT")))
        .build();
    run(&agent, "go").await;

    let requests = seen.lock().unwrap().clone();
    assert_eq!(
        requests[0].system_prompt.as_deref(),
        Some("FORCED REPLACEMENT")
    );
    assert!(no_system_rows(&requests[0]));
    let transcript = agent.snapshot().await.messages;
    assert!(
        replay(&transcript).contains("<tools>"),
        "the transcript keeps its own sections: {transcript:?}"
    );
}
