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

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

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
                value
                    .get("stream")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                value
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                value,
            )
        })
        .collect()
}

fn events(logs_dir: &Path) -> Vec<String> {
    entries(logs_dir)
        .into_iter()
        .map(|(_, event, _)| event)
        .collect()
}

fn record<'a>(
    all: &'a [(String, String, serde_json::Value)],
    event: &str,
) -> &'a serde_json::Value {
    all.iter()
        .find(|(_, name, _)| name == event)
        .map(|(_, _, value)| value)
        .unwrap_or_else(|| {
            panic!(
                "no `{event}` entry; got {:?}",
                all.iter().map(|e| &e.1).collect::<Vec<_>>()
            )
        })
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
    for dir in [
        &location.session_root,
        &location.requests_dir,
        &location.responses_dir,
    ] {
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
    assert!(
        !decision.approved,
        "an unanswered forwarded ask fails CLOSED"
    );

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
    assert!(
        created["requestId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(
        created["responsePath"]
            .as_str()
            .is_some_and(|p| p.ends_with(".json"))
    );

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
        &ExtensionConfig {
            yolo_mode: true,
            ..ExtensionConfig::default()
        },
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
    let auto_at = names
        .iter()
        .position(|e| e == "forwarded_permission.auto_approved")
        .unwrap();
    assert!(
        auto_at < approved_at,
        "the decision entry follows the resolution entry"
    );
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
/// The malformed sibling is asserted here too, and **this assertion was inverted until 2026-08-14**.
///
/// The previous version of this test pinned SILENCE for an unparseable request, reasoning that
/// `if (!request) { safeDeleteFile(...); continue; }` (v0.8.0 `index.ts:1144-1147`) carries no
/// `logPermissionForwardingWarning` above it, unlike every branch below it. That reading of the
/// CALLER is correct and the conclusion drawn from it is not: upstream raises the entry one frame
/// down, inside `readForwardedPermissionRequest` itself — `Failed to read forwarded permission
/// request '${filePath}'` in its `catch` (`:942`) and `Ignoring invalid forwarded permission
/// request format in '${filePath}'` when the field ladder rejects well-formed JSON (`:928`). A
/// non-JSON request file therefore DOES warn upstream, and cyrup's silence was the divergence the
/// old assertion had frozen in place.
///
/// It is left as a two-request test precisely because the ORDER matters: cyrup reads and services
/// each request in one sorted pass, so `broken.json` produces its reader warning before
/// `elsewhere.json` produces its off-session warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_off_session_request_and_a_malformed_one_each_warn() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = AuditTrail::detached(logs_dir.clone());
    let session_id = "parent-session-offsession";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    for dir in [
        &location.session_root,
        &location.requests_dir,
        &location.responses_dir,
    ] {
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
        vec![
            "permission_forwarding.warning".to_string(),
            "permission_forwarding.warning".to_string()
        ],
        "TWO warnings: the reader's for `broken.json`, then the loop's for `elsewhere.json`"
    );
    let all = entries(&logs_dir);
    let messages: Vec<&str> = all
        .iter()
        .filter(|(_, event, _)| event == "permission_forwarding.warning")
        .filter_map(|(_, _, value)| value["message"].as_str())
        .collect();

    // The reader's entry, upstream `index.ts:942`. Non-JSON bytes land in pi's `catch`, so this is
    // the "Failed to read" wording and it carries the parse error as its cause — NOT the
    // "Ignoring invalid … format" wording, which is reserved for well-formed JSON of the wrong
    // shape (`:928`, exercised by `a_structurally_invalid_request_warns_about_its_format`).
    assert!(
        messages[0].starts_with("Failed to read forwarded permission request '")
            && messages[0].contains("broken.json"),
        "the malformed request must be reported by the reader: {:?}",
        messages[0]
    );
    let broken_entry = &all[0].2;
    assert!(
        broken_entry["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "pi passes the caught error to this call site, so the entry carries a cause: {broken_entry}"
    );

    // The loop's entry, upstream `index.ts:1149-1151`.
    assert!(
        messages[1].contains("req-elsewhere") && messages[1].contains("some-other-parent"),
        "the off-session warning must name the request and the session it was addressed to: {:?}",
        messages[1]
    );
    assert!(
        all[1].2.get("error").is_none(),
        "pi passes NO error to the off-session call site, so the key is ABSENT: {}",
        all[1].2
    );

    // Both files are consumed either way — a request that cannot be serviced never lingers.
    assert!(!broken.exists(), "a malformed request is deleted");
    assert!(!elsewhere.exists(), "an off-session request is deleted");
}

/// The reader's OTHER entry: well-formed JSON whose fields fail the shape ladder is reported as
/// `Ignoring invalid forwarded permission request format in '<path>'` (v0.8.0 `index.ts:928`) —
/// a DIFFERENT message from the unparseable case above, and one upstream raises with **no** cause
/// attached, because at that point it is holding a parsed object and not an error.
///
/// The two are kept in separate tests because collapsing them is exactly the mistake a Rust port
/// invites: `serde_json::from_str::<T>` fails identically for "not JSON" and "JSON of the wrong
/// shape", so a single `.ok()?` would have produced one message for both and matched neither
/// upstream site faithfully.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_structurally_invalid_request_warns_about_its_format() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = AuditTrail::detached(logs_dir.clone());
    let session_id = "parent-session-shape";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    for dir in [
        &location.session_root,
        &location.requests_dir,
        &location.responses_dir,
    ] {
        std::fs::create_dir_all(dir).expect("creates the spool dirs");
    }
    // Valid JSON, valid object, every field the right TYPE — and `message` simply absent. This is
    // upstream's `typeof parsed.message !== "string"` leg and nothing else.
    let shaped = location.requests_dir.join("shaped.json");
    std::fs::write(
        &shaped,
        serde_json::json!({
            "id": "req-shape",
            "responseNonce": "nonce-3",
            "createdAt": 1_i64,
            "requesterSessionId": "child-session",
            "targetSessionId": session_id,
            "requesterAgentName": "coder"
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

    let all = entries(&logs_dir);
    assert_eq!(
        events(&logs_dir),
        vec!["permission_forwarding.warning".to_string()],
        "exactly one entry, from the reader"
    );
    let warning = &all[0].2;
    let message = warning["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("Ignoring invalid forwarded permission request format in '")
            && message.contains("shaped.json"),
        "a shape failure must use upstream's FORMAT wording, not the read wording: {message:?}"
    );
    assert!(
        warning.get("error").is_none(),
        "upstream passes no cause at `:928`, so the key is ABSENT: {warning}"
    );
    assert!(!shaped.exists(), "an unusable request is still consumed");
}

/// The two response-binding rejections, upstream `isForwardedPermissionResponseBoundToRequest`
/// (v0.8.0 `index.ts:890` and `:894`). cyrup dropped BOTH: a forged or misaddressed response was
/// discarded in silence, leaving only the `response_received` review entry with every field null —
/// enough for an operator to notice that something arrived, not enough to say what was wrong with
/// it, which is the whole point of a security trail.
///
/// Driven end to end rather than against the private predicate: the request id and nonce are read
/// back out of the spool the waiter actually wrote, so the forged responses differ from a genuine
/// one in exactly one field each.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_and_a_misaddressed_response_are_each_named_in_the_trail() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let logs_dir = agent_dir.path().join("logs");
    let audit = Arc::new(AuditTrail::detached(logs_dir.clone()));
    let target = "parent-session-binding";
    let location = forwarding_location(agent_dir.path(), target).expect("derives a location");

    let waiter = {
        let root = agent_dir.path().to_path_buf();
        let audit = Arc::clone(&audit);
        tokio::spawn(async move {
            wait_for_forwarded_approval(
                &root,
                "parent-session-binding",
                "child-session",
                "coder",
                "run bash?",
                // Generous, because the test never relies on it expiring — the final GOOD response
                // is what ends the wait. It exists only so a broken build fails instead of hanging.
                Duration::from_secs(20),
                &audit,
            )
            .await
        })
    };

    // The waiter names its own request; read it back rather than guessing the uuid.
    let request = loop {
        if let Ok(read_dir) = std::fs::read_dir(&location.requests_dir)
            && let Some(path) = read_dir
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            && let Some(request) = cyrup_permission_system_request(&path)
        {
            break request;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let request_id = request.0;
    let nonce = request.1;
    let response_path = location.responses_dir.join(format!("{request_id}.json"));

    let respond = |nonce: &str, responder: &str, approved: bool| {
        std::fs::write(
            &response_path,
            serde_json::json!({
                "requestId": request_id.clone(),
                "responseNonce": nonce,
                "approved": approved,
                "state": "approved",
                "responderSessionId": responder,
                "respondedAt": 1_i64,
            })
            .to_string(),
        )
        .expect("writes a response");
    };
    // The waiter CONSUMES each response file it reads (pi `:1066`), so the file vanishing is the
    // handshake that says "this one has been judged" — no sleeps, no margins.
    async fn until_consumed(path: &Path) {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // 1. Right request id, WRONG nonce → "is not bound to request".
    respond("forged-nonce", target, true);
    until_consumed(&response_path).await;
    // 2. Right nonce, WRONG responder session → "does not match target session".
    respond(&nonce, "some-other-session", true);
    until_consumed(&response_path).await;
    // 3. Genuine, which ends the wait.
    respond(&nonce, target, true);

    let decision = waiter.await.expect("the waiter task completes");
    assert!(
        decision.approved,
        "the third, genuine response must be honoured"
    );

    let all = entries(&logs_dir);
    let warnings: Vec<&str> = all
        .iter()
        .filter(|(_, event, _)| event == "permission_forwarding.warning")
        .filter_map(|(_, _, value)| value["message"].as_str())
        .collect();
    assert_eq!(
        warnings.len(),
        2,
        "exactly one warning per rejected response: {warnings:?}"
    );
    assert!(
        warnings[0].contains("is not bound to request")
            && warnings[0].contains(&request_id)
            && warnings[0].contains("Ignoring forwarded permission response '"),
        "the nonce mismatch must be reported as an unbound response: {:?}",
        warnings[0]
    );
    assert!(
        warnings[1].contains("does not match target session")
            && warnings[1].contains("some-other-session")
            && warnings[1].contains(target),
        "the responder mismatch must name BOTH sessions: {:?}",
        warnings[1]
    );
    // Presence-before-absence: the review entry for every observed response is still written, so
    // the two warnings are an ADDITION to the trail, not a replacement for it.
    assert_eq!(
        all.iter()
            .filter(|(_, e, _)| e == "forwarded_permission.response_received")
            .count(),
        3,
        "all three response files are still recorded as received"
    );
}

/// `(id, responseNonce)` of a request file, or `None` while it is still being written.
fn cyrup_permission_system_request(path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        value["id"].as_str()?.to_string(),
        value["responseNonce"].as_str()?.to_string(),
    ))
}
