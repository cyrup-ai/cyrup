//! EXT-086 — what a MISBEHAVING guest can do with an open model stream, live: a real
//! `wasm32-wasip2` component opens a stream through `models.stream` and then hangs, panics, forgets
//! the handle, or is unloaded. In every case the provider request must not survive it.
//!
//! The backend here is a stand-in whose every model stream HANGS — the worst case, a provider that
//! never answers — and counts how many streams it handed out and how many were dropped. A dropped
//! provider stream is a cancelled provider request (the host also fires the call's `signal`; that
//! half is pinned in `cyrup-ext`'s own `host::model_calls` tests). The happy path, through the real
//! session and the faux provider, is `session_svc::wasm_model_calls`.
//!
//! These need a real component in a real store, because the failure modes ARE the guest's: an epoch
//! trap, a wasm `unreachable`, a handle the guest's own code lost.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::fixture;

use cyrup_core::{CancelToken, EventStream};
use cyrup_ext::host::{HostServices, LiveExtension, ModelCall};
use cyrup_ext::{Capabilities, ExtError, ExtensionHost};
use cyrup_ext_sdk::example::model_calls::{
    COMPLETE_COMMAND, HANG_COMMAND, LEAK_COMMAND, PANIC_COMMAND, STREAM_COMMAND,
};
use cyrup_ext_sdk::example::{DEMO_ROUTER_ID, DEMO_ROUTER_PROVIDER};
use cyrup_provider::StreamEvent;
use futures::StreamExt;

/// Counts a drop.
struct Dropped(Arc<AtomicUsize>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A backend whose provider never answers.
#[derive(Default)]
struct StalledProvider {
    opened: AtomicUsize,
    dropped: Arc<AtomicUsize>,
}

impl HostServices for StalledProvider {
    fn model_stream(&self, _call: ModelCall) -> EventStream<StreamEvent> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let guard = Dropped(Arc::clone(&self.dropped));
        Box::pin(futures::stream::pending::<StreamEvent>().map(move |event| {
            let _owned = &guard;
            event
        }))
    }
}

async fn load(
    backend: &Arc<StalledProvider>,
    caps: &Capabilities,
) -> (ExtensionHost, Arc<LiveExtension>) {
    let bytes = std::fs::read(fixture::component()).expect("read the guest component");
    let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm");
    let ext = host
        .load_wasm_with_caps(
            "demo".into(),
            &bytes,
            Arc::clone(backend) as Arc<dyn HostServices>,
            caps,
        )
        .await
        .expect("load + init the demo guest");
    (host, ext)
}

/// Poll `counter` until it reaches `want` or `within` elapses.
async fn reaches(counter: &AtomicUsize, want: usize, within: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < within {
        if counter.load(Ordering::SeqCst) >= want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    counter.load(Ordering::SeqCst) >= want
}

/// A guest that opens a stream and then never returns is cut off by the epoch deadline — and its
/// stream with it. The trap poisons the instance for good, so nothing in the guest could ever close
/// the stream; the host closes it at the trap (`LiveExtension::fault`) instead of letting the
/// provider request run until the two-minute abandonment window. Red against a `fault` that does
/// not `close_all`: the request is still open when this test gives up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_that_hangs_with_a_stream_open_leaves_no_provider_request_running() {
    let backend = Arc::new(StalledProvider::default());
    let (host, _ext) = load(&backend, &Capabilities::host_granted()).await;
    let err = host
        .run_command(HANG_COMMAND, "p/m", &CancelToken::new())
        .await
        .expect_err("a guest that never returns is cut off");
    assert!(matches!(err, ExtError::EpochTimeout), "{err:?}");
    assert_eq!(backend.opened.load(Ordering::SeqCst), 1);
    assert!(
        reaches(&backend.dropped, 1, Duration::from_secs(3)).await,
        "the hung guest's provider request is still running"
    );
}

/// A guest that panics with a stream open: the same, through a wasm trap rather than the epoch.
/// Red against a `fault` that does not `close_all`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_that_panics_with_a_stream_open_leaves_no_provider_request_running() {
    let backend = Arc::new(StalledProvider::default());
    let (host, _ext) = load(&backend, &Capabilities::host_granted()).await;
    let err = host
        .run_command(PANIC_COMMAND, "p/m", &CancelToken::new())
        .await
        .expect_err("a panicking guest traps");
    assert!(matches!(err, ExtError::Trap(_)), "{err:?}");
    assert_eq!(backend.opened.load(Ordering::SeqCst), 1);
    assert!(
        reaches(&backend.dropped, 1, Duration::from_secs(3)).await,
        "the panicked guest's provider request is still running"
    );
}

/// A guest that returns normally but forgets its handle: the instance is healthy, so nothing traps;
/// the stream simply stops being polled, and the host abandons it. The window is shortened on this
/// extension's own table so the proof does not take two minutes. Red against a pump with no
/// abandonment reaper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_that_forgets_its_stream_has_it_abandoned() {
    let backend = Arc::new(StalledProvider::default());
    let (host, ext) = load(&backend, &Capabilities::host_granted()).await;
    ext.guest()
        .model_streams()
        .set_abandon_after(Duration::from_millis(300));
    host.run_command(LEAK_COMMAND, "p/m", &CancelToken::new())
        .await
        .expect("the leaking command returns normally");
    assert_eq!(backend.opened.load(Ordering::SeqCst), 1);
    assert_eq!(ext.guest().model_streams().open_count(), 1);
    assert_eq!(
        backend.dropped.load(Ordering::SeqCst),
        0,
        "dropped before it was abandoned"
    );
    assert!(
        reaches(&backend.dropped, 1, Duration::from_secs(5)).await,
        "the forgotten stream's provider request is still running"
    );
}

/// Unloading the extension — the whole host going away — closes every stream it left open, well
/// inside the abandonment window rather than at its end. Red against an `ExtensionHost` without the
/// `Drop` that closes them: a loaded guest is held by its own registered tools and is not freed with
/// its host, so the stream outlives it (found by this test).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unloading_the_guest_closes_every_stream_it_left_open() {
    let backend = Arc::new(StalledProvider::default());
    let (host, ext) = load(&backend, &Capabilities::host_granted()).await;
    host.run_command(LEAK_COMMAND, "p/m", &CancelToken::new())
        .await
        .expect("the leaking command returns normally");
    assert_eq!(backend.opened.load(Ordering::SeqCst), 1);
    drop(ext);
    drop(host);
    assert!(
        reaches(&backend.dropped, 1, Duration::from_secs(3)).await,
        "an unloaded guest's provider request is still running"
    );
}

/// `stream-simple` on a virtual model whose router is the CALLING guest is refused up front: the
/// instance is suspended in the very call that would have to route it. Red against an import that
/// does not refuse — the stream opens and the guest's drain loop holds its own instance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_simple_on_the_guests_own_virtual_model_is_refused_not_deadlocked() {
    let backend = Arc::new(StalledProvider::default());
    let (host, ext) = load(&backend, &Capabilities::host_granted()).await;
    // Bounded: without the refusal the guest's drain loop would wait on a stream that its own
    // instance would have to route, and this test would hang instead of failing.
    let reply = tokio::time::timeout(
        Duration::from_secs(10),
        host.run_command(
            STREAM_COMMAND,
            &format!("{DEMO_ROUTER_PROVIDER}/{DEMO_ROUTER_ID} hi"),
            &CancelToken::new(),
        ),
    )
    .await
    .expect("the guest is still waiting on a call it can never complete")
    .expect("the command reports the refusal");
    let reply = reply.unwrap_or_default();
    assert!(
        reply.starts_with(
            "model denied: virtual model `router/demo-auto` is routed by this extension"
        ),
        "{reply}"
    );
    assert_eq!(backend.opened.load(Ordering::SeqCst), 0);
    assert!(ext.guest().notifications().iter().any(|n| n == &reply));
}

/// Without `capabilities.modelCalls` a guest gets the host's refusal and no call reaches the
/// backend. Red against an import that skips the gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_without_the_capability_is_refused_before_the_backend_is_asked() {
    let backend = Arc::new(StalledProvider::default());
    let caps = Capabilities {
        model_calls: false,
        ..Capabilities::host_granted()
    };
    let (host, _ext) = load(&backend, &caps).await;
    for command in [COMPLETE_COMMAND, STREAM_COMMAND] {
        let reply = host
            .run_command(command, "p/m hi", &CancelToken::new())
            .await
            .expect("the command reports the refusal")
            .unwrap_or_default();
        assert_eq!(
            reply,
            format!("model denied: {}", cyrup_ext::DENIED_MODEL_CALLS),
            "/{command}"
        );
    }
    assert_eq!(backend.opened.load(Ordering::SeqCst), 0);
}
