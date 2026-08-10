//! Session **replay** into the transcript (TUI-003; Pi `renderInitialMessages()` →
//! `renderSessionEntries(buildContextEntries(), {updateFooter, populateHistory})`,
//! interactive-mode.ts:3548-3562, and the `renderSessionItems` walk at `:3415-3497`).
//!
//! A `/resume`, `/fork`, `/import` or `--resume` boot installs a session that already holds a
//! conversation. Before the fix the TUI reset to a fresh `TranscriptView` and showed NOTHING of it.
//! These tests read the committed `insert_before` scrollback — what the user actually sees — after
//! driving the real seams:
//!
//! * an end-to-end run against the faux provider, whose persisted branch is then replayed;
//! * a hand-built message list covering the interleaving Pi's walk must preserve (user → reasoning →
//!   answer → tool call → tool result → next answer) and the editor-history seeding
//!   (`populateHistory`, `:3387`);
//! * the four NON-core roles Pi keeps intact for the replay —
//!   `compactionSummary`/`branchSummary`/`custom`/`bashExecution` — which must reach their own
//!   components rather than the `user` prose `convertToLlm` renders them to at the LLM boundary
//!   (`messages.ts:148-195`; Pi feeds `renderSessionEntries` the raw
//!   `sessionEntryToContextMessages` projection, `:3506-3516`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;

use cyrup_core::{
    ApiId, AssistantMessage, Content, EntryId, Message, ProviderId, StopReason, ToolCall,
    ToolCallId,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::agent_message::{
    AgentMessage, BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage,
    CustomRoleMessage,
};
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::Core(Message::User {
        content: vec![Content::Text { text: text.to_string(), text_signature: None }],
        timestamp: 0,
    })
}

fn assistant(content: Vec<Content>) -> AgentMessage {
    let mut msg = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::Stop,
        String::new(),
    );
    msg.error_message = None;
    msg.content = content;
    AgentMessage::Core(Message::Assistant(msg))
}

fn text(t: &str) -> Content {
    Content::Text { text: t.to_string(), text_signature: None }
}

fn tool_call(name: &str, args: serde_json::Value) -> Content {
    tool_call_id("call_1", name, args)
}

fn tool_call_id(id: &str, name: &str, args: serde_json::Value) -> Content {
    let arguments = args.as_object().cloned().unwrap_or_default();
    Content::ToolCall(ToolCall {
        id: ToolCallId::from(id),
        name: name.to_string(),
        arguments,
        thought_signature: None,
    })
}

fn tool_result(name: &str, body: &str) -> AgentMessage {
    tool_result_id("call_1", name, body)
}

fn tool_result_id(id: &str, name: &str, body: &str) -> AgentMessage {
    AgentMessage::Core(Message::ToolResult {
        tool_call_id: ToolCallId::from(id),
        tool_name: name.to_string(),
        content: vec![Content::Text { text: body.to_string(), text_signature: None }],
        is_error: false,
        details: None,
        timestamp: 0,
        usage: None,
        added_tool_names: Vec::new(),
    })
}

fn bash(command: &str, output: &str, exclude: bool) -> AgentMessage {
    AgentMessage::BashExecution(BashExecutionMessage {
        command: command.to_string(),
        output: output.to_string(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 0,
        exclude_from_context: Some(exclude),
    })
}

/// The full end-to-end path: a real session runs a turn, is re-bound as if swapped in, and its
/// persisted branch is replayed. Both sides of the conversation must be back on screen.
#[tokio::test]
async fn a_swapped_in_session_replays_its_conversation() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("the answer is 42")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, config));
    let rt = AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap();
    let session = rt.session().await;
    let _ = session.prompt("what is the meaning of life").await.unwrap();
    session.wait_for_idle().await;

    let restored = session.raw_context_messages().await;
    assert!(!restored.is_empty(), "the session persisted its turn");

    // Exactly what the run loop does on a generation bump.
    let mut app = app();
    app.rebind_session();
    app.replay_session(&restored);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(
        out.contains("what is the meaning of life"),
        "the replayed view must show the prior user message; got:\n{out}"
    );
    assert!(
        out.contains("the answer is 42"),
        "the replayed view must show the prior assistant answer; got:\n{out}"
    );
}

/// Pi's `renderSessionItems` walk preserves conversation order across roles, including the tool
/// block that sits between two assistant turns.
#[test]
fn replay_preserves_message_order_including_tools() {
    let mut app = app();
    app.replay_session(&[
        user("read the config"),
        assistant(vec![
            Content::Thinking {
                thinking: "I should open it first".to_string(),
                thinking_signature: None,
                redacted: false,
            },
            text("Opening the file."),
            tool_call("read", serde_json::json!({ "file_path": "/etc/app.toml" })),
        ]),
        tool_result("read", "port = 8080"),
        assistant(vec![text("The port is 8080.")]),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    for needle in [
        "read the config",
        "I should open it first",
        "Opening the file.",
        "/etc/app.toml",
        "The port is 8080.",
    ] {
        assert!(out.contains(needle), "replay must render {needle:?}; got:\n{out}");
    }
    let order = |needle: &str| out.find(needle).unwrap();
    assert!(order("read the config") < order("I should open it first"));
    assert!(order("I should open it first") < order("Opening the file."));
    assert!(
        order("Opening the file.") < order("/etc/app.toml"),
        "the tool block follows the assistant text that called it; got:\n{out}"
    );
    assert!(
        order("/etc/app.toml") < order("The port is 8080."),
        "the tool block precedes the next assistant turn; got:\n{out}"
    );
}

/// The tool RESULT body is attached to the replayed call, not dropped (Pi matches `toolResult`
/// messages onto the pending `ToolExecutionComponent`, `:3481-3487`).
#[test]
fn replay_attaches_tool_results_to_their_calls() {
    let mut app = app();
    app.state_mut().transcript.tool_expanded = true;
    app.replay_session(&[
        user("what is in the file"),
        assistant(vec![tool_call("read", serde_json::json!({ "file_path": "/etc/app.toml" }))]),
        tool_result("read", "port = 8080"),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("port = 8080"), "the tool result body must render; got:\n{out}");
}

/// `populateHistory` (interactive-mode.ts:3387): replayed prompts are recallable with Up, newest
/// first — so a resumed session can immediately re-edit its own last message.
#[test]
fn replay_seeds_the_editor_prompt_history() {
    let mut app = app();
    app.replay_session(&[
        user("first prompt"),
        assistant(vec![text("ok")]),
        user("second prompt"),
        assistant(vec![text("ok again")]),
    ]);

    let history: Vec<String> = app.editor_mut().history().iter().cloned().collect();
    assert_eq!(
        history,
        vec!["second prompt".to_string(), "first prompt".to_string()],
        "replayed prompts seed the Up-arrow history, most recent first"
    );
}

/// A `<skill …>` submission still splits into its `[skill]` invocation block plus the trailing user
/// message on the replay path, exactly as on the live path (`parseSkillBlock`, `:3364-3384`).
#[test]
fn replay_splits_a_skill_block_submission() {
    let mut app = app();
    app.replay_session(&[user(
        "<skill name=\"deploy\" location=\"/skills/deploy.md\">\nrun the deploy\n</skill>\n\nship it",
    )]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("[skill]"), "the skill invocation block renders; got:\n{out}");
    assert!(out.contains("deploy"), "the skill name renders; got:\n{out}");
    assert!(out.contains("ship it"), "the trailing user message renders; got:\n{out}");
}

/// A `compactionSummary` must reach `CompactionSummaryMessageComponent`, NOT the `user` block —
/// and the `convertToLlm` wrapper prose the model conditions on (`messages.ts:11-17`) must never be
/// on screen (interactive-mode.ts:3337-3343).
#[test]
fn a_compaction_summary_replays_as_its_own_block_not_a_user_turn() {
    let mut app = app();
    // X14 — the summary BODY is the expanded form (`interactive-mode.ts:3486`
    // `setExpanded(this.toolOutputExpanded)`); assert it in the state that produces it.
    app.transcript_mut().set_tool_expanded(true);
    app.replay_session(&[
        AgentMessage::CompactionSummary(CompactionSummaryMessage {
            summary: "we refactored the parser".to_string(),
            tokens_before: 42_000,
            timestamp: 0,
        }),
        user("keep going"),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("[compaction]"), "the compaction block renders; got:\n{out}");
    assert!(out.contains("Compacted from 42,000 tokens"), "with its token count; got:\n{out}");
    assert!(out.contains("we refactored the parser"), "and its summary; got:\n{out}");
    assert!(
        !out.contains("The conversation history before this point was compacted"),
        "the LLM wrapper prose must never be shown; got:\n{out}"
    );
    // ...nor may it pollute the Up-arrow history — only the real prompt does.
    let history: Vec<String> = app.editor_mut().history().iter().cloned().collect();
    assert_eq!(history, vec!["keep going".to_string()]);
}

/// A `branchSummary` gets `BranchSummaryMessageComponent` (`:3344-3350`), not the
/// "The following is a summary of a branch…" wrapper `convertToLlm` builds (`messages.ts:19-24`).
#[test]
fn a_branch_summary_replays_as_its_own_block() {
    let mut app = app();
    // X14 — see the compaction test above (`interactive-mode.ts:3493`).
    app.transcript_mut().set_tool_expanded(true);
    app.replay_session(&[AgentMessage::BranchSummary(BranchSummaryMessage {
        summary: "tried the async rewrite, abandoned it".to_string(),
        from_id: EntryId::from("entry_1"),
        timestamp: 0,
    })]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("[branch]"), "the branch block renders; got:\n{out}");
    assert!(out.contains("Branch Summary"), "with its header; got:\n{out}");
    assert!(out.contains("tried the async rewrite"), "and its summary; got:\n{out}");
    assert!(
        !out.contains("The following is a summary of a branch"),
        "the LLM wrapper prose must never be shown; got:\n{out}"
    );
    assert!(
        app.editor_mut().history().is_empty(),
        "a summary is not something the user typed"
    );
}

/// A `bashExecution` replays as a bash block (`:3310-3322`), not as the ``Ran `cmd` `` prose
/// `bashExecutionToText` renders it to for the model (`messages.ts:82-98`). A `!!` run is present
/// too — Pi's raw context keeps `excludeFromContext` messages and drops them only in `convertToLlm`.
#[test]
fn a_bash_execution_replays_as_a_bash_block() {
    let mut app = app();
    app.state_mut().transcript.tool_expanded = true;
    app.replay_session(&[
        bash("git status", "nothing to commit", false),
        bash("cat .env", "SECRET=xyz", true),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("$ git status"), "the bash header renders; got:\n{out}");
    assert!(out.contains("nothing to commit"), "with its output; got:\n{out}");
    assert!(
        out.contains("$ cat .env") && out.contains("SECRET=xyz"),
        "a `!!` run survives the replay too; got:\n{out}"
    );
    assert!(
        !out.contains("Ran `git status`"),
        "the LLM prose form must never be shown; got:\n{out}"
    );
    assert!(
        app.editor_mut().history().is_empty(),
        "a bash run is not an editor prompt (it was typed with a `!` prefix)"
    );
}

/// A `custom` message renders as the labeled extension block, and ONLY when it opted into display
/// (`if (message.display)`, `:3324`).
#[test]
fn a_custom_message_replays_only_when_it_asked_to_be_displayed() {
    let mut app = app();
    app.replay_session(&[
        AgentMessage::Custom(CustomRoleMessage {
            custom_type: "review.note".to_string(),
            content: serde_json::json!("three findings, all minor"),
            display: true,
            details: None,
            timestamp: 0,
        }),
        AgentMessage::Custom(CustomRoleMessage {
            custom_type: "telemetry.ping".to_string(),
            content: serde_json::json!("internal bookkeeping"),
            display: false,
            details: None,
            timestamp: 0,
        }),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("[review.note]"), "the displayed custom block renders; got:\n{out}");
    assert!(out.contains("three findings, all minor"), "with its body; got:\n{out}");
    assert!(
        !out.contains("internal bookkeeping"),
        "a `display: false` custom message stays hidden; got:\n{out}"
    );
    assert!(app.editor_mut().history().is_empty(), "an extension message is not a user prompt");
}

/// Replaying nothing (a fresh `/new` session) must not invent entries.
#[test]
fn replaying_an_empty_session_renders_nothing() {
    let mut app = app();
    app.replay_session(&[]);
    app.draw().unwrap();
    assert!(app.scrollback_text().trim().is_empty(), "an empty session replays nothing");
}

/// **Two calls to the SAME tool in one assistant turn** — the batched-tool shape cyrup's own
/// parallel execution produces routinely. Pi keys every rendered call component by `content.id`
/// (`renderedPendingTools.set(content.id, component)`, interactive-mode.ts:3473) and resolves each
/// result with `renderedPendingTools.get(message.toolCallId)` (`:3481-3487`), so result → call is an
/// exact id lookup. Matching by tool NAME instead pairs each result with the LAST still-pending run
/// of that name, which silently swaps the two bodies: file A's header would be shown above file B's
/// contents.
#[test]
fn replay_pairs_same_name_tool_results_by_call_id() {
    let mut app = app();
    app.state_mut().transcript.tool_expanded = true;
    app.replay_session(&[
        user("read both configs"),
        assistant(vec![
            tool_call_id("call_a", "read", serde_json::json!({ "file_path": "/etc/alpha.toml" })),
            tool_call_id("call_b", "read", serde_json::json!({ "file_path": "/etc/bravo.toml" })),
        ]),
        tool_result_id("call_a", "read", "alpha_port = 1111"),
        tool_result_id("call_b", "read", "bravo_port = 2222"),
    ]);
    app.draw().unwrap();

    let out = app.scrollback_text();
    let at = |needle: &str| {
        out.find(needle).unwrap_or_else(|| panic!("replay must render {needle:?}; got:\n{out}"))
    };
    assert!(
        at("/etc/alpha.toml") < at("alpha_port = 1111"),
        "call_a's result belongs under call_a's header; got:\n{out}"
    );
    assert!(
        at("alpha_port = 1111") < at("/etc/bravo.toml"),
        "call_a's block is complete before call_b's begins; got:\n{out}"
    );
    assert!(
        at("/etc/bravo.toml") < at("bravo_port = 2222"),
        "call_b's result belongs under call_b's header; got:\n{out}"
    );
}

/// The same id-keyed pairing on the LIVE event path — Pi's `pendingTools` map is keyed by
/// `event.toolCallId` for `tool_execution_start`/`_update`/`_end` alike (`:3080-3116`), which is
/// exactly the batched two-`read` case the agent emits when a turn issues parallel calls.
#[test]
fn live_tool_events_pair_same_name_results_by_call_id() {
    use cyrup_session_svc::AgentSessionEvent;

    let mut app = app();
    app.state_mut().transcript.tool_expanded = true;
    for ev in [
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call_a"),
            tool_name: "read".to_string(),
            args: serde_json::json!({ "file_path": "/etc/alpha.toml" }),
        },
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call_b"),
            tool_name: "read".to_string(),
            args: serde_json::json!({ "file_path": "/etc/bravo.toml" }),
        },
        AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: ToolCallId::from("call_a"),
            tool_name: "read".to_string(),
            result: serde_json::json!({
                "content": [{ "type": "text", "text": "alpha_port = 1111" }],
            }),
            is_error: false,
        },
        AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: ToolCallId::from("call_b"),
            tool_name: "read".to_string(),
            result: serde_json::json!({
                "content": [{ "type": "text", "text": "bravo_port = 2222" }],
            }),
            is_error: false,
        },
    ] {
        app.ingest_event(&ev);
    }
    app.draw().unwrap();

    let out = app.scrollback_text();
    let at = |needle: &str| {
        out.find(needle).unwrap_or_else(|| panic!("the live path must render {needle:?}; got:\n{out}"))
    };
    assert!(
        at("/etc/alpha.toml") < at("alpha_port = 1111")
            && at("alpha_port = 1111") < at("/etc/bravo.toml"),
        "call_a's result must land in call_a's block, not call_b's; got:\n{out}"
    );
    assert!(
        at("/etc/bravo.toml") < at("bravo_port = 2222"),
        "call_b's result must land in call_b's block; got:\n{out}"
    );
}
