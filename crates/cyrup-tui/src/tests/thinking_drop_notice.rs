//! TUI-117 — `Anthropic dropped N thinking block(s) (details in session)`.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/interactive-mode.ts:3985-4025
//! private maybeShowThinkingDropNotice(message: AssistantMessage): void {
//!     if (!this.settingsManager.getShowCacheMissNotices()) return;
//!     const droppedCount = InteractiveMode.countDroppedThinkingBlocks(message);
//!     if (droppedCount === 0) return;
//!     let previousDroppedCount = 0;
//!     // message_end reaches the UI before the current message is persisted,
//!     // so the branch's last assistant message is the previous response.
//!     …
//!     if (droppedCount <= previousDroppedCount) return;
//!     const noun = droppedCount === 1 ? "thinking block" : "thinking blocks";
//!     … `Anthropic dropped ${droppedCount} ${noun} (details in session)`
//! }
//! ```
//!
//! cyrup persists a finished message BEFORE it fans `MessageEnd` out, so the finishing message is
//! already the session's last assistant entry. These tests drive the real seam
//! ([`App::ingest_session_event_owned`]) against a real session file, so a reading that takes "the
//! last entry" as the previous response (`prev == cur`, never fires) is caught here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::transcript::Entry;
use crate::{App, UiTheme};
use cyrup_agent::AgentMessage;
use cyrup_core::{
    AssistantMessage, AssistantMessageDiagnostic, ProviderId, StopReason,
    append_assistant_message_diagnostic,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionRuntime, SessionConfig, SessionFactory,
    SessionTarget,
};
use ratatui::backend::TestBackend;
use serde_json::json;
use tempfile::TempDir;

fn assistant(dropped: usize, stop: StopReason) -> AssistantMessage {
    let mut a = AssistantMessage::errored(ProviderId::from("anthropic"), "claude", None, stop, "");
    a.error_message = None;
    if dropped > 0 {
        let transformations: Vec<_> = (0..dropped)
            .map(|i| json!({"type": "thinking_dropped", "path": format!("messages.{i}.content.0")}))
            .collect();
        append_assistant_message_diagnostic(
            &mut a.diagnostics,
            AssistantMessageDiagnostic {
                r#type: "anthropic_input_transformations".to_string(),
                timestamp: 1,
                error: None,
                details: Some(json!({ "transformations": transformations })),
            },
        );
    }
    a
}

/// A session file: one user message, then one assistant message per entry of `counts`.
fn session_file(dir: &Path, cwd: &Path, counts: &[usize]) -> PathBuf {
    let ts = "2026-01-01T00:00:00.000Z";
    let mut lines = vec![
        format!(
            r#"{{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0002","timestamp":"{ts}","cwd":{}}}"#,
            serde_json::to_string(&cwd.to_string_lossy()).unwrap()
        ),
        format!(
            r#"{{"type":"message","id":"u1","parentId":null,"timestamp":"{ts}","message":{{"role":"user","content":"hi","timestamp":1}}}}"#
        ),
    ];
    let mut parent = "u1".to_string();
    for (i, n) in counts.iter().enumerate() {
        let id = format!("a{i}");
        let mut message = serde_json::to_value(assistant(*n, StopReason::Stop)).unwrap();
        message["role"] = json!("assistant");
        lines.push(
            json!({"type": "message", "id": id, "parentId": parent, "timestamp": ts, "message": message})
                .to_string(),
        );
        parent = id;
    }
    let path = dir.join("s.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

async fn open(tmp: &TempDir, counts: &[usize]) -> (Arc<AgentSessionRuntime>, Arc<AgentSession>) {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let path = session_file(tmp.path(), &cwd, counts);
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let rt = AgentSessionRuntime::create(
        Arc::new(SessionFactory::new(provider, config)),
        SessionTarget::New,
    )
    .await
    .unwrap();
    rt.switch_session(&path).await.unwrap();
    let session = rt.session().await;
    (rt, session)
}

/// Finish an assistant turn through the real seam and return the warnings it produced.
async fn finish_turn(counts: &[usize], stop: StopReason, notices_on: bool) -> Vec<String> {
    let tmp = TempDir::new().unwrap();
    let (_rt, session) = open(&tmp, counts).await;
    let mut app = App::new(TestBackend::new(80, 14), UiTheme::dark()).unwrap();
    app.state_mut().show_cache_miss_notices = notices_on;
    // The message on the wire is the session's last assistant entry (persist-then-fan-out), with
    // the stop reason under test.
    let message = assistant(*counts.last().unwrap(), stop);
    let message = Arc::new(message);
    app.ingest_session_event_owned(
        AgentSessionEvent::MessageStart {
            message: AgentMessage::Assistant(Arc::clone(&message)),
        },
        &session,
    )
    .await;
    app.ingest_session_event_owned(
        AgentSessionEvent::MessageEnd {
            message: AgentMessage::Assistant(message),
        },
        &session,
    )
    .await;
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Warning(w) => Some(w.clone()),
            _ => None,
        })
        .collect()
}

/// `(0 then 1)`: the count rose, so one warning, singular noun.
#[tokio::test]
async fn a_rising_count_warns_once_in_the_singular() {
    let w = finish_turn(&[0, 1], StopReason::Stop, true).await;
    assert_eq!(
        w,
        vec!["Anthropic dropped 1 thinking block (details in session)".to_string()]
    );
}

/// `(1 then 1)`: the same drop repeated is reported only at the turn it started.
#[tokio::test]
async fn an_unchanged_count_does_not_warn() {
    let w = finish_turn(&[1, 1], StopReason::Stop, true).await;
    assert!(w.is_empty(), "{w:?}");
}

/// `count === 1 ? "thinking block" : "thinking blocks"`, and a first-ever assistant message has
/// previous = 0.
#[tokio::test]
async fn a_plural_count_uses_the_plural_noun_and_a_first_message_has_no_previous() {
    let w = finish_turn(&[1, 3], StopReason::Stop, true).await;
    assert_eq!(
        w,
        vec!["Anthropic dropped 3 thinking blocks (details in session)".to_string()]
    );
    let w = finish_turn(&[2], StopReason::Stop, true).await;
    assert_eq!(
        w,
        vec!["Anthropic dropped 2 thinking blocks (details in session)".to_string()]
    );
}

/// `if (!getShowCacheMissNotices()) return;`
#[tokio::test]
async fn notices_off_shows_nothing() {
    let w = finish_turn(&[0, 1], StopReason::Stop, false).await;
    assert!(w.is_empty(), "{w:?}");
}

/// pi calls it only on the clean branch of `message_end` (`:3468`, the `else` of the
/// aborted/error test).
#[tokio::test]
async fn an_errored_turn_shows_nothing() {
    let w = finish_turn(&[0, 1], StopReason::Error, true).await;
    assert!(!w.iter().any(|w| w.contains("thinking block")), "{w:?}");
}
