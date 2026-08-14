//! FULLY-WIRED regression proof for pi's `preserveLocation` option on
//! `processForwardedPermissionRequests` (`pi-permission-system` v0.7.1 `src/index.ts:1358`,
//! `:1501-1503`):
//!
//! ```text
//! export async function processForwardedPermissionRequests(
//!   ctx: ExtensionContext,
//!   options: { preserveLocation?: boolean } = {},
//! ): Promise<void> {
//!   …
//!   if (!options.preserveLocation) {
//!     cleanupPermissionForwardingLocationIfEmpty(location);
//!   }
//! }
//! ```
//!
//! pi's ONE production caller is the parent's re-entrant scan (`index.ts:1935`), and it passes
//! `{ preserveLocation: true }`. Cyrup's equivalent is the long-lived
//! [`spawn_forwarding_watcher`] task, which `ensure_location`s the spool once and then attaches a
//! `notify::PollWatcher` to `requests_dir` for the rest of the session. Running the unconditional
//! `cleanupPermissionForwardingLocationIfEmpty` there tears down the very inbox the watcher owns on
//! every drained scan — including the mandatory startup scan, which runs milliseconds after the
//! directories are created — leaving the watch attached to a deleted path and opening a window in
//! which a child that has already `ensure_location`d can have its request write land in (or fail
//! against) a directory the parent is removing.
//!
//! The assertions below drive the REAL watcher task, not the scan function directly. The signal
//! that a scan actually completed is a positive, observable event — the watcher deleting a request
//! addressed to a different session (`isForwardedPermissionRequestForSession`,
//! `index.ts:1397-1403`) — so nothing here depends on a sleep racing the scan.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::HostServices;
use crate::{
    forwarding::{forwarding_location, ProcessForwardedOptions, FORWARDING_AGENT_DIR_ENV},
    process_forwarded_requests, spawn_forwarding_watcher, ExtensionConfig,
    ForwardedPermissionRequest,
};

/// A `HostServices` that publishes a fixed session id — the only thing the watcher's attach phase
/// needs (`services.session_id()`), and the id every spool path is derived from.
struct SessionIdServices(String);

impl HostServices for SessionIdServices {
    fn session_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

fn request_json(target_session_id: &str) -> String {
    serde_json::to_string(&ForwardedPermissionRequest {
        id: "req-1".to_string(),
        response_nonce: "nonce-1".to_string(),
        created_at: i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0),
        requester_session_id: "child-session".to_string(),
        target_session_id: target_session_id.to_string(),
        requester_agent_name: "reviewer".to_string(),
        message: "Subagent wants to run bash".to_string(),
    })
    .expect("the request serializes")
}

/// Poll `predicate` until it holds or `bound` elapses. Bounded polling rather than a fixed sleep, so
/// the test stays honest under heavy CPU contention.
async fn wait_until(bound: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        if predicate() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// THE WIRED ASSERTION: the parent watcher must keep its own spool across a drained scan.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_parent_watcher_preserves_its_spool_across_a_drained_scan() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    // Scope the env override this crate's `forwarding_root_dir` honours to the temp dir. (Kept
    // unset here — the default agent dir argument is what we pass — but asserted so a stray
    // ambient value from another test cannot silently redirect the spool.)
    assert!(
        std::env::var(FORWARDING_AGENT_DIR_ENV).is_err(),
        "this test derives the spool from its own temp agent dir; an ambient \
         {FORWARDING_AGENT_DIR_ENV} override would invalidate it"
    );

    let session_id = "parent-session-preserve";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");

    let services: Arc<dyn HostServices> = Arc::new(SessionIdServices(session_id.to_string()));
    let config = Arc::new(Mutex::new(ExtensionConfig::default()));
    let watcher = spawn_forwarding_watcher(
        agent_dir.path().to_path_buf(),
        Arc::clone(&services),
        Arc::clone(&config),
        Arc::new(crate::logging::AuditTrail::detached(agent_dir.path().join("logs"))),
        // PERM-031: a UI is present for this test, which is the precondition for the spool being
        // serviced at all.
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    );

    // The watcher's attach phase creates the spool (`ensure_location`). Without the fix this
    // assertion is already where the test dies: the mandatory startup scan runs microseconds after
    // `ensure_location` and — with an unconditional `cleanupPermissionForwardingLocationIfEmpty` —
    // deletes all three directories before a single poll tick can ever observe them. The parent's
    // inbox is destroyed faster than a child could ever find it.
    assert!(
        wait_until(Duration::from_secs(10), || location.requests_dir.is_dir()).await,
        "the watcher must create AND KEEP its own request inbox at {} — if it never appears, the \
         startup scan's cleanup is deleting it as fast as the attach phase creates it (pi \
         `{{ preserveLocation: true }}`, index.ts:1935)",
        location.requests_dir.display()
    );

    // Drop in a request addressed to a DIFFERENT session. The watcher deletes it without needing a
    // human decision — which is our positive, race-free proof that a scan ran to completion and
    // left the inbox empty (exactly the state that triggers the cleanup).
    let stray = location.requests_dir.join("stray.json");
    std::fs::write(&stray, request_json("some-other-session")).expect("writes the stray request");
    assert!(
        wait_until(Duration::from_secs(10), || !stray.exists()).await,
        "the watcher must consume the mis-addressed request, proving a scan completed"
    );

    // THE DEFECT: pre-fix, the scan that just drained the inbox also `remove_dir`s all three spool
    // directories, so the watcher's own inbox — and the `PollWatcher` target — are gone.
    assert!(
        location.requests_dir.is_dir(),
        "the watcher must PRESERVE its request inbox across a drained scan (pi \
         `{{ preserveLocation: true }}`, index.ts:1935); missing: {}",
        location.requests_dir.display()
    );
    assert!(
        location.responses_dir.is_dir(),
        "the watcher must preserve its response dir: {}",
        location.responses_dir.display()
    );
    assert!(
        location.session_root.is_dir(),
        "the watcher must preserve its session root: {}",
        location.session_root.display()
    );

    // And it must STAY preserved across the later wakes (the ticker + watch loop), not just the
    // startup scan.
    let vanished = wait_until(Duration::from_secs(2), || !location.requests_dir.is_dir()).await;
    assert!(!vanished, "the spool must survive every subsequent scan, not only the first");

    watcher.abort();
}

/// The OTHER half of the option, so the fix is a faithful port and not a blanket removal of the
/// cleanup: the DEFAULT bag (pi's `= {}`) still tears an empty spool down.
#[tokio::test]
async fn the_default_option_bag_still_cleans_up_an_empty_spool() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let session_id = "parent-session-cleanup";
    let location = forwarding_location(agent_dir.path(), session_id).expect("derives a location");
    for dir in [&location.session_root, &location.requests_dir, &location.responses_dir] {
        std::fs::create_dir_all(dir).expect("creates the spool dirs");
    }

    let services: Arc<dyn HostServices> = Arc::new(SessionIdServices(session_id.to_string()));
    process_forwarded_requests(
        agent_dir.path(),
        session_id,
        &services,
        &ExtensionConfig::default(),
        ProcessForwardedOptions::default(),
        &crate::logging::AuditTrail::detached(agent_dir.path().join("logs")),
        true,
    )
    .await;

    assert!(
        !Path::new(&location.requests_dir).exists(),
        "the default (non-preserving) bag must still clean an empty spool up"
    );
    assert!(
        !Path::new(&location.session_root).exists(),
        "the default bag cleans the session root up too"
    );
}
