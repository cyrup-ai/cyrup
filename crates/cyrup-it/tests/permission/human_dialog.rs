//! FULLY-WIRED PROOF for the LIVE in-session human permission dialog (P-1/P-3, R-PERM-022). On a REAL
//! assembled INTERACTIVE `AgentSession`, an ask-tier `bash` tool call blocks the `before_tool_call`
//! hook; a scripted UI sink — the SAME `LiveHostServices::set_ui_sink` seam the TUI/RPC renderer feeds
//! at runtime — answers the permission `select` dialog; and the resolved decision gates the call:
//!
//! - "Allow Once" → the tool PROCEEDS through the hook + the real `LocalAskChannel` dialog path.
//! - "Reject" → the tool is BLOCKED (never executes) and the denial surfaces as an is_error result.
//! - "Allow Always" → the tool proceeds AND the always-decision persists a session approval rule, so a
//!   later same-subject call auto-allows with NO second dialog (proves the always-* store write).
//!
//! This exercises the whole surface end-to-end: `set_host_services` (P-1 capture) → the registered
//! `before_tool_call` gate → `resolve_ask` under a `HostCtx::begin_human_wait` (P-3) guard →
//! `HostServices::select` → the scripted sink → the decision → the tool. Not a unit test of the dialog
//! in isolation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_config::AppMode;
use cyrup_core::{
    CancelToken, Content, Message, TerminateHint, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_permission_system::PermissionSystemExtension;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use cyrup_test_support::{FauxResponse, HarnessOptions, TestTempDir, create_harness};
use tokio::sync::mpsc::UnboundedReceiver;

/// A fake `bash` tool that RECORDS every command it actually executes (overrides the built-in of the
/// same name, R-08-012). A blocked call never reaches it; an allowed call does — the observable proof
/// the gate + dialog let the call THROUGH the registered hook.
struct RecordingBash {
    executed: Arc<Mutex<Vec<String>>>,
    params: serde_json::Value,
}

impl RecordingBash {
    fn new(executed: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            executed,
            params: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } }
            }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for RecordingBash {
    fn name(&self) -> &str {
        "bash"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "recording bash (test double)"
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let command = params
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        self.executed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command);
        Ok(ToolResult {
            content: vec![Content::text("EXECUTED")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

/// Spawn a scripted UI responder: every `select` dialog is answered with `answer` (a dialog option
/// string); other kinds get a `None` text. Records how many `select` prompts it saw so a test can
/// assert the dedup / session-store paths render ZERO extra dialogs. Returns the shared counter.
fn spawn_select_responder(
    mut ui_rx: UnboundedReceiver<UiRequest>,
    answer: &'static str,
) -> Arc<AtomicUsize> {
    let selects_seen = Arc::new(AtomicUsize::new(0));
    let counter = selects_seen.clone();
    tokio::spawn(async move {
        while let Some(req) = ui_rx.recv().await {
            let reply = match req.kind {
                UiKind::Select => {
                    counter.fetch_add(1, Ordering::SeqCst);
                    UiReply::Text(Some(answer.to_string()))
                }
                UiKind::Confirm => UiReply::Confirm(false),
                _ => UiReply::Text(None),
            };
            let _ = req.reply.send(reply);
        }
    });
    selects_seen
}

/// Build an interactive harness (has_ui=true) wiring the recording `bash` tool + the permission
/// extension over an empty policy (default ASK for bash), then install a scripted UI sink answering
/// `answer`. Returns the harness, the executed-command log, the select-prompt counter, and the RAII
/// agent dir. `commands` are the bash commands the scripted provider requests, in order (each drives a
/// tool call within the single run).
async fn interactive_harness(
    answer: &'static str,
    commands: &[&str],
) -> (
    cyrup_test_support::Harness,
    Arc<Mutex<Vec<String>>>,
    Arc<AtomicUsize>,
    TestTempDir,
) {
    let agent_dir = TestTempDir::new().unwrap();
    // Empty policy ⇒ default ASK for every category (pi `permission-manager.ts:44-50`).
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), "{}").unwrap();

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bash = Arc::new(RecordingBash::new(executed.clone()));
    let ext = Arc::new(PermissionSystemExtension::new(
        agent_dir.path().to_path_buf(),
        agent_dir.path().to_path_buf(),
    ));

    let mut responses: Vec<FauxResponse> = commands
        .iter()
        .map(|c| FauxResponse::tool_call("bash", serde_json::json!({ "command": c })))
        .collect();
    responses.push(FauxResponse::text("done"));

    let options = HarnessOptions {
        responses,
        queue_responses: true,
        // Interactive ⇒ has_ui=true, so the gate reaches the human via the live dialog (pi
        // `confirmPermission` `ctx.hasUI` branch, index.ts:1509-1511).
        app_mode: AppMode::Interactive,
        tools: vec![bash],
        native_extensions: vec![ext],
        ..Default::default()
    };
    let harness = create_harness(options).await.unwrap();

    // Install the scripted UI sink on the session's OWN `LiveHostServices` — the SAME `Arc` the
    // builder late-bound into the extension via `set_host_services` (P-1), so the extension's
    // `LocalAskChannel::select` round-trips to this sink.
    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
    harness
        .session()
        .services()
        .host_services
        .set_ui_sink(ui_tx);
    let selects = spawn_select_responder(ui_rx, answer);

    (harness, executed, selects, agent_dir)
}

fn has_error_tool_result_containing(msgs: &[Message], needle: &str) -> bool {
    msgs.iter().any(|m| match m {
        Message::ToolResult {
            is_error, content, ..
        } => {
            *is_error
                && content.iter().any(|c| match c {
                    Content::Text { text, .. } => text.contains(needle),
                    _ => false,
                })
        }
        _ => false,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_allow_once_lets_the_ask_tier_tool_proceed() {
    let (harness, executed, selects, _dir) = interactive_harness("Allow Once", &["ls -la"]).await;

    harness.run("go").await.unwrap();

    // The human approved via the real select dialog → the tool PROCEEDED through the hook.
    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["ls -la".to_string()],
        "an ask-tier command the human approved must execute through the registered hook"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "exactly one permission dialog was surfaced"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_reject_blocks_the_ask_tier_tool() {
    let (harness, executed, selects, _dir) = interactive_harness("Reject", &["rm -rf /"]).await;

    let run = harness.run("go").await.unwrap();

    // The human rejected → the tool NEVER executed (blocked at the hook).
    assert!(
        executed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty(),
        "a human-rejected command must not execute; run: {run:?}"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "the reject was surfaced through one dialog"
    );
    // The denial surfaced as an is_error tool result (pi `formatUserDeniedReason`).
    let msgs = harness.session().messages().await;
    assert!(
        has_error_tool_result_containing(&msgs, "User denied"),
        "the user-denied reason surfaced as an is_error result; messages: {msgs:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_allow_always_persists_a_session_rule_so_the_next_call_needs_no_dialog() {
    // Two identical bash calls in one run. The FIRST asks and the human picks "Allow Always"; the
    // decision persists a session approval rule for (bash, "echo hi"), so the SECOND call (a different
    // tool_call_id, a dedup MISS) auto-ALLOWS via the store overlay WITHOUT a second dialog.
    let (harness, executed, selects, _dir) =
        interactive_harness("Allow Always", &["echo hi", "echo hi"]).await;

    harness.run("go").await.unwrap();

    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["echo hi".to_string(), "echo hi".to_string()],
        "both calls executed: the first after the human allowed-always, the second via the session rule"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "only the FIRST call prompted; the always-decision persisted a session rule for the second"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_allow_always_survives_into_a_later_turn_of_the_same_session() {
    // PERM-034 — the SAME always-rule, read back a TURN LATER instead of a tool-call later.
    //
    // The sibling test above proves the store write is visible to a second call inside ONE
    // `prompt()`. That is a weaker claim than the one the always-* options actually make, and it is
    // not the shape users report against: the complaint is always "I approved it, and it asked me
    // again LATER". Between two turns a session runs a whole extra lap of the hook pipeline, so a
    // store hung off the wrong lifetime — rebuilt per prompt, per hook registration, per extension
    // re-init — would pass the sibling and fail here. Nothing else in this suite crosses a turn
    // boundary, so nothing else can catch that class of regression.
    //
    // Why one turn boundary is the whole proof: `SessionApprovalStore::clear` is reachable from
    // exactly two triggers, SessionStart and SessionShutdown (`extension.rs:2617`/`:2705`), matching
    // pi's `session_start` / `session_shutdown` handlers (`index.ts:1830`, `:1864`) — and pi's
    // `resources_discover` reload (`:1844-1859`) deliberately does NOT clear it. `session_start`
    // fires once per session, latched by `start_announced.swap(true)`
    // (`cyrup-session-svc/src/session.rs:2941`), so a second `prompt()` on this same session must
    // leave the store standing. If a future change re-announces start per turn, or re-installs the
    // extension mid-session, that latch breaks and this test goes red — which is the point.
    let (harness, executed, selects, _dir) =
        interactive_harness("Allow Always", &["echo hi"]).await;

    // Turn 1: the human is asked once and picks "Allow Always".
    harness.run("first turn").await.unwrap();
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "the first turn's call is ask-tier, so it must surface exactly one dialog"
    );

    // Turn 2: a byte-identical command on the SAME session. `append_responses` extends the
    // consumable queue (`harness.rs:161-164`) so this is a genuinely separate `prompt()` — a new
    // run, new tool_call_id, new hook pass — not another call spliced into the first run.
    harness.append_responses(vec![
        FauxResponse::tool_call("bash", serde_json::json!({ "command": "echo hi" })),
        FauxResponse::text("done"),
    ]);
    harness.run("second turn").await.unwrap();

    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec!["echo hi".to_string(), "echo hi".to_string()],
        "both turns executed: turn 1 after the human allowed-always, turn 2 via the surviving rule"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "STILL one dialog after a second turn: the always-rule outlived the turn that created it"
    );
}

// ---------------------------------------------------------------- PERM-034 diagnostic probes
//
// The two tests above prove the gate LOGIC: an always-decision written on an instance is read back
// by that same instance, across a tool call and across a turn. Both build the extension with the
// BARE constructor and hand it to the harness pre-built, so neither can see the two mechanisms the
// live report is consistent with. These add them.

/// The owner's actual command rather than `echo hi`. `rm -rf ./tmp/test` is a single command unit,
/// so `b3e1a6d`'s tree-sitter decomposition is a no-op for it and the stored subject is the literal
/// string — if this diverges from the `echo hi` result, the subject derivation is implicated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perm034_allow_always_sticks_for_the_reported_command() {
    let (harness, executed, selects, _dir) =
        interactive_harness("Allow Always", &["rm -rf ./tmp/test", "rm -rf ./tmp/test"]).await;

    harness.run("go").await.unwrap();

    assert_eq!(
        executed.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec![
            "rm -rf ./tmp/test".to_string(),
            "rm -rf ./tmp/test".to_string()
        ],
        "both calls executed"
    );
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "PERM-034: only the FIRST call may prompt; the always-rule must cover the second"
    );
}

/// A COMPOUND command. Post-`b3e1a6d` the bash arm decomposes into units and
/// `pick_most_restrictive` returns ONE unit's result, so `result.command` — and therefore the
/// approval subject — is the winning UNIT, not the string the user typed. The next identical call
/// re-decomposes and must land on the same unit for the stored rule to match.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perm034_allow_always_sticks_for_a_compound_command() {
    let cmd = "rm -rf ./tmp/test && echo done";
    let (harness, _executed, selects, _dir) =
        interactive_harness("Allow Always", &[cmd, cmd]).await;

    harness.run("go").await.unwrap();

    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "PERM-034/compound: the always-rule must cover the second identical compound call"
    );
}

/// **PIN — this is UPSTREAM behaviour, not a defect. Do not "fix" it.**
///
/// A reload DOES wipe every "Allow Always" grant, and pi does exactly the same. Both sides clear
/// `sessionApprovals` unconditionally from `session_start` AND from `session_shutdown`
/// (`pi-permission-system` `index.ts:1828-1831`/`:1862-1865` @v0.8.0 ↔ `extension/native.rs`'s two
/// arms), both take `reason: "reload"` on those events (`extensions/types.ts:565`/`:618`
/// @pi v0.83.0), and on both sides it is only the `resources_discover` reload that deliberately
/// spares the store (pi `:1844-1859`) — it clears the dedup cache alone.
///
/// This was measured while diagnosing PERM-034 ("Allow Always does not stick"). It reproduces the
/// reported symptom exactly — approve always, reload, get re-prompted — which makes it an
/// attractive and WRONG explanation. Filed here as a pin so the next reader does not re-derive it
/// and file a port bug against faithful code. If cyrup turns out to re-prompt where pi does not,
/// the divergence is in WHEN the two dispatch these events, not in what the handlers do with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perm034_a_reload_wipes_always_grants_exactly_as_upstream_does() {
    use cyrup_core::CancelToken;
    use cyrup_ext::event::HostEvent;

    let (harness, _executed, selects, _dir) =
        interactive_harness("Allow Always", &["rm -rf ./tmp/test"]).await;

    harness.run("first turn").await.unwrap();
    assert_eq!(
        selects.load(Ordering::SeqCst),
        1,
        "turn 1 prompts exactly once"
    );

    // The store-clearing half of `ExtensionFacade::reload` (`cyrup-ext/src/facade.rs:2142`), in its
    // order: shutdown the outgoing set, then start the fresh one. Note the session's own
    // `start_announced` latch (`session/lifecycle.rs:193`) does NOT gate these — the facade
    // dispatches to extensions directly — which is why a reload reaches the handler at all.
    let cancel = CancelToken::new();
    let dispatcher = harness.session().services().ext_host.dispatcher();
    dispatcher
        .dispatch_notify(
            &HostEvent::SessionShutdown {
                reason: "reload".into(),
                target_session_file: None,
            },
            &cancel,
        )
        .await;
    dispatcher
        .dispatch_notify(
            &HostEvent::SessionStart {
                reason: "reload".into(),
                previous_session_file: None,
            },
            &cancel,
        )
        .await;

    harness.append_responses(vec![
        FauxResponse::tool_call(
            "bash",
            serde_json::json!({ "command": "rm -rf ./tmp/test" }),
        ),
        FauxResponse::text("done"),
    ]);
    harness.run("second turn").await.unwrap();

    assert_eq!(
        selects.load(Ordering::SeqCst),
        2,
        "the reload cleared the store, so the second turn prompts again — upstream does this too"
    );
}
