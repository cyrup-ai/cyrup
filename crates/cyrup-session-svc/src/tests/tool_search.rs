//! TOOL-052, session half — `tool_search` inside a real [`AgentSession`]: a `deferred` tool is not in
//! the provider's request, the model calls `tool_search`, the VERY NEXT request of the same run
//! declares the loaded tool, and the model can call it.
//!
//! Every schema assertion reads `Context::tools` inside the faux provider's response factory, which
//! is the exact value a real provider adapter serialises. pi's rules, read at v1.0.1:
//! `searchAndLoad` (`extensions/tool-search/tool.ts:200-216`), `_applyToolLoadout`
//! (`core/agent-session.ts:1528-1572`), `_installAgentNextTurnRefresh`
//! (`core/agent-session.ts:519-540`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolExposure, ToolNamespace, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Provider, ToolDef};
use cyrup_tool_search::ToolSearchExtension;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

/// A `deferred` tool in an MCP-style namespace that counts its executions.
struct Deferred {
    name: &'static str,
    description: &'static str,
    namespace: ToolNamespace,
    params: Value,
    runs: Arc<AtomicUsize>,
}

impl Deferred {
    fn new(name: &'static str, description: &'static str, runs: &Arc<AtomicUsize>) -> Self {
        Self {
            name,
            description,
            namespace: ToolNamespace {
                name: "mcp__docs".to_owned(),
                description: Some("Documentation server".to_owned()),
                instructions: None,
            },
            params: json!({ "type": "object", "properties": {} }),
            runs: Arc::clone(runs),
        }
    }
}

#[async_trait::async_trait]
impl Tool for Deferred {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        self.description
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn namespace(&self) -> Option<&ToolNamespace> {
        Some(&self.namespace)
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text("widget found")],
            ..ToolResult::default()
        })
    }
}

/// Registers its tools at `init`, as an MCP adapter's registration does.
struct DeferredExt(Vec<Arc<dyn Tool>>);

#[async_trait::async_trait]
impl NativeExtension for DeferredExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("deferred-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for tool in &self.0 {
            api.register_tool(Arc::clone(tool));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
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

/// `tool_search` is a built-in extension: `no_extensions` would drop it, as it does in pi.
fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The tool names each provider request declared, in request order.
type Requests = Arc<Mutex<Vec<Vec<String>>>>;

/// One scripted model reply: a tool call with arguments, or text.
enum Reply {
    Call(&'static str, Value),
    Text(&'static str),
}

fn script(requests: &Requests, replies: Vec<Reply>) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(
                    ctx.tools
                        .iter()
                        .map(|tool: &ToolDef| tool.name.clone())
                        .collect(),
                );
                match &reply {
                    Reply::Call(name, args) => faux_assistant_message(
                        vec![faux_tool_call((*name).to_owned(), args.clone())],
                        StopReason::ToolUse,
                    ),
                    Reply::Text(text) => faux_assistant_message(
                        vec![faux_text((*text).to_owned())],
                        StopReason::Stop,
                    ),
                }
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

struct Rig {
    /// Holds the temp dirs for the life of the test.
    _fx: Fixture,
    session: Arc<AgentSession>,
    requests: Requests,
    runs: Arc<AtomicUsize>,
}

const LOOKUP: &str = "mcp__docs__lookup_widget";
const UNRELATED: &str = "mcp__docs__rotate_logs";

async fn rig(replies: Vec<Reply>) -> Rig {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let provider = script(&requests, replies);
    let runs = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(Deferred::new(LOOKUP, "Look up a widget by name.", &runs)),
        Arc::new(Deferred::new(UNRELATED, "Rotate the log files.", &runs)),
    ];
    let session = SessionBuilder::new(provider as Arc<dyn Provider>, config(&fx))
        .with_native_extension(Arc::new(DeferredExt(tools)))
        .with_native_extension(Arc::new(ToolSearchExtension::new()))
        .build()
        .await
        .unwrap()
        .into_shared();
    Rig {
        _fx: fx,
        session,
        requests,
        runs,
    }
}

impl Rig {
    /// `tool_search` activated, the way `--tools` or `defaultTools` would.
    async fn activate_tool_search(&self) {
        let mut active = self.session.active_tool_names();
        active.push("tool_search".to_owned());
        self.session.set_active_tools_by_name(&active).await;
    }

    async fn run(&self, prompt: &str) {
        let _ = self.session.prompt(prompt).await.unwrap();
        self.session.wait_for_idle().await;
    }

    fn requests(&self) -> Vec<Vec<String>> {
        self.requests.lock().unwrap().clone()
    }

    async fn tool_result(&self, tool: &str) -> Message {
        self.session
            .messages()
            .await
            .into_iter()
            .rev()
            .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == tool))
            .unwrap_or_else(|| panic!("a `{tool}` tool result"))
    }
}

fn declares(request: &[String], name: &str) -> bool {
    request.iter().any(|declared| declared == name)
}

fn result_text(message: &Message) -> String {
    let Message::ToolResult { content, .. } = message else {
        panic!("not a tool result");
    };
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// `tool_search` is registered inactive, and a `deferred` tool never reaches the provider on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_search_is_registered_inactive_and_deferred_tools_are_not_declared() {
    let rig = rig(vec![Reply::Text("done")]).await;
    assert!(
        !rig.session
            .active_tool_names()
            .iter()
            .any(|n| n == "tool_search"),
        "registered `defaultActive: false`"
    );
    let row = rig
        .session
        .all_tools()
        .into_iter()
        .find(|row| row.name == "tool_search")
        .expect("registered");
    assert_eq!(row.exposure, ToolExposure::ModelOnly);

    rig.run("go").await;
    let requests = rig.requests();
    assert_eq!(requests.len(), 1);
    assert!(!declares(&requests[0], "tool_search"), "{:?}", requests[0]);
    assert!(!declares(&requests[0], LOOKUP), "{:?}", requests[0]);
}

/// HEADLINE. Request 1 declares `tool_search` but not the `deferred` tool; the model calls
/// `tool_search`; request 2 OF THE SAME RUN declares exactly the matching tool; the model calls it
/// and it runs; the unrelated `deferred` tool stays out throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_tool_found_by_tool_search_is_declared_on_the_very_next_request_and_callable() {
    let rig = rig(vec![
        Reply::Call("tool_search", json!({ "query": "widget" })),
        Reply::Call(LOOKUP, json!({})),
        Reply::Text("all done"),
    ])
    .await;
    rig.activate_tool_search().await;

    rig.run("find me the widget tool").await;

    let requests = rig.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    // Request 1: `tool_search` is declared, the deferred tools are not.
    assert!(declares(&requests[0], "tool_search"), "{:?}", requests[0]);
    assert!(!declares(&requests[0], LOOKUP), "{:?}", requests[0]);
    assert!(!declares(&requests[0], UNRELATED), "{:?}", requests[0]);
    // Request 2, the very next one: the loaded tool is declared, the unrelated one is not.
    assert!(declares(&requests[1], LOOKUP), "{:?}", requests[1]);
    assert!(!declares(&requests[1], UNRELATED), "{:?}", requests[1]);
    assert!(declares(&requests[1], "tool_search"), "{:?}", requests[1]);
    // The model called the loaded tool and it ran.
    assert_eq!(rig.runs.load(Ordering::SeqCst), 1);
    assert!(declares(&requests[2], LOOKUP), "{:?}", requests[2]);

    // What the model was told.
    let result = rig.tool_result("tool_search").await;
    assert_eq!(
        result_text(&result),
        format!(
            "Loaded 1 tool. They are available from your next call:\n- {LOOKUP}: Look up a widget by name."
        )
    );
    let Message::ToolResult {
        details, is_error, ..
    } = &result
    else {
        unreachable!()
    };
    assert!(!is_error);
    assert_eq!(details.as_ref().unwrap(), &json!({ "loaded": [LOOKUP] }));
    // The load is the session's active set, so the next prompt keeps it.
    assert!(rig.session.active_tool_names().iter().any(|n| n == LOOKUP));
    assert!(
        !rig.session
            .active_tool_names()
            .iter()
            .any(|n| n == UNRELATED)
    );
}

/// A second identical search finds nothing: the loaded tool is active and candidates exclude the
/// active set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repeated_search_does_not_offer_a_loaded_tool_again() {
    let rig = rig(vec![
        Reply::Call("tool_search", json!({ "query": "widget" })),
        Reply::Call("tool_search", json!({ "query": "widget" })),
        Reply::Text("done"),
    ])
    .await;
    rig.activate_tool_search().await;
    rig.run("go").await;

    let results: Vec<String> = rig
        .session
        .messages()
        .await
        .iter()
        .filter(
            |m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "tool_search"),
        )
        .map(result_text)
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].starts_with("Loaded 1 tool."), "{results:?}");
    assert_eq!(results[1], "No matching tools found.");
}

/// The error texts reach the model as tool errors, and nothing is loaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_arguments_come_back_as_tool_errors_and_load_nothing() {
    let rig = rig(vec![
        Reply::Call("tool_search", json!({ "query": "   " })),
        Reply::Call("tool_search", json!({ "query": "widget", "limit": 0 })),
        Reply::Text("done"),
    ])
    .await;
    rig.activate_tool_search().await;
    rig.run("go").await;

    let results: Vec<(String, bool)> = rig
        .session
        .messages()
        .await
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult {
                tool_name,
                is_error,
                ..
            } if tool_name == "tool_search" => Some((result_text(m), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            ("query must not be empty".to_owned(), true),
            ("limit must be a positive integer".to_owned(), true)
        ]
    );
    assert!(!rig.session.active_tool_names().iter().any(|n| n == LOOKUP));
    assert!(!declares(&rig.requests()[2], LOOKUP));
}
