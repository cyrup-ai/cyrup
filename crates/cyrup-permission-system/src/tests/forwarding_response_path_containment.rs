//! FULLY-WIRED regression proof that a forwarded permission request cannot steer the PARENT into
//! writing outside its own response spool.
//!
//! `process_forwarded_requests` reads `ForwardedPermissionRequest` documents out of
//! `<spool>/requests/`, a directory any subagent child able to forward a request may write into,
//! and then names the response file after a field of that document:
//!
//! ```text
//! let response_path = location.responses_dir.join(format!("{}.json", request.id));
//! write_json_atomic(&response_path, &response)
//! ```
//!
//! `request.id` arrives verbatim from the untrusted JSON body. `Path::join` resolves `..`
//! components, so an id of `../../../../.bashrc` makes the parent — the TRUSTED process, running
//! with the human's authority — write an attacker-influenced JSON document at an arbitrary
//! filesystem location. That is an arbitrary-file-write primitive reachable from any child that has
//! permission forwarding available at all.
//!
//! Two properties are asserted here, and BOTH are load-bearing:
//!
//! 1. **The write is contained.** A traversal id never produces a file outside `responses_dir`.
//!
//! 2. **The rejection happens BEFORE the human is asked.** `resolve_forwarded_decision` surfaces
//!    the live ask dialog, and the response is written on *either* answer — so validating after the
//!    decision would neither stop the write nor stop a hostile request from interrupting the user
//!    with a plausible-looking prompt. Denial is not a defence: the expired-on-read case below
//!    auto-DENIES and still writes its response file, which is precisely what makes the traversal
//!    exploitable without any user interaction at all.
//!
//! The crate already owns the right primitive and already applies it one level up — `forwarding.rs`
//! validates the session-id token with `validate_safe_token` before deriving the spool location
//! (`:187`). This is the same check, applied to the other untrusted token that reaches a
//! `Path::join`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::sync::Arc;

use crate::{
    ExtensionConfig, ForwardedPermissionRequest,
    forwarding::{
        FORWARDING_AGENT_DIR_ENV, ProcessForwardedOptions, forwarding_location,
        process_forwarded_requests,
    },
};
use cyrup_ext::HostServices;

struct SessionIdServices(String);

impl HostServices for SessionIdServices {
    fn session_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// A request that is already past `PERMISSION_FORWARDING_TIMEOUT` (`created_at: 0` is 1970), so
/// `resolve_forwarded_decision` takes the expired-on-read branch: an automatic DENY, with no dialog
/// and no host interaction.
///
/// Using the auto-deny path is deliberate. It keeps the test free of any UI plumbing, and it
/// demonstrates the sharpest form of the defect: the traversal write lands even when the request is
/// refused, so nothing about the decision protects the filesystem.
fn expired_request_json(id: &str, target_session_id: &str) -> String {
    serde_json::to_string(&ForwardedPermissionRequest {
        id: id.to_string(),
        response_nonce: "nonce-1".to_string(),
        created_at: 0,
        requester_session_id: "child-session".to_string(),
        target_session_id: target_session_id.to_string(),
        requester_agent_name: "hostile-child".to_string(),
        message: "Subagent wants to run bash".to_string(),
    })
    .expect("the request serializes")
}

/// Recursively hunt for `pwned.json` anywhere beneath `root`. Returns where it landed, if it did.
fn find_marker(root: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == "pwned.json") {
            return Some(path);
        }
        if path.is_dir()
            && let Some(hit) = find_marker(&path)
        {
            return Some(hit);
        }
    }
    None
}

async fn drain(agent_dir: &Path, session_id: &str) {
    let services: Arc<dyn HostServices> = Arc::new(SessionIdServices(session_id.to_string()));
    process_forwarded_requests(
        agent_dir,
        session_id,
        &services,
        &ExtensionConfig::default(),
        ProcessForwardedOptions::preserve_location(),
        // PERM-008/PERM-031: the shared audit trail, and the live `has_ui` pi re-reads per scan.
        &crate::logging::AuditTrail::detached(agent_dir.join("logs")),
        true,
    )
    .await;
}

/// THE SECURITY ASSERTION: a `..` id in the request body must not escape the response spool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_traversal_request_id_cannot_write_outside_the_response_spool() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    assert!(
        crate::envx::var(FORWARDING_AGENT_DIR_ENV).is_none(),
        "this test derives the spool from its own temp agent dir; an ambient \
         {FORWARDING_AGENT_DIR_ENV} override would invalidate it"
    );

    let session_id = "parent-session-containment";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    std::fs::create_dir_all(&location.requests_dir).expect("creates the inbox");
    std::fs::create_dir_all(&location.responses_dir).expect("creates the outbox");

    // The oracle is a recursive sweep for the escaped filename, NOT a hand-computed target path.
    //
    // Two ways to get that wrong, both of which produce a test that passes against vulnerable code:
    //   - `Path::starts_with` is LEXICAL and does not resolve `..`. `responses/../../x` does "start
    //     with" `responses` as far as it is concerned, so a containment assertion written that way
    //     is vacuous — and so is a containment *fix* written that way.
    //   - Counting `..` components by hand is off-by-one bait. `../../../../` from `responses/`
    //     lands on `<agent>/sessions/`, not on `<agent>/sessions/permission-forwarding/`.
    //
    // Sweeping the whole temp agent dir for the marker filename is immune to both: wherever the
    // write lands under the agent dir, the sweep finds it.
    let hostile_id = "../../../../pwned";
    assert!(
        find_marker(agent_dir.path()).is_none(),
        "the marker must start absent"
    );

    let hostile = location.requests_dir.join("hostile.json");
    std::fs::write(&hostile, expired_request_json(hostile_id, session_id))
        .expect("writes the hostile request");

    drain(agent_dir.path(), session_id).await;

    // THE DEFECT: pre-fix, `responses_dir.join("../../../../pwned.json")` resolves out of the spool
    // and `write_json_atomic` creates the file — even though the request was auto-DENIED.
    if let Some(found) = find_marker(agent_dir.path()) {
        panic!(
            "a forwarded request with a `..` id must never cause a write outside the response \
             spool, but one appeared at {} — the parent process was steered into an arbitrary file \
             write by an untrusted child's JSON body",
            found.display()
        );
    }

    // The hostile request must also be consumed, not left to be retried on every subsequent scan.
    assert!(
        !hostile.exists(),
        "the rejected request must be deleted, otherwise the watcher re-processes it forever"
    );

    // Nothing at all should have been written into the legitimate outbox either.
    let responses: Vec<_> = std::fs::read_dir(&location.responses_dir)
        .expect("reads the outbox")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        responses.is_empty(),
        "a rejected request must leave no response behind, found {responses:?}"
    );
}

/// NON-VACUITY: the same drain, with an ordinary id, MUST still write its response.
///
/// Without this the security assertion above would also pass against a build that simply refused
/// every forwarded request — a "fix" that silently breaks permission forwarding entirely.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_request_id_still_receives_its_response() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-benign";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    std::fs::create_dir_all(&location.requests_dir).expect("creates the inbox");
    std::fs::create_dir_all(&location.responses_dir).expect("creates the outbox");

    let benign = location.requests_dir.join("benign.json");
    std::fs::write(&benign, expired_request_json("req-1", session_id))
        .expect("writes the benign request");

    drain(agent_dir.path(), session_id).await;

    let response_path = location.responses_dir.join("req-1.json");
    assert!(
        response_path.is_file(),
        "an ordinary request id must still be answered at {} — the containment check must reject \
         traversal, not forwarding",
        response_path.display()
    );
    assert!(
        !benign.exists(),
        "an answered request is deleted once its response is written"
    );
}
