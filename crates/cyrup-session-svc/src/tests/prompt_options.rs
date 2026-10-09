//! EXT-084 — `before_agent_start` handlers edit ONE `systemPromptOptions` object, and the run's
//! prompt is built from what they leave (pi `emitBeforeAgentStart`,
//! `core/extensions/runner.ts:1420-1476` @v1.1.0, and `prompt()`, `core/agent-session.ts:2061-2110`).
//!
//! The ledger's Verify, on the native tier: one extension adds a section through the options, the
//! next reads `systemPrompt` and sees it, the final prompt contains it, and an extension that
//! returns a string replaces everything. Every assertion reads what a handler was handed, the
//! request the PROVIDER received, or the session FILE.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult};
use cyrup_core::{ToolExposure, ToolUpdateSink};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use serde_json::{Value, json};

use super::tool_transcript::{
    Reply, Requests, ToolsExt, config, fixture, prompt, script, system_rows,
};
use crate::{AgentSession, SessionBuilder, SessionTarget};

/// A tool the prompt lists (it has a snippet).
struct Listed(&'static str, Value);

#[async_trait::async_trait]
impl Tool for Listed {
    fn name(&self) -> &str {
        self.0
    }
    fn parameters(&self) -> &Value {
        &self.1
    }
    fn description(&self) -> &str {
        "listed tool"
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(if self.0 == "early" {
            "EARLY_SNIPPET"
        } else {
            "OTHER_SNIPPET"
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
            content: vec![Content::text("ran")],
            ..Default::default()
        })
    }
}

fn tools() -> Vec<Arc<dyn Tool>> {
    let params = json!({"type": "object", "properties": {}});
    vec![
        Arc::new(Listed("early", params.clone())),
        Arc::new(Listed("other", params)),
    ]
}

/// What a `before_agent_start` handler was handed: `event.systemPrompt`, `ctx.getSystemPrompt()`
/// and `event.systemPromptOptions`.
#[derive(Clone, Debug)]
struct Handed {
    system_prompt: String,
    ctx_prompt: Option<String>,
    options: Value,
}

type Log = Arc<Mutex<Vec<Handed>>>;

/// One `before_agent_start` handler: records what it was handed, then edits the options with
/// `edit` (on the first `runs` prompts only) and/or returns `force` as the prompt.
struct Handler {
    id: &'static str,
    log: Log,
    edit: Option<fn(&mut Value)>,
    force: Option<&'static str>,
    runs: usize,
    seen: AtomicUsize,
}

impl Handler {
    fn new(id: &'static str, log: &Log) -> Self {
        Self {
            id,
            log: Arc::clone(log),
            edit: None,
            force: None,
            runs: usize::MAX,
            seen: AtomicUsize::new(0),
        }
    }
    fn editing(mut self, edit: fn(&mut Value)) -> Self {
        self.edit = Some(edit);
        self
    }
    fn forcing(mut self, force: &'static str) -> Self {
        self.force = Some(force);
        self
    }
    fn first_run_only(mut self) -> Self {
        self.runs = 1;
        self
    }
}

#[async_trait::async_trait]
impl NativeExtension for Handler {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        let HostEvent::BeforeAgentStart {
            system_prompt,
            options,
            ..
        } = ev
        else {
            return HookOutcome::Noop;
        };
        self.log.lock().unwrap().push(Handed {
            system_prompt: system_prompt.clone(),
            ctx_prompt: ctx.system_prompt().map(str::to_owned),
            options: options.clone(),
        });
        if self.seen.fetch_add(1, Ordering::SeqCst) >= self.runs {
            return HookOutcome::Noop;
        }
        let edited = self.edit.map(|edit| {
            let mut copy = options.clone();
            edit(&mut copy);
            copy
        });
        if edited.is_none() && self.force.is_none() {
            return HookOutcome::Noop;
        }
        HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
            system: self.force.map(str::to_owned),
            inject: Vec::new(),
            options: edited,
        })
    }
}

fn add_team_section(options: &mut Value) {
    options["sections"]["team"] = json!("TEAM_RULE");
}

async fn open(
    requests: &Requests,
    replies: Vec<Reply>,
    handlers: Vec<Handler>,
) -> (super::tool_transcript::Fixture, Arc<AgentSession>) {
    let fx = fixture();
    let mut builder = SessionBuilder::new(
        script(requests, replies) as Arc<dyn Provider>,
        config(&fx, SessionTarget::New),
    )
    .with_native_extension(Arc::new(ToolsExt(tools())));
    for handler in handlers {
        builder = builder.with_native_extension(Arc::new(handler));
    }
    let session = builder.build().await.unwrap().into_shared();
    (fx, session)
}

fn keys_of(sections: &Value) -> Vec<String> {
    sections
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The ledger's Verify, first half: a section one extension adds is what the next one reads, in
/// `event.systemPrompt` and in `ctx.getSystemPrompt()`, and what the model is sent; the transcript
/// stores it after `cwd`, where pi's `buildSystemPromptSections` appends a new name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_section_one_handler_adds_is_read_by_the_next_and_sent_to_the_model() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![Reply::Text("ok")],
        vec![
            Handler::new("adds", &log).editing(add_team_section),
            Handler::new("reads", &log),
        ],
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let handed = log.lock().unwrap().clone();
    assert!(
        !handed[0].system_prompt.contains("TEAM_RULE"),
        "the first handler sees the base prompt"
    );
    assert_eq!(handed[0].options["sections"], json!({}), "{:?}", handed[0]);
    assert!(
        handed[1]
            .system_prompt
            .contains("<team>\nTEAM_RULE\n</team>"),
        "the second handler reads the section the first added: {}",
        handed[1].system_prompt
    );
    assert_eq!(
        handed[1].ctx_prompt.as_deref(),
        Some(handed[1].system_prompt.as_str()),
        "and `ctx.getSystemPrompt()` renders the same options"
    );
    assert_eq!(handed[1].options["sections"], json!({"team": "TEAM_RULE"}));

    let sent = requests.lock().unwrap()[0].system_prompt.clone();
    assert!(sent.contains("<team>\nTEAM_RULE\n</team>"), "{sent}");
    let rows = system_rows(&file);
    assert_eq!(
        keys_of(&rows[0].1["sections"]),
        ["preamble", "tools", "rules", "cwd", "team"],
        "the transcript stores the structured prompt, the new section last"
    );
}

/// The ledger's Verify, second half: an extension that returns a prompt string replaces
/// everything the model is sent (pi `forceSystemPrompt`), while the transcript keeps the
/// structured sections, the added one included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returned_prompt_replaces_everything_and_the_transcript_keeps_the_sections() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![Reply::Text("ok")],
        vec![
            Handler::new("adds", &log).editing(add_team_section),
            Handler::new("forces", &log).forcing("FORCED, EXACTLY"),
            Handler::new("reads", &log),
        ],
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let handed = log.lock().unwrap().clone();
    assert_eq!(handed[2].system_prompt, "FORCED, EXACTLY");
    assert_eq!(
        handed[2].options["forceSystemPrompt"],
        json!("FORCED, EXACTLY"),
        "a returned prompt is recorded in the options"
    );
    assert_eq!(requests.lock().unwrap()[0].system_prompt, "FORCED, EXACTLY");
    let rows = system_rows(&file);
    assert_eq!(
        keys_of(&rows[0].1["sections"]),
        ["preamble", "tools", "rules", "cwd", "team"]
    );
    assert!(!rows[0].1.to_string().contains("FORCED"));
}

/// A custom section named like a built-in one replaces it where it stands, wrapped in its tag as
/// every section is (`buildSystemPromptSections`, `core/system-prompt.ts:183-191` @v1.1.0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_section_named_like_a_built_in_one_replaces_it_in_place() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![Reply::Text("ok")],
        vec![Handler::new("rules", &log).editing(|options| {
            options["sections"]["rules"] = json!("ONLY_RULE");
        })],
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let rows = system_rows(&file);
    assert_eq!(
        keys_of(&rows[0].1["sections"]),
        ["preamble", "tools", "rules", "cwd"]
    );
    assert_eq!(
        rows[0].1["sections"]["rules"],
        json!("<rules>\nONLY_RULE\n</rules>")
    );
    let sent = requests.lock().unwrap()[0].system_prompt.clone();
    assert!(sent.contains("ONLY_RULE"), "{sent}");
    assert!(
        !sent.contains("Be concise in your responses"),
        "the built-in rules are gone: {sent}"
    );
}

/// An invalid section name refuses the prompt with pi's text (`buildSystemPromptSections` throws
/// out of `prompt()`, `core/system-prompt.ts:141` @v1.1.0), and nothing is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_section_name_refuses_the_prompt_with_pis_message() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![Reply::Text("ok")],
        vec![Handler::new("bad", &log).editing(|options| {
            options["sections"]["Bad Name"] = json!("x");
        })],
    )
    .await;
    let Err(err) = session.prompt("hi").await else {
        panic!("the prompt is refused");
    };
    assert_eq!(
        err.to_string(),
        "Invalid system prompt section name: Bad Name"
    );
    session.wait_for_idle().await;
    assert!(requests.lock().unwrap().is_empty(), "nothing was sent");
}

/// An edited `selectedTools` is applied as the loadout (pi `prompt()`: "An explicit edit wins",
/// then `_applyToolLoadout(options.selectedTools)`), so the request carries those tools and the
/// prompt lists them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edited_tool_selection_is_the_runs_loadout() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![Reply::Text("ok")],
        vec![Handler::new("narrows", &log).editing(|options| {
            options["selectedTools"] = json!(["other"]);
        })],
    )
    .await;
    prompt(&session, "hi").await;

    let handed = log.lock().unwrap().clone();
    let selected = handed[0].options["selectedTools"].clone();
    assert!(
        selected
            .as_array()
            .is_some_and(|s| s.contains(&json!("early"))),
        "the handler was handed the live tools: {selected}"
    );
    let seen = requests.lock().unwrap();
    assert!(
        !seen[0].tools.contains(&"early".to_string()),
        "{:?}",
        seen[0].tools
    );
    assert!(seen[0].tools.contains(&"other".to_string()));
    assert!(!seen[0].system_prompt.contains("EARLY_SNIPPET"));
    assert!(seen[0].system_prompt.contains("OTHER_SNIPPET"));
}

/// The run's options outlive a turn boundary (pi refreshes them from the live loadout,
/// `agent-session.ts:908-917` @v1.1.0) and end with the run (`:1846`): a tool turn keeps the
/// section, and the next prompt, whose handler adds nothing, takes it away again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_runs_options_hold_across_its_turns_and_end_with_it() {
    let requests: Requests = Arc::default();
    let log: Log = Arc::default();
    let (_dir, session) = open(
        &requests,
        vec![
            Reply::Call("early"),
            Reply::Text("done"),
            Reply::Text("again"),
        ],
        vec![
            Handler::new("adds", &log)
                .editing(add_team_section)
                .first_run_only(),
        ],
    )
    .await;
    prompt(&session, "first").await;
    assert!(
        !session.effective_system_prompt().contains("TEAM_RULE"),
        "the run's options ended with it"
    );
    prompt(&session, "second").await;

    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(
        seen[1].system_prompt.contains("TEAM_RULE"),
        "the section holds across the run's tool turn: {}",
        seen[1].system_prompt
    );
    assert!(
        !seen[2].system_prompt.contains("TEAM_RULE"),
        "and is gone from the next run: {}",
        seen[2].system_prompt
    );
}
