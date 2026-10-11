//! CODE-005, transcript half — the active tool set lives in the PERSISTED session file.
//!
//! pi records the loadout as `toolsAdded`/`toolsRemoved` on system messages in the transcript
//! (`declareToolChanges`, `packages/agent/src/agent-loop.ts:327-376` @v1.0.1) and restores it from
//! there on resume and on `/tree` navigation (`_restoreToolsFromTranscript`,
//! `core/agent-session.ts:1762-1769`), keeping the restored tools that have not registered yet as
//! pending (`_pendingToolNames`, `:431`, `:1487-1499`, `:3541-3542`, `:1778-1782`). Every assertion
//! below reads the session FILE a second session is then built from; none reads in-memory state of
//! the session that wrote it.
//!
//! The scenario is pi's `tool_search`: a tool that runs, widens the active set with a registered
//! but unactivated `deferred` tool, and finishes (`agent-session-mcp.test.ts`, "loads the tools
//! tool_search found").
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_core::{
    CancelToken, Content, ExtensionId, LoadoutView, StopReason, Tool, ToolCallId, ToolError,
    ToolExposure, ToolLoadoutChanges, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, NavigateTreeOptions, SessionBuilder, SessionConfig, SessionTarget};

// ------------------------------------------------------------------------------ fixtures ----

pub(super) struct Fixture {
    _tmp: TempDir,
    pub(super) cwd: PathBuf,
    pub(super) agent_dir: PathBuf,
}

pub(super) fn fixture() -> Fixture {
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

pub(super) fn config(fx: &Fixture, target: SessionTarget) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg.target = target;
    cfg
}

fn compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// A registered tool with a settable exposure that can hide other declarations from requests.
struct Probe {
    name: String,
    exposure: ToolExposure,
    hides: Vec<String>,
    params: Value,
    ran: Arc<AtomicBool>,
}

impl Probe {
    fn new(name: &str, exposure: ToolExposure) -> Self {
        Self {
            name: name.to_string(),
            exposure,
            hides: Vec::new(),
            params: json!({"type": "object", "properties": {}}),
            ran: Arc::new(AtomicBool::new(false)),
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
        "probe tool"
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
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
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text("ran")],
            ..Default::default()
        })
    }
}

pub(super) type SessionSlot = Arc<OnceLock<Weak<AgentSession>>>;

/// pi's `tool_search`, reduced to what matters here: while it runs, it widens the active set with
/// the registered-but-inactive `late` tool.
pub(super) struct Loader {
    pub(super) slot: SessionSlot,
    pub(super) params: Value,
}

#[async_trait::async_trait]
impl Tool for Loader {
    fn name(&self) -> &str {
        "loader"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let session = self
            .slot
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| ToolError::new("no session"))?;
        let mut names = session.active_tool_names();
        if !names.iter().any(|n| n == "late") {
            names.push("late".to_string());
        }
        session.set_active_tools_by_name(&names).await;
        Ok(ToolResult {
            content: vec![Content::text("loaded")],
            ..Default::default()
        })
    }
}

pub(super) struct ToolsExt(pub(super) Vec<Arc<dyn Tool>>);

#[async_trait::async_trait]
impl NativeExtension for ToolsExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("tool-transcript-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for t in &self.0 {
            api.register_tool(Arc::clone(t));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// One request as the provider received it.
pub(super) struct Seen {
    pub(super) tools: Vec<String>,
    pub(super) messages: String,
    /// The system prompt the PROVIDER renders for this request: `Context::system_prompt`, the one
    /// field every adapter reads (they ignore system messages in the message list). It was once
    /// derived here by replaying the transcript itself, which proved only that the transcript
    /// replays, not that the request carried it (PROMPT-001).
    pub(super) system_prompt: String,
}

pub(super) type Requests = Arc<Mutex<Vec<Seen>>>;

#[derive(Clone)]
pub(super) enum Reply {
    Call(&'static str),
    Text(&'static str),
    /// A response that ends in a (non-retryable) error.
    Fail(&'static str),
}

pub(super) fn script(requests: &Requests, replies: Vec<Reply>) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(Seen {
                    tools: ctx.tools.iter().map(|t| t.name.clone()).collect(),
                    messages: serde_json::to_string(&ctx.messages).unwrap(),
                    system_prompt: ctx.system_prompt.clone().unwrap_or_default(),
                });
                match reply {
                    Reply::Call(name) => faux_assistant_message(
                        vec![faux_tool_call(name.to_string(), json!({}))],
                        StopReason::ToolUse,
                    ),
                    Reply::Text(t) => {
                        faux_assistant_message(vec![faux_text(t.to_string())], StopReason::Stop)
                    }
                    Reply::Fail(e) => {
                        let mut failed = faux_assistant_message(Vec::new(), StopReason::Error);
                        failed.error_message = Some(e.to_string());
                        failed
                    }
                }
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

/// The tools of the `tool_search` scenario: the loader, one tool that is active from the start,
/// and `late`, registered `deferred` so that only the loader activates it.
/// One direct probe tool, for a scenario that only needs a tool to call.
pub(super) fn scenario_probe(name: &str) -> Arc<dyn Tool> {
    Probe::new(name, ToolExposure::Direct).arc()
}

pub(super) fn scenario_tools(slot: &SessionSlot) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(Loader {
            slot: Arc::clone(slot),
            params: json!({"type": "object", "properties": {}}),
        }),
        Probe::new("early", ToolExposure::Direct).arc(),
        Probe::new("late", ToolExposure::Deferred).arc(),
    ]
}

pub(super) async fn open_with(
    fx: &Fixture,
    provider: Arc<FauxProvider>,
    tools: impl FnOnce(&SessionSlot) -> Vec<Arc<dyn Tool>>,
    target: SessionTarget,
    settings: Option<cyrup_config::Settings>,
) -> Arc<AgentSession> {
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let mut builder = SessionBuilder::new(provider as Arc<dyn Provider>, config(fx, target))
        .with_native_extension(Arc::new(ToolsExt(tools(&slot))));
    if let Some(settings) = settings {
        builder = builder.cli_settings(settings);
    }
    let session = builder.build().await.unwrap().into_shared();
    slot.set(Arc::downgrade(&session)).unwrap();
    session
}

pub(super) async fn open(
    fx: &Fixture,
    provider: Arc<FauxProvider>,
    tools: impl FnOnce(&SessionSlot) -> Vec<Arc<dyn Tool>>,
    target: SessionTarget,
) -> Arc<AgentSession> {
    open_with(fx, provider, tools, target, None).await
}

pub(super) async fn prompt(session: &AgentSession, text: &str) {
    let _ = session.prompt(text).await.unwrap();
    session.wait_for_idle().await;
}

/// The session file's lines, as written.
pub(super) fn lines(file: &Path) -> Vec<String> {
    cyrup_session::flush_session_writes();
    let text = std::fs::read_to_string(file).unwrap();
    text.lines().map(str::to_string).collect()
}

fn role_of(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    (v["type"] == "message")
        .then(|| v["message"]["role"].as_str().map(str::to_string))
        .flatten()
}

/// The system messages persisted in the file, in order, with the index of the line they sit on.
pub(super) fn system_rows(file: &Path) -> Vec<(usize, Value)> {
    lines(file)
        .iter()
        .enumerate()
        .filter(|(_, l)| role_of(l).as_deref() == Some("system"))
        .map(|(i, l)| {
            (
                i,
                serde_json::from_str::<Value>(l).unwrap()["message"].clone(),
            )
        })
        .collect()
}

pub(super) fn names_of(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn has(active: &[String], name: &str) -> bool {
    active.iter().any(|n| n == name)
}

/// Run the scenario once and hand back the session that wrote the file, and the file.
pub(super) async fn written_session(fx: &Fixture) -> (Arc<AgentSession>, PathBuf) {
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Call("loader"), Reply::Text("done")]);
    let session = open(fx, faux, scenario_tools, SessionTarget::New).await;
    let file = session.session_file().await.expect("a persisted session");
    prompt(&session, "find me a tool").await;
    (session, file)
}

// -------------------------------------------------------------------------------- tests ----

/// HEADLINE (1). A tool-set change made mid-run is in the session FILE: the initial loadout leads
/// the transcript ahead of the prompt it rode in with, and the change that the loader made is a
/// second system row holding only the tool it added — earlier declarations are not repeated
/// (`agent-session-mcp.test.ts`: "Only the loaded tool is added").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loadout_change_made_mid_run_is_persisted_as_a_system_row() {
    let fx = fixture();
    let (_session, file) = written_session(&fx).await;

    let rows = system_rows(&file);
    assert_eq!(rows.len(), 2, "two declarations: {rows:#?}");
    let all = lines(&file);
    let first_user = all
        .iter()
        .position(|l| role_of(l).as_deref() == Some("user"))
        .unwrap();
    assert!(
        rows[0].0 < first_user,
        "the initial loadout precedes the prompt: system at line {}, user at {first_user}",
        rows[0].0
    );

    let initial = names_of(&rows[0].1["toolsAdded"]);
    assert!(
        has(&initial, "loader") && has(&initial, "early"),
        "{initial:?}"
    );
    assert!(
        !has(&initial, "late"),
        "late was not active yet: {initial:?}"
    );
    assert_eq!(names_of(&rows[1].1["toolsAdded"]), ["late"]);
    assert!(
        rows[1].1.get("toolsRemoved").is_none(),
        "an empty list is absent, not []: {}",
        rows[1].1
    );
}

/// pi interop, WRITE direction. The persisted row is the shape pi's `declareToolChanges` writes
/// for `{ role: "system", content: "", timestamp }` plus the delta: the same keys with the same
/// values, a tool declaration being `name`, `description`, `parameters` and nothing else
/// (`withToolChanges`, `agent-loop.ts:359`, `:368-375`).
///
/// Key ORDER is not compared: `SystemMessage` serialises in the order of `types.ts`' declaration
/// (pinned by the `cyrup-core` test), while pi's loop appends the tool fields after `timestamp`;
/// `JSON.parse` on either side does not see the difference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_persisted_row_has_the_shape_pi_writes() {
    let fx = fixture();
    let (_session, file) = written_session(&fx).await;
    let (_, message) = system_rows(&file)[1].clone();

    let mut expected = json!({
        "role": "system",
        "content": "",
        "timestamp": message["timestamp"],
        "toolsAdded": [
            {
                "name": "late",
                "description": "probe tool",
                "parameters": {"type": "object", "properties": {}}
            }
        ]
    });
    assert!(message["timestamp"].is_u64(), "{message}");
    expected["timestamp"] = message["timestamp"].clone();
    assert_eq!(message, expected);
}

/// HEADLINE (1), continued. A session built from that file in a FRESH builder — nothing shared
/// with the session that wrote it — has the same active set, including the tool the loader added;
/// the request it then makes declares that tool once and records nothing new.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_session_restores_the_active_set_from_the_file() {
    let fx = fixture();
    let (session, file) = written_session(&fx).await;
    let before = session.active_tool_names();
    assert!(has(&before, "late"), "the writer had it active: {before:?}");
    drop(session);

    // CONTROL: a session with no transcript to restore starts without `late`.
    let control = open(
        &fx,
        script(&Arc::new(Mutex::new(Vec::new())), Vec::new()),
        scenario_tools,
        SessionTarget::New,
    )
    .await;
    assert!(!has(&control.active_tool_names(), "late"));

    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(&requests, vec![Reply::Text("again")]);
    let resumed = open(
        &fx,
        faux,
        scenario_tools,
        SessionTarget::Resume(file.clone()),
    )
    .await;
    assert_eq!(
        resumed.active_tool_names().len(),
        before.len(),
        "same set: {:?} vs {before:?}",
        resumed.active_tool_names()
    );
    assert!(has(&resumed.active_tool_names(), "late"));

    prompt(&resumed, "go on").await;
    let seen = requests.lock().unwrap();
    assert_eq!(
        seen[0].tools.iter().filter(|n| *n == "late").count(),
        1,
        "{:?}",
        seen[0].tools
    );
    assert_eq!(
        system_rows(&file).len(),
        2,
        "the restored loadout needs no new declaration"
    );
}

/// (1), the pending half. A restored tool that registers AFTER the session is built (an MCP server
/// still connecting) becomes active when it does (`_pendingToolNames`, `:3541-3542`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_tool_that_registers_late_is_activated_when_it_does() {
    let fx = fixture();
    let (session, file) = written_session(&fx).await;
    drop(session);

    let without_late = |slot: &SessionSlot| {
        let mut tools = scenario_tools(slot);
        tools.retain(|t| t.name() != "late");
        tools
    };
    let resumed = open(
        &fx,
        script(&Arc::new(Mutex::new(Vec::new())), Vec::new()),
        without_late,
        SessionTarget::Resume(file),
    )
    .await;
    assert!(
        !has(&resumed.active_tool_names(), "late"),
        "it is not registered yet"
    );

    resumed
        .services()
        .ext_host
        .register_late_tool(
            ExtensionId::from("tool-transcript-ext"),
            Probe::new("late", ToolExposure::Deferred).arc(),
        )
        .unwrap();
    resumed.refresh_extension_tools().await;
    assert!(
        has(&resumed.active_tool_names(), "late"),
        "a deferred tool is not activated by registration; only the restored name does it: {:?}",
        resumed.active_tool_names()
    );
}

/// …and the pending names do not outlive the next run: a tool that registers after a prompt has
/// started is not activated (`agent-session-mcp.test.ts`: "does not activate restored tools that
/// register after the next prompt starts").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restored_tool_that_registers_after_the_next_run_starts_is_not_activated() {
    let fx = fixture();
    let (session, file) = written_session(&fx).await;
    drop(session);

    let without_late = |slot: &SessionSlot| {
        let mut tools = scenario_tools(slot);
        tools.retain(|t| t.name() != "late");
        tools
    };
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = open(
        &fx,
        script(&requests, vec![Reply::Text("done")]),
        without_late,
        SessionTarget::Resume(file),
    )
    .await;
    prompt(&resumed, "go").await;

    resumed
        .services()
        .ext_host
        .register_late_tool(
            ExtensionId::from("tool-transcript-ext"),
            Probe::new("late", ToolExposure::Deferred).arc(),
        )
        .unwrap();
    resumed.refresh_extension_tools().await;
    assert!(
        !has(&resumed.active_tool_names(), "late"),
        "{:?}",
        resumed.active_tool_names()
    );
}

/// HEADLINE (2). `/tree` navigation restores the loadout of the target branch's leaf: before the
/// load the tool is gone, at the later entry it is back (`agent-session-mcp.test.ts`: "Loads are
/// recorded in the transcript: navigating back before the load drops the tool, navigating to a
/// later entry restores it").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tree_navigation_restores_the_loadout_of_the_target_leaf() {
    let fx = fixture();
    let (session, _file) = written_session(&fx).await;
    assert!(has(&session.active_tool_names(), "late"));
    let last = session.leaf_id().await.expect("a leaf");
    let first_user = session.user_messages_for_forking().await[0]
        .entry_id
        .clone();

    session
        .navigate_tree(first_user, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(
        !has(&session.active_tool_names(), "late"),
        "before the load: {:?}",
        session.active_tool_names()
    );
    assert!(has(&session.active_tool_names(), "loader"));

    session
        .navigate_tree(last, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(
        has(&session.active_tool_names(), "late"),
        "at the later entry: {:?}",
        session.active_tool_names()
    );
}

/// HEADLINE (3). A hidden declaration is recorded and never sent. The row holds it, so a resume
/// keeps the tool executable (`codemode.mode: "only"` surviving a restart); no request, before or
/// after the resume, declares it or mentions it in a message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_declaration_is_persisted_and_never_sent() {
    let fx = fixture();
    let tools = |_: &SessionSlot| -> Vec<Arc<dyn Tool>> {
        vec![
            Probe::new("searcher", ToolExposure::Direct)
                .hiding(&["secret"])
                .arc(),
            Probe::new("secret", ToolExposure::Direct).arc(),
        ]
    };
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = open(
        &fx,
        script(&requests, vec![Reply::Text("one")]),
        tools,
        SessionTarget::New,
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "go").await;

    let rows = system_rows(&file);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let recorded = names_of(&rows[0].1["toolsAdded"]);
    assert!(
        has(&recorded, "secret") && has(&recorded, "searcher"),
        "the transcript holds the hidden declaration: {recorded:?}"
    );
    assert!(
        has(&session.active_tool_names(), "secret"),
        "still executable"
    );
    drop(session);

    let resumed_requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = open(
        &fx,
        script(&resumed_requests, vec![Reply::Text("two")]),
        tools,
        SessionTarget::Resume(file.clone()),
    )
    .await;
    assert!(
        has(&resumed.active_tool_names(), "secret"),
        "the hidden tool survives the resume: {:?}",
        resumed.active_tool_names()
    );
    prompt(&resumed, "again").await;

    for seen in requests
        .lock()
        .unwrap()
        .iter()
        .chain(resumed_requests.lock().unwrap().iter())
    {
        assert!(
            has(&seen.tools, "searcher") && !has(&seen.tools, "secret"),
            "the request's tools: {:?}",
            seen.tools
        );
        assert!(
            !seen.messages.contains("secret"),
            "no message of a request mentions the hidden declaration: {}",
            seen.messages
        );
    }
    assert_eq!(system_rows(&file).len(), 1, "nothing was re-declared");
}

/// HEADLINE (4), READ direction. A session file written by pi — `system` rows with `toolsAdded`
/// and `toolsRemoved` in the order pi's `JSON.stringify` writes them, the first with the prompt
/// `sections` pi's `_preparePromptAndToolLoadout` puts on the same row — loads, and its loadout is
/// restored: the replay of the two rows, not the defaults and not the last row alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_file_written_by_pi_restores_its_loadout() {
    let fx = fixture();
    let (session, file) = written_session(&fx).await;
    drop(session);

    let rows = system_rows(&file);
    let all = lines(&file);
    // The declarations as the tools define themselves: the loader has no description, the probes
    // have one. A declaration that differs from the tool would be a redefinition to the loop.
    let tool = |name: &str| {
        let description = if name == "loader" { "" } else { "probe tool" };
        format!(
            "{{\"name\":\"{name}\",\"description\":\"{description}\",\"parameters\":{{\"type\":\"object\",\"properties\":{{}}}}}}"
        )
    };
    // Row 0: the prompt sections and the first loadout (`withToolChanges` over a pending message:
    // `role, content, sections, timestamp, toolsAdded`). Row 1: a delta, `toolsAdded` then
    // `toolsRemoved` after `timestamp`.
    let pi_messages = [
        format!(
            "{{\"role\":\"system\",\"content\":\"\",\"sections\":{{\"preamble\":\"You are an expert coding assistant.\"}},\"timestamp\":1700000000000,\"toolsAdded\":[{},{}]}}",
            tool("early"),
            tool("loader")
        ),
        format!(
            "{{\"role\":\"system\",\"content\":\"\",\"timestamp\":1700000001000,\"toolsAdded\":[{}],\"toolsRemoved\":[{{\"name\":\"early\"}}]}}",
            tool("late")
        ),
    ];
    let mut rewritten = all.clone();
    for ((index, _), message) in rows.iter().zip(&pi_messages) {
        let original: Value = serde_json::from_str(&all[*index]).unwrap();
        let line = format!(
            "{{\"type\":\"message\",\"id\":{},\"parentId\":{},\"timestamp\":{},\"message\":{message}}}",
            original["id"], original["parentId"], original["timestamp"]
        );
        let entry: cyrup_session::Entry = serde_json::from_str(&line).unwrap();
        assert!(
            matches!(entry, cyrup_session::Entry::Known(_)),
            "a pi system row is a known entry, not an unknown one: {line}"
        );
        rewritten[*index] = line;
    }
    std::fs::write(&file, rewritten.join("\n") + "\n").unwrap();

    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = open(
        &fx,
        script(&requests, vec![Reply::Text("hello")]),
        scenario_tools,
        SessionTarget::Resume(file.clone()),
    )
    .await;
    assert_eq!(
        resumed.active_tool_names(),
        ["loader", "late"],
        "the replay of what pi declared"
    );
    prompt(&resumed, "hi").await;
    assert_eq!(requests.lock().unwrap()[0].tools, ["loader", "late"]);
    // pi's declarations already say what the session RUNS, so no tool is declared again. The prompt
    // pi's row carries is not cyrup's, so the one row the session appends is a patch of prompt
    // sections (CODE-014) with no loadout in it.
    let rows = system_rows(&file);
    assert_eq!(
        rows.len(),
        3,
        "pi's two rows and one prompt patch: {rows:#?}"
    );
    assert!(rows[2].1.get("sections").is_some(), "{:#?}", rows[2].1);
    assert!(
        rows[2].1.get("toolsAdded").is_none() && rows[2].1.get("toolsRemoved").is_none(),
        "no tool is declared a second time: {:#?}",
        rows[2].1
    );
}

/// (1) across a compaction. The system rows before the cut are gone from the context; the
/// compaction entry carries the replayed loadout (`systemMessage`), and a session resumed from the
/// file has it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_carries_the_loadout_across_the_resume() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let faux = script(
        &requests,
        vec![
            Reply::Call("loader"),
            Reply::Text("done"),
            Reply::Text("second"),
            Reply::Text("SUMMARY"),
            Reply::Text("SUMMARY"),
        ],
    );
    let session = open_with(
        &fx,
        faux,
        scenario_tools,
        SessionTarget::New,
        Some(compaction_settings()),
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "find me a tool").await;
    prompt(&session, "and another turn").await;
    session.compact(None).await.expect("compaction");
    drop(session);

    let compaction = lines(&file)
        .into_iter()
        .find(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["type"] == "compaction"))
        .expect("a compaction entry");
    let entry: Value = serde_json::from_str(&compaction).unwrap();
    let carried = names_of(&entry["systemMessage"]["toolsAdded"]);
    assert!(
        has(&carried, "late") && has(&carried, "loader") && has(&carried, "early"),
        "the compaction holds the whole replayed loadout: {compaction}"
    );

    let resumed = open_with(
        &fx,
        script(&Arc::new(Mutex::new(Vec::new())), Vec::new()),
        scenario_tools,
        SessionTarget::Resume(file),
        Some(compaction_settings()),
    )
    .await;
    assert!(
        has(&resumed.active_tool_names(), "late"),
        "{:?}",
        resumed.active_tool_names()
    );
}
