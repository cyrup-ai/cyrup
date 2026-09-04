//! Teardown must mark the session's extension instances stale — on the ordered `dispose` path AND
//! when the session is dropped without ever reaching it.
//!
//! Upstream, `AgentSession.dispose()` ends in
//! `this._extensionRunner.invalidate("This extension ctx is stale after session replacement or
//! reload…")` (`core/agent-session.ts:850-852` @v0.84.2), reached from `teardownCurrent`'s
//! `this.beforeSessionInvalidate?.(); this.session.dispose();`
//! (`core/agent-session-runtime.ts:176-177`). `invalidate` sets a one-shot `staleMessage` and runs
//! every tracked event-bus unsubscribe (`core/extensions/loader.ts:206-215`); `assertActive` then
//! refuses any later call from that instance (`:180-184`).
//!
//! # What these tests see PRE-FIX
//!
//! They assert on `ExtensionHost::live_invalidations()`, which counts the call itself, because a
//! stale latch is otherwise only observable THROUGH a live wasm guest and these sessions have
//! none — precisely the blind spot that let the seam sit uncalled.
//!
//! * `dispose_invalidates_the_extension_host` — GREEN already. The ordered call landed in
//!   `37c2833`; this pins it so it cannot rot back out, and pins the ORDER against the
//!   `before_session_invalidate` hook. It is not offered as proof of this change.
//! * `dropping_a_session_without_dispose_still_invalidates` — RED pre-fix. Without
//!   `impl Drop for AgentSession` the count stays `0` after the session is dropped, and the
//!   assertion fails on `0 != 1`.
//! * `invalidation_is_scoped_to_the_disposed_session` — RED pre-fix for the same reason (its second
//!   host is only invalidated by the drop).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::{SessionBuilder, SessionConfig};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

async fn build(fx: &Fixture) -> crate::AgentSession {
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    SessionBuilder::new(faux, base_config(fx))
        .build()
        .await
        .unwrap()
}

/// The ordered path (already green — see the module doc).
#[tokio::test]
async fn dispose_invalidates_the_extension_host() {
    let fx = fixture();
    let session = build(&fx).await;
    let host = session.ext_host().clone();

    assert_eq!(
        host.live_invalidations(),
        0,
        "a fresh session has invalidated nothing"
    );
    session.dispose("quit").await;
    assert!(
        host.live_invalidations() >= 1,
        "dispose must reach `invalidate_live` — pi's `_extensionRunner.invalidate` at the tail of \
         `AgentSession.dispose()`"
    );
}

/// PRE-FIX: fails on `0 != 1`. Nothing invalidated, because every invalidation sat after three
/// `.await`s in `dispose_with` that this session never runs.
///
/// This is the shape the sweep was told to look for: a future can be dropped at any `.await`, so
/// cleanup that lives only on the success path leaks forever. Here the leak is a set of extension
/// instances that stay ACTIVE — `assertActive` keeps passing, so a call still in flight on one of
/// them goes on acting for a session that no longer exists.
#[tokio::test]
async fn dropping_a_session_without_dispose_still_invalidates() {
    let fx = fixture();
    let session = build(&fx).await;
    let host = session.ext_host().clone();

    assert_eq!(host.live_invalidations(), 0);
    // No `dispose`. This models both the embedder that drops a `cyrup_sdk::Session` without
    // `close()` and the teardown future that is cut at one of its awaits.
    drop(session);
    assert_eq!(
        host.live_invalidations(),
        1,
        "dropping a session must still mark its extension instances stale"
    );
}

/// The idempotence and scoping the `Drop` relies on: disposing THEN dropping must not double-count
/// into a second host, and one session's teardown must never touch another session's instances —
/// which is what makes invalidate-on-drop safe across `/new`, `/fork` and `/resume`, where the
/// outgoing `Arc<AgentSession>` can outlive the installation of its replacement.
///
/// PRE-FIX: fails — `b`'s count is `0` after the drop.
#[tokio::test]
async fn invalidation_is_scoped_to_the_disposed_session() {
    let fx = fixture();
    let a = build(&fx).await;
    let b = build(&fx).await;
    let host_a = a.ext_host().clone();
    let host_b = b.ext_host().clone();

    assert!(
        !Arc::ptr_eq(&host_a, &host_b),
        "each session builds its own ExtensionHost — the whole reason invalidating on teardown \
         cannot disable the session that replaces it (pi gets a fresh `createExtensionRuntime()` \
         per `resourceLoader.reload()` for the same reason)"
    );

    a.dispose("new").await;
    assert!(host_a.live_invalidations() >= 1);
    assert_eq!(
        host_b.live_invalidations(),
        0,
        "b's instances are untouched by a's teardown"
    );

    // The outgoing session is still alive here, exactly as it is between `dispose_with` and the
    // last `Arc` clone being released on a replacement path.
    drop(a);
    assert_eq!(
        host_b.live_invalidations(),
        0,
        "still untouched after a is dropped"
    );

    drop(b);
    assert_eq!(
        host_b.live_invalidations(),
        1,
        "b invalidates on its own drop, and only then"
    );
}
