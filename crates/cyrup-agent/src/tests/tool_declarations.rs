//! CODE-005, transcript half — the loop records the executable set in the transcript.
//!
//! pi's `declareToolChanges` (`packages/agent/src/agent-loop.ts:327-376` @v1.0.1): before each
//! request the difference between the tools the transcript has declared (replayed from its system
//! messages) and the tools the run can execute becomes `toolsAdded`/`toolsRemoved` on a system
//! message. The message is an ordinary transcript message, so the session persists it, and that is
//! how a loadout survives a resume. A `prepareLoadout` hook's hidden declarations are recorded too
//! and projected out of every request (`_installHiddenDeclarationsProjection`,
//! `agent-session.ts:1721-1738`).

use std::sync::{Arc, Mutex};

use crate::{Agent, AgentMessage, StreamFn};
use cyrup_core::{
    CancelToken, Content, EventStream, LoadoutView, Message, ModelRef, StopReason, SystemMessage,
    Tool, ToolCallId, ToolDef, ToolError, ToolExposure, ToolLoadout, ToolLoadoutChanges,
    ToolReference, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_provider::{Context, StreamEvent, StreamOptions, get_current_tools, normalize_context};
use serde_json::{Value, json};

use super::support::model_ref;

/// A direct tool, optionally hiding other declarations from requests.
struct Probe {
    name: String,
    params: Value,
    hides: Vec<String>,
}

impl Probe {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            params: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            hides: Vec::new(),
        }
    }

    fn hiding(mut self, names: &[&str]) -> Self {
        self.hides = names.iter().map(|n| n.to_string()).collect();
        self
    }

    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "probe"
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }
    fn prepare_loadout(&self, _view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        Ok(ToolLoadoutChanges {
            hidden_declarations: self.hides.clone(),
            ..ToolLoadoutChanges::default()
        })
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("ok")],
            ..Default::default()
        })
    }
}

type Requests = Arc<Mutex<Vec<Context>>>;

/// Records every request context, then delegates.
struct ContextSpy {
    inner: Arc<dyn StreamFn>,
    seen: Requests,
}

impl StreamFn for ContextSpy {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.seen.lock().unwrap().push(ctx.clone());
        self.inner.stream(model, ctx, opts)
    }
}

fn spy(replies: usize) -> (Arc<dyn StreamFn>, Requests) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(
        (0..replies)
            .map(|_| faux_assistant_message(vec![faux_text("done")], StopReason::Stop))
            .collect(),
    );
    let seen: Requests = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(ContextSpy {
        inner: Arc::new(crate::ProviderStreamFn::new(faux)),
        seen: Arc::clone(&seen),
    });
    (sf, seen)
}

fn loadout(tools: Vec<Arc<dyn Tool>>) -> ToolLoadout {
    ToolLoadout::from_tools(tools)
}

fn system_rows(messages: &[AgentMessage]) -> Vec<&SystemMessage> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::System(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn added(row: &SystemMessage) -> Vec<&str> {
    row.tools_added.iter().map(|t| t.name.as_str()).collect()
}

fn removed(row: &SystemMessage) -> Vec<&str> {
    row.tools_removed.iter().map(|t| t.name.as_str()).collect()
}

async fn run(agent: &Agent, text: &str) {
    agent.prompt(text).await.unwrap().finished().await;
    agent.wait_for_idle().await;
}

/// The first request of a run records the whole loadout, as a system message that precedes the
/// prompt it rides in with (`agent-loop.ts:359-362`).
#[tokio::test]
async fn the_first_request_records_the_loadout_ahead_of_the_prompt() {
    let (sf, _seen) = spy(1);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![Probe::new("a").arc(), Probe::new("b").arc()]))
        .build();
    run(&agent, "go").await;

    let messages = agent.snapshot().await.messages;
    assert!(
        matches!(messages.first(), Some(AgentMessage::System(_))),
        "the declaration leads the transcript: {messages:?}"
    );
    let rows = system_rows(&messages);
    assert_eq!(rows.len(), 1, "one declaration row: {rows:?}");
    assert_eq!(added(rows[0]), ["a", "b"]);
    assert!(removed(rows[0]).is_empty());
    assert!(
        matches!(messages.get(1), Some(AgentMessage::User { .. })),
        "the prompt follows the declaration: {messages:?}"
    );
}

/// CONTROL for the delta test: a run with the loadout the transcript already declares adds no row.
#[tokio::test]
async fn an_unchanged_loadout_records_nothing_on_the_next_run() {
    let (sf, _seen) = spy(2);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![Probe::new("a").arc()]))
        .build();
    run(&agent, "one").await;
    run(&agent, "two").await;

    let messages = agent.snapshot().await.messages;
    assert_eq!(system_rows(&messages).len(), 1, "{messages:?}");
}

/// A loadout change between runs is recorded as the delta, and only the delta: the added tool is
/// declared, the dropped one is removed by name, the unchanged one is not repeated.
#[tokio::test]
async fn a_loadout_change_is_recorded_as_a_delta() {
    let (sf, _seen) = spy(2);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![
            Probe::new("keep").arc(),
            Probe::new("drop").arc(),
        ]))
        .build();
    run(&agent, "one").await;
    agent
        .set_loadout(loadout(vec![
            Probe::new("keep").arc(),
            Probe::new("new").arc(),
        ]))
        .await;
    run(&agent, "two").await;

    let messages = agent.snapshot().await.messages;
    let rows = system_rows(&messages);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(added(rows[1]), ["new"]);
    assert_eq!(removed(rows[1]), ["drop"]);
    assert_eq!(
        rows[1].tools_removed,
        vec![ToolReference::new("drop")],
        "a removal is name-only"
    );
    // The replay of what was recorded is exactly the executable set.
    let replayed: Vec<Message> = rows.iter().map(|s| Message::System((*s).clone())).collect();
    let names: Vec<String> = get_current_tools(&replayed)
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["keep", "new"]);
}

/// A transcript that already declares the loadout — the shape a resumed session seeds — is not
/// declared again, and a transcript that declares something else gets the difference.
#[tokio::test]
async fn a_seeded_declaration_is_reconciled_not_repeated() {
    let declared = |names: &[&str]| {
        AgentMessage::System(SystemMessage {
            tools_added: names
                .iter()
                .map(|n| ToolDef {
                    name: (*n).to_string(),
                    description: "probe".to_string(),
                    parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                    constrained_sampling: None,
                })
                .collect(),
            timestamp: 1,
            ..SystemMessage::default()
        })
    };

    // Same loadout: nothing new.
    let (sf, _seen) = spy(1);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![Probe::new("a").arc()]))
        .messages(vec![declared(&["a"])])
        .build();
    run(&agent, "go").await;
    assert_eq!(system_rows(&agent.snapshot().await.messages).len(), 1);

    // A different loadout: the delta, against the seeded declaration.
    let (sf, _seen) = spy(1);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![Probe::new("b").arc()]))
        .messages(vec![declared(&["a"])])
        .build();
    run(&agent, "go").await;
    let messages = agent.snapshot().await.messages;
    let rows = system_rows(&messages);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(added(rows[1]), ["b"]);
    assert_eq!(removed(rows[1]), ["a"]);
}

/// A hidden declaration is recorded and never sent. The transcript row holds the hidden tool (so
/// a resume restores it), the request's `Context::tools` does not, and no message of the request
/// carries it either.
#[tokio::test]
async fn a_hidden_declaration_is_recorded_but_never_sent() {
    let (sf, seen) = spy(1);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![
            Probe::new("search").hiding(&["secret"]).arc(),
            Probe::new("secret").arc(),
        ]))
        .build();
    run(&agent, "go").await;

    let messages = agent.snapshot().await.messages;
    let rows = system_rows(&messages);
    assert_eq!(
        added(rows[0]),
        ["search", "secret"],
        "the hidden declaration is in the transcript"
    );

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let ctx = &requests[0];
    let advertised: Vec<&str> = ctx.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(advertised, ["search"], "the request's tools");
    let wire = serde_json::to_string(&ctx.messages).unwrap();
    assert!(
        !wire.contains("secret"),
        "no message of the request mentions the hidden declaration: {wire}"
    );
}

/// Whatever a provider replays out of the request is the advertised set, each tool once: the
/// declarations inside the transcript are not sent a second time beside `Context::tools`.
#[tokio::test]
async fn a_provider_replaying_the_request_sees_each_advertised_tool_once() {
    let (sf, seen) = spy(2);
    let agent = Agent::builder(model_ref(), sf)
        .loadout(loadout(vec![Probe::new("a").arc(), Probe::new("b").arc()]))
        .build();
    run(&agent, "one").await;
    agent
        .set_loadout(loadout(vec![Probe::new("a").arc(), Probe::new("c").arc()]))
        .await;
    run(&agent, "two").await;

    for ctx in seen.lock().unwrap().iter() {
        let transcript = normalize_context(ctx);
        let replayed: Vec<String> = get_current_tools(transcript.messages())
            .into_iter()
            .map(|t| t.name)
            .collect();
        let advertised: Vec<String> = ctx.tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(replayed, advertised, "request: {ctx:?}");
    }
}
