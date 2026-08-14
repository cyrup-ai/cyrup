//! Conformance tests for arch-05 / A-05-1..10 (compaction & branch-summary generation).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cyrup_core::{
    AssistantMessage, CancelToken, Content, EntryId, Message, ModelThinkingLevel, StopReason,
    ToolCall, ToolCallId, Usage,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, FauxProvider, FauxResponseStep,
};
use cyrup_provider::{CacheRetention, Model, RetryPolicy, StreamOptions};
use crate::compaction::cutpoint::{find_cut_point, CutPoint};
use crate::compaction::hooks::{
    BeforeCompactDecision, BeforeCompactEvent, BeforeTreeDecision, BeforeTreeEvent,
    CompactionHooks, CompactionReason, PostCompactEvent, PostTreeEvent,
};
use crate::compaction::summarize::{
    ProviderSummarizer, SummarizationRequest, Summarizer,
};
use crate::compaction::tokens::{
    estimate_context_tokens, estimate_context_tokens_raw, TokenCache,
};
use crate::compaction::{
    branch, prepare_compaction, serialize_conversation, CompactionError,
    BRANCH_SUMMARY_EMPTY_PLACEHOLDER,
};
use crate::context::{build_context_agent_messages, build_context_messages};
use crate::agent_message::{AgentMessage, BashExecutionMessage};
use crate::{
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
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
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
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
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
        usage: None,
        added_tool_names: Vec::new(),
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
            usage: None,
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
        first_kept_entry_id: Some(EntryId::from("e1")),
        tokens_before: 1234,
        details: None,
        usage: None,
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
        usage: None,
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
        usage: None,
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
            first_kept_entry_id: Some(EntryId::from("e0")),
            tokens_before: 99_999,
            details: None,
            usage: None,
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

// ------------------------------------------------------- THEME F (live coding-agent fork) -----
//
// Pi has two forked compaction implementations. cyrup ported
// `packages/agent/src/harness/compaction/compaction.ts`; pi's LIVE path is
// `packages/coding-agent/src/core/compaction/compaction.ts`. They split in pi commit a6f720e6
// (2026-07-09), which replaced every structural `switch (entry.type)` in the cut-point layer with a
// projection through `sessionEntryToContextMessages(entry)`. The tests below pin the LIVE behavior.

/// A `type:"message"` entry holding a NON-core `AgentMessage` (bash/custom role).
fn agent_msg_entry(id: &str, parent: Option<&str>, message: AgentMessage) -> Entry {
    Entry::known(KnownEntry::Message {
        base: EntryBase {
            id: EntryId::from(id),
            parent_id: parent.map(EntryId::from),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
        message,
    })
}

fn bash_msg(command: &str, output: &str, excluded: bool) -> AgentMessage {
    AgentMessage::BashExecution(BashExecutionMessage {
        command: command.to_string(),
        output: output.to_string(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 0,
        exclude_from_context: excluded.then_some(true),
    })
}

/// A hook dispatcher that captures the `BeforeCompactEvent` it was handed.
#[derive(Default)]
struct CapturingHooks {
    events: Mutex<Vec<BeforeCompactEvent>>,
}

impl CompactionHooks for CapturingHooks {
    async fn before_compact(
        &self,
        ev: &BeforeCompactEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeCompactDecision, CompactionError> {
        self.events.lock().unwrap().push(ev.clone());
        Ok(BeforeCompactDecision::Proceed)
    }
    async fn post_compact(&self, _ev: &PostCompactEvent) {}
    async fn before_tree(
        &self,
        _ev: &BeforeTreeEvent,
        _cancel: CancelToken,
    ) -> Result<BeforeTreeDecision, CompactionError> {
        Ok(BeforeTreeDecision::Proceed)
    }
    async fn post_tree(&self, _ev: &PostTreeEvent) {}
}

// ------------------------------------------------------------------ F-1 -----------------------

#[test]
fn f1_empty_branch_summary_is_neither_a_cut_point_nor_a_turn_start() {
    // `sessionEntryToContextMessages` projects a `branch_summary` entry only `if (entry.summary)`
    // (`session-manager.ts:400-402`), so an EMPTY branch summary is context-invisible: it is not a
    // valid cut point and it does not start a turn. cyrup's ported fork matched
    // `KnownEntry::BranchSummary { .. }` structurally at both sites, so it snapped the cut onto the
    // empty entry AND reported it as the turn start — producing an empty turn prefix.
    let entries = vec![
        msg_entry("e0", None, user("please refactor the parser module thoroughly")),
        msg_entry("e1", Some("e0"), assistant_tool("read", "src/main.rs")),
        msg_entry(
            "e2",
            Some("e1"),
            tool_result("read", "src/main.rs", "0123456789012345678901234567890123456789"),
        ),
        branch_summary_entry("e3", Some("e2"), ""),
        msg_entry("e4", Some("e3"), user("recent")),
    ];
    let cache = TokenCache::default();
    assert_eq!(cache.estimate_raw_entry(&entries[3]), 0, "an empty branch summary costs nothing");

    // Budget walk: e4 = 2 tokens, e3 = 0 (skipped), e2 = 10 ⇒ crosses 8 at e2, snapping forward.
    let cut = find_cut_point(&entries, &cache, 0, entries.len(), 8);
    assert_eq!(cut.first_kept_index, 3, "back-scan folds the invisible entry in — got {cut:?}");
    assert!(
        cut.is_split_turn,
        "the empty branch summary does not start a turn, so the cut splits e0's turn — got {cut:?}"
    );
    assert_eq!(cut.turn_start_index, Some(0), "the turn starts at the user message e0");
}

#[tokio::test]
async fn f1_empty_branch_summary_compaction_summarizes_the_split_turn() {
    // Observable end-to-end consequence of the predicate above.
    let summ = RecordingSummarizer::new(vec!["PREFIX-SUMMARY"]);
    let compactor = Compactor::new(summ, NoHooks);
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 100, keep_recent_tokens: 8 };

    let cwd = PathBuf::from("/proj/f1");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("please refactor the parser module thoroughly")).unwrap();
    m.append_message(assistant_tool("read", "src/main.rs")).unwrap();
    m.append_message(tool_result(
        "read",
        "src/main.rs",
        "0123456789012345678901234567890123456789",
    ))
    .unwrap();
    let from = m.leaf_id().cloned().unwrap();
    m.append_branch_summary(from, String::new(), None, None, false).unwrap();
    m.append_message(user("recent")).unwrap();

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
        .expect("compaction runs");

    let prompts = compactor.summarizer().prompts();
    assert_eq!(prompts.len(), 1, "only the turn-prefix half is summarized: {prompts:?}");
    assert!(
        prompts[0].contains("This is the PREFIX of a turn"),
        "the single call is the turn-prefix prompt: {}",
        prompts[0]
    );
    assert!(
        entry.summary.starts_with("No prior history."),
        "no history precedes the split turn: {}",
        entry.summary
    );
    assert!(
        entry.summary.contains("**Turn Context (split turn):**"),
        "the split-turn marker is present: {}",
        entry.summary
    );

    // The built context keeps only the empty branch summary (invisible) + the recent user message.
    let ctx = m.build_context();
    let joined = ctx.messages.iter().map(first_text).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("recent"), "the kept tail survives: {joined}");
    assert!(!joined.contains("refactor the parser"), "the summarized history is gone: {joined}");
}

// ------------------------------------------------------------------ F-2 -----------------------

#[test]
fn f2_custom_role_message_entry_starts_a_turn() {
    // `isTurnStartMessage` returns true for role `custom` (`compaction.ts:323-336`). cyrup's
    // `AgentMessage::is_turn_start` covered only `user`/`bashExecution`, so a cut landing on a
    // custom-role message was mis-reported as a mid-turn split.
    let entries = vec![
        msg_entry("e0", None, user("older question about the parser")),
        msg_entry("e1", Some("e0"), assistant("older answer")),
        agent_msg_entry(
            "e2",
            Some("e1"),
            AgentMessage::Custom(crate::agent_message::CustomRoleMessage {
                custom_type: "ext.note".to_string(),
                content: json!("please continue"),
                display: true,
                details: None,
                timestamp: 0,
            }),
        ),
    ];
    let cut = find_cut_point(&entries, &TokenCache::default(), 0, entries.len(), 3);
    assert_eq!(cut.first_kept_index, 2);
    assert!(!cut.is_split_turn, "a custom-role message is a clean turn boundary — got {cut:?}");
    assert_eq!(cut.turn_start_index, None);
}

#[test]
fn f2_bash_execution_cut_is_not_a_split_turn() {
    // Regression lock on behavior that is already correct: `bashExecution` is a turn start, so a cut
    // landing on one must NOT be a split turn (which would burn an extra summarization call and emit
    // a bogus "**Turn Context (split turn):**" section).
    let entries = vec![
        msg_entry("e0", None, user("older question about the parser")),
        msg_entry("e1", Some("e0"), assistant("older answer")),
        agent_msg_entry("e2", Some("e1"), bash_msg("npm test", "all green here ok", false)),
    ];
    let cut = find_cut_point(&entries, &TokenCache::default(), 0, entries.len(), 3);
    assert_eq!(cut.first_kept_index, 2);
    assert!(!cut.is_split_turn, "a bashExecution starts a turn — got {cut:?}");
    assert_eq!(cut.turn_start_index, None);
}

// ------------------------------------------------------------------ F-3 -----------------------

/// Build the F-3 fixture session: two bash entries (one `!!`-excluded) and a `custom_message`
/// followed by one recent turn. With `keep_recent_tokens = 7` the cut lands on the recent user
/// message, so the whole prefix is the summarized history.
fn f3_session() -> SessionManager {
    let cwd = PathBuf::from("/proj/f3");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_agent_message(bash_msg("npm test", "ok", false)).unwrap();
    m.append_agent_message(bash_msg("cat secret", "s3cr3t", true)).unwrap();
    m.append_custom_message("ext.note", json!("injected note"), true, None).unwrap();
    m.append_message(user("recent question here")).unwrap();
    m.append_message(assistant("recent answer here")).unwrap();
    m
}

#[tokio::test]
async fn f3_before_compact_event_carries_raw_agent_messages() {
    // Pi's `getMessageFromEntryForCompaction` returns `sessionEntryToContextMessages(entry)[0]` — a
    // RAW `AgentMessage` with its role intact (`compaction.ts:80-85`), and `convertToLlm` is applied
    // later, inside `generateSummary`. cyrup rendered to core `Message`s in `prepareCompaction`, so a
    // guest saw `{role:"user", text:"Ran `npm test`…"}`, could not read `customType`, and never saw
    // `!!`-excluded commands at all.
    let summ = RecordingSummarizer::new(vec!["HISTORY-SUMMARY"]);
    let compactor = Compactor::new(summ, CapturingHooks::default());
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 100, keep_recent_tokens: 7 };
    let mut m = f3_session();

    compactor
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
        .expect("compaction runs");

    let events = compactor.hooks().events.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    let msgs = serde_json::to_value(&events[0].messages_to_summarize).unwrap();
    let arr = msgs.as_array().expect("array");
    assert_eq!(arr.len(), 3, "every projected entry is present, `!!` included: {msgs}");
    assert_eq!(arr[0]["role"], "bashExecution", "roles are preserved: {msgs}");
    assert_eq!(arr[0]["command"], "npm test");
    assert_eq!(arr[0]["output"], "ok");
    assert_eq!(arr[0]["exitCode"], 0);
    assert_eq!(arr[1]["role"], "bashExecution");
    assert_eq!(arr[1]["command"], "cat secret");
    assert_eq!(arr[1]["excludeFromContext"], true);
    assert_eq!(arr[2]["role"], "custom", "a custom_message keeps its role: {msgs}");
    assert_eq!(arr[2]["customType"], "ext.note");
    assert_eq!(arr[2]["content"], "injected note");

    // ...and the summarization prompt text is UNCHANGED: `convertToLlm` still runs, just later.
    let prompts = compactor.summarizer().prompts();
    assert_eq!(prompts.len(), 1, "one summarization call: {prompts:?}");
    assert!(prompts[0].contains("[User]: Ran `npm test`"), "bash renders as before: {}", prompts[0]);
    assert!(prompts[0].contains("[User]: injected note"), "custom renders as before: {}", prompts[0]);
    assert!(
        !prompts[0].contains("cat secret"),
        "an `!!` command is still excluded from the LLM transcript: {}",
        prompts[0]
    );
}

#[test]
fn f3_history_of_only_excluded_bash_still_compacts() {
    // Pi's `if (messagesToSummarize.length === 0 && turnPrefixMessages.length === 0) return undefined`
    // counts an `excludeFromContext` bash message as content (it is a raw `AgentMessage`). cyrup had
    // already dropped it in `convertToLlm`, so a history made entirely of `!!` commands looked empty
    // and compaction silently did not run.
    let entries = vec![
        agent_msg_entry("e0", None, bash_msg("cat secret", "s3cr3t", true)),
        agent_msg_entry("e1", Some("e0"), bash_msg("cat other", "s3cr3t2", true)),
        msg_entry("e2", Some("e1"), user("recent question here")),
        msg_entry("e3", Some("e2"), assistant("recent answer here")),
    ];
    let settings = CompactionSettings { enabled: true, reserve_tokens: 100, keep_recent_tokens: 7 };
    let prep = prepare_compaction(&entries, &TokenCache::default(), &settings)
        .expect("a history of `!!` bash entries is still compactable");
    assert_eq!(prep.first_kept_entry_id, EntryId::from("e2"));
    assert_eq!(prep.messages_to_summarize.len(), 2, "both excluded commands are carried");
}

#[tokio::test]
async fn f3_compaction_is_append_only_on_disk() {
    // Constraint: the JSONL record is append-only and lossless — compaction appends ONE entry and
    // never rewrites history (DI-9 / R-05-011).
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f3_append");
    let lay = layout(root.path(), &cwd);
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_agent_message(bash_msg("npm test", "ok", false)).unwrap();
    m.append_agent_message(bash_msg("cat secret", "s3cr3t", true)).unwrap();
    m.append_custom_message("ext.note", json!("injected note"), true, None).unwrap();
    m.append_message(user("recent question here")).unwrap();
    m.append_message(assistant("recent answer here")).unwrap();

    let file = m.session_file().unwrap().to_path_buf();
    let before = std::fs::read_to_string(&file).unwrap();

    let compactor = Compactor::new(RecordingSummarizer::new(vec!["HISTORY-SUMMARY"]), NoHooks);
    let settings = CompactionSettings { enabled: true, reserve_tokens: 100, keep_recent_tokens: 7 };
    compactor
        .run_compaction(
            &mut m,
            &faux_model(),
            &settings,
            CompactionReason::Manual,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("compaction runs");

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.starts_with(&before), "existing JSONL bytes are untouched");
    let added: Vec<&str> = after[before.len()..].lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(added.len(), 1, "exactly one line was appended: {added:?}");
    let v: serde_json::Value = serde_json::from_str(added[0]).unwrap();
    assert_eq!(v["type"], "compaction");
}

#[test]
fn context_message_role_stays_in_lockstep_with_the_raw_projection() {
    // `context_message_role` is the no-clone classification half of `raw_context_messages`; the
    // cut-point layer trusts them to agree. Drift between the two is exactly the defect the live
    // fork removed (an entry classified as a cut point / turn start while projecting no context),
    // so pin the invariant across every entry kind cyrup can hold.
    let entries = vec![
        msg_entry("m0", None, user("hello")),
        msg_entry("m1", None, assistant("hi")),
        msg_entry("m2", None, tool_result("read", "a.rs", "body")),
        agent_msg_entry("m3", None, bash_msg("ls", "a.rs", false)),
        agent_msg_entry("m4", None, bash_msg("cat secret", "s3cr3t", true)),
        agent_msg_entry(
            "m5",
            None,
            AgentMessage::Custom(crate::agent_message::CustomRoleMessage {
                custom_type: "ext.note".to_string(),
                content: json!("note"),
                display: true,
                details: None,
                timestamp: 0,
            }),
        ),
        custom_message_entry("m6", None, "injected"),
        branch_summary_entry("m7", None, "a summary"),
        branch_summary_entry("m8", None, ""),
        Entry::known(KnownEntry::Compaction {
            base: EntryBase {
                id: EntryId::from("m9"),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            summary: "prior".to_string(),
            first_kept_entry_id: Some(EntryId::from("m0")),
            tokens_before: 10,
            details: None,
            usage: None,
            from_hook: None,
        }),
        Entry::known(KnownEntry::ModelChange {
            base: EntryBase {
                id: EntryId::from("m10"),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            provider: "p".into(),
            model_id: "m".into(),
        }),
        Entry::known(KnownEntry::Custom {
            base: EntryBase {
                id: EntryId::from("m11"),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            custom_type: "ext.state".to_string(),
            data: None,
        }),
    ];

    for e in &entries {
        let projected = crate::context::raw_context_messages(e);
        let role = crate::context::context_message_role(e);
        assert_eq!(
            role.is_some(),
            !projected.is_empty(),
            "visibility disagrees for {:?}",
            e.id()
        );
        assert_eq!(
            role,
            projected.first().map(AgentMessage::role),
            "role disagrees for {:?}",
            e.id()
        );
    }

    // The two entries that the harness fork got wrong, spelled out.
    assert_eq!(crate::context::context_message_role(&entries[8]), None, "empty branch_summary");
    assert_eq!(
        crate::context::context_message_role(&entries[5]),
        Some(crate::MessageRole::Custom),
        "custom-role message entry"
    );
}

// ------------------------------------------------------------------ F-4 -----------------------

/// A `Usage` with the fields these tests care about set and the conditional ones absent.
fn usage_of(input: u64, output: u64, cost_total: f64) -> Usage {
    Usage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output,
        cost: cyrup_core::Cost {
            input: cost_total,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: cost_total,
        },
    }
}

/// A summarizer whose completions carry scripted `Usage`s, so the persisted
/// `CompactionEntry.usage` can be checked against the exact spend of the exact calls made.
struct UsageSummarizer {
    usages: Mutex<VecDeque<Usage>>,
    calls: Mutex<usize>,
}

impl UsageSummarizer {
    fn new(usages: Vec<Usage>) -> Self {
        Self { usages: Mutex::new(usages.into()), calls: Mutex::new(0) }
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

impl Summarizer for UsageSummarizer {
    async fn complete(
        &self,
        _req: SummarizationRequest<'_>,
        _cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        *self.calls.lock().unwrap() += 1;
        let u = self.usages.lock().unwrap().pop_front().unwrap_or_default();
        let mut msg = faux_assistant_message(vec![faux_text("SUMMARY")], StopReason::Stop);
        msg.usage = u;
        Ok(msg)
    }
}

/// The compaction entry of a session, as it was written to the JSONL file.
fn compaction_line(m: &SessionManager) -> serde_json::Value {
    let path = m.session_file().expect("persisted session");
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "compaction")
        .expect("a compaction line on disk")
}

#[tokio::test]
async fn f4_compaction_entry_records_the_summed_usage_of_a_split_turn() {
    // Pi threads the summarization spend end to end — `CompactionResult.usage`
    // (`compaction.ts:88-89`) → `appendCompaction(..., usage)` (`session-manager.ts:1096-1116`) →
    // the persisted `CompactionEntry.usage` (`session-manager.ts:69-80`). On a SPLIT turn BOTH
    // calls are billed and Pi records `combineUsage(historyUsage, turnPrefixResult.usage)`
    // (`compaction.ts:877`). cyrup discarded the `AssistantMessage` after reading its text, so the
    // single most expensive operation a session performs left no trace on disk.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f4_usage");
    let lay = layout(root.path(), &cwd);

    let history = usage_of(100, 20, 0.50);
    let mut prefix = usage_of(7, 3, 0.25);
    prefix.cache_read = 11;
    prefix.reasoning = Some(4); // present on ONE side only — Pi still emits the merged key
    let compactor = Compactor::new(UsageSummarizer::new(vec![history, prefix]), NoHooks);
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 };

    // The A-05-3 transcript: two complete turns then one oversized final turn, so the cut lands
    // mid-turn and both summarization halves run.
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("first turn question short")).unwrap();
    m.append_message(assistant("first turn answer short")).unwrap();
    m.append_message(user("second turn question short")).unwrap();
    m.append_message(assistant(&"x ".repeat(120))).unwrap();

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
        .expect("split-turn compaction produces an entry");

    assert_eq!(compactor.summarizer().calls(), 2, "history + turn-prefix halves both ran");
    let recorded = entry.usage.clone().expect("the entry records the summarization spend");
    assert_eq!(recorded.input, 107, "inputs of both calls are summed");
    assert_eq!(recorded.output, 23);
    assert_eq!(recorded.cache_read, 11);
    assert_eq!(recorded.total_tokens, 130);
    assert!((recorded.cost.total - 0.75).abs() < 1e-9, "costs are summed: {}", recorded.cost.total);
    assert_eq!(recorded.reasoning, Some(4), "a one-sided optional field still merges");
    assert_eq!(recorded.cache_write_1h, None, "an absent-on-both optional stays absent");

    // On disk, in Pi's camelCase shape, on the compaction line itself.
    let line = compaction_line(&m);
    assert_eq!(line["usage"]["input"], 107);
    assert_eq!(line["usage"]["totalTokens"], 130);
    assert_eq!(line["usage"]["reasoning"], 4);
    assert!(
        line["usage"].get("cacheWrite1h").is_none(),
        "Pi's conditional spread omits the key entirely: {}",
        line["usage"]
    );

    // And it survives a reload — this is the audit trail the field exists for.
    let reopened = SessionManager::open(m.session_file().unwrap()).unwrap();
    let reloaded = reopened
        .entries()
        .iter()
        .find_map(|e| match e {
            Entry::Known(KnownEntry::Compaction { usage, .. }) => Some(usage.clone()),
            _ => None,
        })
        .expect("compaction entry reloads");
    assert_eq!(reloaded, Some(recorded));
}

#[tokio::test]
async fn f4_a_single_summarization_call_records_exactly_its_own_usage() {
    // The other half of `combineUsage`'s guard: when the history summary is skipped (Pi's
    // `historyUsage ? combineUsage(...) : turnPrefixResult.usage`, `compaction.ts:877`) the entry
    // must carry the turn-prefix call's usage VERBATIM — no zero-valued phantom addend, and no
    // optional field materialized as `0`.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f4_single");
    let lay = layout(root.path(), &cwd);

    let only = usage_of(64, 8, 0.125);
    let compactor = Compactor::new(UsageSummarizer::new(vec![only.clone()]), NoHooks);
    let model = faux_model();
    let settings = CompactionSettings { enabled: true, reserve_tokens: 100, keep_recent_tokens: 8 };

    // The F-1 transcript: an empty branch summary makes the cut a split turn with NO history half.
    let mut m = SessionManager::create(&cwd, &lay, NewSessionOpts::default()).unwrap();
    m.append_message(user("please refactor the parser module thoroughly")).unwrap();
    m.append_message(assistant_tool("read", "src/main.rs")).unwrap();
    m.append_message(tool_result(
        "read",
        "src/main.rs",
        "0123456789012345678901234567890123456789",
    ))
    .unwrap();
    let from = m.leaf_id().cloned().unwrap();
    m.append_branch_summary(from, String::new(), None, None, false).unwrap();
    m.append_message(user("recent")).unwrap();

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
        .expect("compaction runs");

    assert_eq!(compactor.summarizer().calls(), 1, "only the turn-prefix half is summarized");
    assert!(entry.summary.starts_with("No prior history."), "no history half ran");
    assert_eq!(entry.usage, Some(only), "the lone call's usage is recorded verbatim");
    let line = compaction_line(&m);
    assert_eq!(line["usage"]["input"], 64);
    assert!(line["usage"].get("reasoning").is_none(), "absent optionals stay absent");
}

#[test]
fn f4_a_pi_written_usage_survives_the_jsonl_roundtrip() {
    // R-00-013: cyrup must re-export a Pi-written session unchanged. Before `usage` existed on
    // `KnownEntry::Compaction`, serde silently dropped it on read, so rewriting a Pi session lost
    // the compaction's cost record.
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("pi-usage.jsonl");
    let contents = concat!(
        r#"{"type":"session","version":3,"id":"11111111-1111-7111-8111-111111111111","timestamp":"2026-01-01T00:00:00Z","cwd":"/proj/x"}"#,
        "\n",
        r#"{"type":"message","id":"aaaa1111","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":0}}"#,
        "\n",
        r#"{"type":"compaction","id":"bbbb2222","parentId":"aaaa1111","timestamp":"2026-01-01T00:00:02Z","summary":"PRIOR","firstKeptEntryId":"aaaa1111","tokensBefore":900,"usage":{"input":11,"output":22,"cacheRead":3,"cacheWrite":4,"totalTokens":40,"cost":{"input":0.1,"output":0.2,"cacheRead":0.0,"cacheWrite":0.0,"total":0.3}},"fromHook":false}"#,
        "\n",
    );
    std::fs::write(&file, contents).unwrap();

    let m = SessionManager::open(&file).unwrap();
    let usage = m
        .entries()
        .iter()
        .find_map(|e| match e {
            Entry::Known(KnownEntry::Compaction { usage, .. }) => usage.clone(),
            _ => None,
        })
        .expect("the Pi-written usage is parsed, not dropped");
    assert_eq!(usage.input, 11);
    assert_eq!(usage.total_tokens, 40);
    assert!((usage.cost.total - 0.3).abs() < 1e-9);

    let mut buf = Vec::new();
    m.export_jsonl(&mut buf).unwrap();
    let out = String::from_utf8(buf).unwrap();
    let line: serde_json::Value = out
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["type"] == "compaction")
        .expect("compaction line on export");
    assert_eq!(line["usage"]["input"], 11);
    assert_eq!(line["usage"]["cacheWrite"], 4);
    assert_eq!(line["usage"]["cost"]["total"], 0.3);
}

// ------------------------------------------------------------------ F-5 -----------------------

#[test]
fn f5_an_earlier_compaction_inside_the_kept_window_stays_in_the_context() {
    // Pi's `buildContextEntries` puts the LATEST compaction at the head of the context list and then
    // iterates `path[0..compactionIdx]` from `firstKeptEntryId` (`session-manager.ts:441-453`);
    // every one of those entries is projected through `sessionEntryToContextMessages`, which has an
    // explicit `compaction` arm (`session-manager.ts:404-406`). So an EARLIER compaction that falls
    // inside the newer one's kept window re-emits its summary. cyrup's per-entry projection had no
    // `compaction` arm, silently deleting that summary from what the model sees.
    let cwd = PathBuf::from("/proj/f5");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("oldest question")).unwrap();
    m.append_message(assistant("oldest answer")).unwrap();
    let keep = m.append_message(user("kept question")).unwrap();
    m.append_message(assistant("kept answer")).unwrap();
    m.append_compaction("SUMMARY-ONE".to_string(), keep.clone(), 100, None, None, false).unwrap();
    m.append_message(user("later question")).unwrap();
    m.append_message(assistant("later answer")).unwrap();
    // A second compaction whose cut lands BEFORE the first compaction entry — what happens on a
    // small context window, where `keep_recent_tokens` is not reachable inside window − reserve.
    m.append_compaction("SUMMARY-TWO".to_string(), keep.clone(), 200, None, None, false).unwrap();
    m.append_message(user("newest question")).unwrap();

    let texts: Vec<String> = m.build_context().messages.iter().map(first_text).collect();
    assert_eq!(
        texts.len(),
        7,
        "latest summary + 2 kept + the earlier summary + 2 after + 1 newest: {texts:?}"
    );
    assert!(texts[0].contains("SUMMARY-TWO"), "the governing summary leads: {texts:?}");
    assert_eq!(texts[1], "kept question");
    assert_eq!(texts[2], "kept answer");
    assert!(
        texts[3].contains("SUMMARY-ONE"),
        "the earlier compaction's summary is still in context: {texts:?}"
    );
    assert!(
        texts[3].contains("The conversation history before this point was compacted"),
        "and it is wrapped exactly like any compaction summary: {}",
        texts[3]
    );
    assert_eq!(texts[4], "later question");
    assert_eq!(texts[5], "later answer");
    assert_eq!(texts[6], "newest question");
    assert_eq!(
        texts.iter().filter(|t| t.contains("SUMMARY-TWO")).count(),
        1,
        "the governing compaction is emitted once, never doubled: {texts:?}"
    );

    // The raw projection that MEASURES the context must see the same two summaries the built
    // context contains — otherwise `tokens_before` / `should_compact` disagree with reality.
    let path: Vec<&Entry> = m.entries().iter().collect();
    let raw = build_context_agent_messages(&path);
    assert_eq!(
        raw.iter().filter(|msg| matches!(msg, AgentMessage::CompactionSummary(_))).count(),
        2,
        "both compaction summaries are measured: {raw:?}"
    );
    assert_eq!(raw.len(), texts.len(), "measured and rendered projections have the same shape");
}

#[test]
fn f5_a_compaction_outside_the_kept_window_is_still_dropped() {
    // The negative control for the arm above: an earlier compaction is re-emitted only because it
    // falls inside the newer cut's range. Pi's loop starts at `firstKeptEntryId`, so a compaction
    // before that point is summarized away like any other entry.
    let cwd = PathBuf::from("/proj/f5b");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    let q1 = m.append_message(user("oldest question")).unwrap();
    m.append_message(assistant("oldest answer")).unwrap();
    m.append_compaction("SUMMARY-ONE".to_string(), q1, 100, None, None, false).unwrap();
    let later = m.append_message(user("later question")).unwrap();
    m.append_message(assistant("later answer")).unwrap();
    m.append_compaction("SUMMARY-TWO".to_string(), later, 200, None, None, false).unwrap();

    let texts: Vec<String> = m.build_context().messages.iter().map(first_text).collect();
    assert_eq!(texts.len(), 3, "latest summary + the two kept messages: {texts:?}");
    assert!(texts[0].contains("SUMMARY-TWO"));
    assert!(
        !texts.iter().any(|t| t.contains("SUMMARY-ONE")),
        "the superseded summary is outside the kept window: {texts:?}"
    );
}

// ------------------------------------------------------------------ F-6 -----------------------
// Summarization request shaping: Pi routes EVERY compaction / turn-prefix / branch-summary call
// through `completeSummarization` (`compaction.ts:555-581`), which (a) retries transient stream
// drops under the configured policy, (b) isolates the request from the session's prompt cache and
// cache routing, and (c) carries the reasoning level `createSummarizationOptions` computed.

/// Captures the resolved `StreamOptions` of every summarization call the faux provider sees.
#[derive(Clone, Default)]
struct OptionSpy(Arc<Mutex<Vec<StreamOptions>>>);

impl OptionSpy {
    fn seen(&self) -> Vec<StreamOptions> {
        self.0.lock().unwrap().clone()
    }
    /// A scripted step that records the options and answers with `body`.
    fn step(&self, body: &'static str) -> FauxResponseStep {
        let sink = self.0.clone();
        FauxResponseStep::factory(move |_ctx, opts, _state, _model| {
            sink.lock().unwrap().push(opts.clone());
            faux_assistant_message(vec![faux_text(body)], StopReason::Stop)
        })
    }
}

/// A step that fails the way a dropped socket does — Pi classifies `terminated` as retryable
/// (`retry.ts:63`).
fn transient_failure_step() -> FauxResponseStep {
    FauxResponseStep::factory(|_ctx, _opts, _state, _model| {
        let mut msg = faux_assistant_message(vec![], StopReason::Error);
        msg.error_message = Some("terminated".to_string());
        msg
    })
}

/// A deterministic failure Pi refuses to retry (`retry.ts:20`).
fn quota_failure_step() -> FauxResponseStep {
    FauxResponseStep::factory(|_ctx, _opts, _state, _model| {
        let mut msg = faux_assistant_message(vec![], StopReason::Error);
        msg.error_message = Some("insufficient_quota: add credits".to_string());
        msg
    })
}

/// A four-turn transcript that compacts under `f6_settings()`, plus the layout it lives in.
fn f6_session(root: &Path, cwd: &Path) -> SessionManager {
    let lay = layout(root, cwd);
    let mut m = SessionManager::create(cwd, &lay, NewSessionOpts::default()).unwrap();
    for i in 0..4 {
        m.append_message(user(&format!("question number {i} with several words to add some size")))
            .unwrap();
        m.append_message(assistant(&format!(
            "answer number {i} with several words to add some size as well"
        )))
        .unwrap();
    }
    m
}

fn f6_settings() -> CompactionSettings {
    CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 40 }
}

#[tokio::test]
async fn f6_a_transient_stream_drop_is_retried_and_the_compaction_still_lands() {
    // Pi wraps the summarization producer in `retryAssistantCall` "so transient stream drops (e.g.
    // `terminated`, socket close) honor the configured retry policy instead of failing the whole
    // compaction on the first attempt" (`compaction.ts:555-560`). Without it a single dropped
    // socket aborts the compaction and, on the overflow path, strands the session over-window.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f6_retry");

    let faux = Arc::new(FauxProvider::new());
    // This transcript compacts as a SPLIT turn, so two summarization calls are expected; the drop
    // hits the first (history) half.
    faux.set_response_steps(vec![
        transient_failure_step(),
        faux_assistant_message(vec![faux_text("HISTORY-OK")], StopReason::Stop).into(),
        faux_assistant_message(vec![faux_text("PREFIX-OK")], StopReason::Stop).into(),
    ]);
    let model = faux.model().clone();
    let compactor = Compactor::new(
        ProviderSummarizer::new(faux.clone(), model.clone())
            .with_retry(RetryPolicy::new(true, 3, 0)),
        NoHooks,
    );

    let mut m = f6_session(root.path(), &cwd);
    let entries_before = m.entries().len();

    let entry = compactor
        .run_compaction(
            &mut m,
            &model,
            &f6_settings(),
            CompactionReason::Threshold,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("the retried attempt produces a compaction entry");

    assert_eq!(faux.call_count(), 3, "two summarization halves, the first one retried once");
    assert!(
        entry.summary.contains("HISTORY-OK"),
        "the SUCCEEDING attempt's text is what is stored: {}",
        entry.summary
    );
    assert!(entry.summary.contains("PREFIX-OK"), "and the second half still ran");

    // Observable end state: the summary heads the rebuilt context, and the append-only JSONL grew
    // by exactly the one compaction entry.
    let ctx = m.build_context();
    assert!(
        first_text(&ctx.messages[0]).contains("HISTORY-OK"),
        "context leads with the summary"
    );
    assert!(ctx.messages.len() < entries_before, "context is reduced vs the full history");
    assert_eq!(m.entries().len(), entries_before + 1);
    let reopened = SessionManager::open(m.session_file().unwrap()).unwrap();
    assert_eq!(reopened.entries().len(), entries_before + 1, "history is intact on reload");
}

#[tokio::test]
async fn f6_a_with_retries_disabled_the_same_drop_kills_the_compaction() {
    // The negative control: `retry: undefined` returns the first response unchanged
    // (`retry.ts:159-160`). This is the behavior cyrup had unconditionally.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f6_noretry");

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        transient_failure_step(),
        faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop).into(),
    ]);
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);

    let mut m = f6_session(root.path(), &cwd);
    let entries_before = m.entries().len();
    let before: Vec<String> = m.build_context().messages.iter().map(first_text).collect();

    let err = compactor
        .run_compaction(
            &mut m,
            &model,
            &f6_settings(),
            CompactionReason::Threshold,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .expect_err("the first drop fails the whole compaction");
    assert!(matches!(err, CompactionError::Summarization(_)), "{err:?}");

    assert_eq!(faux.call_count(), 1, "no second attempt was made");
    assert!(!has_compaction(&m), "nothing was appended");
    assert_eq!(m.entries().len(), entries_before);
    let after: Vec<String> = m.build_context().messages.iter().map(first_text).collect();
    assert_eq!(after, before, "the built context is untouched by the failed compaction");
}

#[tokio::test]
async fn f6_a_a_quota_error_fails_fast_even_with_retries_enabled() {
    // `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN` wins over the retryable set (`retry.ts:225`), so
    // a billing/quota failure must not burn the retry budget with backoff sleeps.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f6_quota");

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        quota_failure_step(),
        faux_assistant_message(vec![faux_text(FULL_SUMMARY)], StopReason::Stop).into(),
    ]);
    let model = faux.model().clone();
    let compactor = Compactor::new(
        ProviderSummarizer::new(faux.clone(), model.clone())
            .with_retry(RetryPolicy::new(true, 5, 60_000)),
        NoHooks,
    );

    let mut m = f6_session(root.path(), &cwd);
    let entries_before = m.entries().len();

    let err = compactor
        .run_compaction(
            &mut m,
            &model,
            &f6_settings(),
            CompactionReason::Threshold,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .expect_err("a deterministic provider error is terminal");
    assert!(matches!(err, CompactionError::Summarization(_)), "{err:?}");
    assert_eq!(faux.call_count(), 1, "no retry, and no 60s backoff sleep");
    assert!(!has_compaction(&m));
    assert_eq!(m.entries().len(), entries_before);
}

#[tokio::test]
async fn f6_b_summarization_is_isolated_from_the_session_cache_and_routing() {
    // "Summaries are standalone requests, so isolate routing and avoid cache writes that cannot be
    // reused" — `cacheRetention: "none"` + a fresh `sessionId` per call (`compaction.ts:570-575`).
    // Leaving `cache_retention` unset lets the encoder resolve it from `PI_CACHE_RETENTION`
    // (defaulting to Short), billing a cache write on a one-shot request.
    let root = tempfile::tempdir().unwrap();
    let cwd = PathBuf::from("/proj/f6_cache");

    let spy = OptionSpy::default();
    let faux = Arc::new(FauxProvider::new());
    // Enough scripted steps for both rounds whether or not either cut splits a turn.
    faux.set_response_steps((0..6).map(|_| spy.step(FULL_SUMMARY)).collect());
    let model = faux.model().clone();
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks);

    let mut m = f6_session(root.path(), &cwd);
    for round in 0..2 {
        let entry = compactor
            .run_compaction(
                &mut m,
                &model,
                &f6_settings(),
                CompactionReason::Threshold,
                None,
                false,
                CancelToken::new(),
            )
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("round {round} compacts"));
        assert!(entry.summary.contains("## Goal"));
        m.append_message(user("another question with several words to add some size")).unwrap();
        m.append_message(assistant("another answer with several words to add some size")).unwrap();
    }

    let seen = spy.seen();
    assert!(seen.len() >= 2, "at least one summarization call per round: {}", seen.len());
    for (i, opts) in seen.iter().enumerate() {
        assert_eq!(
            opts.cache_retention,
            Some(CacheRetention::None),
            "call {i} must not write a prompt-cache entry it can never read back"
        );
        assert!(opts.session_id.is_some(), "call {i} carries its own routing id");
    }
    let mut ids: Vec<String> = seen
        .iter()
        .filter_map(|o| o.session_id.as_ref().map(|id| id.as_str().to_string()))
        .collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "each summarization gets a FRESH id, not the session's own affinity"
    );
    let session_id = m.header().id.as_str().to_string();
    assert!(
        !ids.contains(&session_id),
        "the live session id is never reused for a summarization"
    );

    // Both compactions are real: two entries on disk, and the newest summary governs the context.
    let compactions = m
        .entries()
        .iter()
        .filter(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
        .count();
    assert_eq!(compactions, 2);
    assert!(first_text(&m.build_context().messages[0]).contains("## Goal"));
}

#[tokio::test]
async fn f6_c_the_session_thinking_level_reaches_both_summarization_halves() {
    // `createSummarizationOptions` sets `options.reasoning = thinkingLevel` when the model supports
    // reasoning and the level is not "off" (`compaction.ts:549-551`), and BOTH halves of a split
    // turn go through it (`compaction.ts:858,875`). cyrup hardcoded `Off` and never read the field,
    // so summaries on a reasoning model were produced without reasoning.
    let spy = OptionSpy::default();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![spy.step("HISTORY-SUMMARY"), spy.step("PREFIX-SUMMARY")]);
    let mut model = faux.model().clone();
    model.reasoning = true;
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks)
        .with_thinking(ModelThinkingLevel::High);

    let cwd = PathBuf::from("/proj/f6_thinking");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("first turn question short")).unwrap();
    m.append_message(assistant("first turn answer short")).unwrap();
    m.append_message(user("second turn question short")).unwrap();
    m.append_message(assistant(&"x ".repeat(120))).unwrap();

    let entry = compactor
        .run_compaction(
            &mut m,
            &model,
            &CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 },
            CompactionReason::Manual,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("split-turn compaction produces an entry");

    let seen = spy.seen();
    assert_eq!(seen.len(), 2, "history half + turn-prefix half");
    for (i, opts) in seen.iter().enumerate() {
        assert_eq!(opts.reasoning, ModelThinkingLevel::High, "half {i} was summarized WITH thinking");
    }
    // And the merged text of both halves is what the session records + shows the model.
    assert!(entry.summary.contains("HISTORY-SUMMARY"));
    assert!(entry.summary.contains("PREFIX-SUMMARY"));
    assert!(first_text(&m.build_context().messages[0]).contains("PREFIX-SUMMARY"));
}

#[tokio::test]
async fn f6_c_thinking_is_withheld_from_non_reasoning_models_and_branch_summaries() {
    // Pi's gate is `model.reasoning && thinkingLevel && thinkingLevel !== "off"`; and branch
    // summaries build their options inline WITHOUT `createSummarizationOptions`
    // (`branch-summarization.ts:348`), so they never carry a reasoning level at all.
    let spy = OptionSpy::default();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![spy.step(FULL_SUMMARY), spy.step(FULL_SUMMARY)]);
    let mut model = faux.model().clone();
    model.reasoning = false;

    let cwd = PathBuf::from("/proj/f6_nothink");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("shared question with some words")).unwrap();
    let shared = m.append_message(assistant("shared answer with some words")).unwrap();
    m.append_message(user("branch one question with some words")).unwrap();
    let l1 = m.append_message(assistant("branch one answer with some words")).unwrap();
    m.branch(&shared).unwrap();
    m.append_message(user("branch two question with some words")).unwrap();
    let l2 = m.append_message(assistant("branch two answer with some words")).unwrap();

    // (a) A non-reasoning model never receives a level, however the session is configured.
    let compactor = Compactor::new(ProviderSummarizer::new(faux.clone(), model.clone()), NoHooks)
        .with_thinking(ModelThinkingLevel::High);
    compactor
        .run_compaction(
            &mut m,
            &model,
            &CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 20 },
            CompactionReason::Manual,
            None,
            false,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("compaction runs");

    // (b) A branch summary on a REASONING model still carries no level.
    let mut reasoning_model = faux.model().clone();
    reasoning_model.reasoning = true;
    let branch_compactor =
        Compactor::new(ProviderSummarizer::new(faux.clone(), reasoning_model.clone()), NoHooks)
            .with_thinking(ModelThinkingLevel::High);
    let summary = branch_compactor
        .run_branch_summary(
            &mut m,
            &reasoning_model,
            l2,
            Some(l1),
            true,
            &BranchSummarySettings { reserve_tokens: 16384, skip_prompt: false },
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("branch summary is appended");
    assert!(summary.summary.contains("## Goal"));

    let seen = spy.seen();
    assert_eq!(seen.len(), 2, "one compaction call + one branch-summary call");
    assert_eq!(seen[0].reasoning, ModelThinkingLevel::Off, "non-reasoning model gets no level");
    assert_eq!(seen[1].reasoning, ModelThinkingLevel::Off, "branch summaries never set reasoning");
}

#[tokio::test]
async fn f6_d_a_zero_context_window_still_caps_the_branch_summary_prompt() {
    // `const contextWindow = model.contextWindow || 128000` (`branch-summarization.ts:315`). Without
    // the fallback the budget is `0 - reserve` → saturates to 0, which `prepare_branch_entries`
    // reads as "no limit" — so a model with an unknown window would serialize an ENTIRE abandoned
    // branch into one prompt instead of capping it at 128000 − 16384 = 111616 tokens.
    const RESERVE: u32 = 16_384;
    // 12 messages × 10000 tokens = 120000 > 111616, so the newest-first walk must drop the oldest.
    let big = |marker: &str| format!("{marker} {}", "x".repeat(40_000 - marker.len() - 1));

    let summarizer = RecordingSummarizer::new(vec![FULL_SUMMARY, FULL_SUMMARY]);
    let compactor = Compactor::new(summarizer, NoHooks);

    let cwd = PathBuf::from("/proj/f6_budget");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(user("shared question")).unwrap();
    let shared = m.append_message(assistant("shared answer")).unwrap();
    for i in 0..12 {
        m.append_message(user(&big(&format!("ABANDONED-{i}")))).unwrap();
    }
    let abandoned_leaf = m.leaf_id().cloned().unwrap();
    m.branch(&shared).unwrap();
    let target = m.append_message(user("the branch we return to")).unwrap();

    let mut zero_window = faux_model();
    zero_window.context_window = 0;
    let settings = BranchSummarySettings { reserve_tokens: RESERVE, skip_prompt: false };

    let entry = compactor
        .run_branch_summary(
            &mut m,
            &zero_window,
            target.clone(),
            Some(abandoned_leaf.clone()),
            true,
            &settings,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("a branch summary is appended");
    assert!(entry.summary.contains("## Goal"));
    assert!(m.build_context().messages.iter().any(|msg| first_text(msg).contains("## Goal")));

    let prompt = compactor.summarizer().prompts().pop().expect("one summarization call");
    assert!(prompt.contains("ABANDONED-11"), "the newest abandoned work is always summarized");
    assert!(
        !prompt.contains("ABANDONED-0 "),
        "the oldest abandoned entry is over the 111616-token cap and must be dropped"
    );

    // Control: a model that DOES report a window big enough for the whole branch keeps all of it,
    // proving the truncation above comes from the 128000 fallback and not from some other limit.
    let mut wide = faux_model();
    wide.context_window = 400_000;
    let wide_compactor = Compactor::new(RecordingSummarizer::new(vec![FULL_SUMMARY]), NoHooks);
    let mut m2 = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m2.append_message(user("shared question")).unwrap();
    let shared2 = m2.append_message(assistant("shared answer")).unwrap();
    for i in 0..12 {
        m2.append_message(user(&big(&format!("ABANDONED-{i}")))).unwrap();
    }
    let leaf2 = m2.leaf_id().cloned().unwrap();
    m2.branch(&shared2).unwrap();
    let target2 = m2.append_message(user("the branch we return to")).unwrap();
    wide_compactor
        .run_branch_summary(
            &mut m2,
            &wide,
            target2,
            Some(leaf2),
            true,
            &settings,
            CancelToken::new(),
        )
        .await
        .unwrap()
        .expect("a branch summary is appended");
    let wide_prompt = wide_compactor.summarizer().prompts().pop().expect("one call");
    assert!(wide_prompt.contains("ABANDONED-0 "), "a 400k window fits the whole branch");
}

// ------------------------------------------- StopReason::Pending guard ------------------------

/// A summarizer whose response never settled (`StopReason::Pending`) with plausible-looking body
/// text — the shape a truncated summarization stream produces.
struct PendingSummarizer;

impl Summarizer for PendingSummarizer {
    async fn complete(
        &self,
        _req: SummarizationRequest<'_>,
        _cancel: CancelToken,
    ) -> Result<AssistantMessage, CompactionError> {
        Ok(faux_assistant_message(
            vec![faux_text("HALF A SUMMA")],
            StopReason::Pending,
        ))
    }
}

/// Compaction REPLACES history with the summary, so accepting an unsettled response destroys the
/// transcript it was supposed to preserve.
///
/// All three summarization call sites (`compaction::summarize` x2, `compaction::branch`, and the
/// branch-summary copy in `cyrup-session-svc/src/session.rs`) previously matched
/// `Error => .. , Aborted => .. , _ => Ok(summary)`. That `_` arm silently accepted
/// `StopReason::Pending` once the variant existed, which is the exact class of catch-all the
/// truncated-stream fix closed on the decoder side — so it is closed here too.
#[tokio::test]
async fn an_unsettled_summarizer_response_is_rejected_not_treated_as_a_summary() {
    let msgs = vec![
        AgentMessage::Core(user("q1")),
        AgentMessage::Core(assistant("a1")),
    ];

    let err = crate::compaction::generate_summary(
        &PendingSummarizer,
        &msgs,
        &faux_model(),
        1000,
        None,
        None,
        ModelThinkingLevel::Off,
        CancelToken::new(),
    )
    .await
    .expect_err("a pending summarization must not be accepted as a summary");
    assert!(
        matches!(err, CompactionError::Summarization(_)),
        "expected a summarization failure, got {err:?}"
    );

    let err = crate::compaction::generate_turn_prefix_summary(
        &PendingSummarizer,
        &msgs,
        &faux_model(),
        1000,
        ModelThinkingLevel::Off,
        CancelToken::new(),
    )
    .await
    .expect_err("a pending turn-prefix summarization must not be accepted");
    assert!(
        matches!(err, CompactionError::Summarization(_)),
        "expected a summarization failure, got {err:?}"
    );
}

/// The control: a SETTLED response on the same path is still accepted, so the guard above is not
/// simply "reject everything".
#[tokio::test]
async fn a_settled_summarizer_response_is_still_accepted() {
    let msgs = vec![
        AgentMessage::Core(user("q1")),
        AgentMessage::Core(assistant("a1")),
    ];
    let out = crate::compaction::generate_summary(
        &RecordingSummarizer::new(vec!["REAL SUMMARY"]),
        &msgs,
        &faux_model(),
        1000,
        None,
        None,
        ModelThinkingLevel::Off,
        CancelToken::new(),
    )
    .await
    .expect("a settled summarization must succeed");
    assert!(out.text.contains("REAL SUMMARY"));
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// G21: `fromHook` suppresses file-list inheritance — PIN, not a change.
//
// A hook/extension-supplied summary carries `details` of an ARBITRARY, extension-defined shape.
// Pi therefore refuses to mine it for `readFiles`/`modifiedFiles`, at both inheritance sites:
//
//   v0.84.1 coding-agent/src/core/compaction/compaction.ts:51-53
//       const prevCompaction = entries[prevCompactionIndex] as CompactionEntry;
//       if (!prevCompaction.fromHook && prevCompaction.details) {
//           // fromHook field kept for session file compatibility
//
//   v0.84.1 coding-agent/src/core/compaction/branch-summarization.ts:202-204
//       // Only extract from pi-generated summaries (fromHook !== true), not extension-generated ones
//       if (entry.type === "branch_summary" && !entry.fromHook && entry.details) {
//
// This is the LIVE fork and it is unchanged from v0.83.0. The harness fork
// (agent/src/harness/compaction/compaction.ts) DROPPED both guards at v0.84.1 in `44289550a`
// ("feat(agent): promote durable harness API") — but ONLY because that rewrite deleted the
// `fromHook` field from `CompactionEntry`/`BranchSummaryEntry`
// (v0.84.1 agent/src/harness/session/types.ts:44-60, no `fromHook`) AND deleted the compaction hook
// that produced it (`emitHook`/`hookResult` for compaction is absent from v0.84.1
// agent/src/harness/agent-harness.ts; cf. v0.83.0 agent-harness.ts:747-755,861,874). Nothing in the
// v0.84.1 harness can mint a `fromHook: true` entry, so dropping the read-side check there is a
// no-op, not a semantic reversal.
//
// cyrup KEEPS both the field (`entry.rs:91,105`) and a hook that sets it
// (`compaction/hooks.rs:35,53`; `compaction/mod.rs:295,433,456`), matching the live fork. Removing
// the guard here would therefore match NEITHER upstream: it would feed extension-shaped `details`
// into pi's `{readFiles, modifiedFiles}` reader. The guards at `prepare.rs:92` and
// `branch.rs:178-179` are correct; these tests exist because nothing covered them.

fn compaction_entry_with_details(
    id: &str,
    parent: Option<&str>,
    first_kept: &str,
    details: serde_json::Value,
    from_hook: Option<bool>,
) -> Entry {
    Entry::known(KnownEntry::Compaction {
        base: EntryBase {
            id: EntryId::from(id),
            parent_id: parent.map(EntryId::from),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        },
        summary: "PRIOR SUMMARY".to_string(),
        first_kept_entry_id: Some(EntryId::from(first_kept)),
        tokens_before: 100,
        details: Some(details),
        usage: None,
        from_hook,
    })
}

fn prev_details() -> serde_json::Value {
    json!({ "readFiles": ["/proj/read-by-hook.rs"], "modifiedFiles": ["/proj/edited-by-hook.rs"] })
}

/// `prepare_compaction` must NOT inherit a `fromHook: true` compaction's `details`
/// (v0.84.1 coding-agent/src/core/compaction/compaction.ts:52).
#[test]
fn g21_prepare_compaction_ignores_from_hook_prev_details() {
    let settings = CompactionSettings { enabled: true, reserve_tokens: 10, keep_recent_tokens: 5 };
    let cache = TokenCache::default();

    let build = |from_hook: Option<bool>| {
        vec![
            msg_entry("e0", None, user("the first user message in this session")),
            compaction_entry_with_details("e1", Some("e0"), "e0", prev_details(), from_hook),
            msg_entry("e2", Some("e1"), user("history that will be summarized away now")),
            msg_entry("e3", Some("e2"), user("recent tail kept verbatim")),
        ]
    };

    // SUPPRESSED: hook-sourced details are not mined.
    let hooked = build(Some(true));
    let prep = prepare_compaction(&hooked, &cache, &settings).expect("history to summarize");
    let (read, modified) = prep.file_ops.compute_lists();
    assert!(
        read.is_empty() && modified.is_empty(),
        "fromHook=true must suppress inheritance, got read={read:?} modified={modified:?}"
    );

    // MIRROR — the very same details ARE inherited when the entry is pi-generated. Without this the
    // test above would also pass if inheritance were broken outright.
    for pi_generated in [None, Some(false)] {
        let entries = build(pi_generated);
        let prep = prepare_compaction(&entries, &cache, &settings).expect("history to summarize");
        let (read, modified) = prep.file_ops.compute_lists();
        assert_eq!(
            read,
            vec!["/proj/read-by-hook.rs".to_string()],
            "pi-generated details must be inherited (from_hook={pi_generated:?})"
        );
        assert_eq!(
            modified,
            vec!["/proj/edited-by-hook.rs".to_string()],
            "pi-generated details must be inherited (from_hook={pi_generated:?})"
        );
    }
}

/// `prepare_branch_entries` must NOT inherit a `fromHook: true` branch summary's `details`
/// (v0.84.1 coding-agent/src/core/compaction/branch-summarization.ts:204).
#[test]
fn g21_prepare_branch_entries_ignores_from_hook_details() {
    let build = |from_hook: Option<bool>| {
        vec![Entry::known(KnownEntry::BranchSummary {
            base: EntryBase {
                id: EntryId::from("b1"),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00Z".to_string(),
            },
            from_id: EntryId::from("from"),
            summary: "abandoned branch summary".to_string(),
            details: Some(prev_details()),
            usage: None,
            from_hook,
        })]
    };

    // SUPPRESSED.
    let prep = branch::prepare_branch_entries(&build(Some(true)), 0);
    let (read, modified) = prep.file_ops.compute_lists();
    assert!(
        read.is_empty() && modified.is_empty(),
        "fromHook=true branch summary must suppress inheritance, got read={read:?} modified={modified:?}"
    );

    // MIRROR — pi-generated branch summaries still seed the file lists.
    for pi_generated in [None, Some(false)] {
        let prep = branch::prepare_branch_entries(&build(pi_generated), 0);
        let (read, modified) = prep.file_ops.compute_lists();
        assert_eq!(read, vec!["/proj/read-by-hook.rs".to_string()], "from_hook={pi_generated:?}");
        assert_eq!(
            modified,
            vec!["/proj/edited-by-hook.rs".to_string()],
            "from_hook={pi_generated:?}"
        );
    }
}
