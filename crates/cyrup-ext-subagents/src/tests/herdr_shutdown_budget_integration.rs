//! **C5 — a hung herdr must not eat the `SessionShutdown` handler's dispatch budget.**
//!
//! `HostEvent::SessionShutdown`'s arm (`extension/host/native_impl.rs`) is the whole of this
//! extension's teardown: the herdr pane release, the supervisor channel, the watchdog, the wait
//! subscriptions, the scheduled-run manager, `teardown_session`, and both widget slots. It runs
//! under the dispatcher's per-handler invocation budget
//! ([`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`], 5 s), and the dispatcher enforces that budget
//! by **dropping the handler future** (`Dispatcher::invoke_contained`'s `tokio::time::timeout`).
//!
//! The arm used to open with `crate::herdr::shutdown().await`, which is bounded by
//! `crate::herdr::reporter::RELEASE_TIMEOUT` — also 5 s. A herdr that accepted the connection and
//! then answered nothing therefore consumed the ENTIRE budget on the first statement, and every
//! disposal below it was dropped mid-flight. Nothing logged and nothing failed: the statements
//! simply never ran, and the next session inherited a live supervisor poller, a live watchdog, a
//! live scheduled-run tick and a stale runtime-agent registry.
//!
//! This file drives the real thing:
//!
//! * a real `UnixListener` that accepts and then answers nothing — the only way to make the
//!   release actually hang with no herdr installed (there is none in this container);
//! * the production arm, `crate::herdr::arm_in`, parking a real bridge in the process-global slot,
//!   exactly as `SessionStart` does;
//! * a real [`cyrup_ext::Dispatcher`] at its real budget, holding the extension through the real
//!   [`cyrup_ext::native::NativeHandle`], so the truncation is the dispatcher's own and not a
//!   timeout this test invented.
//!
//! The observable is the LAST in-process thing the arm does before the widget slots:
//! `teardown_session()` clearing the runtime-agent registry. If the herdr release starves the
//! budget, that registry still holds its agent when the dispatch returns.
//!
//! The global bridge slot is process-wide, which is safe here because `nextest` runs each test in
//! its own process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;
use cyrup_ext::event::{EventKind, Subscriptions};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension, NativeHandle};
use cyrup_ext::{Dispatcher, HostEvent};

use crate::discovery::runtime_registry::RuntimeAgentDefinition;
use crate::extension::SubagentsExtension;

/// A fake herdr that accepts connections and never writes a byte.
///
/// This is the failure mode `reporter::RELEASE_TIMEOUT` exists for and the one the socket's own
/// per-call deadline (`cyrup_herdr::DEFAULT_TIMEOUT`, 15 s) cannot shorten: the connection is
/// established, so nothing errors — the release simply waits.
struct BlackHoleHerdr {
    path: std::path::PathBuf,
    accept: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for BlackHoleHerdr {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl BlackHoleHerdr {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bind the black-hole socket");
        let accept = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let Ok((stream, _addr)) = listener.accept().await else {
                    return;
                };
                // Held, never read, never answered.
                held.push(stream);
            }
        });
        Self {
            path,
            accept,
            _dir: dir,
        }
    }

    /// The environment herdr injects into a pane it owns, pointed at this socket.
    fn pane_env(&self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned()),
            (
                "HERDR_SOCKET_PATH".to_owned(),
                self.path.display().to_string(),
            ),
            ("HERDR_TAB_ID".to_owned(), "w1:t1".to_owned()),
        ])
    }
}

/// **The whole `SessionShutdown` teardown survives a herdr that stopped answering.**
///
/// *Gutted by*: putting `crate::herdr::shutdown().await` back as the arm's first statement (the
/// dispatch then burns the full budget on it and the registry below is never cleared); raising
/// `HERDR_RELEASE_JOIN_BUDGET` to the dispatch budget or beyond (same outcome — and the `const`
/// assertion beside it refuses that at compile time, which is the point of it being a `const`);
/// deleting the join's `tokio::time::timeout` so the handler waits out the full
/// `RELEASE_TIMEOUT` after the teardown and is dropped at the deadline anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_hung_herdr_cannot_starve_the_rest_of_the_session_shutdown() {
    let herdr = BlackHoleHerdr::start();
    let base = tempfile::tempdir().expect("tempdir");
    let cwd = base.path().join("project");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");

    // `SessionStart`'s arm, with the environment as a parameter (`arm_in`'s own doc).
    let armed = crate::herdr::arm_in(&herdr.pane_env(), true, None);
    assert!(
        armed.is_some(),
        "a pane env with a UI arms the bridge; without one this test proves nothing"
    );
    assert!(
        crate::herdr::bridge().is_some(),
        "armed into the global slot"
    );

    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        Default::default(),
        cwd.clone(),
    ));
    // The observable: the registry `teardown_session()` clears, which the arm reaches only AFTER
    // the herdr release.
    extension
        .register_agent(
            "shutdown-scout",
            &RuntimeAgentDefinition::new("Shutdown scout", "Scout."),
        )
        .expect("registers");
    assert_eq!(extension.executor().runtime_agents().list().len(), 1);

    // The real dispatcher, at its real budget, holding the extension the real way.
    let dispatcher = Dispatcher::new();
    let ctx = HostCtx::event(ExtMode::Json, false, cwd.clone());
    dispatcher
        .add(Arc::new(NativeHandle::new(
            Arc::clone(&extension) as Arc<dyn NativeExtension>,
            Subscriptions::empty().with(EventKind::SessionShutdown),
            ctx,
        )))
        .expect("the dispatcher takes the extension");

    let started = Instant::now();
    dispatcher
        .dispatch_notify(
            &HostEvent::SessionShutdown {
                reason: "quit".to_owned(),
                target_session_file: None,
            },
            &CancelToken::new(),
        )
        .await;
    let elapsed = started.elapsed();

    assert!(
        extension.executor().runtime_agents().list().is_empty(),
        "the herdr release ate the dispatch budget and the rest of the teardown was dropped: \
         teardown_session() never ran (elapsed {elapsed:?})"
    );
    assert!(
        elapsed < cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET,
        "the handler must RETURN inside the dispatch budget rather than be dropped at it; \
         took {elapsed:?}"
    );
    // The bridge is out of the global slot even though its release never completed — `shutdown()`
    // TAKES the slot before it awaits anything, so a rebuilt session cannot report through it.
    assert!(
        crate::herdr::bridge().is_none(),
        "shutdown must clear the process-global slot"
    );
    // And the release really was still in flight: a socket that never answers cannot have
    // completed inside the join budget.
    assert!(
        elapsed >= Duration::from_secs(1),
        "the join budget was not actually exercised; the release must still have been pending"
    );
}
