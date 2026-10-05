//! CODE-006, session half — `AgentSession::execute_nested_tool`: a tool that calls other tools while
//! it runs.
//!
//! Pi `_executeNestedToolCall` (`core/agent-session.ts:699-740` @v1.0.1) and the stamping at
//! `:1075-1082`: the nested call resolves against the CALLABLE tools, runs through the agent's
//! tool pipeline with the session's hooks, emits `tool_execution_*` events with `parentToolCallId`,
//! never enters the transcript, and is recorded — with its usage — on the model-issued call's tool
//! result.
//!
//! Every test drives a real session, a faux provider, and a tool that holds the session and calls
//! `execute_nested_tool` with its own call id, which is exactly what `ctx.executeTool` does.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cyrup_agent::NestedToolCallOptions;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, NestedCallStatus, NestedToolCalls, StopReason,
    TerminateHint, Tool, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink, Usage,
};
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};

// ------------------------------------------------------------------------------ fixtures ----

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
    std::fs::write(cwd.join("a.txt"), "hello from a.txt\n").unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        total_tokens: input + output,
        ..Usage::default()
    }
}

fn schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": true })
}

/// The session a calling tool reaches back into: set once the session exists, which is after the
/// tool was registered.
type SessionSlot = Arc<OnceLock<Arc<AgentSession>>>;

/// `(event kind, call id, parent)` per tool event, as the extension's ctx reports them.
type ExtSeen = Arc<Mutex<Vec<(&'static str, String, Option<String>)>>>;

/// See [`ToolsExt::ends`].
type ExtEnds = Arc<Mutex<Vec<(String, bool)>>>;

/// What a nested call came back as: `(call id, is_error, text)`.
type Outcomes = Arc<Mutex<Vec<(String, bool, String)>>>;

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool that, while it runs, calls the listed tools through the session with its own call id.
struct Caller {
    name: &'static str,
    session: SessionSlot,
    calls: Vec<(&'static str, Value)>,
    outcomes: Outcomes,
    params: Value,
}

impl Caller {
    fn tool(
        name: &'static str,
        session: &SessionSlot,
        calls: Vec<(&'static str, Value)>,
        outcomes: &Outcomes,
    ) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            session: Arc::clone(session),
            calls,
            outcomes: Arc::clone(outcomes),
            params: schema(),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Caller {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        _params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let session = self.session.get().expect("session is set before the run");
        for (name, args) in &self.calls {
            let outcome = session
                .execute_nested_tool(
                    &call_id,
                    name,
                    args.clone(),
                    NestedToolCallOptions {
                        cancel: Some(cancel.clone()),
                        on_update: None,
                    },
                )
                .await;
            self.outcomes.lock().unwrap().push((
                outcome.tool_call.id.to_string(),
                outcome.is_error,
                text_of(&outcome.result.content),
            ));
        }
        Ok(ToolResult {
            content: vec![Content::text("caller done")],
            ..Default::default()
        })
    }
}

/// A leaf tool with a settable exposure that reports usage for its own execution and counts runs.
struct Leaf {
    name: &'static str,
    exposure: ToolExposure,
    default_active: bool,
    reports: Option<Usage>,
    ran: Arc<Mutex<usize>>,
    params: Value,
}

impl Leaf {
    fn new(name: &'static str, exposure: ToolExposure) -> Self {
        Self {
            name,
            exposure,
            default_active: true,
            reports: None,
            ran: Arc::new(Mutex::new(0)),
            params: schema(),
        }
    }
    fn inactive(mut self) -> Self {
        self.default_active = false;
        self
    }
    fn reporting(mut self, u: Usage) -> Self {
        self.reports = Some(u);
        self
    }
    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Leaf {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        *self.ran.lock().unwrap() += 1;
        Ok(ToolResult {
            content: vec![Content::text(format!("{} ran", self.name))],
            usage: self.reports.clone(),
            ..Default::default()
        })
    }
}

/// Registers its tools at `init`, the way an extension's `registerTool` calls do — which is what
/// gives `codemode`, `deferred`, `hidden` and inactive tools a registry entry without activating
/// them.
struct ToolsExt {
    tools: Vec<Arc<dyn Tool>>,
    /// Block any `tool_call` for this tool name, the way a permission gate does.
    deny: Option<&'static str>,
    /// `(kind, call id, parent)` of every tool event, as the ctx reports it.
    seen: ExtSeen,
    /// `(tool name, carries nestedCalls)` of every tool-result `message_end` the extension was
    /// handed — the message as it will be persisted.
    ends: ExtEnds,
}

#[async_trait::async_trait]
impl NativeExtension for ToolsExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("nested-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for t in &self.tools {
            api.register_tool(Arc::clone(t));
        }
        api.subscribe(&[
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::ToolExecStart,
            EventKind::ToolExecEnd,
            EventKind::MessageEnd,
        ]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::MessageEnd {
            message:
                Message::ToolResult {
                    tool_name,
                    nested_calls,
                    ..
                },
        } = ev
        {
            self.ends
                .lock()
                .unwrap()
                .push((tool_name.clone(), nested_calls.is_some()));
            return HookOutcome::Noop;
        }
        let (kind, id) = match ev {
            HostEvent::ToolCall { call_id, .. } => ("tool_call", call_id),
            HostEvent::ToolResult { call_id, .. } => ("tool_result", call_id),
            HostEvent::ToolExecStart { call_id, .. } => ("tool_execution_start", call_id),
            HostEvent::ToolExecEnd { call_id, .. } => ("tool_execution_end", call_id),
            _ => return HookOutcome::Noop,
        };
        self.seen.lock().unwrap().push((
            kind,
            id.to_string(),
            ctx.parent_tool_call_id().map(ToString::to_string),
        ));
        if let HostEvent::ToolCall { name, .. } = ev
            && Some(name.as_str()) == self.deny
        {
            return HookOutcome::Block {
                reason: Some(format!("{name} is denied by policy")),
                terminate: TerminateHint::Unspecified,
            };
        }
        HookOutcome::Noop
    }
}

/// What the provider saw on its second request: the messages the model is shown after the tool ran.
type SeenMessages = Arc<Mutex<Vec<Message>>>;

/// Two replies: the model calls `caller`, then answers.
fn faux_calling(caller: &'static str, seen: &SeenMessages) -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::clone(seen);
    faux.set_response_steps(vec![
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call(caller.to_string(), json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(move |ctx, _o, _s, _m| {
            *seen.lock().unwrap() = ctx.messages.clone();
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    faux
}

struct Run {
    session: Arc<AgentSession>,
    events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    outcomes: Outcomes,
    ext_seen: ExtSeen,
    ext_ends: ExtEnds,
    provider_messages: SeenMessages,
}

/// Build a session with `caller` active and `others` registered by an extension, run one prompt in
/// which the model calls `caller`, and settle.
async fn run_caller(
    fx: &Fixture,
    calls: Vec<(&'static str, Value)>,
    others: Vec<Arc<dyn Tool>>,
    deny: Option<&'static str>,
) -> Run {
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let outcomes: Outcomes = Arc::new(Mutex::new(Vec::new()));
    let provider_messages: SeenMessages = Arc::new(Mutex::new(Vec::new()));
    let ext_seen = Arc::new(Mutex::new(Vec::new()));
    let ext_ends: ExtEnds = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = config(fx);
    cfg.custom_tools = vec![Caller::tool("caller", &slot, calls, &outcomes)];
    let session = SessionBuilder::new(
        faux_calling("caller", &provider_messages) as Arc<dyn Provider>,
        cfg,
    )
    .with_native_extension(Arc::new(ToolsExt {
        tools: others,
        deny,
        seen: Arc::clone(&ext_seen),
        ends: Arc::clone(&ext_ends),
    }))
    .build()
    .await
    .unwrap()
    .into_shared();
    let _ = slot.set(Arc::clone(&session));
    let mut names = session.active_tool_names();
    names.push("caller".to_string());
    session.set_active_tools_by_name(&names).await;

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = session.subscribe();
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(ev) = stream.next().await {
            sink.lock().unwrap().push(ev);
        }
    });

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    // The collector drains on its own task: wait for it to reach the run's last event.
    let settled = std::time::Instant::now();
    while !events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AgentSessionEvent::AgentSettled))
        && settled.elapsed() < std::time::Duration::from_secs(10)
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Run {
        session,
        events,
        outcomes,
        ext_seen,
        ext_ends,
        provider_messages,
    }
}

/// The persisted tool-result row of the model-issued `caller` call.
async fn persisted_caller_result(run: &Run) -> Message {
    run.session
        .messages()
        .await
        .into_iter()
        .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "caller"))
        .expect("the caller's tool result was persisted")
}

fn nested_of(message: &Message) -> Option<NestedToolCalls> {
    match message {
        Message::ToolResult { nested_calls, .. } => nested_calls.clone(),
        _ => None,
    }
}

// ------------------------------------------------------------------------------- headline ----

/// (a) HEADLINE. A tool that calls `read` through its own call id gets the result; the session's
/// subscribers see `parentToolCallId` events; the persisted tool result carries a `nestedCalls`
/// record with `status: ok`; and the model is shown ZERO nested rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_that_calls_read_gets_the_result_events_a_record_and_no_transcript_row() {
    let fx = fixture();
    let run = run_caller(
        &fx,
        vec![("read", json!({ "path": "a.txt" }))],
        vec![],
        None,
    )
    .await;

    // The nested result.
    let outcomes = run.outcomes.lock().unwrap().clone();
    assert_eq!(outcomes.len(), 1);
    let (id, is_error, text) = &outcomes[0];
    assert!(!is_error, "{text}");
    assert!(text.contains("hello from a.txt"), "{text}");
    let parent = {
        let events = run.events.lock().unwrap();
        events
            .iter()
            .find_map(|e| match e {
                AgentSessionEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    ..
                } if tool_name == "caller" => Some(tool_call_id.to_string()),
                _ => None,
            })
            .expect("the model-issued call started")
    };
    assert_eq!(id, &format!("{parent}/1"), "the nested id is <caller>/<n>");

    // The session's subscribers: start and end of the nested call, each with the parent; and the
    // loop's own events for `caller` carry none.
    {
        let events = run.events.lock().unwrap();
        let nested: Vec<(&'static str, String, String)> = events
            .iter()
            .filter_map(|e| match e {
                AgentSessionEvent::NestedToolExecutionStart {
                    tool_call_id,
                    parent_tool_call_id,
                    ..
                } => Some((
                    "start",
                    tool_call_id.to_string(),
                    parent_tool_call_id.to_string(),
                )),
                AgentSessionEvent::NestedToolExecutionEnd {
                    tool_call_id,
                    parent_tool_call_id,
                    is_error,
                    ..
                } => {
                    assert!(!is_error);
                    Some((
                        "end",
                        tool_call_id.to_string(),
                        parent_tool_call_id.to_string(),
                    ))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            nested,
            vec![
                ("start", id.clone(), parent.clone()),
                ("end", id.clone(), parent.clone()),
            ]
        );
        for e in events.iter() {
            if let AgentSessionEvent::ToolExecutionStart { .. }
            | AgentSessionEvent::ToolExecutionEnd { .. } = e
            {
                assert!(
                    !serde_json::to_string(e)
                        .unwrap()
                        .contains("parentToolCallId"),
                    "a loop event gained a key"
                );
            }
        }
    }

    // The persisted row.
    let row = persisted_caller_result(&run).await;
    let nested = nested_of(&row).expect("the persisted result carries nestedCalls");
    assert!(nested.complete);
    assert_eq!(nested.calls.len(), 1);
    assert_eq!(nested.calls[0].id, *id);
    assert_eq!(nested.calls[0].name, "read");
    assert_eq!(nested.calls[0].status, NestedCallStatus::Ok);
    assert_eq!(
        nested.calls[0].arguments,
        json!({ "path": "a.txt" }).as_object().cloned()
    );
    assert!(nested.calls[0].duration_ms.is_some());
    // The extension seam's `message_end` handler was handed the message WITH its record, i.e. as
    // it is persisted (pi stamps before any listener runs).
    assert_eq!(
        *run.ext_ends.lock().unwrap(),
        vec![("caller".to_string(), true)]
    );
    // …and it is the pi wire shape on disk.
    let wire = serde_json::to_value(&row).unwrap();
    assert_eq!(wire["nestedCalls"]["calls"][0]["status"], "ok");

    // ZERO nested rows anywhere the model can see: the persisted branch, the agent's transcript and
    // the messages of the provider's follow-up request each hold exactly the one tool result.
    let persisted: Vec<_> = run
        .session
        .messages()
        .await
        .into_iter()
        .filter(|m| matches!(m, Message::ToolResult { .. }))
        .collect();
    assert_eq!(persisted.len(), 1, "{persisted:?}");
    let live: Vec<_> = run
        .session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) => Some(t.tool_call_id.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(live, vec![parent.clone()]);
    let seen_by_model: Vec<String> = run
        .provider_messages
        .lock()
        .unwrap()
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(seen_by_model, vec![parent]);
    let model_json = serde_json::to_string(&*run.provider_messages.lock().unwrap()).unwrap();
    assert!(
        !model_json.contains("a.txt/1") && !model_json.contains("/1\""),
        "a nested call id reached the model: {model_json}"
    );
}

/// The usage of nested results is summed onto the model-issued result — in the persisted row and in
/// the `message_end` the subscribers see (pi `:1075-1082`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_usage_is_summed_onto_the_result_row_and_the_live_message_end() {
    let fx = fixture();
    let billing = Leaf::new("billing", ToolExposure::Codemode)
        .reporting(usage(11, 22))
        .arc();
    let run = run_caller(
        &fx,
        vec![("billing", json!({})), ("billing", json!({}))],
        vec![billing],
        None,
    )
    .await;

    let row = persisted_caller_result(&run).await;
    let Message::ToolResult { usage: got, .. } = &row else {
        panic!("not a tool result");
    };
    let got = got.clone().expect("nested usage reached the persisted row");
    assert_eq!((got.input, got.output), (22, 44));
    assert_eq!(nested_of(&row).unwrap().calls.len(), 2);

    let live: Vec<Option<Usage>> = run
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::MessageEnd {
                message: cyrup_agent::AgentMessage::ToolResult(t),
            } if t.tool_name == "caller" => Some(t.usage.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(
        live[0].as_ref().map(|u| (u.input, u.output)),
        Some((22, 44))
    );
}

/// (b) Exposure decides what a nested call can reach: the callable view, never the declared one. A
/// `hidden`, a `model-only` and an inactive `direct` tool are refused with a result that names the
/// tool; a `codemode` and a `deferred` one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_callable_tools_are_reachable_from_a_nested_call() {
    let fx = fixture();
    let hidden = Leaf::new("hidden_t", ToolExposure::Hidden);
    let model_only = Leaf::new("model_only_t", ToolExposure::ModelOnly);
    let inactive = Leaf::new("inactive_t", ToolExposure::Direct).inactive();
    let codemode = Leaf::new("codemode_t", ToolExposure::Codemode);
    let deferred = Leaf::new("deferred_t", ToolExposure::Deferred);
    let ran: BTreeMap<&str, Arc<Mutex<usize>>> = [
        ("hidden_t", hidden.ran.clone()),
        ("model_only_t", model_only.ran.clone()),
        ("inactive_t", inactive.ran.clone()),
        ("codemode_t", codemode.ran.clone()),
        ("deferred_t", deferred.ran.clone()),
    ]
    .into_iter()
    .collect();
    let calls: Vec<(&'static str, Value)> = [
        "hidden_t",
        "model_only_t",
        "inactive_t",
        "codemode_t",
        "deferred_t",
    ]
    .into_iter()
    .map(|n| (n, json!({})))
    .collect();
    let run = run_caller(
        &fx,
        calls,
        vec![
            hidden.arc(),
            model_only.arc(),
            inactive.arc(),
            codemode.arc(),
            deferred.arc(),
        ],
        None,
    )
    .await;

    let outcomes = run.outcomes.lock().unwrap().clone();
    assert_eq!(outcomes.len(), 5);
    for (i, name) in ["hidden_t", "model_only_t", "inactive_t"]
        .into_iter()
        .enumerate()
    {
        assert!(outcomes[i].1, "{name} must be refused: {outcomes:?}");
        assert_eq!(outcomes[i].2, format!("Tool {name} not found"));
        assert_eq!(*ran[name].lock().unwrap(), 0, "{name} ran");
    }
    for (i, name) in [(3, "codemode_t"), (4, "deferred_t")] {
        assert!(!outcomes[i].1, "{name} must run: {outcomes:?}");
        assert_eq!(outcomes[i].2, format!("{name} ran"));
        assert_eq!(*ran[name].lock().unwrap(), 1);
    }
    // The refusals are in the record as errors; the two that ran as ok.
    let nested = nested_of(&persisted_caller_result(&run).await).unwrap();
    let statuses: Vec<_> = nested.calls.iter().map(|c| c.status).collect();
    assert_eq!(
        statuses,
        vec![
            NestedCallStatus::Error,
            NestedCallStatus::Error,
            NestedCallStatus::Error,
            NestedCallStatus::Ok,
            NestedCallStatus::Ok
        ]
    );
    assert_eq!(
        nested.calls[0].error.as_deref(),
        Some("Tool hidden_t not found")
    );
}

/// Calls made by nested tools are recorded on the model-issued result too, with `<id>/<n>/<m>` ids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_nested_inside_nested_calls_are_recorded_at_their_depth() {
    let fx = fixture();
    // `middle` is a callable tool that itself calls `leaf`.
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let leaf = Leaf::new("leaf", ToolExposure::Codemode).arc();
    let middle_outcomes: Outcomes = Arc::new(Mutex::new(Vec::new()));
    let middle = CallerTool {
        inner: Caller {
            name: "middle",
            session: Arc::clone(&slot),
            calls: vec![("leaf", json!({}))],
            outcomes: Arc::clone(&middle_outcomes),
            params: schema(),
        },
        exposure: ToolExposure::Codemode,
    };
    let run = run_caller_with_slot(
        &fx,
        &slot,
        vec![("middle", json!({}))],
        vec![leaf, Arc::new(middle)],
    )
    .await;

    let nested = nested_of(&persisted_caller_result(&run).await).unwrap();
    let ids: Vec<&str> = nested.calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(nested.calls.len(), 2, "{ids:?}");
    assert!(ids[0].ends_with("/1"));
    assert_eq!(ids[1], format!("{}/1", ids[0]));
    assert_eq!(
        middle_outcomes.lock().unwrap()[0].0,
        ids[1],
        "the middle tool's own nested call has the id the record shows"
    );
}

/// A `Caller` that is also a callable (`codemode`) tool: `middle` in the nesting test.
struct CallerTool {
    inner: Caller,
    exposure: ToolExposure,
}

#[async_trait::async_trait]
impl Tool for CallerTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn parameters(&self) -> &Value {
        self.inner.parameters()
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.inner.execute(call_id, params, cancel, on_update).await
    }
}

/// [`run_caller`] with a caller-supplied session slot, for a nested tool that needs the session
/// too.
async fn run_caller_with_slot(
    fx: &Fixture,
    slot: &SessionSlot,
    calls: Vec<(&'static str, Value)>,
    others: Vec<Arc<dyn Tool>>,
) -> Run {
    let outcomes: Outcomes = Arc::new(Mutex::new(Vec::new()));
    let provider_messages: SeenMessages = Arc::new(Mutex::new(Vec::new()));
    let ext_seen = Arc::new(Mutex::new(Vec::new()));
    let ext_ends: ExtEnds = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = config(fx);
    cfg.custom_tools = vec![Caller::tool("caller", slot, calls, &outcomes)];
    let session = SessionBuilder::new(
        faux_calling("caller", &provider_messages) as Arc<dyn Provider>,
        cfg,
    )
    .with_native_extension(Arc::new(ToolsExt {
        tools: others,
        deny: None,
        seen: Arc::clone(&ext_seen),
        ends: Arc::clone(&ext_ends),
    }))
    .build()
    .await
    .unwrap()
    .into_shared();
    let _ = slot.set(Arc::clone(&session));
    let mut names = session.active_tool_names();
    names.push("caller".to_string());
    session.set_active_tools_by_name(&names).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    Run {
        session,
        events,
        outcomes,
        ext_seen,
        ext_ends,
        provider_messages,
    }
}

/// A nested call meets the session's hooks: a permission gate that blocks a tool for the model
/// blocks it for a tool that calls it. The block comes back as an error outcome, never a panic or
/// an unwound run, and the handlers are told the parent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_call_meets_the_same_permission_gate_and_the_handlers_see_the_parent() {
    let fx = fixture();
    let forbidden = Leaf::new("forbidden", ToolExposure::Codemode);
    let ran = forbidden.ran.clone();
    let run = run_caller(
        &fx,
        vec![
            ("forbidden", json!({})),
            ("read", json!({ "path": "a.txt" })),
        ],
        vec![forbidden.arc()],
        Some("forbidden"),
    )
    .await;

    let outcomes = run.outcomes.lock().unwrap().clone();
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].1, "the gate's block is an error outcome");
    assert_eq!(outcomes[0].2, "forbidden is denied by policy");
    assert!(!outcomes[1].1, "an allowed call still runs: {outcomes:?}");
    assert_eq!(*ran.lock().unwrap(), 0, "the blocked tool executed");

    let nested = nested_of(&persisted_caller_result(&run).await).unwrap();
    assert_eq!(nested.calls[0].status, NestedCallStatus::Error);
    assert_eq!(
        nested.calls[0].error.as_deref(),
        Some("forbidden is denied by policy")
    );
    assert_eq!(nested.calls[1].status, NestedCallStatus::Ok);

    // Handlers: the model-issued call reports no parent; both nested calls report the caller's id.
    let seen = run.ext_seen.lock().unwrap().clone();
    let caller_id = outcomes[0].0.rsplit_once('/').unwrap().0.to_string();
    let tool_calls: Vec<(String, Option<String>)> = seen
        .iter()
        .filter(|(k, _, _)| *k == "tool_call")
        .map(|(_, id, parent)| (id.clone(), parent.clone()))
        .collect();
    assert!(
        tool_calls.contains(&(caller_id.clone(), None)),
        "the model-issued call has no parent: {tool_calls:?}"
    );
    for nested_id in [&outcomes[0].0, &outcomes[1].0] {
        assert!(
            tool_calls.contains(&(nested_id.clone(), Some(caller_id.clone()))),
            "{nested_id} should report its parent: {tool_calls:?}"
        );
    }
    // `tool_result` and `tool_execution_*` of the nested read carry the parent as well; the
    // blocked call never executed, so it has no result.
    for kind in ["tool_result", "tool_execution_start", "tool_execution_end"] {
        assert!(
            seen.iter().any(|(k, id, parent)| *k == kind
                && id == &outcomes[1].0
                && parent.as_deref() == Some(caller_id.as_str())),
            "{kind} of the nested read did not report its parent: {seen:?}"
        );
    }
}

/// With no assistant message to attribute the call to, the call comes back as pi's error text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_assistant_message_the_call_is_refused_with_pis_text() {
    let fx = fixture();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .unwrap()
        .into_shared();
    let outcome = session
        .execute_nested_tool(
            &ToolCallId::from("call"),
            "read",
            json!({ "path": "a.txt" }),
            NestedToolCallOptions::default(),
        )
        .await;
    assert!(outcome.is_error);
    assert_eq!(
        text_of(&outcome.result.content),
        "No assistant message issued this call"
    );
}

/// (c) The limits, through the session: the 257th call is dropped from the record, which says it is
/// incomplete, while every call still ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_257th_nested_call_is_dropped_from_the_record_and_marks_it_incomplete() {
    let fx = fixture();
    let leaf = Leaf::new("leaf", ToolExposure::Codemode);
    let ran = leaf.ran.clone();
    let calls: Vec<(&'static str, Value)> = (0..257).map(|_| ("leaf", json!({}))).collect();
    let run = run_caller(&fx, calls, vec![leaf.arc()], None).await;

    assert_eq!(*ran.lock().unwrap(), 257, "every call ran");
    assert_eq!(run.outcomes.lock().unwrap().len(), 257);
    let nested = nested_of(&persisted_caller_result(&run).await).unwrap();
    assert_eq!(nested.calls.len(), 256);
    assert!(!nested.complete);
}

/// Oversize arguments are recorded as their size; an error is truncated to 500 characters.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversize_arguments_are_recorded_as_a_size_and_long_errors_are_truncated() {
    let fx = fixture();
    let big = "x".repeat(9000);
    let run = run_caller(&fx, vec![("nope", json!({ "text": big }))], vec![], None).await;
    let nested = nested_of(&persisted_caller_result(&run).await).unwrap();
    assert_eq!(nested.calls[0].arguments, None);
    assert!(nested.calls[0].arguments_bytes.unwrap() > 8 * 1024);
    assert!(!nested.complete);
}

/// A session event serializes with pi's wire shape: the loop's `tool_execution_*` events have no
/// `parentToolCallId`; a nested one has it, last.
#[test]
fn nested_session_events_carry_the_parent_key_and_loop_events_do_not() {
    let nested = AgentSessionEvent::NestedToolExecutionStart {
        tool_call_id: "c/1".into(),
        tool_name: "read".into(),
        args: json!({ "path": "a" }),
        parent_tool_call_id: "c".into(),
    };
    assert_eq!(
        serde_json::to_string(&nested).unwrap(),
        r#"{"type":"tool_execution_start","toolCallId":"c/1","toolName":"read","args":{"path":"a"},"parentToolCallId":"c"}"#
    );
    assert_eq!(nested.kind(), "tool_execution_start");
    let normal = AgentSessionEvent::ToolExecutionStart {
        tool_call_id: "c".into(),
        tool_name: "read".into(),
        args: json!({ "path": "a" }),
    };
    assert_eq!(
        serde_json::to_string(&normal).unwrap(),
        r#"{"type":"tool_execution_start","toolCallId":"c","toolName":"read","args":{"path":"a"}}"#
    );
}
