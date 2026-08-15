//! Test-only serialization for the PROCESS-GLOBAL `httpProxy` setting (PROV-047).
//!
//! [`crate::stream::sse::configure_http_proxy`] is the stand-in for pi's `applyHttpProxySettings`
//! writing `process.env.HTTP_PROXY` (`coding-agent/src/core/http-dispatcher.ts:43-48` @v0.83.0), and
//! like that env write it is global to the process. Every test in this crate's single test binary
//! therefore shares it, and this crate has many tests that talk to loopback mock servers — a leaked
//! setting reroutes their requests into a proxy that is no longer listening.
//!
//! Two hazards, both of the JS→Rust class this port keeps hitting:
//!
//! 1. Two tests writing the setting concurrently is a race with no lock in JS's model. [`guard`]
//!    serializes them.
//! 2. Clearing the setting on the SUCCESS path leaks it forever the moment an assertion panics or
//!    a future is dropped at an `.await`. [`ClearOnDrop`] does the clearing in `Drop`, which runs on
//!    the panic unwind too.

/// Serializes every test that writes the process-global `httpProxy` setting. Weakens no assertion:
/// it only prevents two tests from observing each other's global.
static PROXY_SETTING_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Take the serialization guard. Hold the returned value for the life of the test.
pub(crate) async fn guard() -> tokio::sync::MutexGuard<'static, ()> {
    PROXY_SETTING_GUARD.lock().await
}

/// Clears the process-global `httpProxy` in `Drop` — never on the success path, so a panicking
/// assertion cannot leak the setting into whichever test takes [`guard`] next.
pub(crate) struct ClearOnDrop;

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        crate::stream::sse::configure_http_proxy(None);
    }
}
