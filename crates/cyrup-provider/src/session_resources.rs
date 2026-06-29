//! Session-resource cleanup registry (1:1 port of Pi `session-resources.ts`).
//!
//! A process-global set of cleanup callbacks that are invoked — best-effort — when a session ends.
//! [`register_session_resource_cleanup`] adds a callback and returns an unregister handle;
//! [`cleanup_session_resources`] runs every registered callback, collecting any failures and
//! surfacing them as a single aggregate error (Pi throws an `AggregateError`,
//! session-resources.ts:13-23). Registration order is preserved (Pi backs the registry with a
//! `Set`).

use std::sync::{Arc, Mutex, OnceLock};

use crate::error::BoxErr;

/// A session-resource cleanup callback (Pi `SessionResourceCleanup = (sessionId?: string) => void`,
/// session-resources.ts:1). Rust has no exceptions, so a fallible cleanup returns `Err` instead of
/// throwing; [`cleanup_session_resources`] aggregates those just as Pi aggregates thrown errors.
pub type SessionResourceCleanup = Arc<dyn Fn(Option<&str>) -> Result<(), BoxErr> + Send + Sync>;

/// The aggregate failure surfaced when one or more cleanups error (Pi's `AggregateError` with the
/// message `"Failed to cleanup session resources"`, session-resources.ts:21).
#[derive(Debug, thiserror::Error)]
#[error("Failed to cleanup session resources ({} error(s))", errors.len())]
pub struct SessionCleanupErrors {
    /// The individual cleanup failures, in registration order.
    pub errors: Vec<BoxErr>,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    cleanups: Vec<(u64, SessionResourceCleanup)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Lock the registry, recovering the inner guard if a previous holder panicked (never panics).
fn lock_registry() -> std::sync::MutexGuard<'static, Registry> {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Register a cleanup callback; returns an unregister handle (Pi `registerSessionResourceCleanup`,
/// session-resources.ts:5-10). The handle is idempotent — calling it more than once is harmless.
pub fn register_session_resource_cleanup(
    cleanup: SessionResourceCleanup,
) -> impl Fn() + Send + Sync {
    let id = {
        let mut reg = lock_registry();
        let id = reg.next_id;
        reg.next_id += 1;
        reg.cleanups.push((id, cleanup));
        id
    };
    move || {
        let mut reg = lock_registry();
        reg.cleanups.retain(|(existing, _)| *existing != id);
    }
}

/// Run every registered cleanup callback for `session_id`, collecting failures (Pi
/// `cleanupSessionResources`, session-resources.ts:12-23). A cleanup that errors does not prevent
/// the rest from running; if any failed, all failures are returned as a [`SessionCleanupErrors`].
pub fn cleanup_session_resources(session_id: Option<&str>) -> Result<(), SessionCleanupErrors> {
    // Snapshot the callbacks so a cleanup that (un)registers does not deadlock or mutate the list
    // mid-iteration (Pi iterates a `Set` directly under JS's single-threaded model).
    let cleanups: Vec<SessionResourceCleanup> = {
        let reg = lock_registry();
        reg.cleanups
            .iter()
            .map(|(_, cleanup)| cleanup.clone())
            .collect()
    };
    let mut errors: Vec<BoxErr> = Vec::new();
    for cleanup in cleanups {
        if let Err(error) = cleanup(session_id) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(SessionCleanupErrors { errors })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The registry is process-global, so these tests must not run concurrently and must each
    /// leave the registry empty. This guard serializes them.
    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn runs_in_order_and_unregister_removes() {
        let _serial = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let o1 = order.clone();
        let unregister1 = register_session_resource_cleanup(Arc::new(move |sid| {
            o1.lock().unwrap().push(format!("a:{}", sid.unwrap_or("-")));
            Ok(())
        }));
        let o2 = order.clone();
        let unregister2 = register_session_resource_cleanup(Arc::new(move |_sid| {
            o2.lock().unwrap().push("b".to_string());
            Ok(())
        }));

        cleanup_session_resources(Some("s1")).expect("no errors");
        assert_eq!(
            *order.lock().unwrap(),
            vec!["a:s1".to_string(), "b".to_string()]
        );

        // Unregister the second callback; only the first runs next time.
        unregister2();
        order.lock().unwrap().clear();
        cleanup_session_resources(None).expect("no errors");
        assert_eq!(*order.lock().unwrap(), vec!["a:-".to_string()]);

        // Calling an unregister handle twice is harmless.
        unregister2();
        unregister1();
    }

    #[test]
    fn collects_all_errors_and_runs_every_cleanup() {
        let _serial = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let ran = Arc::new(AtomicUsize::new(0));

        let r1 = ran.clone();
        let unregister1 = register_session_resource_cleanup(Arc::new(move |_| {
            r1.fetch_add(1, Ordering::SeqCst);
            Err("boom-1".into())
        }));
        let r2 = ran.clone();
        let unregister2 = register_session_resource_cleanup(Arc::new(move |_| {
            r2.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        let r3 = ran.clone();
        let unregister3 = register_session_resource_cleanup(Arc::new(move |_| {
            r3.fetch_add(1, Ordering::SeqCst);
            Err("boom-3".into())
        }));

        let err = cleanup_session_resources(None).expect_err("two cleanups failed");
        assert_eq!(
            ran.load(Ordering::SeqCst),
            3,
            "every cleanup runs despite failures"
        );
        assert_eq!(err.errors.len(), 2);
        assert!(
            err.to_string()
                .contains("Failed to cleanup session resources")
        );

        // Leave the registry empty for any other test.
        unregister1();
        unregister2();
        unregister3();
    }
}
