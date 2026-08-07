//! Round-5 facade parity tests for the unified `/tree` navigation op (gap #15, Pi
//! `agent-session.ts:2704-2895` `navigateTree`): re-editing a user message, the no-op short-circuit,
//! branch summarization with custom instructions, label attachment, the `session_before_tree` veto,
//! and the `SessionCommand::NavigateTree` seam.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{Content, EntryId, ExtensionId, Message, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, FauxProvider, FauxResponseStep,
};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    NavigateTreeOptions, SessionBuilder, SessionCommand, SessionCommandOutput, SessionConfig,
};
use tempfile::TempDir;

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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Concatenate the text of a core `user` message (the faux summary call's prompt lives here).
fn user_text(m: &Message) -> Option<String> {
    let Message::User { content, .. } = m else { return None };
    Some(
        content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    )
}

// ========================================================== #15 navigate_tree: re-edit a message ==

/// Navigating to a user message re-roots the leaf at its PARENT, returns the message text as
/// `editor_text`, and rebuilds the agent transcript (Pi agent-session.ts:2823-2872).
#[tokio::test]
async fn navigate_tree_reedit_user_message_returns_editor_text_and_truncates() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;

    // Two user/assistant pairs are on the branch.
    assert_eq!(session.messages().await.len(), 4, "u1,a1,u2,a2");
    let anchors = session.user_messages_for_forking().await;
    assert_eq!(anchors.len(), 2);
    let u2: EntryId = anchors[1].entry_id.clone();

    // Navigate to the SECOND user message → re-edit it: leaf moves to its parent (a1).
    let outcome = session
        .navigate_tree(u2, NavigateTreeOptions { summarize: false, ..Default::default() })
        .await
        .unwrap();
    assert!(!outcome.cancelled);
    assert!(!outcome.aborted);
    assert!(outcome.summary_entry.is_none(), "no summary when summarize=false");
    assert_eq!(outcome.editor_text.as_deref(), Some("second"), "the target text is re-editable");

    // The transcript is truncated to [u1, a1] and the agent's in-memory state mirrors it.
    assert_eq!(session.messages().await.len(), 2, "u2/a2 are off the active branch");
    assert_eq!(session.agent_messages().await.len(), 2, "agent transcript rebuilt from context");
}

// ====================================================================== #15 navigate_tree: no-op ==

/// Navigating to the current leaf is a no-op: `{cancelled:false}` with no editor text and no
/// transcript change (Pi agent-session.ts:2712).
#[tokio::test]
async fn navigate_tree_to_current_leaf_is_noop() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;

    let leaf = session.leaf_id().await.expect("a leaf exists");
    let before = session.messages().await.len();
    let outcome = session
        .navigate_tree(leaf, NavigateTreeOptions::default())
        .await
        .unwrap();
    assert!(!outcome.cancelled, "no-op is not a cancellation");
    assert!(outcome.editor_text.is_none());
    assert!(outcome.summary_entry.is_none());
    assert_eq!(session.messages().await.len(), before, "transcript unchanged");
}

// ================================================ #15 navigate_tree: summarize + customInstructions ==

/// Summarizing the abandoned branch appends a `BranchSummaryEntry`, and `custom_instructions` are
/// threaded into the summarizer prompt as an "Additional focus" (Pi branch-summarization.ts:322-323).
#[tokio::test]
async fn navigate_tree_with_summary_appends_entry_and_threads_custom_instructions() {
    let fx = fixture();
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)),
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a2")], StopReason::Stop)),
        // The third provider call is the branch summarizer; capture its prompt.
        FauxResponseStep::factory(move |ctx, _o, _s, _m| {
            if let Some(t) = ctx.messages.iter().find_map(user_text) {
                cap.lock().unwrap().push(t);
            }
            faux_assistant_message(vec![faux_text("BRANCH-BODY")], StopReason::Stop)
        }),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u1: EntryId = anchors[0].entry_id.clone();

    // Navigate back to the FIRST user message WITH summarization of the abandoned a1/u2/a2 branch.
    let outcome = session
        .navigate_tree(
            u1,
            NavigateTreeOptions {
                summarize: true,
                custom_instructions: Some("focus-on-the-files".to_string()),
                replace_instructions: false,
                label: None,
            },
        )
        .await
        .unwrap();
    assert!(!outcome.cancelled);
    assert!(!outcome.aborted);
    assert_eq!(outcome.editor_text.as_deref(), Some("first"));
    let entry = outcome.summary_entry.expect("a branch summary entry was appended");
    assert!(entry.summary.contains("BRANCH-BODY"), "summary wraps the model body: {}", entry.summary);

    // The summarizer prompt carried the custom instruction as an "Additional focus".
    {
        let prompts = captured.lock().unwrap();
        assert_eq!(prompts.len(), 1, "exactly one summarizer call");
        assert!(
            prompts[0].contains("Additional focus: focus-on-the-files"),
            "custom instructions are threaded into the summary prompt, got: {}",
            prompts[0]
        );
    }

    // The appended summary is durable in the exported JSONL.
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(jsonl.contains("branch_summary"), "the branch summary is persisted");
}

// ==================================================================== #15 navigate_tree: label ====

/// With no summary, the `label` lands on the navigation target as a persisted `label` entry (Pi
/// agent-session.ts:2867 `appendLabelChange(targetId, label)`).
#[tokio::test]
async fn navigate_tree_label_attaches_to_target() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u2: EntryId = anchors[1].entry_id.clone();

    let outcome = session
        .navigate_tree(
            u2,
            NavigateTreeOptions {
                summarize: false,
                label: Some("checkpoint-alpha".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!outcome.cancelled);

    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(jsonl.contains("checkpoint-alpha"), "the label is persisted on the target");
    assert!(jsonl.contains("\"type\":\"label\""), "a label entry was appended");
}

// ============================================================== #15 navigate_tree: before_tree veto ==

/// A native extension subscribed to `session_before_tree` that returns `Block` cancels the
/// navigation (Pi agent-session.ts:2757 `if (result?.cancel) return { cancelled: true }`).
struct TreeVeto;
#[async_trait::async_trait]
impl NativeExtension for TreeVeto {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("tree-veto")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionBeforeTree]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionBeforeTree { .. } => {
                HookOutcome::Block { reason: Some("vetoed".into()) }
            }
            _ => HookOutcome::Noop,
        }
    }
}

#[tokio::test]
async fn navigate_tree_before_tree_veto_cancels() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(TreeVeto))
        .build()
        .await
        .unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u1: EntryId = anchors[0].entry_id.clone();
    let before = session.messages().await.len();

    let outcome = session
        .navigate_tree(u1, NavigateTreeOptions { summarize: false, ..Default::default() })
        .await
        .unwrap();
    assert!(outcome.cancelled, "the before_tree veto cancels the navigation");
    assert!(outcome.editor_text.is_none());
    assert_eq!(session.messages().await.len(), before, "vetoed navigation leaves the leaf intact");
}

// ============================================================ #15 navigate_tree: SessionCommand seam ==

/// The unified op routes through the `SessionCommand::NavigateTree` control verb (arch-11 §2.1).
#[tokio::test]
async fn navigate_tree_via_command_seam() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;

    let anchors = session.user_messages_for_forking().await;
    let u2: EntryId = anchors[1].entry_id.clone();

    let out = session
        .execute(SessionCommand::NavigateTree {
            target: u2,
            options: NavigateTreeOptions { summarize: false, ..Default::default() },
        })
        .await
        .unwrap();
    match out {
        SessionCommandOutput::TreeNavigation(outcome) => {
            assert!(!outcome.cancelled);
            assert_eq!(outcome.editor_text.as_deref(), Some("second"));
        }
        other => panic!("expected TreeNavigation, got {other:?}"),
    }
}

// ============================================ SESS-023 blast radius: the manager lock ============

/// Branch summarization must NOT hold the session-manager mutex across its provider round-trip.
///
/// `navigate_tree`'s summarize leg used to run inside `let mut guard = self.manager.lock().await`,
/// so a summarization stalled EVERY other session-manager consumer (a TUI polling `session_dag`, an
/// extension's `getEntries`, a concurrent `compact`) for the full model call plus its retry backoff
/// — `AgentSession::compact` already scopes its guard for exactly this reason. It was invisible only
/// because no front end could reach the summarize branch at all (SESS-023); making `/tree` reach it
/// makes the stall reachable, so the guard now spans only the append.
///
/// The summarizer is gated on a channel, so this is deterministic rather than timing-dependent:
/// with the lock held, `session_dag()` cannot return until the gate is released, which happens only
/// after the assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn branch_summarization_does_not_hold_the_manager_lock() {
    let fx = fixture();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let gate_rx = Mutex::new(gate_rx);

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)),
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a2")], StopReason::Stop)),
        // The branch summarizer: announce, then block until the test releases the gate.
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            let _ = entered_tx.send(());
            let _ = gate_rx.lock().unwrap().recv();
            faux_assistant_message(vec![faux_text("BRANCH-BODY")], StopReason::Stop)
        }),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = Arc::new(SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap());

    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;
    let u1: EntryId = session.user_messages_for_forking().await[0].entry_id.clone();

    let nav_session = session.clone();
    let nav = tokio::spawn(async move {
        nav_session
            .navigate_tree(u1, NavigateTreeOptions { summarize: true, ..Default::default() })
            .await
    });

    // Wait until the summarizer call is genuinely in flight.
    tokio::task::spawn_blocking(move || entered_rx.recv())
        .await
        .unwrap()
        .expect("the branch summarizer was invoked");

    // THE ASSERTION: another manager consumer must still be serviceable.
    let dag = tokio::time::timeout(std::time::Duration::from_secs(5), session.session_dag())
        .await
        .expect(
            "session_dag() blocked while a branch summarization was running — the manager mutex is \
             being held across the provider round-trip",
        );
    assert!(!dag.is_empty(), "the tree is readable mid-summarization");

    let _ = gate_tx.send(());
    let outcome = nav.await.unwrap().unwrap();
    assert!(outcome.summary_entry.is_some(), "the summary still lands once the gate opens");
}
