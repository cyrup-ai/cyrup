//! PERM-008's outstanding Verify recipe: the forwarding path's review/debug entries must actually
//! LAND in the JSONL, not merely be called.
//!
//! PERM-008 landed the eleven call sites and `AuditTrail`, but its own Verify — "drive a forwarded
//! ask through approve / timeout / malformed with a logs dir set and assert
//! `forwarded_permission.request_created`, `.approved`, `.response_timed_out` and a
//! `permission_forwarding.warning`" — was left outstanding, and the audited path was only exercised
//! transitively by `forwarding_has_ui_guard.rs`, which asserts nothing about the trail. A trail
//! nobody reads back is exactly the class of defect that ships broken: `review()` swallows its own
//! write failure into the warning reporter, so a path that never writes and a path that fails to
//! write look identical from inside the extension.
//!
//! Upstream sites, all v0.8.0 `pi-permission-system/src/index.ts`:
//! `:1032` `forwarded_permission.request_created`, `:1077` the `permission_forwarding.warning`,
//! `:1078-1083` `.response_timed_out`, `:1184` `.auto_approved`, `:1228` `.approved`/`.denied`,
//! `:735` `logPermissionForwardingEntry`'s `permission_forwarding.error`.
//!
//! # Why no env var is set here
//!
//! The item's recipe says "with a logs dir set", meaning `LOGS_DIR_ENV_KEY`. This module's own
//! doc (`tests/mod.rs`) forbids process-env mutation in-crate, and the env var is only the
//! OVERRIDE arm of [`crate::logging::resolve_logs_dir`] — its other arm is the `default_logs_dir`
//! [`AuditTrail::detached`] takes, which reaches the identical `write_line`. Directing the trail at
//! a temp dir by construction tests the same writer with no process-global hazard.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_ext::{DialogOptions, HostServices};

use crate::ExtensionConfig;
use crate::forwarding::{
    ProcessForwardedOptions, forwarding_location, process_forwarded_requests,
    wait_for_forwarded_approval,
};
use crate::logging::{AuditTrail, debug_path};

/// Every entry the trail wrote, in write order, as `(stream, event)` plus the raw record.
fn entries(logs_dir: &Path) -> Vec<(String, String, serde_json::Value)> {
    let path = debug_path(logs_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("every trail line is one JSON object");
            (
                value.get("stream").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                value.get("event").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                value,
            )
        })
        .collect()
}

fn events(logs_dir: &Path) -> Vec<String> {
    entries(logs_dir).into_iter().map(|(_, event, _)| event).collect()
}

fn record<'a>(
    all: &'a [(String, String, serde_json::Value)],
    event: &str,
) -> &'a serde_json::Value {
    all.iter()
        .find(|(_, name, _)| name == event)
        .map(|(_, _, value)| value)
        .unwrap_or_else(|| panic!("no `{event}` entry; got {:?}", all.iter().map(|e| &e.1).collect::<Vec<_>>()))
}

/// A backend that resolves its own session id and refuses every dialog — the approval below comes
/// from yolo mode, so reaching `select` would mean the auto-approve arm was skipped.
struct SessionOnlyServices(String);

impl HostServices for SessionOnlyServices {
    fn session_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
    fn select(&self, _t: &str, _o: &serde_json::Value, _d: &DialogOptions) -> Option<String> {
        unreachable!("yolo mode auto-approves before any dialog opens");
    }
}

fn seed_request(agent_dir: &Path, session_id: &str) -> PathBuf {
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
            "message": "run bash?"
        })
        .to_string(),
    )
    .expect("writes the request");
    path
}

/// THE CHILD HALF — `forwarded_permission.request_created` on the way in, then, on expiry, the
/// `permission_forwarding.warning` FIRST and `forwarded_permission.response_timed_out` second.
///
/// That ORDER is upstream's (`index.ts:1077` then `:1078-1083`) and is the reason this asserts
/// positions rather than mere membership: the warning is what an operator greps for, and a trail
/// that reported the timeout before warning about it would read as two unrelated events.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forwarded_ask_that_times_out_leaves_the_created_warning_and_timed_out_entries() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = AuditTrail::detached(logs_dir.clone());

    let decision = wait_for_forwarded_approval(
        agent_dir.path(),
        "parent-session-audit",
        "child-session",
        "coder",
        "run bash?",
        // No parent is servicing this spool, so the wait can only expire. Short, because the bound
        // is a parameter here — nothing about this test is racing a real 10-minute default.
        Duration::from_millis(80),
        &audit,
    )
    .await;
    assert!(!decision.approved, "an unanswered forwarded ask fails CLOSED");

    let all = entries(&logs_dir);
    let names: Vec<&str> = all.iter().map(|(_, event, _)| event.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "forwarded_permission.request_created",
            "permission_forwarding.warning",
            "forwarded_permission.response_timed_out",
        ],
        "the trail must carry exactly upstream's three entries, in upstream's order"
    );

    // The security stream is UNCONDITIONAL at v0.8.0 (`logging.ts:98-100`), and `debug` is off in
    // the default config used here — so the two review entries had to reach disk without anyone
    // enabling anything.
    for (stream, event, _) in &all {
        if event.starts_with("forwarded_permission.") {
            assert_eq!(stream, "review", "`{event}` belongs to the security stream");
        }
    }

    // The created entry carries the correlation keys an operator joins on (`index.ts:1030-1037`).
    let created = record(&all, "forwarded_permission.request_created");
    assert_eq!(created["requesterAgentName"], "coder");
    assert_eq!(created["targetSessionId"], "parent-session-audit");
    assert!(created["requestId"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(created["responsePath"].as_str().is_some_and(|p| p.ends_with(".json")));

    // …and the timed-out entry names the SAME request.
    let timed_out = record(&all, "forwarded_permission.response_timed_out");
    assert_eq!(timed_out["requestId"], created["requestId"]);
}

/// THE PARENT HALF — a serviced request writes `forwarded_permission.auto_approved` (`index.ts:1184`)
/// and then `.approved` (`:1228`), with the resolution on the second.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_serviced_request_leaves_the_auto_approved_and_approved_entries() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = AuditTrail::detached(logs_dir.clone());
    let session_id = "parent-session-approve";
    seed_request(agent_dir.path(), session_id);
    let services: Arc<dyn HostServices> = Arc::new(SessionOnlyServices(session_id.to_string()));

    process_forwarded_requests(
        agent_dir.path(),
        session_id,
        &services,
        // yolo is upstream's `shouldAutoApprovePermissionState("ask", …)` arm, which resolves the
        // decision with no dialog — so the approval is deterministic, not scripted through a mock.
        &ExtensionConfig { yolo_mode: true, ..ExtensionConfig::default() },
        ProcessForwardedOptions::preserve_location(),
        &audit,
        true,
    )
    .await;

    let all = entries(&logs_dir);
    let names = events(&logs_dir);
    assert!(
        names.contains(&"forwarded_permission.auto_approved".to_string()),
        "yolo's auto-approve entry is missing: {names:?}"
    );
    let approved_at = names
        .iter()
        .position(|e| e == "forwarded_permission.approved")
        .unwrap_or_else(|| panic!("no `.approved` entry: {names:?}"));
    let auto_at = names.iter().position(|e| e == "forwarded_permission.auto_approved").unwrap();
    assert!(auto_at < approved_at, "the decision entry follows the resolution entry");
    assert!(
        !names.contains(&"forwarded_permission.denied".to_string()),
        "an approved request must not also record a denial"
    );

    let approved = record(&all, "forwarded_permission.approved");
    assert_eq!(approved["requestId"], "req-1");
    assert_eq!(approved["source"], "primary");
    assert_eq!(approved["requesterAgentName"], "coder");
    assert!(
        approved.get("responsePath").is_some() && approved.get("requestPath").is_none(),
        "the decision entry's shape is upstream's: responsePath, NOT requestPath ({approved})"
    );
}

/// THE OFF-SESSION CASE — a request addressed to a different parent is reported through
/// `permission_forwarding.warning` (pi `logPermissionForwardingWarning`, v0.8.0 `index.ts:1149-1151`)
/// and then deleted.
///
/// The malformed sibling is asserted here too, and it asserts the OPPOSITE: an unparseable request
/// is deleted with NO entry at all. That is upstream's actual shape — `if (!request) {
/// safeDeleteFile(...); continue; }` at `index.ts:1144-1147` has no `logPermissionForwardingWarning`
/// above it, unlike every branch below it. Pinning the silence stops a future pass from "improving"
/// the trail into a divergence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_off_session_request_warns_while_a_malformed_one_is_deleted_silently() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = AuditTrail::detached(logs_dir.clone());
    let session_id = "parent-session-offsession";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    for dir in [&location.session_root, &location.requests_dir, &location.responses_dir] {
        std::fs::create_dir_all(dir).expect("creates the spool dirs");
    }
    // Sorted scan order (pi `.sort()`, `index.ts:1375`) puts `broken` before `elsewhere`.
    let broken = location.requests_dir.join("broken.json");
    std::fs::write(&broken, "{ not json").expect("writes");
    let elsewhere = location.requests_dir.join("elsewhere.json");
    std::fs::write(
        &elsewhere,
        serde_json::json!({
            "id": "req-elsewhere",
            "responseNonce": "nonce-2",
            "createdAt": 1_i64,
            "requesterSessionId": "child-session",
            "targetSessionId": "some-other-parent",
            "requesterAgentName": "coder",
            "message": "run bash?"
        })
        .to_string(),
    )
    .expect("writes");

    let services: Arc<dyn HostServices> = Arc::new(SessionOnlyServices(session_id.to_string()));
    process_forwarded_requests(
        agent_dir.path(),
        session_id,
        &services,
        &ExtensionConfig::default(),
        ProcessForwardedOptions::preserve_location(),
        &audit,
        true,
    )
    .await;

    let names = events(&logs_dir);
    assert_eq!(
        names,
        vec!["permission_forwarding.warning".to_string()],
        "exactly ONE entry: the off-session warning. The malformed request is upstream-silent."
    );
    let all = entries(&logs_dir);
    let warning = record(&all, "permission_forwarding.warning");
    assert!(
        warning["message"].as_str().is_some_and(|m| m.contains("req-elsewhere")
            && m.contains("some-other-parent")),
        "the warning must name the request and the session it was addressed to: {warning}"
    );

    // Both files are consumed either way — a request that cannot be serviced never lingers.
    assert!(!broken.exists(), "a malformed request is deleted");
    assert!(!elsewhere.exists(), "an off-session request is deleted");
}
