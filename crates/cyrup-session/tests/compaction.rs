//! Conformance tests for arch-05 / A-05-1..10 (compaction & branch-summary generation).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cyrup_core::{
    AssistantMessage, CancelToken, Content, EntryId, Message, StopReason, ToolCall, ToolCallId,
    Usage,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Model;
use cyrup_session::compaction::cutpoint::{find_cut_point, CutPoint};
use cyrup_session::compaction::hooks::{
    BeforeCompactDecision, BeforeCompactEvent, BeforeTreeDecision, BeforeTreeEvent,
    CompactionHooks, CompactionReason, PostCompactEvent, PostTreeEvent,
};
use cyrup_session::compaction::summarize::{
    ProviderSummarizer, SummarizationRequest, Summarizer,
};
use cyrup_session::compaction::tokens::{
    estimate_context_tokens, estimate_context_tokens_raw, TokenCache,
};
use cyrup_session::compaction::{
    branch, prepare_compaction, serialize_conversation, CompactionError,
    BRANCH_SUMMARY_EMPTY_PLACEHOLDER,
};
use cyrup_session::context::{build_context_agent_messages, build_context_messages};
use cyrup_session::agent_message::{AgentMessage, BashExecutionMessage};
use cyrup_session::{
    BranchSummarySettings, Compactor, CompactionSettings, Entry, EntryBase, KnownEntry,
    NewSessionOpts, NoHooks, SessionLayout, SessionManager,
};
use serde_json::json;

// ----------------------------------------------------------------- fixtures -------------------

fn layout(root: &Path, cwd: &Path) -> SessionLayout {
    SessionLayout::new(root.to_path_buf(), cwd.to_path_buf())
}

fn faux_model() -> Model {
    FauxProvider::new().model().clone()
}

fn user(s: &str) -> Message {
    Message::User { content: vec![Content::text(s)], timestamp: 0 }
}

fn assistant(s: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(s)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}

/// An assistant message whose only content is a tool call (drives file tracking).
fn assistant_tool(name: &str, path: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::ToolCall(ToolCall {
            id: ToolCallId::from(format!("tc-{name}-{path}")),
            name: name.to_string(),
            arguments: json!({ "path": path }).as_object().cloned().expect("object"),
            thought_signature: None,
        })],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    })
}

fn tool_result(name: &str, path: &str, body: &str) -> Message {
    Message::ToolResult {
        tool_call_id: ToolCallId::from(format!("tc-{name}-{path}")),
        tool_name: name.to_string(),
        content: vec![Content::text(body)],
        is_error: false,
        details: None,
        timestamp: 0,
    }
}

/// A markdown summary with every required §6 section (no file blocks; those are appended by core).
const FULL_SUMMARY: &str = "## Goal\nShip the feature.\n\n## Constraints & Preferences\nRust only.\n\n## Progress\n### Done\nWired it.\n### In Progress\nTesting.\n### Blocked\nNothing.\n\n## Key Decisions\nUse a tree because it is simple.\n\n## Next Steps\nAdd docs.\n\n## Critical Context\nThe faux provider is scripted.";

fn msg_entry(id: &str, parent: Option<&str>, message: Message) -> Entry {
    Entry::known(KnownEntry::Message {
        base: EntryBase {
            id: EntryId::from(id),
            parent_id: parent.map(EntryId::from),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
        message: AgentMessage::Core(message),
    })
}

// A scripted summarizer that records requests and pops canned summary bodies (a "scripted
// summary" per the spec). Used where the request itself must be inspected (A-05-4).
struct RecordingSummarizer {
    responses: Mutex<VecDeque<String>>,
    captured: Mutex<Vec<String>>,
}

impl RecordingSummarizer {
    fn new(responses: Vec<&str>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(str::to_string).collect()),
            captured: Mutex::new(Vec::new()),
        }
    }
    fn prompts(&self) -> Vec<String> {
        self.captured.lock().unwrap().clone()
    }
}

impl Summarizer for RecordingSummarizer {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        _cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        self.captured.lock().unwrap().push(req.prompt_text.clone());
        let body = self.responses.lock().unwrap().pop_front().unwrap_or_else(|| "SUMMARY".into());
        Ok(faux_assistant_message(vec![faux_text(body)], StopReason::Stop))
    }
}

// A scripted hook dispatcher: returns canned decisions and records notifications.
#[derive(Default)]
struct ScriptHooks {
    before_compact: Mutex<Option<BeforeCompactDecision>>,
    before_tree: Mutex<Option<BeforeTreeDecision>>,
    post_compact: Mutex<Vec<PostCompactEvent>>,
    post_tree: Mutex<Vec<PostTreeEvent>>,
}

impl CompactionHooks for ScriptHooks {
    async fn before_compact(
        &self,
        _ev: &BeforeCompactEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeCompactDecision, CompactionError> {
        Ok(self
            .before_compact
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(BeforeCompactDecision::Proceed))
    }
    async fn post_compact(&self, ev: &PostCompactEvent) {
        self.post_compact.lock().unwrap().push(ev.clone());
    }
    async fn before_tree(
        &self,
        _ev: &BeforeTreeEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeTreeDecision, CompactionError> {
        Ok(self.before_tree.lock().unwrap().clone().unwrap_or(BeforeTreeDecision::Proceed))
    }
    async fn post_tree(&self, ev: &PostTreeEvent) {
        self.post_tree.lock().unwrap().push(ev.clone());
    }
}

fn has_compaction(m: &SessionManager) -> bool {
    m.entries().iter().any(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
}

fn first_text(m: &Message) -> String {
    let blocks = match m {
        Message::User { content, .. } | Message::ToolResult { content, .. } => content,
        Message::Assistant(a) => &a.content,
    };
    blocks
        .iter()
        .find_map(|b| match b {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

// ----------------------------------------------------------------- A-05-1 ---------------------

#[tokio::test]
async fn a05_1_auto_compaction_appends_entry_keeps_jsonl() {
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/a05_1");
    let lay = layout(root.path(), &cwd);

    let faux = Arc::new(FauxProvider::new());
    // Two scripted summaries so the test is agnostic to whether the cut lands mid-turn.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop),
        faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop),
    ]);
    let model = faux.model().clone();
    let summ = ProviderSummarizer::new(faux.clone(), model.clone());
    let compactor = Compactor::new(summ, NoHooks);

    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 40 };

    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    for i in 0..4 {
        m.append_message(user(&format!("question number {i} with several words to add some size")))
            .unwrap();
        m.append_message(assistant(&format!(
            "answer number {i} with several words to add some size as well"
        )))
        .unwrap();
    }
    let original_entry_count = m.entries().len();

    // Trigger check fires below the window (R-05-001).
    let path: Vec<Entry> = m.branch_path(None).into_iter().cloned().collect();
    assert!(compactor.should_compact(&path, 60, &settings), "should trigger over threshold");

    let entry = compactor
        .run_compaction(
            &mut m,
            &model,
            &settings,
            CompactionReason::Threshold,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("compaction should produce an entry");

    assert!(has_compaction(&m), "a CompactionEntry was appended");
    assert!(entry.summary.contains("## Goal"));

    // Next built context = summary + kept-recent (R-05-009).
    let ctx = m.build_context();
    assert!(first_text(&ctx.messages[0]).contains("## Goal"));
    assert!(ctx.messages.len() < original_entry_count, "context is reduced vs full history");

    // Full JSONL still has every original message + the compaction (R-05-011, DI-9).
    assert_eq!(m.entries().len(), original_entry_count + 1);
    let reopened = SessionManager::open(m.session_file().unwrap()).unwrap();
    assert_eq!(reopened.entries().len(), original_entry_count + 1);
}

// ----------------------------------------------------------------- A-05-2 ---------------------

#[test]
fn a05_2_cut_never_splits_tool_call_from_result() {
    // user, assistant(tool call), tool result, assistant(final).
    let entries = vec![
        msg_entry("e0", None, user("do the thing with enough words to matter here")),
        msg_entry("e1", Some("e0"), assistant_tool("read", "src/main.rs")),
        msg_entry("e2", Some("e1"), tool_result("read", "src/main.rs", "fn main() {}")),
        msg_entry("e3", Some("e2"), assistant("done with a short final answer here")),
    ];
    let cache = TokenCache::default();
    let cut: CutPoint = find_cut_point(&entries, &cache, 0, entries.len(), 8);

    // The cut never lands on the tool-result entry (R-05-005)...
    assert!(
        !matches!(
            entries.get(cut.first_kept_index),
            Some(Entry::Known(KnownEntry::Message {
                message: AgentMessage::Core(Message::ToolResult { .. }),
                ..
            }))
        ),
        "first kept entry must not be a tool result"
    );
    // ...and the call (e1) + its result (e2) end up on the SAME side of the cut.
    let call_kept = cut.first_kept_index <= 1;
    let result_kept = cut.first_kept_index <= 2;
    assert_eq!(call_kept, result_kept, "tool call and its result must not be split by the cut");
}

// ----------------------------------------------------------------- A-05-3 ---------------------

#[tokio::test]
async fn a05_3_split_turn_two_summaries_merged() {
    let faux = Arc::new(FauxProvider::new());
    // Two scripted summaries: history half + turn-prefix half.
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("HISTORY-SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("PREFIX-SUMMARY")], StopReason::Stop),
    ]);
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    let cwd = PathBuf::from("/proj/a05_3");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("first turn question short")).unwrap();
    m.append_message(assistant("first turn answer short")).unwrap();
    // A single oversized final turn (user + a very large assistant reply).
    m.append_message(user("second turn question short")).unwrap();
    let big = "x ".repeat(120);
    m.append_message(assistant(&big)).unwrap();

    let entry = compactor
        .run_compaction(
            &mut m,
            &model,
            &settings,
            CompactionReason::Manual,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("split-turn compaction should produce an entry");

    assert!(entry.summary.contains("HISTORY-SUMMARY"), "history summary present");
    assert!(entry.summary.contains("PREFIX-SUMMARY"), "turn-prefix summary present");
    assert!(entry.summary.contains("split turn"), "the two summaries are merged with a marker");
    assert_eq!(faux.call_count(), 2, "exactly two summarization calls (R-05-006)");
}

// ----------------------------------------------------------------- A-05-4 ---------------------

#[tokio::test]
async fn a05_4_compact_custom_instructions_in_request() {
    let summarizer = Arc::new(RecordingSummarizer::new(vec![FULL_SUMMARY]));
    let compactor = Compactor::new(RecordingArc(summarizer.clone()), NoHooks);
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    let cwd = PathBuf::from("/proj/a05_4");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    for i in 0..3 {
        m.append_message(user(&format!("auth question {i} with enough words here to matter"))).unwrap();
        m.append_message(assistant(&format!("auth answer {i} with enough words here to matter"))).unwrap();
    }

    compactor
        .run_compaction(
            &mut m,
            &model,
            &settings,
            CompactionReason::Manual,
            Some("focus on the auth refactor".to_string()),
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("compaction should run");

    let prompts = summarizer.prompts();
    assert!(!prompts.is_empty());
    assert!(
        prompts.iter().any(|p| p.contains("focus on the auth refactor")),
        "custom instructions must appear in the summarization request (R-05-014)"
    );
    assert!(prompts.iter().any(|p| p.contains("Additional focus:")));
}

// Newtype so an Arc<RecordingSummarizer> satisfies the Summarizer bound by value.
struct RecordingArc(Arc<RecordingSummarizer>);
impl Summarizer for RecordingArc {
    async fn complete(
        &self,
        req: SummarizationRequest<'_>,
        cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        self.0.complete(req, cancel).await
    }
}

// ----------------------------------------------------------------- A-05-5 ---------------------

#[tokio::test]
async fn a05_5_overflow_recovery_then_retry() {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop)]);
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    let cwd = PathBuf::from("/proj/a05_5");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    for i in 0..4 {
        m.append_message(user(&format!("overflow question {i} with enough words to matter"))).unwrap();
        m.append_message(assistant(&format!("overflow answer {i} with enough words to matter"))).unwrap();
    }

    // Overflow recovery: compaction runs with reason=Overflow and will_retry=true (R-05-003).
    let entry = compactor
        .run_compaction(
            &mut m,
            &model,
            &settings,
            CompactionReason::Overflow,
            None,
            true,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("overflow compaction should produce an entry");
    assert!(has_compaction(&m));
    assert!(entry.summary.contains("## Goal"));

    // The retried request now sees a reduced context (summary + kept), so the loop can proceed.
    let ctx = m.build_context();
    assert!(first_text(&ctx.messages[0]).contains("## Goal"), "rebuilt context leads with summary");
}

// ----------------------------------------------------------------- A-05-6 ---------------------

#[tokio::test]
async fn a05_6_cumulative_file_lists_across_two_compactions() {
    let summarizer = Arc::new(RecordingSummarizer::new(vec![FULL_SUMMARY, FULL_SUMMARY]));
    let compactor = Compactor::new(RecordingArc(summarizer.clone()), NoHooks);
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    let cwd = PathBuf::from("/proj/a05_6");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();

    // First batch: read a.rs, edit c.rs.
    m.append_message(user("first batch start with enough words to matter here now")).unwrap();
    m.append_message(assistant_tool("read", "a.rs")).unwrap();
    m.append_message(tool_result("read", "a.rs", "contents")).unwrap();
    m.append_message(assistant_tool("edit", "c.rs")).unwrap();
    m.append_message(tool_result("edit", "c.rs", "ok")).unwrap();
    m.append_message(user("first batch end with enough words to matter here now ok")).unwrap();
    m.append_message(assistant("first batch reply with enough words to matter here ok")).unwrap();

    compactor
        .run_compaction(&mut m, &model, &settings, CompactionReason::Manual, None, false, CancelToken::new())
        .await
        .unwrap()
        .expect("first compaction");

    // Second batch: read b.rs.
    m.append_message(user("second batch start with enough words to matter here now ok")).unwrap();
    m.append_message(assistant_tool("read", "b.rs")).unwrap();
    m.append_message(tool_result("read", "b.rs", "more")).unwrap();
    m.append_message(user("second batch end with enough words to matter here now okay")).unwrap();
    m.append_message(assistant("second batch reply with enough words to matter here ok")).unwrap();

    let second = compactor
        .run_compaction(&mut m, &model, &settings, CompactionReason::Manual, None, false, CancelToken::new())
        .await
        .unwrap()
        .expect("second compaction");

    let details = second.details.expect("second compaction has details");
    let read: Vec<String> = serde_json::from_value(details["readFiles"].clone()).unwrap();
    let modified: Vec<String> = serde_json::from_value(details["modifiedFiles"].clone()).unwrap();

    assert!(read.contains(&"a.rs".to_string()), "first-compaction read file accumulates (R-05-015)");
    assert!(read.contains(&"b.rs".to_string()), "second-compaction read file present");
    assert!(modified.contains(&"c.rs".to_string()), "first-compaction modified file accumulates");
}

// ----------------------------------------------------------------- A-05-7 ---------------------

#[tokio::test]
async fn a05_7_branch_summary_appended_at_nav_abandoned_intact() {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop)]);
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);
    let settings = BranchSummarySettings { reserve_tokens: 16384, skip_prompt: false };

    let cwd = PathBuf::from("/proj/a05_7");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("shared question")).unwrap();
    let shared_a = m.append_message(assistant("shared answer")).unwrap();
    let b1q = m.append_message(user("branch one question")).unwrap();
    let l1 = m.append_message(assistant("branch one answer")).unwrap();

    // Sibling branch off the shared assistant.
    m.branch(&shared_a).unwrap();
    let _b2q = m.append_message(user("branch two question")).unwrap();
    let l2 = m.append_message(assistant("branch two answer")).unwrap();

    let total_before = m.entries().len();

    let entry = compactor
        .run_branch_summary(
            &mut m,
            &model,
            l2.clone(),
            Some(l1.clone()),
            true,
            &settings,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("branch summary should be appended");

    // Appended at the navigation point: from_id is the abandoned leaf, parent is the target.
    assert_eq!(entry.from_id, l1, "from_id is the leaf navigated from (R-05-016)");
    assert_eq!(entry.parent_id.as_ref(), Some(&l2), "appended at the navigation point");

    // The abandoned branch is never deleted (R-05-017).
    assert!(m.entry(&b1q).is_some());
    assert!(m.entry(&l1).is_some());
    assert_eq!(m.entries().len(), total_before + 1, "only one entry appended");
}

#[test]
fn branch_budget_is_reserve_tokens_newest_first() {
    // prepare_branch_entries uses the branchSummary.reserveTokens budget, newest-first (R-05-016).
    let entries = vec![
        msg_entry("e0", None, user(&"old ".repeat(40))),
        msg_entry("e1", Some("e0"), assistant(&"mid ".repeat(40))),
        msg_entry("e2", Some("e1"), user("newest short")),
    ];
    // A tiny budget keeps only the newest entry.
    let prep = branch::prepare_branch_entries(&entries, 5);
    let texts: Vec<String> = prep.messages.iter().map(first_text).collect();
    assert!(texts.iter().any(|t| t.contains("newest short")), "newest entry kept");
    assert!(!texts.iter().any(|t| t.contains("old old")), "oldest dropped under tiny budget");
}

// ----------------------------------------------------------------- A-05-8 ---------------------

#[tokio::test]
async fn a05_8_before_compact_cancel_and_custom() {
    let model = faux_model();

    // (a) Cancel prevents compaction entirely (R-05-020a).
    {
        let hooks = ScriptHooks::default();
        *hooks.before_compact.lock().unwrap() = Some(BeforeCompactDecision::Cancel);
        let compactor = Compactor::new(RecordingArc(Arc::new(RecordingSummarizer::new(vec![]))), hooks);
        let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

        let cwd = PathBuf::from("/proj/a05_8a");
        let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
        m.append_message(user("q one with enough words to matter here now")).unwrap();
        m.append_message(assistant("a one with enough words to matter here now")).unwrap();
        m.append_message(user("q two with enough words to matter here now")).unwrap();
        m.append_message(assistant("a two with enough words to matter here now")).unwrap();

        let out = compactor
            .run_compaction(&mut m, &model, &settings, CompactionReason::Manual, None, false, CancelToken::new())
            .await
            .unwrap();
        assert!(out.is_none(), "cancel returns no entry");
        assert!(!has_compaction(&m), "cancel appends nothing");
        assert!(
            compactor.hooks().post_compact.lock().unwrap().is_empty(),
            "post_compact not fired on cancel"
        );
    }

    // (b) Custom summary is used verbatim and marked extension-sourced (R-05-020b/021).
    {
        let first_kept = EntryId::from("custom-keep");
        let hooks = ScriptHooks::default();
        *hooks.before_compact.lock().unwrap() = Some(BeforeCompactDecision::Custom {
            summary: "CUSTOM-EXTENSION-SUMMARY".to_string(),
            first_kept_entry_id: first_kept.clone(),
            tokens_before: 42,
            details: Some(json!({ "readFiles": ["x.rs"], "modifiedFiles": [] })),
        });
        let compactor = Compactor::new(RecordingArc(Arc::new(RecordingSummarizer::new(vec![]))), hooks);
        let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

        let cwd = PathBuf::from("/proj/a05_8b");
        let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
        m.append_message(user("q one with enough words to matter here now")).unwrap();
        m.append_message(assistant("a one with enough words to matter here now")).unwrap();
        m.append_message(user("q two with enough words to matter here now")).unwrap();
        m.append_message(assistant("a two with enough words to matter here now")).unwrap();

        let entry = compactor
            .run_compaction(&mut m, &model, &settings, CompactionReason::Manual, None, false, CancelToken::new())
            .await
            .unwrap()
            .expect("custom compaction produces an entry");

        assert_eq!(entry.summary, "CUSTOM-EXTENSION-SUMMARY", "custom summary used verbatim");
        assert!(entry.from_hook, "entry marked extension-sourced");
        assert_eq!(entry.tokens_before, 42);
        assert_eq!(entry.first_kept_entry_id, first_kept);
        let posted = compactor.hooks().post_compact.lock().unwrap().clone();
        assert_eq!(posted.len(), 1);
        assert!(posted[0].from_extension, "post-compact reports extension source (R-05-021)");
    }
}

// ----------------------------------------------------------------- A-05-9 ---------------------

#[tokio::test]
async fn a05_9_before_tree_cancel_and_replace() {
    let model = faux_model();
    let settings = BranchSummarySettings { reserve_tokens: 16384, skip_prompt: false };

    // (a) Cancel aborts navigation: leaf unchanged, nothing appended.
    {
        let hooks = ScriptHooks::default();
        *hooks.before_tree.lock().unwrap() = Some(BeforeTreeDecision::Cancel);
        let compactor = Compactor::new(RecordingArc(Arc::new(RecordingSummarizer::new(vec![]))), hooks);

        let cwd = PathBuf::from("/proj/a05_9a");
        let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
        m.append_message(user("shared")).unwrap();
        let shared = m.append_message(assistant("shared-a")).unwrap();
        let l1 = m.append_message(assistant("branch-one")).unwrap();
        m.branch(&shared).unwrap();
        let l2 = m.append_message(assistant("branch-two")).unwrap();
        m.branch(&l1).unwrap();

        let before = m.entries().len();
        let out = compactor
            .run_branch_summary(&mut m, &model, l2, Some(l1.clone()), true, &settings, CancelToken::new())
            .await
            .unwrap();
        assert!(out.is_none(), "cancel returns no entry");
        assert_eq!(m.leaf_id(), Some(&l1), "navigation cancelled — leaf unchanged");
        assert_eq!(m.entries().len(), before, "nothing appended on cancel");
    }

    // (b) Replace supplies a custom branch summary used verbatim.
    {
        let hooks = ScriptHooks::default();
        *hooks.before_tree.lock().unwrap() = Some(BeforeTreeDecision::CustomSummary {
            summary: "REPLACED-BRANCH-SUMMARY".to_string(),
            details: None,
        });
        let compactor = Compactor::new(RecordingArc(Arc::new(RecordingSummarizer::new(vec![]))), hooks);

        let cwd = PathBuf::from("/proj/a05_9b");
        let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
        m.append_message(user("shared")).unwrap();
        let shared = m.append_message(assistant("shared-a")).unwrap();
        let l1 = m.append_message(assistant("branch-one")).unwrap();
        m.branch(&shared).unwrap();
        let l2 = m.append_message(assistant("branch-two")).unwrap();
        m.branch(&l1).unwrap();

        let entry = compactor
            .run_branch_summary(&mut m, &model, l2.clone(), Some(l1), true, &settings, CancelToken::new())
            .await
            .unwrap()
            .expect("replace produces a branch summary");
        assert_eq!(entry.summary, "REPLACED-BRANCH-SUMMARY");
        assert!(entry.from_hook, "replacement marked extension-sourced");
        assert_eq!(m.leaf_id(), Some(&entry.id), "navigated, summary at the nav point");
    }
}

// ----------------------------------------------------------------- A-05-10 --------------------

#[tokio::test]
async fn a05_10_summary_has_all_sections_and_file_blocks() {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop)]);
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    let cwd = PathBuf::from("/proj/a05_10");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("start with enough words to matter here now okay then")).unwrap();
    m.append_message(assistant_tool("read", "src/lib.rs")).unwrap();
    m.append_message(tool_result("read", "src/lib.rs", "code")).unwrap();
    m.append_message(assistant_tool("edit", "src/main.rs")).unwrap();
    m.append_message(tool_result("edit", "src/main.rs", "ok")).unwrap();
    m.append_message(user("end with enough words to matter here now okay then done")).unwrap();
    m.append_message(assistant("reply with enough words to matter here now okay done")).unwrap();

    let entry = compactor
        .run_compaction(&mut m, &model, &settings, CompactionReason::Manual, None, false, CancelToken::new())
        .await
        .unwrap()
        .expect("compaction should run");

    let s = &entry.summary;
    for section in [
        "## Goal",
        "## Constraints & Preferences",
        "## Progress",
        "### Done",
        "### In Progress",
        "### Blocked",
        "## Key Decisions",
        "## Next Steps",
        "## Critical Context",
    ] {
        assert!(s.contains(section), "summary missing section: {section}");
    }
    assert!(s.contains("<read-files>"), "machine read-files block present (R-05-013)");
    assert!(s.contains("</read-files>"));
    assert!(s.contains("<modified-files>"), "machine modified-files block present");
    assert!(s.contains("src/main.rs"), "modified file listed");
}

// ----------------------------------------------------------------- G-1 tokensBefore -----------

#[test]
fn g1_tokens_before_pure_core_raw_equals_rendered() {
    // Pi sets CompactionEntry.tokensBefore to
    //   estimateContextTokens(buildSessionContext(pathEntries).messages).tokens   (compaction.ts:678)
    // over the RAW AgentMessage context. For a path of pure core user/assistant messages the raw
    // and convertToLlm-rendered estimates are IDENTICAL, so this pins the value both ways.
    let cwd = PathBuf::from("/proj/g1");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    // 4 turns, no assistant usage (Usage::default → all zero) so the estimate has no provider-usage
    // anchor and sums chars/4 ceil over every message.
    for _ in 0..4 {
        m.append_message(user("uuuuuuuu")).unwrap(); //  8 chars → ceil(8/4)  = 2
        m.append_message(assistant("aaaaaaaaaaaa")).unwrap(); // 12 chars → ceil(12/4) = 3
    }
    let path: Vec<Entry> = m.branch_path(None).into_iter().cloned().collect();

    let cache = TokenCache::default();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 5 };
    let prep = prepare_compaction(&path, &cache, &settings).expect("history to summarize");

    // 4 × (2 + 3) = 20 tokens; raw and rendered agree on pure core.
    assert_eq!(prep.tokens_before, 20, "tokensBefore pins the raw-context estimate");
    let refs: Vec<&Entry> = path.iter().collect();
    let raw = build_context_agent_messages(&refs);
    let rendered = build_context_messages(&refs);
    assert_eq!(prep.tokens_before, estimate_context_tokens_raw(&raw).tokens);
    assert_eq!(estimate_context_tokens_raw(&raw).tokens, estimate_context_tokens(&rendered).tokens);
}

/// M1 (CRITICAL): `tokensBefore` / `should_compact` estimate over Pi's RAW `AgentMessage` context
/// (`compaction.ts:192-228,678`; `session-manager.ts:389-403`), NOT the convertToLlm-rendered text.
/// The two diverge exactly for the entries below; the raw value is the one Pi persists.
///
/// BYTE-DIFF vs Pi: the identical 5-entry transcript was fed to
///   `estimateContextTokens(buildSessionContext(entries,"e5").messages).tokens`
/// in live Pi (packages/coding-agent, via tsx) → **77**, with per-message
/// estimateTokens = [compactionSummary 14, user 11, bashExecution 23, branchSummary 17, custom 12].
/// Note bashExecution is `excludeFromContext:true` yet STILL counted (23) — Pi's raw context never
/// drops it — and the summaries are counted WITHOUT the LLM wrapper prefix/suffix.
#[test]
fn m1_tokens_before_byte_matches_pi_over_raw_agent_context() {
    fn base(id: &str, parent: Option<&str>) -> EntryBase {
        EntryBase { id: EntryId::from(id), parent_id: parent.map(EntryId::from), timestamp: "2026-01-01T00:00:00Z".into() }
    }

    // e1: user core message (content len 42 → 11)
    let e1 = msg_entry("e1", None, user("hello world this is the first user message"));
    // e2: compaction (summary len 53 → 14); firstKeptEntryId = e1
    let e2 = Entry::known(KnownEntry::Compaction {
        base: base("e2", Some("e1")),
        summary: "PRIOR SUMMARY TEXT that was compacted earlier here ok".into(),
        first_kept_entry_id: EntryId::from("e1"),
        tokens_before: 1234,
        details: None,
        from_hook: None,
    });
    // e3: EXCLUDED bash (cmd 15 + out 75 = 90 → 23) — Pi raw context still counts it.
    let e3 = Entry::known(KnownEntry::Message {
        base: base("e3", Some("e2")),
        message: AgentMessage::BashExecution(BashExecutionMessage {
            command: "cat /etc/passwd".into(),
            output: "root:x:0:0:root:/root:/bin/bash\nsecret data here that is fairly long output".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
            exclude_from_context: Some(true),
        }),
    });
    // e4: branch_summary (summary len 68 → 17)
    let e4 = Entry::known(KnownEntry::BranchSummary {
        base: base("e4", Some("e3")),
        from_id: EntryId::from("x"),
        summary: "a branch summary body describing the abandoned branch in some detail".into(),
        details: None,
        from_hook: None,
    });
    // e5: custom_message (content len 48 → 12)
    let e5 = Entry::known(KnownEntry::CustomMessage {
        base: base("e5", Some("e4")),
        custom_type: "ext.note".into(),
        content: json!("a custom injected message from an extension here"),
        display: false,
        details: None,
    });

    let path = [e1, e2, e3, e4, e5];
    let refs: Vec<&Entry> = path.iter().collect();

    // The fixed RAW path byte-matches Pi's captured tokensBefore = 77.
    let raw = build_context_agent_messages(&refs);
    let raw_tokens = estimate_context_tokens_raw(&raw).tokens;
    assert_eq!(raw_tokens, 77, "tokensBefore must byte-match Pi's captured value (77)");

    // And it genuinely DIFFERS from the old rendered basis (excluded bash dropped + summary
    // wrappers over-counted), proving the fix is load-bearing, not a no-op.
    let rendered = build_context_messages(&refs);
    let rendered_tokens = estimate_context_tokens(&rendered).tokens;
    assert_ne!(
        rendered_tokens, 77,
        "the rendered-context estimate must diverge from Pi here (it dropped the excluded bash + padded the summaries)"
    );

    // prepare_compaction persists exactly this raw value into CompactionEntry.tokensBefore.
    let cache = TokenCache::default();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 5 };
    let prep = prepare_compaction(&path, &cache, &settings).expect("history to summarize");
    assert_eq!(prep.tokens_before, 77, "persisted tokensBefore must equal Pi's 77");
}

// ----------------------------------------------------------------- M2 truncation encoding -----

/// M2: the summarizer transcript truncation slices/counts by **UTF-16 code unit**, matching Pi
/// `truncateForSummary` (`utils.ts:95-99`: `text.length` / `text.slice(0, 2000)`), NOT Unicode
/// scalars. For non-BMP text the two diverge in BOTH the cut boundary and the truncated count.
///
/// BYTE-DIFF vs Pi: feeding live Pi `serializeConversation` a toolResult of 1500 `U+1F600` emoji
/// (1500 scalars, 3000 UTF-16 units) yielded emojiKept=1000, remaining=1000, and the output ended
/// with `😀\n\n[... 1000 more characters truncated]` (captured via tsx). The OLD scalar logic would
/// NOT truncate at all (1500 ≤ 2000) — a gross divergence in the text handed to the summarizer.
#[test]
fn m2_truncation_counts_utf16_code_units_like_pi() {
    let text: String = "\u{1F600}".repeat(1500); // 1500 scalars, 3000 UTF-16 units
    let msg = tool_result("read", "f", &text);
    let out = serialize_conversation(std::slice::from_ref(&msg));

    let expected = format!(
        "[Tool result]: {}\n\n[... 1000 more characters truncated]",
        "\u{1F600}".repeat(1000)
    );
    assert_eq!(out, expected, "UTF-16-unit truncation must byte-match Pi");
    // Pi's captured invariants.
    assert_eq!(out.encode_utf16().count(), 2053);
    assert!(out.ends_with("\u{1F600}\n\n[... 1000 more characters truncated]"));
    // The OLD scalar logic would have left all 1500 emoji untruncated (no marker at all).
    assert_eq!(text.chars().count(), 1500);
    assert!(!out.contains(&text), "must not pass the full untruncated body through");
}

// ----------------------------------------------------------------- G-3/G-8 empty branch -------

#[tokio::test]
async fn g3_empty_branch_appends_no_content_placeholder() {
    // Pi generateBranchSummary returns "No content to summarize" when the abandoned branch yields
    // no summarizable messages (branch-summarization.ts:309-311), and navigateTree's caller still
    // APPENDS it because `if (summaryText)` is truthy (agent-session.ts:2844). cyrup must not drop
    // the branch: it appends the placeholder entry too.
    let faux = Arc::new(FauxProvider::new()); // no scripted response needed: short-circuits the model
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);
    let settings = BranchSummarySettings { reserve_tokens: 16384, skip_prompt: false };

    let cwd = PathBuf::from("/proj/g3");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("shared question")).unwrap();
    let shared_a = m.append_message(assistant("shared answer")).unwrap();
    // Abandoned branch off shared_a that filters to NO messages (a lone tool result is dropped).
    let abandoned = m.append_message(tool_result("read", "x.rs", "data")).unwrap();

    let entry = compactor
        .run_branch_summary(
            &mut m,
            &model,
            shared_a.clone(),
            Some(abandoned.clone()),
            true,
            &settings,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("the placeholder summary is appended, not dropped");

    assert_eq!(entry.summary, BRANCH_SUMMARY_EMPTY_PLACEHOLDER);
    assert_eq!(entry.summary, "No content to summarize");
    assert_eq!(entry.from_id, abandoned, "from_id is the abandoned leaf navigated from");
    assert_eq!(entry.parent_id.as_ref(), Some(&shared_a), "appended at the navigation target");
    // The abandoned branch is never deleted (R-05-017).
    assert!(m.entry(&abandoned).is_some());
}

// ----------------------------------------------------------------- SESS-002 -------------------

fn custom_message_entry(id: &str, parent: Option<&str>, content: &str) -> Entry {
    Entry::known(KnownEntry::CustomMessage {
        base: EntryBase {
            id: EntryId::from(id),
            parent_id: parent.map(EntryId::from),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
        custom_type: "ext.injected".to_string(),
        content: json!(content),
        display: true,
        details: None,
    })
}

fn branch_summary_entry(id: &str, parent: Option<&str>, summary: &str) -> Entry {
    Entry::known(KnownEntry::BranchSummary {
        base: EntryBase {
            id: EntryId::from(id),
            parent_id: parent.map(EntryId::from),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
        summary: summary.to_string(),
        from_id: EntryId::from("from"),
        details: None,
        from_hook: None,
    })
}

#[test]
fn sess002_custom_message_tokens_count_toward_the_keep_recent_budget() {
    // Pi's live `findCutPoint` accumulates `sessionEntryToContextMessages(entry)` for EVERY entry
    // (`coding-agent/src/core/compaction/compaction.ts:418-427`), so a context-visible
    // `custom_message` counts against `keepRecentTokens`. cyrup ported the harness fork, which
    // `continue`d past every non-`message` entry, so a 40k-char extension-injected custom_message
    // contributed 0 and the walk ran off the front of the history, keeping everything.
    let injected = "x".repeat(40_000); // 40_000 chars ⇒ 10_000 estimated tokens
    let entries = vec![
        msg_entry("e0", None, user("older turn one")),
        msg_entry("e1", Some("e0"), assistant("older answer one")),
        custom_message_entry("e2", Some("e1"), &injected),
        msg_entry("e3", Some("e2"), user("recent turn two")),
        msg_entry("e4", Some("e3"), assistant("recent answer two")),
    ];
    let cache = TokenCache::default();
    assert_eq!(cache.estimate_raw_entry(&entries[2]), 10_000, "the injected entry is not free");

    let keep_recent_tokens = 2_000;
    let cut = find_cut_point(&entries, &cache, 0, entries.len(), keep_recent_tokens);

    assert_eq!(
        cut.first_kept_index, 2,
        "the budget must be exhausted BY the custom_message, cutting there — got {cut:?}"
    );
    assert!(
        cut.first_kept_index >= 2,
        "the two older turns must fall into the summarized history, not the kept tail"
    );
}

#[test]
fn sess002_branch_summary_tokens_count_toward_the_keep_recent_budget() {
    // Same rule for a `branch_summary` entry: `sessionEntryToContextMessages` projects a non-empty
    // summary into the context, so it is not free either.
    let summary = "s".repeat(40_000);
    let entries = vec![
        msg_entry("e0", None, user("older turn one")),
        msg_entry("e1", Some("e0"), assistant("older answer one")),
        branch_summary_entry("e2", Some("e1"), &summary),
        msg_entry("e3", Some("e2"), user("recent turn two")),
        msg_entry("e4", Some("e3"), assistant("recent answer two")),
    ];
    let cache = TokenCache::default();
    assert_eq!(cache.estimate_raw_entry(&entries[2]), 10_000);
    let cut = find_cut_point(&entries, &cache, 0, entries.len(), 2_000);
    assert_eq!(cut.first_kept_index, 2, "cut lands at the branch_summary, got {cut:?}");
}

#[test]
fn sess002_back_scan_stops_at_a_context_visible_entry() {
    // Pi's back-scan breaks on `sessionEntryToContextMessages(prevEntry).length > 0`
    // (`compaction.ts:439-446`), so a preceding `custom_message` is NOT folded into the kept region
    // — folding it back in would re-inflate the very tail the budget walk just measured. cyrup's
    // ported fork broke only on `compaction`/`message`, so it swallowed the custom_message.
    let entries = vec![
        msg_entry("e0", None, user("history that is being summarized away")),
        custom_message_entry("e1", Some("e0"), "a note the extension injected"),
        msg_entry("e2", Some("e1"), user("recent enough words here to matter")),
    ];
    let cache = TokenCache::default();
    let cut = find_cut_point(&entries, &cache, 0, entries.len(), 5);
    assert_eq!(
        cut.first_kept_index, 2,
        "back-scan must stop at the context-visible custom_message, got {cut:?}"
    );

    // A NON-context-visible entry in the same position is still folded in (unchanged behavior).
    let entries2 = vec![
        msg_entry("e0", None, user("history that is being summarized away")),
        Entry::known(KnownEntry::ModelChange {
            base: EntryBase {
                id: EntryId::from("e1"),
                parent_id: Some(EntryId::from("e0")),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            provider: "p".into(),
            model_id: "m".into(),
        }),
        msg_entry("e2", Some("e1"), user("recent enough words here to matter")),
    ];
    let cut2 = find_cut_point(&entries2, &TokenCache::default(), 0, entries2.len(), 5);
    assert_eq!(cut2.first_kept_index, 1, "a model_change is still folded into the kept region");
}

#[test]
fn sess002_previous_compaction_summary_counts_toward_the_keep_recent_budget() {
    // `boundaryStart` is the PREVIOUS compaction's first-kept index, so that compaction entry sits
    // inside the walked range and Pi's `sessionEntryToContextMessages` projects its summary
    // (`session-manager.ts:398-400`) — it is context-visible and consumes budget. cyrup's ported
    // fork skipped it along with every other non-`message` entry.
    let prior_summary = "z".repeat(40_000); // ⇒ 10_000 estimated tokens
    let entries = vec![
        msg_entry("e0", None, user("kept across the previous compaction")),
        Entry::known(KnownEntry::Compaction {
            base: EntryBase {
                id: EntryId::from("e1"),
                parent_id: Some(EntryId::from("e0")),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            summary: prior_summary,
            first_kept_entry_id: EntryId::from("e0"),
            tokens_before: 99_999,
            details: None,
            from_hook: None,
        }),
        msg_entry("e2", Some("e1"), user("recent turn")),
        msg_entry("e3", Some("e2"), assistant("recent answer")),
    ];
    let cache = TokenCache::default();
    assert_eq!(cache.estimate_raw_entry(&entries[1]), 10_000, "a compaction summary is not free");

    let cut = find_cut_point(&entries, &cache, 0, entries.len(), 2_000);
    assert_eq!(
        cut.first_kept_index, 2,
        "the budget is exhausted by the prior summary, so e0 is re-summarized — got {cut:?}"
    );
}
