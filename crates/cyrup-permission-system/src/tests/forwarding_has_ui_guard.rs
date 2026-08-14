//! PERM-031 — the forwarding scan re-checks `has_ui` on EVERY pass, not only at watcher start.
//!
//! pi opens `processForwardedPermissionRequests` with `if (!ctx.hasUI) { return; }`
//! (v0.8.0 `index.ts:1113-1116`), and the `ctx` it reads is the LIVE `permissionForwardingContext`
//! object (`:1666`), so a UI that detaches mid-session stops the spool being serviced without any
//! hook having to fire. Each pending request then stays on disk until the UI returns or the CHILD's
//! own bound expires — pi DEFERS.
//!
//! Cyrup's only `has_ui` test lived at watcher (re)start, which is re-evaluated solely when a
//! `ToolCall` / `Input` / `BeforeAgentStart` / `SessionStart` hook fires. Between hooks the spawned
//! task kept polling, and its decision path terminates in `AskOutcome::NoLiveChannel => denied()`,
//! which writes a nonce-bound DENY the child consumes as the operator's FINAL answer. Cyrup
//! answered "denied" on behalf of an absent human.
//!
//! These tests drive `process_forwarded_requests` — the exact function the watcher calls on every
//! wake — against a `HostServices` whose `select` PANICS, so a scan that reaches the dialog fails
//! loudly rather than silently producing the denial it used to.

use std::path::Path;
use std::sync::Arc;

use cyrup_ext::{DialogOptions, HostServices};

use crate::forwarding::{
    forwarding_location, process_forwarded_requests, ProcessForwardedOptions,
};
use crate::logging::AuditTrail;
use crate::ExtensionConfig;

/// A backend that resolves its own session id and REFUSES to be asked anything. Reaching `select`
/// at all is the defect this file pins, so it is an assertion failure rather than a scripted answer.
struct NoDialogServices(String);

impl HostServices for NoDialogServices {
    fn session_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
    fn select(&self, _title: &str, _options: &serde_json::Value, _opts: &DialogOptions) -> Option<String> {
        unreachable!("a scan with no UI must never open the forwarded-permission dialog")
    }
}

fn seed_request(agent_dir: &Path, session_id: &str) -> std::path::PathBuf {
    let location = forwarding_location(agent_dir, session_id).expect("derives a location");
    for dir in [&location.session_root, &location.requests_dir, &location.responses_dir] {
        std::fs::create_dir_all(dir).expect("creates the spool dirs");
    }
    let path = location.requests_dir.join("req-1.json");
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    std::fs::write(
        &path,
        serde_json::json!({
            "id": "req-1",
            "responseNonce": "nonce-1",
            "createdAt": created_at,
            "requesterSessionId": "child-session",
            "targetSessionId": session_id,
            "requesterAgentName": "coder",
            "message": "run bash 'rm -rf /'?"
        })
        .to_string(),
    )
    .expect("writes the request");
    path
}

/// THE ASSERTION: with `has_ui == false` the request is neither answered nor consumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_without_a_ui_leaves_the_request_on_disk_and_writes_no_response() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-noui";
    let request_path = seed_request(agent_dir.path(), session_id);
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    let services: Arc<dyn HostServices> = Arc::new(NoDialogServices(session_id.to_string()));

    // Several passes, exactly as the watcher's ticker would produce between two hooks.
    for _ in 0..3 {
        process_forwarded_requests(
            agent_dir.path(),
            session_id,
            &services,
            &ExtensionConfig::default(),
            ProcessForwardedOptions::preserve_location(),
            &AuditTrail::detached(agent_dir.path().join("logs")),
            false,
        )
        .await;
    }

    assert!(
        request_path.is_file(),
        "pi DEFERS: the request must still be on disk for the UI to come back to"
    );
    assert!(
        !location.responses_dir.join("req-1.json").exists(),
        "no response may be written on the child's behalf while no human is reachable — before the \
         fix this was a nonce-bound DENY the child consumed as final"
    );
}

/// The mirror case: the same fixture with `has_ui == true` DOES reach the dialog, which proves the
/// test above is measuring the guard and not a spool that was never serviceable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_request_is_serviced_once_a_ui_is_present() {
    /// Answers the dialog with a plain deny, so the scan completes and writes a response.
    struct RejectingServices(String);
    impl HostServices for RejectingServices {
        fn session_id(&self) -> Option<String> {
            Some(self.0.clone())
        }
        fn select(&self, _t: &str, _o: &serde_json::Value, _d: &DialogOptions) -> Option<String> {
            Some("reject".to_string())
        }
    }

    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-ui";
    let request_path = seed_request(agent_dir.path(), session_id);
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    let services: Arc<dyn HostServices> = Arc::new(RejectingServices(session_id.to_string()));

    process_forwarded_requests(
        agent_dir.path(),
        session_id,
        &services,
        &ExtensionConfig::default(),
        ProcessForwardedOptions::preserve_location(),
        &AuditTrail::detached(agent_dir.path().join("logs")),
        true,
    )
    .await;

    assert!(!request_path.exists(), "a serviced request is consumed");
    assert!(
        location.responses_dir.join("req-1.json").exists(),
        "with a UI the parent answers, so the control case must produce a response"
    );
}
