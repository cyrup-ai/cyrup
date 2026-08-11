//! G145 — `/intercom-id` must exist and must reach the editor.
//!
//! # What upstream does
//!
//! `pi-intercom` v0.8.0 added the command (`v0.9.2 CHANGELOG.md:31`: "Added `/intercom-id` to insert
//! a stable handoff snippet for the current session into the editor. Thanks to dataforxyz for PR
//! #60."). At v0.9.2 it is three moving parts:
//!
//! ```text
//! // index.ts:412-414
//! function formatIntercomContactSnippet(sessionId: string): string {
//!   return `Use pi-intercom: intercom({ action: "send", to: "${sessionId}", message: "..." })`;
//! }
//! // index.ts:2261-2268
//! function insertIntoEditor(ctx, text): boolean {
//!   if (!ctx.hasUI) return false;
//!   const ui = ctx.ui as { getEditorText?, setEditorText? };
//!   if (typeof ui.setEditorText !== "function") return false;
//!   const existing = typeof ui.getEditorText === "function" ? ui.getEditorText() : "";
//!   ui.setEditorText(existing.trim() ? `${existing.trimEnd()}\n\n${text}` : text);
//!   return true;
//! }
//! // index.ts:2365-2368
//! pi.registerCommand("intercom-id", {
//!   description: "Insert a stable pi-intercom handoff snippet for this session into the editor",
//!   handler: async (_args, ctx) => insertIntercomId(ctx),
//! });
//! ```
//!
//! and `insertIntercomId` (`index.ts:2270-2289`) connects with `ensureConnected("tool")`, reads
//! `contactClient.sessionId`, and notifies `Inserted intercom contact target: <id>` on success or
//! `Intercom contact target: <id>` when there is no editor to insert into.
//!
//! Upstream pins it at `intercom.integration.test.ts:856-880` — it seeds the editor with
//! `"Existing note"` and asserts the buffer becomes
//! `/Existing note\n\nUse pi-intercom: intercom\(\{ action: "send", to: "session-child-test", …/`.
//!
//! # The gap this closes
//!
//! cyrup registered exactly ONE command (`extension.rs`, `register_command(INTERCOM_COMMAND, …)`),
//! and `execute_command` returned
//! `Err("native extension has no handler for command `intercom-id`")` for anything else. The
//! `HostServices::editor_text`/`set_editor_text` seam was already live end to end
//! (`cyrup-session-svc/src/host_services.rs:667`, consumed in `cyrup-tui/src/app.rs`) — no crate in
//! the port had a reader for it here. VERSION-LAG: `git grep intercom-id v0.7.0` (cyrup's ported
//! baseline) returns nothing.
//!
//! # Why this drives the REAL entry point
//!
//! Everything here is production: a real broker subprocess, a real Unix socket, the real
//! `SessionStart` connect, and the real `NativeExtension::execute_command` command-tier dispatch
//! that the TUI slash-command router calls. Nothing calls `run_intercom_id_command` or
//! `format_intercom_contact_snippet` directly — both are private.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::{ExtMode, HostCtx, HostEvent, HostServices, NativeExtension};
use cyrup_intercom::config::{config_path, load_config};
use cyrup_intercom::extension::{INTERCOM_ID_COMMAND, IntercomExtension};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::spawn::wait_for_broker;

const MY_SESSION_ID: &str = "session-aaaabbbbccccdddd";

/// An editor-backed `HostServices`: `editor_text`/`set_editor_text` are the REAL seam the live
/// backend implements (`cyrup-session-svc/src/host_services.rs:667`), mirrored here so the buffer
/// the command produced is observable.
struct EditorSink {
    buffer: Mutex<String>,
}

impl EditorSink {
    fn with_text(initial: &str) -> Arc<Self> {
        Arc::new(Self { buffer: Mutex::new(initial.to_string()) })
    }

    fn text(&self) -> String {
        self.buffer.lock().unwrap().clone()
    }
}

impl HostServices for EditorSink {
    fn editor_text(&self) -> String {
        self.buffer.lock().unwrap().clone()
    }
    fn set_editor_text(&self, text: &str, is_paste: bool) {
        assert!(!is_paste, "pi calls setEditorText (REPLACE), never pasteEditorText (index.ts:2266)");
        *self.buffer.lock().unwrap() = text.to_string();
    }
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
}

/// Records every notification raised, so a test can assert on the LEVEL the human sees. Per the
/// `Ok(None)` convention on `NativeExtension::execute_command`, a handler needing a non-Info level
/// notifies itself and returns nothing — so for those paths the notification IS the whole output and
/// asserting on it is strictly stronger than asserting on a returned string the session would only
/// have re-emitted at Info.
#[derive(Default)]
struct NotifyRecorder {
    raised: std::sync::Mutex<Vec<(String, cyrup_ext::NotifyKind)>>,
}

impl NotifyRecorder {
    fn taken(&self) -> Vec<(String, cyrup_ext::NotifyKind)> {
        self.raised.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl HostServices for NotifyRecorder {
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
    fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        if let Ok(mut g) = self.raised.lock() {
            g.push((message.to_string(), kind));
        }
    }
}

/// A backend with NO editor at all — the `hasUI === false` branch (`index.ts:2262`).
struct NoEditorSink;

impl HostServices for NoEditorSink {
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
}

fn broker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-intercom-broker"))
}

fn write_broker_command(intercom_dir: &Path) {
    std::fs::create_dir_all(intercom_dir).expect("create intercom dir");
    let body = serde_json::json!({
        "brokerCommand": broker_bin().to_string_lossy(),
        "brokerArgs": [],
    });
    std::fs::write(config_path(intercom_dir), serde_json::to_string(&body).expect("serialize config"))
        .expect("write config.json");
}

fn spawn_broker(agent_dir: &Path) -> tokio::process::Child {
    tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess")
}

async fn within<F: FnMut() -> bool>(budget: Duration, mut predicate: F) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Bring a real session up on a real broker and hand back everything the assertions need.
async fn live_session(
    agent_dir: &Path,
    services: Arc<dyn HostServices>,
    has_ui: bool,
) -> (Arc<IntercomExtension>, HostCtx, tokio::process::Child) {
    let intercom_dir = intercom_dir_path(agent_dir);
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let broker = spawn_broker(agent_dir);
    wait_for_broker(&socket, Duration::from_secs(20)).await.expect("broker up");

    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.to_path_buf(),
            PathBuf::from("/tmp/work"),
            load_config(&intercom_dir),
            None,
        )
        .expect("build the extension"),
    );
    ext.set_host_services(services);
    let ctx = HostCtx::command(ExtMode::Tui, has_ui, agent_dir.to_path_buf());
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "test".to_string() }, &ctx).await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state.client().is_some_and(|c| c.is_connected())).await,
        "the session connects on SessionStart"
    );
    (ext, ctx, broker)
}

/// THE FIX. The user types `/intercom-id` into a TUI session with text already in the editor; the
/// snippet is appended after a blank line and the command reports the insert.
///
/// Against the pre-fix `extension.rs` this fails at the very first assertion — `execute_command`
/// returns `Err(ExtError::Component("native extension has no handler for command `intercom-id`"))`,
/// because `/intercom-id` was never registered and the dispatch had no arm for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_id_appends_the_handoff_snippet_to_the_editor() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let editor = EditorSink::with_text("Existing note");
    let (ext, ctx, mut broker) = live_session(agent_dir.path(), editor.clone(), true).await;

    // --- The user types `/intercom-id` ---
    let reply = ext
        .execute_command(INTERCOM_ID_COMMAND, "", &ctx)
        .await
        .expect("`/intercom-id` is a registered command with a handler")
        .expect("the command produces output");

    let session_id = ext.state().client().and_then(|c| c.session_id()).expect("registered session id");

    // pi `insertIntoEditor`: `existing.trim() ? `${existing.trimEnd()}\n\n${text}` : text`
    // (`v0.9.2 index.ts:2266`) — the seeded note is KEPT and the snippet follows a blank line,
    // exactly as upstream's own test asserts (`intercom.integration.test.ts:874`).
    assert_eq!(
        editor.text(),
        format!(
            "Existing note\n\nUse cyrup-intercom: intercom({{ action: \"send\", to: \"{session_id}\", message: \"...\" }})"
        ),
        "the editor buffer must be the pre-existing text, a blank line, then pi's snippet"
    );

    // pi `notifyIfLive(liveContext, `Inserted intercom contact target: ${sessionId}`, "info", …)`
    // (`v0.9.2 index.ts:2285`), degraded to the command's return string (the port doc §4.3).
    assert_eq!(
        reply,
        format!("Inserted intercom contact target: {session_id}"),
        "the success branch reports WHICH id was inserted"
    );

    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The empty-editor branch: `existing.trim()` is falsy, so the snippet REPLACES rather than being
/// prefixed by a stray blank line (`v0.9.2 index.ts:2266`, the `: text` arm).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_id_into_an_empty_editor_inserts_the_bare_snippet() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    // Whitespace only — `existing.trim()` is falsy for `"   \n "`, not just for `""`.
    let editor = EditorSink::with_text("   \n ");
    let (ext, ctx, mut broker) = live_session(agent_dir.path(), editor.clone(), true).await;

    let reply = ext
        .execute_command(INTERCOM_ID_COMMAND, "", &ctx)
        .await
        .expect("the command dispatches")
        .expect("the command produces output");
    let session_id = ext.state().client().and_then(|c| c.session_id()).expect("registered session id");

    assert_eq!(
        editor.text(),
        format!(
            "Use cyrup-intercom: intercom({{ action: \"send\", to: \"{session_id}\", message: \"...\" }})"
        ),
        "a blank editor gets the snippet alone, with no leading newlines"
    );
    assert_eq!(reply, format!("Inserted intercom contact target: {session_id}"));

    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The no-editor branch (`if (!ctx.hasUI) return false;`, `v0.9.2 index.ts:2262`): the user still
/// gets the id, via upstream's OTHER message (`index.ts:2288`), and nothing is written.
///
/// This is the control that proves the assertion above is about the editor seam and not about the
/// command merely returning a string: same command, same broker, only `has_ui` differs, and the
/// reply text changes with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_id_without_a_ui_reports_the_id_instead_of_inserting() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let (ext, ctx, mut broker) = live_session(agent_dir.path(), Arc::new(NoEditorSink), false).await;

    let reply = ext
        .execute_command(INTERCOM_ID_COMMAND, "", &ctx)
        .await
        .expect("the command dispatches")
        .expect("the command produces output");
    let session_id = ext.state().client().and_then(|c| c.session_id()).expect("registered session id");

    assert_eq!(
        reply,
        format!("Intercom contact target: {session_id}"),
        "pi's insert-failed branch still surfaces the id (`index.ts:2288`), it does not go silent"
    );

    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The connect-failure branch (`v0.9.2 index.ts:2277-2280`): no broker at all, so
/// `ensureConnected("tool")` throws and the user is told, rather than the command hanging or
/// erroring out of the dispatcher.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_intercom_id_reports_intercom_unavailable_when_the_broker_cannot_be_reached() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    std::fs::create_dir_all(&intercom_dir).expect("create intercom dir");
    // A broker command that exits immediately: `ensure_broker` can never bring a socket up.
    std::fs::write(
        config_path(&intercom_dir),
        serde_json::to_string(&serde_json::json!({ "brokerCommand": "/bin/false", "brokerArgs": [] }))
            .expect("serialize"),
    )
    .expect("write config.json");

    let recorder = Arc::new(NotifyRecorder::default());
    let ext = IntercomExtension::new(
        agent_dir.path().to_path_buf(),
        PathBuf::from("/tmp/work"),
        load_config(&intercom_dir),
        None,
    )
    .expect("build the extension");
    ext.set_host_services(recorder.clone() as Arc<dyn HostServices>);
    let ctx = HostCtx::command(ExtMode::Tui, true, agent_dir.path().to_path_buf());

    let reply = ext
        .execute_command(INTERCOM_ID_COMMAND, "", &ctx)
        .await
        .expect("the command dispatches even with no broker");

    // Upstream raises this at "error" (`v0.9.2 index.ts:2278-2279`). The session surfaces a RETURNED
    // string at `NotifyKind::Info`, so returning the text here would show a connect FAILURE as an
    // ordinary info toast. The handler therefore notifies at Error itself and returns nothing.
    assert!(
        reply.is_none(),
        "a handler that notified at its own level must return None, or the session re-surfaces the \
         same sentence a second time at Info: {reply:?}"
    );
    let raised = recorder.taken();
    assert_eq!(raised.len(), 1, "exactly one notification for one failure: {raised:?}");
    let (message, kind) = raised.first().expect("one notification");
    assert!(
        message.starts_with("Intercom unavailable: "),
        "pi `Intercom unavailable: ${{getErrorMessage(error)}}` (`v0.9.2 index.ts:2278`), got: {message}"
    );
    assert_eq!(
        *kind,
        cyrup_ext::NotifyKind::Error,
        "pi passes \"error\" (`v0.9.2 index.ts:2279`); Info would understate a connect failure"
    );
}
