//! `ComposeOverlay::send_message` — the overlay's own broker send leg (pi's private
//! `ComposeOverlay.sendMessage`, `compose.ts:76-103`), driven against a real broker child process.
//!
//! Drained from `crates/cyrup-intercom/src/ui/compose.rs`'s `#[cfg(test)]` module for the same
//! reason as [`super::tool_actions`]: both spawn the real `cyrup-intercom-broker` binary, so they
//! are seam tests, and they went red the moment the package stopped having an integration target
//! that incidentally caused cargo to link that binary.
//!
//! The two originals asserted on the overlay's PRIVATE `sending` / `error` fields, which an
//! external crate cannot reach. Rather than weaken those assertions to something render-shaped,
//! `ComposeOverlay` grew the read halves of the two public setters it already had —
//! `is_sending()` / `error()`, next to `set_sending()` / `set_error()` — so the assertions below
//! are the same claims about the same state, spelled through the accessors. Nothing else changed.

use std::sync::Arc;

use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::protocol::{SessionInfo, SessionRegistration};
use cyrup_intercom::ui::DefaultKeybindings;
use cyrup_intercom::ui::compose::ComposeOverlay;

use super::common::Broker;

fn session() -> SessionInfo {
    SessionInfo {
        endpoint_epoch: None,
        // ICOM-041: `runtimeFallbackAlias` (v0.10.1 types.ts:6-7) — these fixtures
        // register under a REAL name, not a synthesized unnamed-runtime alias.
        runtime_fallback_alias: None,
        id: "session-12345678".to_string(),
        name: Some("subagent-chat-019ecaf6".to_string()),
        cwd: "/Users/envvar/.config/ghostty".to_string(),
        model: "bsy-deepseek-v4-pro".to_string(),
        pid: 1u32.into(),
        started_at: 0u64.into(),
        last_activity: 0u64.into(),
        status: None,
        peer_uid: None,
        trusted_local: None,
        context_pct: None,
        context_tokens: None,
        context_window: None,
        tmux_pane: None,
        extra: Default::default(),
    }
}

/// The compose originals' own `registration`, which takes a CWD and carries no name — deliberately
/// not [`super::common::registration`], which takes a name and hardcodes `/tmp/work`.
fn registration(cwd: &str) -> SessionRegistration {
    SessionRegistration {
        // ICOM-041: `runtimeFallbackAlias` (v0.10.1 types.ts:6-7) — these fixtures
        // register under a REAL name, not a synthesized unnamed-runtime alias.
        runtime_fallback_alias: None,
        name: None,
        cwd: cwd.to_string(),
        model: "test-model".to_string(),
        pid: std::process::id().into(),
        started_at: 0u64.into(),
        last_activity: 0u64.into(),
        status: None,
        tmux_pane: None,
        extra: Default::default(),
    }
}

// Regression proof for the dossier item "ComposeOverlay's send leg (pi `sendMessage`,
// `compose.ts:76-103`) and its `ComposeResult` outcome (`compose.ts:7-11`) have no cyrup
// equivalent": against the PRE-FIX code neither `ComposeOverlay::send_message` nor `ComposeResult`
// existed, so this test would not even compile. Drives a real broker round trip and asserts the
// success shape matches pi exactly: `sent:true`, the broker message id, the trimmed sent text, no
// error, and (mirroring pi never resetting `sending` on success) `sending` left true.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_message_delivers_and_returns_the_compose_result() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("/me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let target_client = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("/target"),
            Some("target-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let mut target = session();
    target.id = "target-session".to_string();
    let mut overlay = ComposeOverlay::new(target, "target-session".to_string());
    overlay.handle_input(&DefaultKeybindings, "hello there");

    let result = overlay
        .send_message(&me)
        .await
        .expect("a delivered send must return Some(ComposeResult)");
    assert!(result.sent);
    assert_eq!(result.text.as_deref(), Some("hello there"));
    assert!(
        result.message_id.is_some(),
        "must carry the broker-assigned message id"
    );
    assert!(
        overlay.error().is_none(),
        "a successful send must not record an error"
    );
    assert!(
        overlay.is_sending(),
        "pi never clears `sending` on the success path (the overlay is torn down instead)"
    );

    me.disconnect();
    target_client.disconnect();
}

// Regression proof for "sendMessage's failure path (error/`sending=false`, buffer preserved) has
// no cyrup equivalent": against the PRE-FIX code (no `send_message` method) this would not
// compile; asserts an undelivered send records the broker's reason as the overlay error, clears
// `sending` so a retry is possible, yields no `ComposeResult`, and — matching pi, which never
// touches `inputBuffer` on failure — leaves the typed text intact for the retry prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_message_records_the_error_and_clears_sending_when_undelivered() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("/me"),
            Some("me-session".to_string()),
        )
        .await
        .expect("connects"),
    );

    let mut target = session();
    target.id = "no-such-session".to_string();
    let mut overlay = ComposeOverlay::new(target, "ghost".to_string());
    overlay.handle_input(&DefaultKeybindings, "hi");

    let result = overlay.send_message(&me).await;
    assert!(
        result.is_none(),
        "an undelivered send must not yield a ComposeResult"
    );
    assert!(
        !overlay.is_sending(),
        "a failed send must clear `sending` so the user can retry"
    );
    assert_eq!(overlay.error(), Some("Session not found"));
    assert_eq!(
        overlay.input(),
        "hi",
        "the typed text must survive the failure for the retry prompt"
    );

    me.disconnect();
}
