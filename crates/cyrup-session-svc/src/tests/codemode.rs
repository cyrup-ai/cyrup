//! CODE-007 … CODE-012, session half — the `codemode` tool inside a real [`AgentSession`]: what
//! the provider receives per `codemode.mode`, scripts calling the agent loop's tools through the
//! session's hooks, the nested calls recorded on the tool result, the branch-scoped `store()`, the
//! output budget, and the `models` namespace over the session's own registry.
//!
//! Ported from pi's `test/agent-session-codemode.test.ts` @v1.0.1 (845 lines), case by case: the
//! upstream test names are kept recognisable. The one thing that differs is the engine: a script is
//! a Rust closure run by [`cyrup_codemode_runtime::testkit::ScriptedSandboxFactory`], so what is
//! under test is the tool, the extension, the host and the session, with the sandbox's own
//! behaviour (QuickJS there, V8 here) pinned in its own crate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::rendered_prompt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_codemode::types::OutputItem;
use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_codemode_runtime::testkit::{ScriptEnv, ScriptedSandboxFactory, completed, failed};
use cyrup_codemode_runtime::tool::CodemodeToolDetails;
use cyrup_codemode_runtime::types::{CodemodeResult, CodemodeStoreWrites, ErrorKind};
use cyrup_config::{InMemorySettingsStore, SettingsScope};
use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink, Usage,
};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Provider, ToolDef};
use futures::FutureExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

// ------------------------------------------------------------------------------------ fixtures --

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

/// A tool answering with fixed text, and optionally structured content and usage.
struct Fixed {
    name: &'static str,
    description: &'static str,
    params: Value,
    output_schema: Option<Value>,
    text: String,
    structured: Option<Value>,
    usage: Option<Usage>,
    exposure: cyrup_core::ToolExposure,
    namespace: Option<cyrup_core::ToolNamespace>,
    images: bool,
}

impl Fixed {
    fn new(name: &'static str, description: &'static str, text: &str) -> Self {
        Self {
            name,
            description,
            params: json!({ "type": "object", "properties": { "text": { "type": "string", "description": "Text to echo" } } }),
            output_schema: None,
            text: text.to_owned(),
            structured: None,
            usage: None,
            exposure: cyrup_core::ToolExposure::Direct,
            namespace: None,
            images: false,
        }
    }
    fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for Fixed {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        self.description
    }
    fn output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
    fn exposure(&self) -> cyrup_core::ToolExposure {
        self.exposure
    }
    fn namespace(&self) -> Option<&cyrup_core::ToolNamespace> {
        self.namespace.as_ref()
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let text = match params.get("text").and_then(Value::as_str) {
            Some(text) if self.name == "echo" => format!("echo: {text}"),
            _ => self.text.clone(),
        };
        let mut content = vec![Content::text(text)];
        if self.images {
            content.push(Content::Image {
                data: TINY_PNG.to_owned(),
                mime_type: "image/png".to_owned(),
            });
        }
        Ok(ToolResult {
            content,
            structured_content: self.structured.clone(),
            usage: self.usage.clone(),
            ..ToolResult::default()
        })
    }
}

fn echo() -> Arc<dyn Tool> {
    Fixed::new("echo", "Echo text back.\n\nSecond paragraph.", "").arc()
}

fn stats() -> Arc<dyn Tool> {
    let mut stats = Fixed::new("stats", "Return structured stats", "2 files");
    stats.output_schema = Some(json!({
        "type": "object",
        "properties": { "files": { "type": "number" }, "names": { "type": "array", "items": { "type": "string" } } },
        "required": ["files", "names"]
    }));
    stats.structured = Some(json!({ "files": 2, "names": ["a", "b"] }));
    stats.arc()
}

fn screenshot() -> Arc<dyn Tool> {
    let mut shot = Fixed::new("screenshot", "Return a screenshot", "captured");
    shot.images = true;
    shot.arc()
}

fn usage(input: u64, cost: f64) -> Usage {
    let mut usage = Usage {
        input,
        total_tokens: input,
        ..Usage::default()
    };
    usage.cost.total = cost;
    usage
}

/// Registers its tools at `init` — the way an extension's `registerTool` calls do — and optionally
/// the hooks upstream's "routes nested calls through extension hooks" installs.
struct ToolsExt {
    tools: Vec<Arc<dyn Tool>>,
    providers: Vec<(&'static str, Arc<dyn Provider>)>,
    /// Block a `tool_call` of this tool whose `text` argument is `forbidden`.
    block_forbidden_echo: bool,
    /// Replace the content of the `stats` tool's result.
    redact_stats: bool,
    /// Replace the content AND the structured content of the `stats` tool's result.
    replace_stats: bool,
}

impl ToolsExt {
    fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self {
            tools,
            providers: Vec::new(),
            block_forbidden_echo: false,
            redact_stats: false,
            replace_stats: false,
        }
    }
}

#[async_trait::async_trait]
impl NativeExtension for ToolsExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("codemode-test-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for tool in &self.tools {
            api.register_tool(Arc::clone(tool));
        }
        for (id, provider) in &self.providers {
            api.register_provider_live(*id, Arc::clone(provider));
        }
        api.subscribe(&[EventKind::ToolCall, EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolCall { name, input, .. }
                if self.block_forbidden_echo
                    && name == "echo"
                    && input.get("text").and_then(Value::as_str) == Some("forbidden") =>
            {
                HookOutcome::Block {
                    reason: Some("echo of forbidden text is blocked".to_owned()),
                    terminate: cyrup_core::TerminateHint::Unspecified,
                }
            }
            HostEvent::ToolResult { name, .. } if self.redact_stats && name == "stats" => {
                HookOutcome::Mutate(EventPatch::ToolResult {
                    content: Some(vec![Content::text("redacted")]),
                    details: None,
                    structured_content: None,
                    is_error: None,
                    usage: None,
                    terminate: None,
                })
            }
            HostEvent::ToolResult { name, .. } if self.replace_stats && name == "stats" => {
                HookOutcome::Mutate(EventPatch::ToolResult {
                    content: Some(vec![Content::text("0 files")]),
                    details: None,
                    structured_content: Some(Box::new(json!({ "files": 0, "names": [] }))),
                    is_error: None,
                    usage: None,
                    terminate: None,
                })
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// A second `tool_result` handler that only touches `details`, like upstream's: it must keep what
/// the handler before it set.
struct AuditExt;

#[async_trait::async_trait]
impl NativeExtension for AuditExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("codemode-audit-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::ToolResult { name, .. } if name == "stats" => {
                HookOutcome::Mutate(EventPatch::ToolResult {
                    content: None,
                    details: Some(json!({ "audited": true })),
                    structured_content: None,
                    is_error: None,
                    usage: None,
                    terminate: None,
                })
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// What the provider received on each request: the declarations and the system prompt.
type Requests = Arc<Mutex<Vec<(Vec<ToolDef>, String)>>>;

type Script = cyrup_codemode_runtime::testkit::Script;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(String, ScriptEnv) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = CodemodeResult> + Send + 'static,
{
    Arc::new(move |code, env| f(code, env).boxed())
}

fn no_script() -> Script {
    script(|_code, _env| async { completed(Vec::new(), None) })
}

struct Rig {
    session: Arc<AgentSession>,
    faux: Arc<FauxProvider>,
    factory: ScriptedSandboxFactory,
    requests: Requests,
    events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    /// The events of the last prompt, in order (`events` is emptied when a prompt returns).
    last_run: Mutex<Vec<AgentSessionEvent>>,
    _fx: Fixture,
}

#[derive(Default)]
struct Options {
    /// A global `settings.json` document.
    settings: Option<&'static str>,
    ext: Option<ToolsExt>,
    /// Native extensions loaded after `ext`, in order.
    also: Vec<Arc<dyn NativeExtension>>,
}

async fn rig(script: Script, options: Options) -> Rig {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let factory = ScriptedSandboxFactory::new(script);
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    // `codemode` is a built-in of the path tier `--no-extensions` collapses, so it is loaded here.
    cfg.no_extensions = false;
    let store = Arc::new(InMemorySettingsStore::new());
    if let Some(settings) = options.settings {
        store.seed(SettingsScope::Global, settings);
    }
    let ext = options.ext.unwrap_or_else(|| ToolsExt::new(Vec::new()));
    let mut builder = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .settings_store(store)
        .with_codemode(CodemodeExtension::new(
            Default::default(),
            Arc::new(factory.clone()),
        ))
        .with_native_extension(Arc::new(ext));
    for extension in options.also {
        builder = builder.with_native_extension(extension);
    }
    let session = builder.build().await.unwrap().into_shared();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = session.subscribe();
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        while let Some(event) = stream.next().await {
            sink.lock().unwrap().push(event);
        }
    });
    Rig {
        session,
        faux,
        factory,
        requests: Arc::new(Mutex::new(Vec::new())),
        events,
        last_run: Mutex::new(Vec::new()),
        _fx: fx,
    }
}

fn steps(requests: &Requests, replies: Vec<Option<Value>>) -> Vec<FauxResponseStep> {
    replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock()
                    .unwrap()
                    .push((ctx.tools.clone(), rendered_prompt(ctx)));
                match &reply {
                    Some(args) => faux_assistant_message(
                        vec![faux_tool_call("codemode".to_string(), args.clone())],
                        StopReason::ToolUse,
                    ),
                    None => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
                }
            })
        })
        .collect()
}

impl Rig {
    async fn set_active(&self, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        self.session.set_active_tools_by_name(&names).await;
    }

    /// One prompt whose model reply calls `codemode` with `code`, then answers.
    async fn run(&self, code: &str) -> Message {
        self.faux.set_response_steps(steps(
            &self.requests,
            vec![Some(json!({ "code": code })), None],
        ));
        self.prompt("go").await;
        self.codemode_result().await
    }

    /// One prompt whose reply is plain text (the provider sees the request, nothing runs).
    async fn ask(&self, text: &str) {
        self.faux
            .set_response_steps(steps(&self.requests, vec![None]));
        self.prompt(text).await;
    }

    async fn prompt(&self, text: &str) {
        let _ = self.session.prompt(text).await.unwrap();
        self.session.wait_for_idle().await;
        // The collector drains on its own task: wait for it to reach the run's last event.
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(10)
            && !self
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, AgentSessionEvent::AgentSettled))
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        *self.last_run.lock().unwrap() = std::mem::take(&mut *self.events.lock().unwrap());
    }

    async fn codemode_result(&self) -> Message {
        self.session
            .messages()
            .await
            .into_iter()
            .rev()
            .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "codemode"))
            .expect("a codemode tool result")
    }

    fn request_tools(&self, index: usize) -> Vec<ToolDef> {
        self.requests.lock().unwrap()[index].0.clone()
    }

    fn request_prompt(&self, index: usize) -> String {
        self.requests.lock().unwrap()[index].1.clone()
    }
}

fn description(tools: &[ToolDef], name: &str) -> String {
    tools
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.description.clone())
        .unwrap_or_default()
}

fn tool_names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}

/// The output after the script header, which is checked on the way (upstream's `resultText`).
fn result_text(message: &Message) -> String {
    let Message::ToolResult { content, .. } = message else {
        panic!("not a tool result");
    };
    let Some(Content::Text { text: header, .. }) = content.first() else {
        panic!("no header");
    };
    let header = header.to_string();
    assert!(
        (header.starts_with("Script completed\nWall time ")
            || header.starts_with("Script failed\nWall time "))
            && header.ends_with(" seconds\nOutput:\n"),
        "bad header {header:?}"
    );
    content[1..]
        .iter()
        .map(|block| match block {
            Content::Text { text, .. } => text.to_string(),
            Content::Image { .. } => "<image>".to_owned(),
            other => format!("<{other:?}>"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(message: &Message) -> bool {
    matches!(message, Message::ToolResult { is_error: true, .. })
}

fn details(message: &Message) -> CodemodeToolDetails {
    let Message::ToolResult { details, .. } = message else {
        panic!("not a tool result");
    };
    serde_json::from_value(details.clone().expect("details")).unwrap()
}

fn text(s: &str) -> OutputItem {
    OutputItem::Text(s.to_owned())
}

// ------------------------------------------------------------------------ AgentSession codemode --

/// Upstream `presents callable tools per codemode.mode`, `on` half: declared tools say how scripts
/// call them and are not listed again in codemode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presents_callable_tools_per_codemode_mode_on() {
    let rig = rig(
        no_script(),
        Options {
            ext: Some(ToolsExt::new(vec![echo(), stats(), screenshot()])),
            ..Options::default()
        },
    )
    .await;

    rig.set_active(&["read", "echo", "codemode"]).await;
    rig.ask("on").await;

    let tools = rig.request_tools(0);
    let names = tool_names(&tools);
    assert!(names.contains(&"read") && names.contains(&"echo") && names.contains(&"codemode"));
    assert!(description(&tools, "echo").contains("Codemode: `tools.echo(args)` resolves to"));
    assert!(!description(&tools, "echo").contains("codemode tool declaration:"));
    let codemode = description(&tools, "codemode");
    assert!(!codemode.contains("### `echo`"));
    assert!(
        !codemode.contains("### `stats`"),
        "an inactive direct tool is not callable"
    );
    assert!(rig.request_prompt(0).contains("\n- read: "));
    // The description announces `models`, which the session's own tool declares.
    assert!(codemode.contains("`models`: classifiers and image generation"));
    assert!(codemode.contains("docs/codemode.md"));
}

/// Upstream `presents callable tools per codemode.mode`, `only` half: codemode lists echo, which
/// stays active but is left out of requests; the prompt's tool list matches the declarations
/// (#10192); and without codemode, tools keep their plain descriptions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presents_callable_tools_per_codemode_mode_only() {
    let rig = rig(
        no_script(),
        Options {
            settings: Some(r#"{ "codemode": { "mode": "only" } }"#),
            ext: Some(ToolsExt::new(vec![echo(), stats(), screenshot()])),
            ..Options::default()
        },
    )
    .await;

    rig.set_active(&["read", "echo", "codemode"]).await;
    rig.ask("only").await;

    let tools = rig.request_tools(0);
    let names = tool_names(&tools);
    assert!(names.contains(&"codemode"));
    assert!(!names.contains(&"echo"), "echo is left out of requests");
    assert!(!names.contains(&"read"), "so is read");
    assert!(!description(&tools, "echo").contains("Codemode: `tools.echo"));
    let codemode = description(&tools, "codemode");
    assert!(codemode.contains("### `echo`"));
    assert!(codemode.contains("### `read`"));
    assert!(!codemode.contains("### `stats`"));
    // The prompt's tool list matches the declarations: hidden tools are not listed.
    assert!(!rig.request_prompt(0).contains("\n- read: "));
    assert!(rig.request_prompt(0).contains("\n- codemode: "));
    assert!(!rig.session.base_system_prompt().contains("\n- read: "));
    // They stay active.
    let active = rig.session.active_tool_names();
    assert!(active.contains(&"echo".to_owned()) && active.contains(&"read".to_owned()));

    // Without codemode, tools keep their plain descriptions.
    rig.set_active(&["echo"]).await;
    rig.ask("plain").await;
    assert_eq!(
        description(&rig.request_tools(1), "echo"),
        "Echo text back.\n\nSecond paragraph."
    );
}

/// CODE-020 (pi `c30840c2e` @v1.0.4, #10343). In `only` mode `read` stays active but its declaration
/// is left out of requests, so the system prompt must not give the model rules for a tool it can
/// reach only through codemode: its guideline moves from the rules to its codemode section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guidelines_of_hidden_tools_move_from_the_rules_to_their_codemode_sections() {
    let rig = rig(
        no_script(),
        Options {
            settings: Some(r#"{ "codemode": { "mode": "only" } }"#),
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;

    rig.set_active(&["read", "echo", "codemode"]).await;
    rig.ask("only").await;

    let prompt = rig.request_prompt(0);
    assert!(
        !prompt.contains("Use read to examine files"),
        "a hidden tool's guideline is not a rule of the prompt: {prompt}"
    );
    assert!(
        !rig.session
            .base_system_prompt()
            .contains("Use read to examine files"),
        "nor of the stored base prompt"
    );
    // The model meets the guideline where it meets the tool.
    assert!(
        description(&rig.request_tools(0), "codemode")
            .contains("- Use read to examine files instead of cat or sed."),
        "{}",
        description(&rig.request_tools(0), "codemode")
    );
}

/// The settings-driven mode, as `codemode.mode` values pi's extension reads them: anything but
/// `"only"` is `on`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_codemode_mode_reads_as_on() {
    let rig = rig(
        no_script(),
        Options {
            settings: Some(r#"{ "codemode": { "mode": "everything" } }"#),
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["echo", "codemode"]).await;
    rig.ask("go").await;
    assert!(tool_names(&rig.request_tools(0)).contains(&"echo"));
}

/// The description stays identical when `tool_search`-style activation changes the active set, and
/// deferred tools are never listed (`tool.ts:324-330`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_description_stays_identical_when_activation_changes_the_active_set() {
    let mut deferred = Fixed::new("mcp__github__issue", "Open an issue.", "");
    deferred.exposure = cyrup_core::ToolExposure::Deferred;
    deferred.namespace = Some(cyrup_core::ToolNamespace {
        name: "mcp__github".into(),
        description: Some("GitHub server".into()),
        instructions: None,
    });
    let rig = rig(
        no_script(),
        Options {
            ext: Some(ToolsExt::new(vec![echo(), deferred.arc()])),
            ..Options::default()
        },
    )
    .await;

    rig.set_active(&["read", "echo", "codemode"]).await;
    rig.ask("before").await;
    // `tool_search` loads the deferred tool: naming it in the active set is the activation.
    rig.set_active(&["read", "echo", "codemode", "mcp__github__issue"])
        .await;
    rig.ask("after").await;

    let before = description(&rig.request_tools(0), "codemode");
    let after_tools = rig.request_tools(1);
    assert!(
        tool_names(&after_tools).contains(&"mcp__github__issue"),
        "declared once activated"
    );
    assert_eq!(before, description(&after_tools, "codemode"));
    assert!(
        !before.contains("mcp__github"),
        "deferred tools are never listed"
    );
    // Deferred tools stay callable.
    assert!(
        rig.session
            .callable_tool_names()
            .contains(&"mcp__github__issue".to_owned())
    );
}

/// `codemode` is `model-only`: a script cannot call it, and it never lists itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codemode_is_not_callable_from_itself() {
    let rig = rig(
        script(|_code, env| async move {
            let nested = env.tool("echo", json!({ "text": "x" })).await.unwrap();
            completed(Vec::new(), nested)
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo"]).await;
    rig.run("go").await;

    assert!(
        !rig.session
            .callable_tool_names()
            .contains(&"codemode".to_owned())
    );
    let seen = rig.factory.runs();
    assert_eq!(
        seen[0].tool_names,
        ["echo"],
        "the sandbox is given no `codemode` tool"
    );
    // And a nested call that names it is refused by the pipeline, not run.
    let call = ToolCallId::from("caller");
    let outcome = rig
        .session
        .execute_nested_tool(
            &call,
            "codemode",
            json!({ "code": "return 1" }),
            cyrup_agent::NestedToolCallOptions::default(),
        )
        .await;
    assert!(outcome.is_error);
}

/// Upstream `runs nested calls in parallel and returns only the script result`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_nested_calls_in_parallel_and_returns_only_the_script_result() {
    let rig = rig(
        script(|_code, env| async move {
            let (a, b, stats) = tokio::join!(
                env.tool("echo", json!({ "text": "one" })),
                env.tool("echo", json!({ "text": "two" })),
                env.tool("stats", json!({})),
            );
            let stats = stats.unwrap().unwrap();
            completed(
                vec![text(&format!("files {}", stats["files"]))],
                Some(json!({ "a": a.unwrap(), "b": b.unwrap(), "names": stats["names"] })),
            )
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo(), stats(), screenshot()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo", "stats"]).await;

    let result = rig.run("go").await;

    assert!(!is_error(&result));
    assert_eq!(
        result_text(&result),
        "files 2\n{\"a\":\"echo: one\",\"b\":\"echo: two\",\"names\":[\"a\",\"b\"]}"
    );
    let Message::ToolResult { tool_call_id, .. } = &result else {
        panic!()
    };
    let details = details(&result);
    assert_eq!(
        details
            .calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "echo", "stats"]
    );
    assert!(
        details
            .calls
            .iter()
            .all(|c| c.status == cyrup_codemode_runtime::tool::CodemodeNestedCallStatus::Ok)
    );
    assert!(
        details
            .calls
            .iter()
            .all(|c| c.id.starts_with(&format!("{tool_call_id}/")))
    );
    // Nested calls never become transcript tool results.
    let tool_results = rig
        .session
        .messages()
        .await
        .into_iter()
        .filter(|m| matches!(m, Message::ToolResult { .. }))
        .count();
    assert_eq!(tool_results, 1);
    // The sandbox was given the callable tools: the active `direct` ones, but not `screenshot`,
    // which is registered and inactive.
    assert_eq!(rig.factory.runs()[0].tool_names, ["echo", "stats"]);
}

/// Nested calls are recorded as `nestedCalls` on the tool result (`agent-session.ts:1075-1082`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_calls_are_recorded_as_nested_calls_on_the_tool_result() {
    let rig = rig(
        script(|_code, env| async move {
            env.tool("echo", json!({ "text": "a" })).await.unwrap();
            env.tool("echo", json!({ "text": "b" })).await.unwrap();
            completed(Vec::new(), None)
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo"]).await;
    let result = rig.run("go").await;
    let Message::ToolResult {
        tool_call_id,
        nested_calls,
        ..
    } = &result
    else {
        panic!()
    };
    let nested = nested_calls.clone().expect("nestedCalls");
    assert!(nested.complete);
    assert_eq!(
        nested
            .calls
            .iter()
            .map(|c| (c.id.clone(), c.name.clone(), c.arguments.clone().unwrap()))
            .collect::<Vec<_>>(),
        [
            (
                format!("{tool_call_id}/1"),
                "echo".to_owned(),
                json!({ "text": "a" }).as_object().unwrap().clone()
            ),
            (
                format!("{tool_call_id}/2"),
                "echo".to_owned(),
                json!({ "text": "b" }).as_object().unwrap().clone()
            ),
        ]
    );
}

/// The LIVE `message_end` of a tool result carries what the persisted row carries (CODE-006): the
/// calls the tool made and what they spent. A subscriber of the live event, such as the terminal
/// renderer of a `codemode` result, reads `nestedCalls` from it; only compaction, export and a
/// resumed session read the row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_message_end_of_a_nested_calling_tool_carries_the_record_and_the_usage() {
    let mut billed = Fixed::new("billed", "Run a model", "ran");
    billed.usage = Some(usage(100, 0.25));
    let rig = rig(
        script(|_code, env| async move {
            env.tool("echo", json!({ "text": "a" })).await.unwrap();
            env.tool("billed", json!({})).await.unwrap();
            completed(Vec::new(), None)
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo(), billed.arc()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo", "billed"]).await;

    let persisted = rig.run("go").await;
    let Message::ToolResult {
        nested_calls: persisted_calls,
        usage: persisted_usage,
        ..
    } = &persisted
    else {
        panic!("not a tool result")
    };
    let persisted_calls = persisted_calls.clone().expect("the persisted record");
    let persisted_usage = persisted_usage.clone().expect("the persisted usage");

    let live: Vec<cyrup_agent::ToolResultMessage> = rig
        .last_run
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentSessionEvent::MessageEnd {
                message: cyrup_agent::AgentMessage::ToolResult(result),
            } => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 1, "one tool result ended: {live:#?}");
    let live = &live[0];
    assert_eq!(live.tool_name, "codemode");
    assert_eq!(
        live.nested_calls.as_ref(),
        Some(&persisted_calls),
        "the live message carries the persisted record"
    );
    assert_eq!(live.nested_calls.as_ref().map(|n| n.calls.len()), Some(2));
    assert_eq!(live.usage.as_ref(), Some(&persisted_usage));
    assert_eq!(
        live.usage.as_ref().map(|u| u.input),
        Some(100),
        "the summed usage of the nested calls"
    );
}

/// pi's `{ name: "codemode", factory, replaceable: true, builtin: true }`
/// (`extensions/index.ts:11-12` @v1.0.1): an extension that registers a tool named `codemode`
/// replaces the built-in instead of colliding with it. The session is built the normal way, with
/// the built-in attached first; the other extension's tool is the one the provider is offered and
/// the one a model call runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_registering_codemode_replaces_the_builtin() {
    let mine = Fixed::new(
        "codemode",
        "The other extension's own codemode.",
        "mine ran",
    )
    .arc();
    let rig = rig(
        script(|_code, _env| async { panic!("the built-in's sandbox must not run") }),
        Options {
            ext: Some(ToolsExt::new(vec![mine])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode"]).await;

    let result = rig.run("go").await;

    let tools = rig.request_tools(0);
    assert_eq!(
        description(&tools, "codemode"),
        "The other extension's own codemode.",
        "the provider is offered the replacement, not the built-in's description"
    );
    assert!(
        rig.factory.runs().is_empty(),
        "no script ran in the built-in's sandbox"
    );
    let Message::ToolResult {
        content, is_error, ..
    } = &result
    else {
        panic!("not a tool result")
    };
    assert!(!is_error, "{content:?}");
    assert_eq!(
        content.iter().find_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        }),
        Some("mine ran".to_owned()),
        "the model's call ran the replacement"
    );
    // Reported as a warning, not as the fatal collision the registry otherwise records.
    let diagnostics = &rig.session.services().startup_diagnostics.extensions;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert!(!diagnostics[0].fatal);
    assert_eq!(diagnostics[0].path, PathBuf::from("codemode"));
    assert!(
        diagnostics[0]
            .error
            .contains("registers tool `codemode`, so built-in extension `codemode` was not loaded"),
        "{}",
        diagnostics[0].error
    );
}

/// An extension registering an unrelated tool leaves the built-in loaded: both are offered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_registering_an_unrelated_tool_leaves_the_builtin_in_place() {
    let rig = rig(
        no_script(),
        Options {
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo"]).await;
    rig.ask("go").await;
    let tools = rig.request_tools(0);
    let names = tool_names(&tools);
    assert!(
        names.contains(&"codemode") && names.contains(&"echo"),
        "{names:?}"
    );
    assert!(description(&tools, "codemode").starts_with("Run JavaScript that calls other tools."));
    assert!(
        rig.session
            .services()
            .startup_diagnostics
            .extensions
            .is_empty()
    );
}

/// Upstream `routes nested calls through extension hooks`: a blocked call rejects with the hook's
/// reason; a `tool_result` handler's replacement is what the script sees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routes_nested_calls_through_extension_hooks() {
    let mut ext = ToolsExt::new(vec![echo(), stats()]);
    ext.block_forbidden_echo = true;
    ext.redact_stats = true;
    let rig = rig(
        script(|_code, env| async move {
            let blocked = env.tool("echo", json!({ "text": "forbidden" })).await;
            let stats = env.tool("stats", json!({})).await;
            completed(
                Vec::new(),
                Some(json!({ "blocked": blocked.unwrap_err(), "stats": stats.unwrap() })),
            )
        }),
        Options {
            ext: Some(ext),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo", "stats"]).await;

    let result = rig.run("go").await;

    // Replacing content without replacing structured content drops the structured result.
    assert_eq!(
        serde_json::from_str::<Value>(&result_text(&result)).unwrap(),
        json!({ "blocked": "echo of forbidden text is blocked", "stats": "redacted" })
    );
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.status)
            .collect::<Vec<_>>(),
        [
            cyrup_codemode_runtime::tool::CodemodeNestedCallStatus::Error,
            cyrup_codemode_runtime::tool::CodemodeNestedCallStatus::Ok
        ]
    );
}

/// Upstream `keeps structured content that tool_result handlers replace along with the content`:
/// the first handler replaces both, a later one that only touches `details` keeps what it set, and
/// the script receives the replacement structured content, not the text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keeps_structured_content_that_tool_result_handlers_replace_along_with_the_content() {
    let mut ext = ToolsExt::new(vec![echo(), stats()]);
    ext.replace_stats = true;
    let rig = rig(
        script(|_code, env| async move {
            let stats = env.tool("stats", json!({})).await;
            completed(Vec::new(), stats.unwrap())
        }),
        Options {
            ext: Some(ext),
            also: vec![Arc::new(AuditExt)],
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "stats"]).await;

    let result = rig.run("return await tools.stats({});").await;

    assert_eq!(
        serde_json::from_str::<Value>(&result_text(&result)).unwrap(),
        json!({ "files": 0, "names": [] })
    );
}

/// Upstream `adds the usage of nested results to the codemode result`: the usage is persisted with
/// the result, so session totals count it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adds_the_usage_of_nested_results_to_the_codemode_result() {
    let mut billed = Fixed::new("billed", "Run a model", "ran");
    billed.usage = Some(usage(100, 0.25));
    let rig = rig(
        script(|_code, env| async move {
            env.tool("billed", json!({})).await.unwrap();
            env.tool("billed", json!({})).await.unwrap();
            env.tool("echo", json!({ "text": "x" })).await.unwrap();
            completed(Vec::new(), None)
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo(), billed.arc()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo", "billed"]).await;

    let result = rig.run("go").await;

    let Message::ToolResult { usage, .. } = &result else {
        panic!()
    };
    let usage = usage.clone().expect("usage");
    assert_eq!((usage.input, usage.total_tokens), (200, 200));
    assert!((usage.cost.total - 0.5).abs() < 1e-10);
    // The usage is persisted with the result, so session totals count it.
    let stats = rig.session.session_stats().await;
    assert!((stats.cost - 0.5).abs() < 1e-10, "{}", stats.cost);
}

/// Upstream `attaches only the images the script passes to image(), in output order`: output items
/// keep their order and the image block reaches the persisted result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attaches_only_the_images_the_script_passes_to_image_in_output_order() {
    let rig = rig(
        script(|_code, env| async move {
            // Tools without an outputSchema resolve to their text; images are not passed on.
            let shot = env.tool("screenshot", json!({})).await.unwrap().unwrap();
            completed(
                vec![
                    OutputItem::Text(shot.as_str().unwrap().to_owned()),
                    OutputItem::Image {
                        data: TINY_PNG.to_owned(),
                        mime_type: "image/png".to_owned(),
                    },
                    text("after"),
                ],
                None,
            )
        }),
        Options {
            ext: Some(ToolsExt::new(vec![screenshot()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "screenshot"]).await;
    let result = rig.run("go").await;
    assert_eq!(result_text(&result), "captured\n<image>\nafter");
    let Message::ToolResult { content, .. } = &result else {
        panic!()
    };
    assert_eq!(
        content[2],
        Content::Image {
            data: TINY_PNG.to_owned(),
            mime_type: "image/png".to_owned()
        }
    );
}

/// Upstream `reports script failures as results that keep partial output and the calls that ran`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_script_failures_as_results_that_keep_partial_output_and_the_calls_that_ran() {
    let rig = rig(
        script(|_code, env| async move {
            env.tool("echo", json!({ "text": "x" })).await.unwrap();
            failed(
                ErrorKind::Script,
                "boom",
                Some("Error: boom\n    at codemode.js:3:7"),
                vec![text("partial")],
            )
        }),
        Options {
            ext: Some(ToolsExt::new(vec![echo()])),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode", "echo"]).await;
    let result = rig.run("go").await;

    assert!(is_error(&result));
    let Message::ToolResult { content, .. } = &result else {
        panic!()
    };
    let Content::Text { text: header, .. } = &content[0] else {
        panic!()
    };
    assert!(header.starts_with("Script failed\n"));
    let body = result_text(&result);
    assert!(
        body.starts_with("partial\nScript error:\nError: boom\n"),
        "{body}"
    );
    assert!(body.contains("codemode.js:3"));
    assert!(body.contains("Tool calls made before the failure (they are not undone): echo (ok)"));
    assert_eq!(
        details(&result)
            .calls
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["echo"]
    );
}

// ----------------------------------------------------------------------- options and the store --

fn counting_script() -> Script {
    // `const next = (load("count") ?? 0) + 1; store("count", next); return next;`
    script(|code, env| async move {
        if code.contains("fail") {
            return CodemodeResult::Failed {
                error: cyrup_codemode_runtime::types::CodemodeError {
                    kind: ErrorKind::Script,
                    name: None,
                    message: "boom".to_owned(),
                    stack: None,
                },
                output: Vec::new(),
                calls: Vec::new(),
            };
        }
        if code.contains("unset") {
            let mut writes = CodemodeStoreWrites::default();
            writes.delete.push("count".to_owned());
            let was_unset = env.store.get("count").is_none();
            return CodemodeResult::Completed {
                value: Some(json!(was_unset)),
                output: Vec::new(),
                calls: Vec::new(),
                store_writes: writes,
            };
        }
        if code.contains("read-only") {
            return completed(
                Vec::new(),
                Some(env.store.get("count").cloned().unwrap_or(json!("missing"))),
            );
        }
        let next = env.store.get("count").and_then(Value::as_i64).unwrap_or(0) + 1;
        let mut writes = CodemodeStoreWrites::default();
        writes.set.insert("count".to_owned(), json!(next));
        CodemodeResult::Completed {
            value: Some(json!(next)),
            output: Vec::new(),
            calls: Vec::new(),
            store_writes: writes,
        }
    })
}

async fn store_entries(rig: &Rig) -> Vec<Value> {
    rig.session
        .entries_json()
        .await
        .into_iter()
        .filter(|entry| entry["type"] == "custom" && entry["customType"] == "codemode-store")
        .map(|entry| entry["data"].clone())
        .collect()
}

/// Upstream `persists store() writes as custom entries for later calls`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persists_store_writes_as_custom_entries_for_later_calls() {
    let rig = rig(counting_script(), Options::default()).await;
    rig.set_active(&["codemode"]).await;

    assert_eq!(result_text(&rig.run("increment").await), "1");
    assert_eq!(result_text(&rig.run("increment").await), "2");
    assert_eq!(
        store_entries(&rig).await,
        [
            json!({ "set": { "count": 1 }, "delete": [] }),
            json!({ "set": { "count": 2 }, "delete": [] }),
        ]
    );
    // `entry_appended` reached the session's subscribers.
    // (The collector is cleared per prompt, so count through the persisted entries above.)

    assert_eq!(result_text(&rig.run("unset").await), "false");
    assert_eq!(
        store_entries(&rig).await.last().cloned(),
        Some(json!({ "set": {}, "delete": ["count"] }))
    );
    assert_eq!(result_text(&rig.run("read-only").await), "missing");
}

/// The `entry_appended` event of a `store()` write reaches subscribers (upstream asserts two
/// events for two writing scripts).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_write_announces_the_entry_to_subscribers() {
    let rig = rig(counting_script(), Options::default()).await;
    rig.set_active(&["codemode"]).await;
    rig.faux.set_response_steps(steps(
        &rig.requests,
        vec![Some(json!({ "code": "increment" })), None],
    ));
    let _ = rig.session.prompt("go").await.unwrap();
    rig.session.wait_for_idle().await;
    let started = std::time::Instant::now();
    let appended = loop {
        let appended = rig
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(event, AgentSessionEvent::EntryAppended { entry }
                    if entry["type"] == "custom" && entry["customType"] == "codemode-store")
            })
            .count();
        if appended > 0 || started.elapsed() > std::time::Duration::from_secs(10) {
            break appended;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    };
    assert_eq!(appended, 1);
}

/// Upstream `appends nothing for failed scripts or scripts without writes`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn appends_nothing_for_failed_scripts_or_scripts_without_writes() {
    let rig = rig(counting_script(), Options::default()).await;
    rig.set_active(&["codemode"]).await;
    assert!(is_error(&rig.run("fail").await));
    assert!(!is_error(&rig.run("read-only").await));
    assert!(store_entries(&rig).await.is_empty());
}

/// Upstream `loads the values written on the current branch`: branch from the first prompt; the
/// store entries written after it are on another path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loads_the_values_written_on_the_current_branch() {
    let rig = rig(counting_script(), Options::default()).await;
    rig.set_active(&["codemode"]).await;
    assert_eq!(result_text(&rig.run("increment").await), "1");
    let first_prompt = rig.session.user_messages_for_forking().await[0]
        .entry_id
        .clone();
    assert_eq!(result_text(&rig.run("increment").await), "2");

    // Branch from the first prompt: the store entries written after it are on another path.
    rig.session.branch(first_prompt).await.unwrap();
    assert_eq!(result_text(&rig.run("increment").await), "1");

    // The sandbox was asked with exactly the branch's values each time.
    let stores: Vec<Value> = rig
        .factory
        .runs()
        .iter()
        .map(|run| Value::Object(run.store.clone()))
        .collect();
    assert_eq!(stores, [json!({}), json!({ "count": 1 }), json!({})]);
}

/// Upstream `truncates output to the token budget and spills the full text`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncates_output_to_the_token_budget_and_spills_the_full_text() {
    let rows: Vec<OutputItem> = (0..100).map(|i| text(&format!("row {i}"))).collect();
    let rig = rig(
        script(move |_code, _env| {
            let mut output = rows.clone();
            output.push(OutputItem::Image {
                data: TINY_PNG.to_owned(),
                mime_type: "image/png".to_owned(),
            });
            async move { completed(output, None) }
        }),
        Options::default(),
    )
    .await;
    rig.set_active(&["codemode"]).await;

    let result = rig
        .run("// @options: {\"max_output_tokens\": 10}\nrows")
        .await;

    let path = details(&result).full_output_path.expect("a spill file");
    let body = result_text(&result);
    assert!(body.starts_with("Warning: truncated output"));
    assert!(
        body.contains("row 0\n") && body.contains("tokens truncated") && body.contains("row 99\n")
    );
    assert!(!body.contains("row 50\n"));
    assert!(body.contains(&format!("[Full output: {path} (read with offset/limit)]")));
    let Message::ToolResult { content, .. } = &result else {
        panic!()
    };
    // Images follow the truncated text.
    assert!(matches!(content.last(), Some(Content::Image { .. })));
    let full = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        full,
        (0..100)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Upstream `applies the timeout_ms option and rejects invalid options` (the tool's side): the
/// deadline reaches the sandbox, and an invalid options line is the result's whole text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_the_timeout_ms_option_and_rejects_invalid_options() {
    let rig = rig(
        script(|_code, _env| async {
            failed(
                ErrorKind::Timeout,
                "the script ran past its 200 ms deadline",
                None,
                Vec::new(),
            )
        }),
        Options::default(),
    )
    .await;
    rig.set_active(&["codemode"]).await;

    let timed_out = rig
        .run("// @options: {\"timeout_ms\": 200}\nwhile (true) {}")
        .await;
    assert!(is_error(&timed_out));
    assert!(result_text(&timed_out).contains("Script error:\nScript timed out"));
    assert_eq!(
        rig.factory.runs()[0].deadline,
        cyrup_codemode_runtime::types::Deadline::After(std::time::Duration::from_millis(200))
    );

    let invalid = rig.run("// @options: {\"yield\": 1}\ntext(1)").await;
    assert!(is_error(&invalid));
    let Message::ToolResult { content, .. } = &invalid else {
        panic!()
    };
    assert_eq!(
        content,
        &vec![Content::text(
            "@options only supports `max_output_tokens` and `timeout_ms`; got `yield`"
        )]
    );
}

// ----------------------------------------------------------------------------- registration --

/// The extension registers the tool inactive and `model-only` (`index.ts:36-43`,
/// `tool.ts:371`): it is in the registry, not active, not callable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_codemode_extension_registers_the_tool_inactive() {
    let rig = rig(no_script(), Options::default()).await;
    let info = rig
        .session
        .all_tools()
        .into_iter()
        .find(|tool| tool.name == "codemode")
        .expect("codemode is registered");
    assert!(!info.active, "registered inactive");
    assert_eq!(info.exposure, cyrup_core::ToolExposure::ModelOnly);
    assert!(
        !rig.session
            .active_tool_names()
            .contains(&"codemode".to_owned())
    );
    assert!(
        !rig.session
            .callable_tool_names()
            .contains(&"codemode".to_owned())
    );

    rig.set_active(&["codemode"]).await;
    assert!(
        rig.session
            .active_tool_names()
            .contains(&"codemode".to_owned())
    );
}

/// `--no-extensions` collapses the built-ins' path tier (`package-manager.ts:972-974`), so the tool
/// is not registered at all; the extension is also hidden from the startup listing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_extensions_drops_the_builtin_codemode_extension() {
    let ext = CodemodeExtension::new(
        Default::default(),
        Arc::new(ScriptedSandboxFactory::new(no_script())),
    );
    assert!(
        ext.is_ambient(),
        "a builtin: path is in the tier --no-extensions collapses"
    );
    assert!(
        ext.is_hidden(),
        "builtin extensions are hidden from the startup listing"
    );

    let fx = fixture();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .with_codemode(ext)
        .build()
        .await
        .unwrap();
    assert!(
        session
            .all_tools()
            .iter()
            .all(|tool| tool.name != "codemode")
    );
}

/// The session factory — what the binary launches sessions from — binds each session it builds
/// into the extension's host slot, so the tool runs against the live session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_built_by_the_factory_runs_scripts_against_itself() {
    let fx = fixture();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = false;
    let faux = Arc::new(FauxProvider::new());
    let sandboxes = ScriptedSandboxFactory::new(script(|_code, env| async move {
        let nested = env
            .tool("echo", json!({ "text": "via factory" }))
            .await
            .unwrap();
        completed(Vec::new(), nested)
    }));
    let factory = crate::SessionFactory::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_codemode(CodemodeExtension::new(
            Default::default(),
            Arc::new(sandboxes),
        ))
        .with_native_extension(Arc::new(ToolsExt::new(vec![echo()])));
    let session = factory
        .build(crate::SessionTarget::New, None)
        .await
        .unwrap()
        .into_shared();
    session
        .set_active_tools_by_name(&["codemode".to_owned(), "echo".to_owned()])
        .await;
    faux.set_response_steps(steps(
        &Arc::new(Mutex::new(Vec::new())),
        vec![Some(json!({ "code": "go" })), None],
    ));
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let result = session
        .messages()
        .await
        .into_iter()
        .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "codemode"))
        .unwrap();
    assert!(!is_error(&result));
    assert_eq!(result_text(&result), "echo: via factory");
}

/// The production path end to end: real JavaScript in the V8 sandbox calls a tool through the
/// session's pipeline, writes the store, and the next script on the branch reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_javascript_runs_against_the_session() {
    let fx = fixture();
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = false;
    let faux = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .with_codemode(CodemodeExtension::new(
            Default::default(),
            Arc::new(cyrup_codemode_runtime::tool::EngineSandboxFactory),
        ))
        .with_native_extension(Arc::new(ToolsExt::new(vec![echo(), stats()])))
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .set_active_tools_by_name(&["codemode".to_owned(), "echo".to_owned(), "stats".to_owned()])
        .await;
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let mut results = Vec::new();
    for code in [
        "const s = await tools.stats({});\nstore('seen', s.files);\nreturn [await tools.echo({ text: 'hi' }), s.names];",
        "return load('seen');",
    ] {
        faux.set_response_steps(steps(&requests, vec![Some(json!({ "code": code })), None]));
        let _ = session.prompt("go").await.unwrap();
        session.wait_for_idle().await;
        results.push(
            session
                .messages()
                .await
                .into_iter()
                .rev()
                .find(|m| matches!(m, Message::ToolResult { tool_name, .. } if tool_name == "codemode"))
                .unwrap(),
        );
    }
    assert_eq!(result_text(&results[0]), "[\"echo: hi\",[\"a\",\"b\"]]");
    assert_eq!(result_text(&results[1]), "2");
}

// ------------------------------------------------------------------------------------- models --

/// A provider that lists a classifier and an image model, serves them with fixed credentials, and
/// records what it was called with (a provider an extension registers).
mod models {
    use super::*;
    use cyrup_provider::auth::types::ModelAuth;
    use cyrup_provider::{
        ApiKeyAuth, AssistantImages, AuthContext, AuthError, AuthResult, ClassifierAnswer,
        ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, Context,
        HeaderMap, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, Modality, Model,
        ModelCost, OrderedMap, ProviderAuth, StreamEvent, StreamOptions,
    };

    pub(super) struct Observed {
        /// `(base_url, api_key)` per call.
        pub calls: Mutex<Vec<(String, Option<String>)>>,
    }

    struct SecretAuth;

    #[async_trait::async_trait]
    impl ApiKeyAuth for SecretAuth {
        fn name(&self) -> &str {
            "secret"
        }
        async fn resolve(
            &self,
            _model: &Model,
            _ctx: &dyn AuthContext,
            _cred: Option<&cyrup_provider::Credential>,
        ) -> Result<Option<AuthResult>, AuthError> {
            Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some("secret-key".to_owned()),
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("test".to_owned()),
            }))
        }
    }

    pub(super) struct Scorer {
        id: cyrup_core::ProviderId,
        chat: Vec<Model>,
        classifier: ClassifierModel,
        painter: ImageModel,
        auth: ProviderAuth,
        pub observed: Arc<Observed>,
    }

    impl Scorer {
        pub(super) fn new() -> Self {
            let chat = Model {
                id: "chatty".into(),
                name: "Chatty".into(),
                api: "openai-completions".into(),
                provider: "scorer".into(),
                base_url: "https://chat.test/v1".into(),
                reasoning: false,
                input: vec![Modality::Text],
                cost: ModelCost::default(),
                context_window: 1000,
                max_tokens: 100,
                sampling_params: None,
                thinking_level_map: None,
                compat: None,
                headers: None,
            };
            Self {
                id: "scorer".into(),
                chat: vec![chat],
                classifier: ClassifierModel {
                    id: "judge".into(),
                    name: "Judge".into(),
                    api: "test-classifier".into(),
                    provider: "scorer".into(),
                    base_url: "https://classifier.test/v1".into(),
                    input: vec![Modality::Text],
                    cost: ModelCost::default(),
                    headers: Some(HeaderMap::from([(
                        "X-Secret".to_owned(),
                        Some("hunter2".to_owned()),
                    )])),
                    context_window: 1000,
                },
                painter: ImageModel {
                    id: "painter".into(),
                    name: "Painter".into(),
                    api: "test-images".into(),
                    provider: "scorer".into(),
                    base_url: "https://images.test/v1".into(),
                    input: vec![Modality::Text, Modality::Image],
                    output: vec![Modality::Text, Modality::Image],
                    cost: ModelCost::default(),
                    headers: None,
                },
                auth: ProviderAuth::with_api_key(Arc::new(SecretAuth)),
                observed: Arc::new(Observed {
                    calls: Mutex::new(Vec::new()),
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for Scorer {
        fn id(&self) -> &cyrup_core::ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            &self.chat
        }
        fn get_all_models(&self) -> Vec<cyrup_provider::AnyModel> {
            vec![
                cyrup_provider::AnyModel::Chat(self.chat[0].clone()),
                cyrup_provider::AnyModel::Classifier(self.classifier.clone()),
                cyrup_provider::AnyModel::Image(self.painter.clone()),
            ]
        }
        fn provider_auth(&self) -> Option<&ProviderAuth> {
            Some(&self.auth)
        }
        async fn classify(
            &self,
            model: &ClassifierModel,
            context: &ClassifierContext,
            options: &ClassifierOptions,
        ) -> ClassifierResult {
            self.observed
                .calls
                .lock()
                .unwrap()
                .push((model.base_url.clone(), options.api_key.clone()));
            let mut result = ClassifierResult::new(model);
            let good = context.state.get("text") == Some(&json!("good"));
            let mut answers = OrderedMap::new();
            answers.insert(
                "approved",
                ClassifierAnswer::Bool {
                    probability: if good { 0.9 } else { 0.1 },
                },
            );
            result.answers = answers;
            result.usage = Some(usage(300, 0.001));
            result
        }
        async fn generate_images(
            &self,
            model: &ImageModel,
            context: &ImagesContext,
            options: &ImagesOptions,
        ) -> AssistantImages {
            self.observed
                .calls
                .lock()
                .unwrap()
                .push((model.base_url.clone(), options.api_key.clone()));
            let mut result = AssistantImages::new(model);
            let prompt = context
                .input
                .iter()
                .find_map(|block| match block {
                    Content::Text { text, .. } => Some(text.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            if prompt == "explode" {
                result.stop_reason = ImagesStopReason::Error;
                result.error_message = Some("painter exploded".to_owned());
                return result;
            }
            result.output = vec![
                Content::text(format!("painted {prompt}")),
                Content::Image {
                    data: TINY_PNG.to_owned(),
                    mime_type: "image/png".to_owned(),
                },
            ];
            result.usage = Some(usage(100, 0.04));
            result
        }
        fn stream(
            &self,
            _: &Model,
            _: &Context,
            _: &StreamOptions,
        ) -> cyrup_core::EventStream<StreamEvent> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
        }
    }
}

async fn models_rig(script: Script) -> (Rig, Arc<models::Observed>) {
    let scorer = models::Scorer::new();
    let observed = Arc::clone(&scorer.observed);
    let mut ext = ToolsExt::new(Vec::new());
    ext.providers
        .push(("scorer", Arc::new(scorer) as Arc<dyn Provider>));
    let rig = rig(
        script,
        Options {
            ext: Some(ext),
            ..Options::default()
        },
    )
    .await;
    rig.set_active(&["codemode"]).await;
    (rig, observed)
}

/// Upstream `declares models only for the session's own codemode tool`: the description names the
/// `models` globals and points to the docs; a tool without a model-capable host does not declare it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declares_models_for_the_sessions_own_codemode_tool() {
    let (rig, _observed) = models_rig(no_script()).await;
    rig.ask("go").await;
    let description = description(&rig.request_tools(0), "codemode");
    assert!(description.contains("`models`: classifiers and image generation"));
    assert!(description.contains("docs/codemode.md"));
}

/// `CODEMODE_DOCS_PATH = join(getDocsPath(), "codemode.md")` (`tool.ts:133`): the description a
/// session sends names the page by an ABSOLUTE path, and a model that `read`s that path gets the
/// shipped reference, from whatever directory it works in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_description_names_a_readable_absolute_path_of_the_shipped_reference() {
    let (rig, _observed) = models_rig(no_script()).await;
    rig.ask("go").await;
    let description = description(&rig.request_tools(0), "codemode");
    let path = description
        .lines()
        .find_map(|line| {
            line.strip_prefix("- `models`: classifiers and image generation. Read ")
                .and_then(|rest| rest.strip_suffix(" first."))
        })
        .expect("the models line names the docs");
    let path = std::path::Path::new(path);
    assert!(path.is_absolute(), "{}", path.display());
    assert_eq!(
        path,
        cyrup_config::docs_dir().unwrap().join("codemode.md"),
        "the path is the shipped docs directory's page"
    );
    let page = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "the page the description names is not readable: {}: {error}",
            path.display()
        )
    });
    assert!(page.starts_with("# Codemode\n"), "{}", path.display());
}

/// Upstream `lists models and classifies with catalog auth, ignoring script-supplied fields`: the
/// provider an extension registered is what scripts list and call, with the catalog's endpoint and
/// the provider's resolved credentials; a script's own `baseUrl` never reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_models_and_classifies_with_catalog_auth_ignoring_script_supplied_fields() {
    let (rig, observed) = models_rig(script(|_code, env| async move {
        let available = env
            .global("models.getAvailableOfType", vec![json!("classifier"), json!("scorer")])
            .await
            .unwrap()
            .unwrap();
        let model = available[0].clone();
        let listed = env
            .global("models.getModelsOfType", vec![json!("classifier")])
            .await
            .unwrap()
            .unwrap();
        let mut evil = model.clone();
        evil["baseUrl"] = json!("https://evil.test");
        let result = env
            .global(
                "models.classify",
                vec![
                    evil,
                    json!({
                        "state": { "text": "good" },
                        "questions": { "approved": { "type": "bool", "instructions": "Approval?", "criteria": { "true": "yes", "false": "no" } } }
                    }),
                ],
            )
            .await
            .unwrap()
            .unwrap();
        completed(
            Vec::new(),
            Some(json!({
                "id": model["id"],
                "headers": model.get("headers").is_some(),
                "listed": listed.as_array().unwrap().iter().any(|e| e["provider"] == "scorer" && e["id"] == "judge"),
                "probability": result["answers"]["approved"]["probability"],
                "cost": result["usage"]["cost"]["total"],
            })),
        )
    }))
    .await;

    let result = rig.run("go").await;

    assert!(!is_error(&result), "{}", result_text(&result));
    assert_eq!(
        serde_json::from_str::<Value>(&result_text(&result)).unwrap(),
        json!({ "id": "judge", "headers": false, "listed": true, "probability": 0.9, "cost": 0.001 })
    );
    // The catalog's endpoint and the provider's credentials, never the script's.
    assert_eq!(
        *observed.calls.lock().unwrap(),
        [(
            "https://classifier.test/v1".to_owned(),
            Some("secret-key".to_owned())
        )]
    );
    let details = details(&result);
    assert_eq!(
        details
            .calls
            .iter()
            .map(|c| (c.name.as_str(), c.args.as_str(), c.cost))
            .collect::<Vec<_>>(),
        [("models.classify", "scorer/judge", Some(0.001))]
    );
    // The classifications' usage becomes the codemode result's usage and the session's cost.
    let Message::ToolResult { usage, .. } = &result else {
        panic!()
    };
    assert_eq!(usage.as_ref().unwrap().input, 300);
    assert!((rig.session.session_stats().await.cost - 0.001).abs() < 1e-10);
}

/// Upstream `generates images with catalog auth and attaches them through image()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generates_images_with_catalog_auth_and_attaches_them_through_image() {
    let (rig, observed) = models_rig(script(|_code, env| async move {
        let model = env
            .global(
                "models.getModelOfType",
                vec![json!("image"), json!("scorer"), json!("painter")],
            )
            .await
            .unwrap()
            .unwrap();
        let mut evil = model.clone();
        evil["baseUrl"] = json!("https://evil.test");
        let generated = env
            .global(
                "models.generateImages",
                vec![
                    evil,
                    json!({ "input": [{ "type": "text", "text": "a fox" }] }),
                ],
            )
            .await
            .unwrap()
            .unwrap();
        let mut output = Vec::new();
        for block in generated["output"].as_array().unwrap() {
            if block["type"] == "image" {
                output.push(OutputItem::Image {
                    data: block["data"].as_str().unwrap().to_owned(),
                    mime_type: block["mimeType"].as_str().unwrap().to_owned(),
                });
            } else {
                output.push(OutputItem::Text(block["text"].as_str().unwrap().to_owned()));
            }
        }
        let failed = env
            .global(
                "models.generateImages",
                vec![
                    model,
                    json!({ "input": [{ "type": "text", "text": "explode" }] }),
                ],
            )
            .await
            .unwrap()
            .unwrap();
        completed(
            output,
            Some(json!([failed["stopReason"], failed["errorMessage"]])),
        )
    }))
    .await;

    let result = rig.run("go").await;

    assert_eq!(
        result_text(&result),
        "painted a fox\n<image>\n[\"error\",\"painter exploded\"]"
    );
    assert!(
        observed
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(url, key)| url == "https://images.test/v1"
                && key.as_deref() == Some("secret-key"))
    );
    let details = details(&result);
    assert_eq!(
        details
            .calls
            .iter()
            .map(|c| (c.status, c.cost, c.error.as_deref()))
            .collect::<Vec<_>>(),
        [
            (
                cyrup_codemode_runtime::tool::CodemodeNestedCallStatus::Ok,
                Some(0.04),
                None
            ),
            (
                cyrup_codemode_runtime::tool::CodemodeNestedCallStatus::Error,
                None,
                Some("painter exploded")
            ),
        ]
    );
    assert!((rig.session.session_stats().await.cost - 0.04).abs() < 1e-10);
}

/// Upstream `notes generated images that the script did not show`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_generated_images_that_the_script_did_not_show() {
    let (rig, _observed) = models_rig(script(|_code, env| async move {
        let generated = env
            .global(
                "models.generateImages",
                vec![
                    json!({ "provider": "scorer", "id": "painter" }),
                    json!({ "input": [{ "type": "text", "text": "a fox" }] }),
                ],
            )
            .await
            .unwrap()
            .unwrap();
        completed(Vec::new(), Some(generated["stopReason"].clone()))
    }))
    .await;
    let result = rig.run("go").await;
    assert_eq!(
        result_text(&result),
        "stop\nNote: models.generateImages() returned 1 image that the script did not show. Show each image block of result.output with image(block)."
    );
}

/// The session's registry composes a provider's chat models too: `getModelsOfType("chat")` lists
/// them, and a chat model cannot be run from a script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_models_are_listed_but_cannot_be_classified() {
    let (rig, observed) = models_rig(script(|_code, env| async move {
        let chat = env
            .global(
                "models.getModelsOfType",
                vec![json!("chat"), json!("scorer")],
            )
            .await
            .unwrap()
            .unwrap();
        let refused = env
            .global(
                "models.classify",
                vec![
                    json!({ "provider": "scorer", "id": "chatty" }),
                    json!({ "state": {}, "questions": {} }),
                ],
            )
            .await
            .unwrap_err();
        completed(
            Vec::new(),
            Some(json!({ "chat": chat.as_array().unwrap().len(), "refused": refused })),
        )
    }))
    .await;
    let result = rig.run("go").await;
    assert_eq!(
        serde_json::from_str::<Value>(&result_text(&result)).unwrap(),
        json!({
            "chat": 1,
            "refused": "\"scorer/chatty\" is a chat model, not a classifier model. List the classifier models you can use with models.getAvailableOfType(\"classifier\").",
        })
    );
    assert!(observed.calls.lock().unwrap().is_empty());
}
