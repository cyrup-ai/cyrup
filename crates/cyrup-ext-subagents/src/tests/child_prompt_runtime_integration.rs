//! The child-side prompt runtime, driven through the REAL extension host — pi
//! `src/runs/shared/subagent-prompt-runtime.ts:97-159,317-341` @v0.34.0.
//!
//! # What these tests are for
//!
//! `prompt_runtime.rs`'s pure functions are unit-tested in-module. That is not enough for this
//! finding, because the defect was never "the rewrite computes the wrong string" — there WAS no
//! rewrite, and the flags the parent wrote (`CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT` and friends)
//! were read by nothing in the workspace. A pure-function test would have passed just as happily
//! against an extension that never subscribed to an event and therefore never ran.
//!
//! So every test here goes through [`cyrup_ext::ExtensionHost`]: the extension is loaded exactly as
//! `crates/cyrup/src/main.rs` loads it (`load_native`, which calls `init` and records the declared
//! subscriptions), and the events are dispatched through the same two production seams the live
//! session uses:
//!
//! * [`cyrup_ext::ExtensionHost::emit_before_agent_start`] — what
//!   `cyrup-session-svc/src/session.rs:1132` dispatches to build every turn's system prompt;
//! * `hooks().transform_context(...)` — what `cyrup-agent/src/agent.rs:683` calls on the message
//!   list before it is converted for the provider.
//!
//! A missing `api.subscribe(...)` fails these tests (the dispatcher short-circuits an event with no
//! declared listener), and so does a handler that returns the wrong patch shape (`apply_patch`
//! silently ignores a mismatch). That is the point: they assert the wiring, not just the algorithm.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_agent::AgentMessage;
use cyrup_core::{CancelToken, Content, ToolCallId};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use crate::prompt_runtime::{
    CHILD_FANOUT_BOUNDARY_INSTRUCTIONS, CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS,
    INHERIT_PROJECT_CONTEXT_ENV, INHERIT_SKILLS_ENV, prompt_runtime_extension_from,
};
use crate::spawn::nested_events::FANOUT_CHILD_ENV;

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: false, cwd: std::path::PathBuf::from(".") }
}

/// The env a real spawn writes for a child (`exec/mod.rs`'s `env_overlay` + `child_role_env`),
/// restricted to the four keys this runtime reads. Injected rather than set on the process, so the
/// tests carry no global state and cannot race the rest of the suite.
fn child_env(
    inherit_project_context: &'static str,
    inherit_skills: &'static str,
    fanout: &'static str,
) -> impl Fn(&str) -> Option<String> {
    move |key: &str| match key {
        k if k == INHERIT_PROJECT_CONTEXT_ENV => Some(inherit_project_context.to_string()),
        k if k == INHERIT_SKILLS_ENV => Some(inherit_skills.to_string()),
        k if k == FANOUT_CHILD_ENV => Some(fanout.to_string()),
        _ => None,
    }
}

/// The three keys above are the exact strings `exec/mod.rs`'s spawn overlay writes into a child's
/// environment. Pinned here so a rename on either side is a test failure rather than a silent
/// reversion to the write-only-flag state this whole file exists to prevent.
#[test]
fn the_env_keys_are_the_ones_the_spawn_overlay_writes() {
    assert_eq!(INHERIT_PROJECT_CONTEXT_ENV, "CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT");
    assert_eq!(INHERIT_SKILLS_ENV, "CYRUP_SUBAGENT_INHERIT_SKILLS");
    assert_eq!(FANOUT_CHILD_ENV, "CYRUP_SUBAGENT_FANOUT_CHILD");
    // The parent writes the fanout flag through `child_role_env` — assert the value the reader
    // treats as "authorized" is the value the writer emits for an authorized child.
    let authorized: std::collections::HashMap<&str, &str> =
        crate::spawn::nested_events::child_role_env(true).into_iter().collect();
    assert_eq!(authorized.get(FANOUT_CHILD_ENV), Some(&"1"));
}

async fn host_with_child_env(
    inherit_project_context: &'static str,
    inherit_skills: &'static str,
    fanout: &'static str,
) -> ExtensionHost {
    let host = ExtensionHost::new(cfg());
    let ext = prompt_runtime_extension_from(&child_env(
        inherit_project_context,
        inherit_skills,
        fanout,
    ))
    .expect("a child env must build the runtime");
    host.load_native(ext).await.expect("load_native");
    host
}

/// A system prompt shaped like the one `cyrup-session/src/prompt/builder.rs` actually assembles for
/// a child that re-execs in the parent's repo: identity, the project-context block built from the
/// repo's `AGENTS.md`, the skills block, then the date/cwd footer.
fn assembled_system_prompt() -> String {
    [
        "You are a coding assistant operating inside cyrup, helping with software engineering tasks.",
        "",
        "<project_context>",
        "",
        "Project-specific instructions follow.",
        "",
        "<project_instructions path=\"/repo/AGENTS.md\">",
        "PARENT-ONLY MARKER: never commit to main.",
        "</project_instructions>",
        "",
        "</project_context>",
        "",
        "Available skills (open the SKILL.md with the read tool to use one):",
        "<available_skills>",
        "  <skill>",
        "    <name>deploy-marker</name>",
        "  </skill>",
        "</available_skills>",
        "",
        "Current date: 2026-08-07",
        "Current working directory: /repo",
    ]
    .join("\n")
}

async fn rewritten_prompt(host: &ExtensionHost) -> Option<String> {
    host.emit_before_agent_start(
        "Task: do the thing",
        serde_json::Value::Null,
        &assembled_system_prompt(),
        serde_json::Value::Null,
        &CancelToken::new(),
    )
    .await
    .and_then(|reduction| reduction.system_prompt)
}

/// THE finding: `inheritProjectContext: false` reached the child as an env var and did nothing.
/// Now it removes the inherited project-context section from the prompt the child actually runs
/// with — while leaving the sections it WAS told to inherit intact (the mirror half, which stays
/// green even with the rewrite removed and so proves the assertion below is not vacuous).
#[tokio::test]
async fn inherit_project_context_false_removes_project_context_from_the_live_prompt() {
    let host = host_with_child_env("0", "1", "0").await;
    let prompt = rewritten_prompt(&host).await.expect("the prompt must be rewritten");

    assert!(
        !prompt.contains("PARENT-ONLY MARKER"),
        "the parent's AGENTS.md must not reach a child that opted out:\n{prompt}"
    );
    assert!(!prompt.contains("<project_context>"));
    // MIRROR: skills were inherited, so they must survive untouched.
    assert!(
        prompt.contains("deploy-marker"),
        "inheritSkills=1 must leave the skills section alone:\n{prompt}"
    );
    assert!(prompt.contains("Current date: 2026-08-07"), "the footer must survive");
}

/// The opposite lever, so neither strip can be a blanket truncation.
#[tokio::test]
async fn inherit_skills_false_removes_skills_and_keeps_project_context() {
    let host = host_with_child_env("1", "0", "0").await;
    let prompt = rewritten_prompt(&host).await.expect("the prompt must be rewritten");

    assert!(!prompt.contains("deploy-marker"), "skills must be gone:\n{prompt}");
    assert!(!prompt.contains("<available_skills>"));
    // MIRROR: project context was inherited.
    assert!(prompt.contains("PARENT-ONLY MARKER"), "project context must survive:\n{prompt}");
}

/// The second half of the finding: no child was ever TOLD it was a child. Every child now opens on
/// the boundary block, and a fanout-authorized child gets the fanout variant instead — the grant
/// and the prompt must not contradict each other.
#[tokio::test]
async fn every_child_is_told_it_is_a_child_and_a_fanout_child_gets_the_fanout_variant() {
    let plain = host_with_child_env("1", "1", "0").await;
    let plain_prompt = rewritten_prompt(&plain).await.expect("even a fully-inheriting child is told");
    assert!(
        plain_prompt.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS),
        "a plain child must open on the child boundary:\n{plain_prompt}"
    );
    assert!(plain_prompt.contains("Do not propose or run subagents."));

    let fanout = host_with_child_env("1", "1", "1").await;
    let fanout_prompt = rewritten_prompt(&fanout).await.expect("a fanout child is told too");
    assert!(
        fanout_prompt.starts_with(CHILD_FANOUT_BOUNDARY_INSTRUCTIONS),
        "a fanout-authorized child must get the fanout boundary:\n{fanout_prompt}"
    );
    assert!(
        !fanout_prompt.contains("Do not propose or run subagents."),
        "the fanout grant must not be contradicted:\n{fanout_prompt}"
    );
}

/// The third half: a forked child starts from the parent's transcript. Its orchestration
/// bookkeeping must not reach the child's model, or the child reads itself as the orchestrator.
#[tokio::test]
async fn a_childs_context_loses_the_parents_orchestration_bookkeeping() {
    let host = host_with_child_env("1", "1", "0").await;
    let hooks = host.hooks();

    let messages = vec![
        AgentMessage::user_text("original user request"),
        assistant_with(vec![Content::text("Delegating."), subagent_tool_call()]),
        subagent_tool_result(),
        parent_notice("subagent-notify"),
        bash_tool_result(),
    ];
    let out = hooks.transform_context(messages, CancelToken::new()).await.unwrap();

    assert!(
        !out.iter().any(|m| matches!(m, AgentMessage::Custom { kind, .. } if kind == "subagent-notify")),
        "the parent's completion notice must not reach the child"
    );
    assert!(
        !out.iter().any(
            |m| matches!(m, AgentMessage::ToolResult(tr) if tr.tool_name == "subagent")
        ),
        "the parent's subagent results must not reach the child"
    );
    let assistant_blocks = out
        .iter()
        .find_map(|m| match m {
            AgentMessage::Assistant(a) => Some(a.content.clone()),
            _ => None,
        })
        .expect("the assistant message survives (it still said something)");
    assert!(
        !assistant_blocks
            .iter()
            .any(|b| matches!(b, Content::ToolCall(tc) if tc.name == "subagent")),
        "the parent's own subagent call must be stripped from the assistant turn"
    );
    // MIRRORS: the child's real work is untouched.
    assert!(
        out.iter().any(|m| matches!(m, AgentMessage::ToolResult(tr) if tr.tool_name == "bash")),
        "an unrelated tool result must survive"
    );
    assert!(
        out.iter().any(|m| matches!(m, AgentMessage::User { .. })),
        "the user's own request must survive"
    );
    assert!(
        assistant_blocks.iter().any(|b| matches!(b, Content::Text { .. })),
        "the assistant's prose must survive"
    );
}

/// A fanout-authorized child keeps its OWN delegation history — those calls are its assigned work,
/// not the parent's — while still losing the parent-only notices.
#[tokio::test]
async fn a_fanout_child_keeps_its_own_delegation_history() {
    let host = host_with_child_env("1", "1", "1").await;
    let hooks = host.hooks();

    let messages = vec![
        assistant_with(vec![subagent_tool_call()]),
        subagent_tool_result(),
        parent_notice("subagent_control_notice"),
    ];
    let out = hooks.transform_context(messages, CancelToken::new()).await.unwrap();

    assert_eq!(out.len(), 2, "only the parent-only notice is dropped: {out:?}");
    assert!(
        out.iter().any(|m| matches!(m, AgentMessage::ToolResult(tr) if tr.tool_name == "subagent")),
        "a fanout child's own subagent result is its own work"
    );
    assert!(out.iter().all(|m| !matches!(m, AgentMessage::Custom { .. })));
}

/// A process that is not a subagent child attaches nothing at all — no prompt rewrite, no context
/// filter. This is every top-level `cyrup` session, and it must be completely unaffected.
#[tokio::test]
async fn a_non_child_process_attaches_no_runtime() {
    assert!(
        prompt_runtime_extension_from(&|_| None).is_none(),
        "an empty environment must not attach the child runtime"
    );

    // And with nothing loaded, the same two seams leave both values exactly as they were.
    let host = ExtensionHost::new(cfg());
    assert!(
        rewritten_prompt(&host).await.is_none(),
        "no listener => the assembled prompt is used verbatim"
    );
    let messages = vec![AgentMessage::user_text("hi"), parent_notice("subagent-notify")];
    let out = host.hooks().transform_context(messages.clone(), CancelToken::new()).await.unwrap();
    assert_eq!(out.len(), messages.len(), "a top-level session keeps its own notices");
}

// ---------------------------------------------------------------------------
// message builders
// ---------------------------------------------------------------------------

fn parent_notice(kind: &str) -> AgentMessage {
    AgentMessage::Custom {
        kind: kind.to_string(),
        payload: serde_json::json!({ "content": "run finished" }),
        details: None,
        timestamp: None,
    }
}

fn subagent_tool_call() -> Content {
    Content::ToolCall(cyrup_core::ToolCall {
        id: ToolCallId::from("tc-subagent"),
        name: "subagent".to_string(),
        arguments: serde_json::Map::new().into(),
        thought_signature: None,
    })
}

fn subagent_tool_result() -> AgentMessage {
    tool_result_named("subagent", "tc-subagent")
}

fn bash_tool_result() -> AgentMessage {
    tool_result_named("bash", "tc-bash")
}

fn tool_result_named(tool_name: &str, id: &str) -> AgentMessage {
    AgentMessage::ToolResult(cyrup_agent::ToolResultMessage {
        tool_call_id: ToolCallId::from(id),
        tool_name: tool_name.to_string(),
        content: vec![Content::text("ok")],
        details: None,
        usage: None,
        added_tool_names: Vec::new(),
        is_error: false,
        timestamp: 0,
    })
}

fn assistant_with(blocks: Vec<Content>) -> AgentMessage {
    let mut msg = cyrup_core::AssistantMessage::errored(
        "faux".into(),
        "m",
        Some("faux".into()),
        cyrup_core::StopReason::Stop,
        "x",
    );
    msg.content = blocks;
    AgentMessage::Assistant(std::sync::Arc::new(msg))
}

/// Sanity: the two `Arc`-shared imports above are the same extension the binary loads.
#[test]
fn the_runtime_the_binary_loads_is_the_one_under_test() {
    let built: Option<Arc<dyn cyrup_ext::NativeExtension>> =
        prompt_runtime_extension_from(&child_env("0", "0", "0"));
    assert_eq!(
        built.map(|ext| ext.id().to_string()),
        Some("subagent-prompt-runtime".to_string())
    );
}

// -------------------------------------------------------------------------------------------
// G81: the advertised `structured_output` parameters, through the real host registry
// -------------------------------------------------------------------------------------------

/// The whole user path for a `$ref`-bearing `outputSchema`, end to end on the CHILD side:
///
/// 1. the parent writes the caller's raw schema to a temp file and points the child at it with
///    `CYRUP_SUBAGENT_STRUCTURED_OUTPUT_SCHEMA` (`exec::structured::create_structured_output_runtime`);
/// 2. the child builds the prompt runtime from that env (`crates/cyrup/src/main.rs:489`);
/// 3. the runtime registers `structured_output` with the host;
/// 4. the host hands the tool's `parameters` to the provider as the model-facing schema.
///
/// Step 4 is where an unrewritten `#/$defs/...` becomes unsatisfiable: nesting the caller's schema
/// under `value` moved every definition a level deeper, so the pointer resolves against the
/// wrapper, which has no `$defs`. This asserts the registered tool's ADVERTISED parameters, not a
/// pure function's return value.
#[tokio::test]
async fn the_registered_structured_output_tool_advertises_rewritten_local_refs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema_path = dir.path().join("schema.json");
    let output_path = dir.path().join("output.json");
    std::fs::write(
        &schema_path,
        serde_json::json!({
            "type": "object",
            "properties": { "root": { "$ref": "#/$defs/Finding" } },
            "required": ["root"],
            "$defs": {
                "Finding": {
                    "type": "object",
                    "properties": {
                        "summary": { "type": "string" },
                        "related": { "type": "array", "items": { "$ref": "#/$defs/Finding" } },
                    },
                    "required": ["summary"],
                },
            },
        })
        .to_string(),
    )
    .expect("write schema file");

    let schema_env = schema_path.display().to_string();
    let capture_env = output_path.display().to_string();
    let ext = prompt_runtime_extension_from(&move |key: &str| match key {
        k if k == crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV => {
            Some(schema_env.clone())
        }
        k if k == crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV => {
            Some(capture_env.clone())
        }
        _ => None,
    })
    .expect("both structured vars build the runtime");

    let host = ExtensionHost::new(cfg());
    host.load_native(ext).await.expect("load_native");

    let tool = host
        .registry()
        .tool("structured_output")
        .expect("registry read")
        .expect("the structured_output tool must be registered with the host");
    let params = tool.parameters();

    assert_eq!(
        params["properties"]["value"]["properties"]["root"]["$ref"],
        serde_json::json!("#/properties/value/$defs/Finding"),
        "the model-facing schema must point at the definitions' real location"
    );
    assert_eq!(
        params["properties"]["value"]["$defs"]["Finding"]["properties"]["related"]["items"]["$ref"],
        serde_json::json!("#/properties/value/$defs/Finding"),
        "including the recursive pointer inside the definition itself"
    );

    // And the tool still VALIDATES against the caller's own root, so a conforming value is
    // captured for the parent to read back.
    let result = tool
        .execute(
            ToolCallId::from("call-ref"),
            serde_json::json!({ "value": { "root": { "summary": "ok" } } }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .expect("a conforming value is captured");
    assert!(result.terminate);
    assert!(output_path.exists());
}
