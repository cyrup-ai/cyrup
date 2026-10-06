//! CODE-014 — the system prompt lives in the PERSISTED session file, as `sections` on system rows.
//!
//! pi stores the prompt in the transcript: its first prompt of a session writes the whole prompt as
//! `sections` on a system message, and every later change is a diff against what the transcript
//! already replays (`_preparePromptAndToolLoadout`, `core/agent-session.ts:1689-1705`, and
//! `diffSystemPromptSections`, `core/system-prompt.ts:204-217` @v1.0.0). The model is sent exactly
//! the replay of those rows — pi's `Agent` is built with `systemPrompt: ""` (`core/sdk.ts:389`) and
//! the loop calls `normalizeContext({ messages })` with no prompt of its own
//! (`agent-loop.ts:395`).
//!
//! Every assertion reads either the session FILE or the request the PROVIDER received.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::tool_transcript::{
    Reply, Requests, fixture, lines, open, prompt, scenario_tools, script, system_rows,
    written_session,
};
use crate::SessionTarget;

/// The declarations of the tools of the `tool_search` scenario, as the tools define themselves.
fn declaration(name: &str) -> Value {
    let description = if name == "loader" { "" } else { "probe tool" };
    json!({
        "name": name,
        "description": description,
        "parameters": {"type": "object", "properties": {}}
    })
}

/// What pi's loop writes for the first prompt of a session: the prompt update
/// `_preparePromptAndToolLoadout` returns (`role, content, sections, timestamp`), then the tool keys
/// `withToolChanges` appends (`agent-loop.ts:368-375`), as one `JSON.stringify` line.
fn pi_first_row(sections: Value) -> String {
    serde_json::to_string(&json!({
        "role": "system",
        "content": "",
        "sections": sections,
        "timestamp": 1_700_000_000_000_i64,
        "toolsAdded": [declaration("early"), declaration("loader")],
    }))
    .unwrap()
}

/// pi 1.0's own prompt for a session started in `/pi/work`, plus one section cyrup has no name for.
fn pi_sections() -> Value {
    json!({
        "preamble": "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.",
        "tools": "<tools>\n- read: Read file contents\n- bash: Execute bash commands\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n</tools>",
        "rules": "<rules>\n- Use bash for file operations like ls, rg, find\n- Be concise in your responses\n- Show file paths clearly when working with files\n</rules>",
        "cwd": "<cwd>\n/pi/work\n</cwd>",
        "pi_only": "<pi_only>\nonly pi writes this section\n</pi_only>",
    })
}

/// Rewrite the two system rows of a cyrup-written file as pi wrote them, and return the lines.
fn rewrite_as_pi(file: &std::path::Path, sections: Value) -> Vec<String> {
    let rows = system_rows(file);
    let all = lines(file);
    let messages = [
        pi_first_row(sections),
        serde_json::to_string(&json!({
            "role": "system",
            "content": "",
            "timestamp": 1_700_000_001_000_i64,
            "toolsAdded": [declaration("late")],
            "toolsRemoved": [{"name": "early"}],
        }))
        .unwrap(),
    ];
    assert!(rows.len() >= 2, "the scenario wrote its two declarations");
    let mut rewritten = all.clone();
    for ((index, _), message) in rows.iter().zip(&messages) {
        let original: Value = serde_json::from_str(&all[*index]).unwrap();
        let line = format!(
            "{{\"type\":\"message\",\"id\":{},\"parentId\":{},\"timestamp\":{},\"message\":{message}}}",
            original["id"], original["parentId"], original["timestamp"]
        );
        rewritten[*index] = line;
    }
    std::fs::write(file, rewritten.join("\n") + "\n").unwrap();
    rewritten
}

/// HEADLINE. A session file pi wrote, whose system rows carry the prompt as `sections`, is resumed:
/// the model must be sent ONE prompt — cyrup's — and not pi's sections on top of it.
///
/// Before CODE-014 cyrup sent its own prompt as a separate blob and replayed the file's sections on
/// top, so the model read "You are an expert coding assistant operating inside pi" AND "You are a
/// coding assistant operating inside cyrup", two `<cwd>`s, and two `<tools>` lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_pi_wrote_is_replaced_not_repeated() {
    let fx = fixture();
    let (session, file) = written_session(&fx).await;
    drop(session);
    let pi_lines = rewrite_as_pi(&file, pi_sections());

    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = open(
        &fx,
        script(&requests, vec![Reply::Text("hello")]),
        scenario_tools,
        SessionTarget::Resume(file.clone()),
    )
    .await;
    prompt(&resumed, "hi").await;

    let seen = requests.lock().unwrap();
    let sent = &seen[0].system_prompt;
    assert_eq!(
        sent.matches("operating inside cyrup").count(),
        1,
        "cyrup's prompt, once:\n{sent}"
    );
    assert!(
        !sent.contains("operating inside pi"),
        "pi's preamble was replaced, not kept beside cyrup's:\n{sent}"
    );
    assert_eq!(
        sent.matches("<cwd>").count(),
        1,
        "one cwd section, not pi's and cyrup's:\n{sent}"
    );
    assert!(
        sent.contains(&format!("<cwd>\n{}\n</cwd>", fx.cwd.display())),
        "and it is this session's cwd, not pi's `/pi/work`:\n{sent}"
    );
    assert_eq!(
        sent.matches("<tools>").count(),
        1,
        "one tools section:\n{sent}"
    );
    assert!(
        !sent.contains("only pi writes this section"),
        "a section pi wrote that cyrup does not build is removed, not left behind:\n{sent}"
    );
    drop(seen);

    // The file only grew: the rows pi wrote are byte for byte what pi wrote.
    let after = lines(&file);
    assert_eq!(
        &after[..pi_lines.len()],
        &pi_lines[..],
        "nothing pi wrote was rewritten"
    );
    // …and the change is ONE appended row: a patch, not a second copy of the prompt.
    let rows = system_rows(&file);
    assert_eq!(rows.len(), 3, "pi's two rows and one patch: {rows:#?}");
    let patch = rows[2].1["sections"].as_object().expect("a sections patch");
    assert_eq!(
        patch.get("pi_only"),
        Some(&Value::Null),
        "the section cyrup does not build is removed with a `null`: {patch:?}"
    );
    assert_ne!(
        patch.get("preamble"),
        Some(&json!(pi_sections()["preamble"])),
        "the preamble is replaced"
    );
}

// ------------------------------------------------------------------------ the write path ----

use cyrup_core::{CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolExposure};
use cyrup_core::{ToolResult, ToolUpdateSink};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;

use super::tool_transcript::{Loader, ToolsExt, config, names_of};
use crate::{SessionBuilder, SessionConfig};

/// A tool the prompt describes: it has a snippet, so it is listed in the `tools` section.
struct Snippet {
    name: &'static str,
    snippet: &'static str,
    exposure: ToolExposure,
    params: Value,
}

impl Snippet {
    fn arc(name: &'static str, snippet: &'static str, exposure: ToolExposure) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            snippet,
            exposure,
            params: json!({"type": "object", "properties": {}}),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Snippet {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "snippet tool"
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some(self.snippet)
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

fn one_tool(_slot: &super::tool_transcript::SessionSlot) -> Vec<Arc<dyn Tool>> {
    vec![Snippet::arc("early", "EARLY_SNIPPET", ToolExposure::Direct)]
}

/// `early` is described from the start; `late` is registered `deferred`, and only the loader tool
/// activates it, mid-run.
fn loader_scenario(slot: &super::tool_transcript::SessionSlot) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(Loader {
            slot: Arc::clone(slot),
            params: json!({"type": "object", "properties": {}}),
        }),
        Snippet::arc("early", "EARLY_SNIPPET", ToolExposure::Direct),
        Snippet::arc("late", "LATE_SNIPPET", ToolExposure::Deferred),
    ]
}

/// The keys of a JSON object, in the order the line spells them.
fn keys_of(sections: &Value) -> Vec<String> {
    sections
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The positions of `needles` in the message part of a persisted line, for byte-order assertions.
fn offsets(line: &str, needles: &[&str]) -> Vec<usize> {
    let message = &line[line.find("\"message\":").expect("a message entry")..];
    needles
        .iter()
        .map(|n| message.find(n).unwrap_or_else(|| panic!("{n} in {line}")))
        .collect()
}

/// A new session writes its whole prompt ONCE, as the `sections` of the system row that declares its
/// tools, and the model is sent exactly the text the session reports as its prompt. A second prompt
/// with the prompt unchanged writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_session_writes_its_prompt_once_and_the_model_reads_that_text() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = open(
        &fx,
        script(&requests, vec![Reply::Text("one"), Reply::Text("two")]),
        one_tool,
        SessionTarget::New,
    )
    .await;
    let file = session.session_file().await.expect("a persisted session");
    prompt(&session, "first").await;
    prompt(&session, "second").await;

    let rows = system_rows(&file);
    assert_eq!(
        rows.len(),
        1,
        "the prompt and the loadout are one row, written once: {rows:#?}"
    );
    let row = &rows[0].1;
    assert_eq!(
        keys_of(&row["sections"]),
        ["preamble", "tools", "rules", "cwd"],
        "pi's names, in pi's order"
    );
    let tools = row["sections"]["tools"].as_str().unwrap();
    assert!(tools.contains("- early: EARLY_SNIPPET"), "{tools}");

    // The row's bytes: the prompt update's keys, then the tool declaration after `timestamp`.
    let line = &lines(&file)[rows[0].0];
    let at = offsets(
        line,
        &[
            "\"role\"",
            "\"content\"",
            "\"sections\"",
            "\"timestamp\"",
            "\"toolsAdded\"",
        ],
    );
    assert!(
        at.windows(2).all(|w| w[0] < w[1]),
        "key order {at:?}: {line}"
    );

    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].system_prompt,
        session.system_prompt(),
        "the model reads the prompt the session reports"
    );
    assert_eq!(seen[1].system_prompt, seen[0].system_prompt);
}

/// A prompt that changes is written as a PATCH of the sections that changed, in the row that
/// declares the tool that changed it — not as a second copy of the prompt. The model's next request
/// reads the patched prompt: one `tools` section, listing both tools.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_that_changes_is_written_as_a_patch_of_what_changed() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = open(
        &fx,
        script(&requests, vec![Reply::Call("loader"), Reply::Text("done")]),
        loader_scenario,
        SessionTarget::New,
    )
    .await;
    let file = session.session_file().await.expect("a persisted session");
    prompt(&session, "go").await;

    let rows = system_rows(&file);
    assert_eq!(rows.len(), 2, "{rows:#?}");
    assert_eq!(
        keys_of(&rows[0].1["sections"]),
        ["preamble", "tools", "rules", "cwd"]
    );
    let patch = &rows[1].1;
    assert_eq!(
        keys_of(&patch["sections"]),
        ["tools"],
        "only the section that changed is written: {patch:#?}"
    );
    assert!(
        patch["sections"]["tools"]
            .as_str()
            .unwrap()
            .contains("LATE_SNIPPET")
    );
    assert_eq!(names_of(&patch["toolsAdded"]), ["late"]);

    let seen = requests.lock().unwrap();
    assert!(seen[0].system_prompt.contains("EARLY_SNIPPET"));
    assert!(!seen[0].system_prompt.contains("LATE_SNIPPET"));
    assert_eq!(
        seen[1].system_prompt.matches("<tools>").count(),
        1,
        "the patch replaced the tools section, it did not add a second: {}",
        seen[1].system_prompt
    );
    assert!(
        seen[1].system_prompt.contains("EARLY_SNIPPET")
            && seen[1].system_prompt.contains("LATE_SNIPPET"),
        "{}",
        seen[1].system_prompt
    );
    assert_eq!(
        seen[1]
            .system_prompt
            .matches("operating inside cyrup")
            .count(),
        1,
        "and the unchanged preamble was not repeated"
    );
}

/// A session resumed from its own file finds the prompt in the transcript already, so it appends
/// nothing, and the model reads the same prompt as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_resumed_from_its_own_file_appends_no_prompt() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = open(
        &fx,
        script(&requests, vec![Reply::Text("one")]),
        one_tool,
        SessionTarget::New,
    )
    .await;
    let file = session.session_file().await.unwrap();
    prompt(&session, "first").await;
    let before = lines(&file);
    drop(session);

    let resumed = open(
        &fx,
        script(&requests, vec![Reply::Text("two")]),
        one_tool,
        SessionTarget::Resume(file.clone()),
    )
    .await;
    prompt(&resumed, "second").await;

    assert_eq!(system_rows(&file).len(), 1, "no second prompt");
    assert_eq!(
        &lines(&file)[..before.len()],
        &before[..],
        "and the rows already there are untouched"
    );
    let seen = requests.lock().unwrap();
    assert_eq!(seen[1].system_prompt, seen[0].system_prompt);
}

/// A `before_agent_start` handler that replaces the prompt.
struct ForcePrompt;

#[async_trait::async_trait]
impl NativeExtension for ForcePrompt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("force-prompt")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::BeforeAgentStart { .. } => {
                HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                    system: Some("FORCED PROMPT, EXACTLY".to_string()),
                    inject: Vec::new(),
                })
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// A handler's replacement prompt is sent as it is, alone, and is NOT written to the transcript: the
/// file keeps the structured sections (pi `forceSystemPrompt`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forced_prompt_is_projected_onto_the_request_and_not_into_the_file() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let provider = script(&requests, vec![Reply::Text("one")]);
    let cfg: SessionConfig = config(&fx, SessionTarget::New);
    let session = SessionBuilder::new(provider as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(ToolsExt(one_tool(&Arc::default()))))
        .with_native_extension(Arc::new(ForcePrompt))
        .build()
        .await
        .unwrap()
        .into_shared();
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let seen = requests.lock().unwrap();
    assert_eq!(
        seen[0].system_prompt, "FORCED PROMPT, EXACTLY",
        "the handler's text, and no section of the transcript beside it"
    );
    drop(seen);

    let rows = system_rows(&file);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let written = rows[0].1.to_string();
    assert!(
        !written.contains("FORCED"),
        "not in the transcript: {written}"
    );
    assert_eq!(
        keys_of(&rows[0].1["sections"]),
        ["preamble", "tools", "rules", "cwd"],
        "the transcript keeps the structured prompt"
    );
    assert_eq!(
        session.system_prompt_override(),
        None,
        "and the replacement ended with its run"
    );
}

/// What the permission companion does at `before_agent_start` (pi-permission-system `index.ts:2155`):
/// restrict the active tool set to the tools the policy lets the model see. It returns no prompt.
struct ShapeTools {
    services: std::sync::OnceLock<Arc<dyn cyrup_ext::host::HostServices>>,
    allow: &'static [&'static str],
}

#[async_trait::async_trait]
impl NativeExtension for ShapeTools {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("shape-tools")
    }
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.services.set(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let (HostEvent::BeforeAgentStart { .. }, Some(services)) = (ev, self.services.get()) {
            let names: Vec<String> = self.allow.iter().map(|n| n.to_string()).collect();
            services.set_active_tools(&names);
        }
        HookOutcome::Noop
    }
}

/// A tool the policy hides is not in the prompt the model is sent, because the prompt is built from
/// the active set the handler left — the tools section is rebuilt for it before the run's first
/// request. (The companion's own prompt sanitizer keys on pre-v0.86 headings and finds nothing in
/// pi's `<tools>` layout, exactly as it does against pi 1.0 itself; this is what makes that safe.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_a_handler_hides_is_not_in_the_prompt_the_model_reads() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let provider = script(&requests, vec![Reply::Text("one")]);
    let tools: Vec<Arc<dyn Tool>> = vec![
        Snippet::arc("early", "ALLOWED_SNIPPET", ToolExposure::Direct),
        Snippet::arc("secret", "SECRET_SNIPPET", ToolExposure::Direct),
    ];
    let session = SessionBuilder::new(
        provider as Arc<dyn Provider>,
        config(&fx, SessionTarget::New),
    )
    .with_native_extension(Arc::new(ToolsExt(tools)))
    .with_native_extension(Arc::new(ShapeTools {
        services: std::sync::OnceLock::new(),
        allow: &["early"],
    }))
    .build()
    .await
    .unwrap()
    .into_shared();
    let file = session.session_file().await.unwrap();
    prompt(&session, "hi").await;

    let seen = requests.lock().unwrap();
    assert_eq!(
        seen[0].tools,
        ["early"],
        "the request declares the allowed tool"
    );
    assert!(
        seen[0].system_prompt.contains("ALLOWED_SNIPPET"),
        "{}",
        seen[0].system_prompt
    );
    assert!(
        !seen[0].system_prompt.contains("SECRET_SNIPPET"),
        "a hidden tool is not described to the model: {}",
        seen[0].system_prompt
    );
    drop(seen);

    let rows = system_rows(&file);
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert!(
        !rows[0].1["sections"]["tools"]
            .as_str()
            .unwrap()
            .contains("SECRET_SNIPPET"),
        "nor written into the transcript: {:#?}",
        rows[0].1
    );
    assert_eq!(
        super::tool_transcript::names_of(&rows[0].1["toolsAdded"]),
        ["early"]
    );
}
