//! SESS-023 — `/tree` must ASK about branch summarization, and be able to run it.
//!
//! Pi's tree-confirm callback (`coding-agent/src/modes/interactive/interactive-mode.ts:4744-4820`)
//! is a small state machine: unless `branchSummary.skipPrompt` is set (`:4753`) it shows a
//! three-option `showExtensionSelector("Summarize branch?", …)` (`:4755-4760`), opens
//! `showExtensionEditor("Custom summarization instructions")` on the third option (`:4769`), aborts
//! any in-flight response (`:4781-4785`), rebinds Escape to `abortBranchSummary` (`:4792-4795`),
//! shows a `BranchSummaryStatusIndicator` (`:4796-4799`), and only then calls
//! `navigateTree(entryId, { summarize, customInstructions })` (`:4803-4806`) — checking
//! `result.aborted` FIRST (`:4805`) and `result.cancelled` second (`:4809`).
//!
//! cyrup shipped `navigate_tree(entry, NavigateTreeOptions::default())` — `summarize` hard-false —
//! so none of that existed and the whole branch-summary stack was dead in the binary.
//!
//! These tests drive the REAL `App::execute_command` / `App::handle_input` paths against a REAL
//! faux-provider-backed `AgentSession`, and assert observable effects: which selector is open, what
//! `AppAction` an Escape produces, and whether a `branch_summary` entry actually lands in the
//! session JSONL.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{Content, EntryId, Message, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider, FauxResponseStep};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSession, NavigateTreeOutcome, SessionBuilder, SessionConfig, Settings,
};
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    Action, App, AppAction, AppCommand, Entry, IndicatorKind, InputEvent, Key, SelectAction,
    SelectorKind, TreeNavMsg, UiTheme,
};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap()
}

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

/// Concatenate the text of a core `user` message (the branch summarizer's prompt lives here).
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

/// A two-turn session plus the entry id of the FIRST user message (a real navigation target with a
/// genuinely abandoned branch behind it), and the prompts the summarizer call received.
async fn two_turn_session(fx: &Fixture) -> (Arc<AgentSession>, EntryId, Arc<Mutex<Vec<String>>>) {
    two_turn_session_with(fx, Settings::new()).await
}

async fn two_turn_session_with(
    fx: &Fixture,
    cli: Settings,
) -> (Arc<AgentSession>, EntryId, Arc<Mutex<Vec<String>>>) {
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)),
        FauxResponseStep::from(faux_assistant_message(vec![faux_text("a2")], StopReason::Stop)),
        // Any further call is the branch summarizer; record its prompt and answer deterministically.
        FauxResponseStep::factory(move |ctx, _o, _s, _m| {
            if let Some(t) = ctx.messages.iter().find_map(user_text) {
                cap.lock().unwrap().push(t);
            }
            faux_assistant_message(vec![faux_text("BRANCH-BODY")], StopReason::Stop)
        }),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session =
        Arc::new(SessionBuilder::new(provider, cfg).cli_settings(cli).build().await.unwrap());
    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("second").await.unwrap();
    session.wait_for_idle().await;
    let anchors = session.user_messages_for_forking().await;
    let first = anchors[0].entry_id.clone();
    (session, first, captured)
}

/// The transcript status lines pushed so far.
fn statuses(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn branch_summaries(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::BranchSummary { summary, .. } => Some(summary.clone()),
            _ => None,
        })
        .collect()
}

fn confirm_tree(target: &EntryId) -> AppCommand {
    AppCommand::ConfirmSelection { kind: SelectorKind::Tree, value: target.to_string() }
}

/// Answer the OPEN selector the way a user does — move the highlight `down` rows and press Enter —
/// and return the [`AppCommand`] the run loop would then execute. Driving the real key path (rather
/// than synthesizing a `ConfirmSelection`) is what makes the slot actually CLOSE, which is exactly
/// the state a subsequent Escape has to be routed against.
fn pick(app: &mut App<TestBackend>, down: usize) -> AppCommand {
    for _ in 0..down {
        app.handle_input(&key(KeyCode::Down));
    }
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(c) => c,
        other => panic!("expected a run-loop command from the selector, got {other:?}"),
    }
}

/// Row indices of the "Summarize branch?" prompt, in Pi's order (`interactive-mode.ts:4756-4759`).
const PICK_NO_SUMMARY: usize = 0;
const PICK_SUMMARIZE: usize = 1;
const PICK_CUSTOM: usize = 2;

// ------------------------------------------------------------------- the prompt itself ----------

/// Confirming a `/tree` row must OPEN Pi's three-option prompt, not navigate straight away.
///
/// This is the SESS-023 headline: before the fix this arm called
/// `navigate_tree(entry, NavigateTreeOptions::default())` and immediately pushed "navigated session
/// tree", so the user was never offered a summary and the whole stack below was unreachable.
#[tokio::test]
async fn tree_confirm_opens_the_summarize_branch_prompt_instead_of_navigating() {
    let fx = fixture();
    let (session, first, _) = two_turn_session(&fx).await;
    let mut app = app();
    let _rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;

    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::BranchSummary),
        "Pi asks 'Summarize branch?' before navigating (interactive-mode.ts:4755)"
    );
    assert!(
        statuses(&app).is_empty(),
        "nothing has been navigated yet, so no status line: {:?}",
        statuses(&app)
    );
    // The three options Pi offers, in Pi's order (`:4756-4759`).
    let screen = rendered(&mut app);
    for opt in ["No summary", "Summarize", "Summarize with custom prompt"] {
        assert!(screen.contains(opt), "prompt offers {opt:?}; screen:\n{screen}");
    }
}

/// Render the app and return the whole buffer as text.
fn rendered(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

// -------------------------------------------------------------------- the three answers ---------

/// "No summary" navigates with `summarize: false` — Pi's `wantsSummary = summaryChoice !== "No
/// summary"` (`:4767`). No summarizer call is made and no `branch_summary` entry is written.
#[tokio::test]
async fn no_summary_navigates_without_summarizing() {
    let fx = fixture();
    let (session, first, captured) = two_turn_session(&fx).await;
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_NO_SUMMARY);
    app.execute_command(cmd, &session, None).await;

    assert!(!app.state().branch_summary_in_flight(), "no summarization was started");
    let msg = rx.recv().await.expect("the navigation settled");
    assert!(app.apply_tree_nav_outcome(msg).is_none(), "a clean navigation asks for nothing more");
    assert!(
        statuses(&app).iter().any(|s| s == "navigated session tree"),
        "{:?}",
        statuses(&app)
    );
    assert!(captured.lock().unwrap().is_empty(), "the summarizer was never called");
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(!jsonl.contains("branch_summary"), "no branch summary was appended");
}

/// "Summarize" runs the REAL branch summarizer through `navigate_tree(.., summarize: true)` and the
/// produced summary lands both in the session JSONL and in the transcript (`push_branch_summary`,
/// dead code before SESS-023).
#[tokio::test]
async fn summarize_choice_runs_the_summarizer_and_records_the_entry() {
    let fx = fixture();
    let (session, first, captured) = two_turn_session(&fx).await;
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_SUMMARIZE);
    app.execute_command(cmd, &session, None).await;

    // While it runs, Pi shows its `BranchSummaryStatusIndicator` and rebinds Escape (`:4792-4799`).
    assert!(app.state().branch_summary_in_flight());
    assert_eq!(app.state().indicator.kind(), IndicatorKind::BranchSummary);

    let msg = rx.recv().await.expect("the navigation settled");
    assert!(app.apply_tree_nav_outcome(msg).is_none());

    assert_eq!(captured.lock().unwrap().len(), 1, "exactly one summarizer call");
    let summaries = branch_summaries(&app);
    assert_eq!(summaries.len(), 1, "one branch summary block: {summaries:?}");
    assert!(
        summaries[0].contains("BRANCH-BODY"),
        "the produced summary reaches the transcript: {summaries:?}"
    );
    let jsonl = session.export_to_jsonl(None).await.unwrap().expect("jsonl");
    assert!(jsonl.contains("branch_summary"), "the branch summary is persisted");
    // The indicator/Escape rebind are torn down in Pi's `finally` (`:4830-4833`).
    assert!(!app.state().branch_summary_in_flight());
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Idle);
}

/// "Summarize with custom prompt" opens the instructions editor, and the typed text is threaded into
/// the summarizer prompt as Pi's "Additional focus" (`branch-summarization.ts:322-323`).
#[tokio::test]
async fn custom_prompt_choice_opens_the_editor_and_threads_the_instructions() {
    let fx = fixture();
    let (session, first, captured) = two_turn_session(&fx).await;
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_CUSTOM);
    app.execute_command(cmd, &session, None).await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::BranchSummaryInstructions),
        "Pi opens showExtensionEditor on the third option (interactive-mode.ts:4769)"
    );

    // Type the instructions into the real inline editor and submit it.
    for c in "focus-on-the-files".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    let cmd = pick(&mut app, 0);
    app.execute_command(cmd, &session, None).await;
    let msg = rx.recv().await.expect("the navigation settled");
    app.apply_tree_nav_outcome(msg);

    let prompts = captured.lock().unwrap();
    assert_eq!(prompts.len(), 1, "exactly one summarizer call");
    assert!(
        prompts[0].contains("Additional focus: focus-on-the-files"),
        "custom instructions are threaded into the summary prompt, got:\n{}",
        prompts[0]
    );
}

/// **U4 + U5 — the instructions editor's hint row is bound to the LIVE tables.**
///
/// `ExtensionEditorComponent` composes its hint from four `keyHint(...)` calls, each of which
/// re-resolves through `keyText` → `getKeybindings().getKeys(...)` on every render
/// (`extension-editor.ts:83-90`, `keybinding-hints.ts:34-44`), so a rebind is reflected on the
/// FIRST paint. Two halves of that landed with E9 and neither was exercised:
///
/// * **U5** — `interactive-mode.ts:4769`'s dialog is the same component as the `ui.editor` one, so
///   `open_branch_summary_instructions` must hand it the live tables too. It did not; dropping the
///   `with_keymaps` call left the suite green and this dialog alone showing stock labels.
/// * **U4** — `keyText` joins EVERY bound key with `/` (`keybinding-hints.ts:29-36`), and
///   `tui.input.newLine` ships with two (`tui/src/keybindings.ts:137`,
///   `defaultKeys: ["shift+enter", "ctrl+j"]`). Reverting the row's `EditorKeymap::keys_label` to
///   the first-key `key_label` also left the suite green, while the hint silently dropped `ctrl+j`.
#[tokio::test]
async fn the_instructions_editor_hint_names_the_users_own_keys() {
    let fx = fixture();
    let (session, first, _) = two_turn_session(&fx).await;
    let mut app = app();
    let _rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_CUSTOM);
    // Rebind AFTER the prompt is answered and BEFORE the dialog is constructed, so the labels can
    // only be right if the dialog resolved them itself. (`pick` presses Enter, which is what is
    // being rebound — doing this earlier would break the prompt, not the assertion.)
    app.state_mut().select_keymap.set_action(SelectAction::Confirm, vec![Key::ctrl('s')]);
    app.state_mut().keymap.set_action(Action::ExternalEditor, vec![Key::ctrl('x')]);
    app.execute_command(cmd, &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::BranchSummaryInstructions));

    let screen = rendered(&mut app);
    assert!(screen.contains("ctrl+s submit"), "U5: the rebound confirm key:\n{screen}");
    assert!(screen.contains("ctrl+x external editor"), "U5: the rebound external key:\n{screen}");
    assert!(!screen.contains("enter submit"), "U5: the stock label must be gone:\n{screen}");
    assert!(
        screen.contains("shift+enter/ctrl+j newline"),
        "U4: `keyText` joins every key bound to `tui.input.newLine`:\n{screen}"
    );
}

// ------------------------------------------------------------------------ escape routing --------

/// Escaping the "Summarize branch?" prompt re-shows the TREE (Pi `:4761-4765`), it does not silently
/// drop the navigation.
#[tokio::test]
async fn escape_on_the_summary_prompt_reshows_the_tree() {
    let fx = fixture();
    let (session, first, _) = two_turn_session(&fx).await;
    let mut app = app();
    let _rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree)),
        "Pi's escape branch is `showTreeSelector(entryId)` (interactive-mode.ts:4763)"
    );

    // …and the re-shown tree lands on the SAME entry Pi re-selects.
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Tree), &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    let screen = rendered(&mut app);
    assert!(screen.contains("first"), "the tree is back on screen:\n{screen}");
}

/// Escaping the custom-instructions editor loops back to the PROMPT (Pi's `continue`, `:4770-4773`),
/// not out of the flow entirely.
#[tokio::test]
async fn escape_in_the_instructions_editor_returns_to_the_prompt() {
    let fx = fixture();
    let (session, first, _) = two_turn_session(&fx).await;
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_CUSTOM);
    app.execute_command(cmd, &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::BranchSummaryInstructions));

    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw, "the loop stays in-crate");
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::BranchSummary),
        "Pi loops back to the summary selector, keeping the pending target"
    );

    // The pending target survived the loop: answering now still navigates that same entry.
    let cmd = pick(&mut app, PICK_NO_SUMMARY);
    app.execute_command(cmd, &session, None).await;
    let msg = rx.recv().await.expect("the navigation settled after the editor loop");
    app.apply_tree_nav_outcome(msg);
    assert!(
        statuses(&app).iter().any(|s| s == "navigated session tree"),
        "{:?}",
        statuses(&app)
    );
}

/// While a branch summarization is in flight, Escape must route to `abort_branch_summary`, NOT to
/// the ordinary turn interrupt — Pi rebinds `defaultEditor.onEscape` for exactly this window
/// (`interactive-mode.ts:4792-4795`). Before SESS-023 `AgentSession::abort_branch_summary` had zero
/// callers in any front end, so the cancel token could never be pulled.
#[tokio::test]
async fn escape_during_a_branch_summarization_aborts_the_summary() {
    let fx = fixture();
    let (session, first, _) = two_turn_session(&fx).await;
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    let cmd = pick(&mut app, PICK_SUMMARIZE);
    app.execute_command(cmd, &session, None).await;
    assert!(app.state().branch_summary_in_flight());
    assert_eq!(app.active_selector_kind(), None, "the prompt closed on confirm");

    assert_eq!(
        app.handle_input(&key(KeyCode::Esc)),
        AppAction::AbortBranchSummary,
        "Esc cancels the summarization, not the (already-finished) turn"
    );
    // Drain so the spawned task cannot outlive the test.
    let msg = rx.recv().await.expect("the navigation settled");
    app.apply_tree_nav_outcome(msg);
}

// ---------------------------------------------------------------- outcome arm ordering ----------

/// An ABORTED summarization re-shows the tree; it must not be mistaken for a plain cancellation.
///
/// `navigate_tree` returns `{cancelled: true, aborted: true}` on abort (matching Pi
/// `agent-session.ts:3000-3001`). cyrup's old arm order tested `cancelled` FIRST, so aborting
/// printed "tree navigation cancelled" and swallowed the tree; Pi tests `result.aborted` first
/// (`interactive-mode.ts:4805`) and only then `result.cancelled` (`:4809`).
#[test]
fn an_aborted_summarization_reshows_the_tree_rather_than_reporting_a_cancellation() {
    let mut app = app();
    let aborted = NavigateTreeOutcome { cancelled: true, aborted: true, ..Default::default() };
    let follow_up = app.apply_tree_nav_outcome(TreeNavMsg::new("e7", Ok(aborted)));

    assert_eq!(
        follow_up,
        Some(AppCommand::OpenSelector(SelectorKind::Tree)),
        "Pi re-shows the tree on `result.aborted` (interactive-mode.ts:4807)"
    );
    assert_eq!(statuses(&app), vec!["Branch summarization cancelled".to_string()]);
}

/// A plain (non-aborted) cancellation still reports a cancellation and does NOT re-show the tree —
/// Pi `:4809-4812`.
#[test]
fn a_plain_cancellation_reports_and_stops() {
    let mut app = app();
    let cancelled = NavigateTreeOutcome { cancelled: true, ..Default::default() };
    assert_eq!(app.apply_tree_nav_outcome(TreeNavMsg::new("e7", Ok(cancelled))), None);
    assert_eq!(statuses(&app), vec!["Navigation cancelled".to_string()]);
}

// ------------------------------------------------------------------------- skipPrompt -----------

/// `branchSummary.skipPrompt` is a FRONT-END decision in Pi (`getBranchSummarySkipPrompt()`,
/// `interactive-mode.ts:4753`): set it and the prompt is skipped entirely, navigating with
/// `wantsSummary = false`. Before SESS-023 nothing in cyrup read this setting on the `/tree` path,
/// so it was a confirmed no-op.
#[tokio::test]
async fn skip_prompt_setting_navigates_without_asking() {
    let fx = fixture();
    let cli = Settings::parse(r#"{"branchSummary":{"skipPrompt":true}}"#).unwrap();
    let (session, first, captured) = two_turn_session_with(&fx, cli).await;
    assert!(
        session.services().settings.effective().branch_summary_skip_prompt(),
        "the fixture really did set branchSummary.skipPrompt"
    );
    let mut app = app();
    let mut rx = app.install_tree_nav_channel();

    app.execute_command(confirm_tree(&first), &session, None).await;
    assert_eq!(app.active_selector_kind(), None, "no prompt is shown");

    let msg = rx.recv().await.expect("the navigation settled");
    app.apply_tree_nav_outcome(msg);
    assert!(statuses(&app).iter().any(|s| s == "navigated session tree"), "{:?}", statuses(&app));
    assert!(captured.lock().unwrap().is_empty(), "skipPrompt implies wantsSummary = false");
}

// -------------------------------------------------------------- re-selection on re-show ---------

/// The re-shown tree lands on the SAME entry Pi re-selects (`showTreeSelector(entryId)`), and an id
/// that is not visible leaves the selection alone rather than panicking.
#[test]
fn tree_selector_can_be_reselected_at_a_given_entry() {
    use crate::{TreeNode, TreeSelector};
    let nodes = vec![
        TreeNode::message("e0", 0, "first"),
        TreeNode::message("e1", 0, "second"),
        TreeNode::message("e2", 0, "third"),
    ];
    let mut tree = TreeSelector::new(nodes);
    assert_eq!(tree.selected_id().as_deref(), Some("e0"));
    tree.select_id("e2");
    assert_eq!(tree.selected_id().as_deref(), Some("e2"), "re-shown at the confirmed entry");
    tree.select_id("nope");
    assert_eq!(tree.selected_id().as_deref(), Some("e2"), "an unknown id is a no-op, not a panic");
}
