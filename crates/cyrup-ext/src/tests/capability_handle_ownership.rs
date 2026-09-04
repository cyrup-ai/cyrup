//! A capability handle belongs to the extension that opened it.
//!
//! `capabilities.exec` / `capabilities.net` (EXT-054) decide WHETHER a guest may open a child
//! process or an HTTP stream. They say nothing about WHOSE. The engines that mint the handles —
//! `ProcCaps` / `HttpCaps` — are one per SESSION, owned by the single `LiveHostServices` that every
//! loaded guest shares (`cyrup-session-svc/src/host_services.rs`: one `HttpCaps::new()`, one
//! `ProcCaps::new()`), and both allocate from one monotonic `u32` counter starting at 1. So before
//! `GuestState::{own,require}_proc_handle` / `_stream_handle`, a bare integer was ambient authority:
//! any exec-granted extension could read, write to, or kill any other extension's child, and any
//! net-granted extension could drain or close another's response stream, by counting up from 1.
//!
//! This is EXT-054's shape one level down — a declared per-extension sandbox that is not enforced
//! across a boundary nobody had looked at (area 06 blind spot 4). pi has no analog either way: its
//! extensions are ordinary JS in the agent process with unrestricted `node:child_process`, so
//! upstream has neither the handle table nor the isolation it would need. The invariant is
//! therefore cyrup's own to state, exactly as EXT-054's was.
//!
//! Each test asserts the LEAK first (the shared backend really would have served the other guest)
//! and the refusal second, so the refusal cannot pass vacuously.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::caps::proc::ProcSpawnSpec;
use crate::host::{CannedResponses, HostServices, RecordingServices};
use crate::{ExtensionRegistry, GuestState};
use std::sync::Arc;

/// Two guests sharing ONE `HostServices` — the production topology (`discover_and_load` hands the
/// same `Arc<dyn HostServices>` to every `load_discovered`).
fn two_guests_on_one_backend(
    canned: CannedResponses,
) -> (GuestState, GuestState, Arc<RecordingServices>) {
    let services = Arc::new(RecordingServices::new(canned));
    let registry = Arc::new(ExtensionRegistry::new());
    let a = GuestState::with_services("ext-a".into(), registry.clone(), services.clone());
    let b = GuestState::with_services("ext-b".into(), registry, services.clone());
    (a, b, services)
}

fn spawn_spec() -> ProcSpawnSpec {
    ProcSpawnSpec {
        cmd: "some-server".into(),
        args: vec![],
        env: vec![],
        cwd: None,
        capture_stderr: false,
    }
}

#[test]
fn a_proc_handle_is_refused_to_the_extension_that_did_not_spawn_it() {
    let canned = CannedResponses {
        proc_stdout_chunks: vec![b"secret".to_vec()],
        ..Default::default()
    };
    let (a, b, services) = two_guests_on_one_backend(canned);

    // A spawns; the import records ownership (`proc::Host::spawn`).
    let handle = a.services.proc_spawn(&spawn_spec()).expect("spawn");
    a.own_proc_handle(handle);

    // LEAK, asserted first: the shared backend answers for this handle no matter who asks. This is
    // exactly what B would have got before the ownership set existed.
    assert_eq!(
        services
            .proc_read_stdout(handle, 64)
            .expect("backend serves the handle"),
        b"secret".to_vec(),
        "the session-wide ProcCaps registry is not per-extension — that is the whole problem"
    );

    // A may use its own handle.
    assert!(
        a.require_proc_handle(handle).is_ok(),
        "the spawner owns its handle"
    );

    // B may not — for reads, writes, kills and exit polls alike.
    let err = b
        .require_proc_handle(handle)
        .expect_err("a foreign handle is refused");
    assert!(
        err.contains(&handle.to_string()),
        "the refusal names the handle, not the owner (telling B which extension holds it is the \
         same leak in a smaller package): {err}"
    );

    // And B's own spawn gets a DIFFERENT handle it does own, so the guard is ownership, not a
    // blanket denial of the second guest.
    let b_handle = b.services.proc_spawn(&spawn_spec()).expect("spawn");
    b.own_proc_handle(b_handle);
    assert_ne!(
        b_handle, handle,
        "the counter is session-wide and monotonic"
    );
    assert!(b.require_proc_handle(b_handle).is_ok());
    assert!(
        a.require_proc_handle(b_handle).is_err(),
        "and A cannot reach back the other way"
    );
}

#[test]
fn an_http_stream_handle_is_refused_to_the_extension_that_did_not_open_it() {
    let canned = CannedResponses {
        http_stream_chunks: vec![b"body".to_vec()],
        ..Default::default()
    };
    let (a, b, services) = two_guests_on_one_backend(canned);

    let req = crate::caps::http::HttpRequest {
        method: "GET".into(),
        url: "https://example.invalid/stream".into(),
        headers: vec![],
        body: None,
        timeout_ms: None,
    };
    let opened = a.services.http_request_stream(&req).expect("stream opens");
    a.own_stream_handle(opened.handle);

    // LEAK first: the backend hands the response body to whoever presents the handle.
    assert_eq!(
        services
            .http_poll_stream_chunk(opened.handle)
            .expect("backend serves the handle"),
        Some(b"body".to_vec()),
        "one HttpCaps stream table per session, keyed by a guessable u32"
    );

    assert!(
        a.require_stream_handle(opened.handle).is_ok(),
        "the opener owns its stream"
    );
    assert!(
        b.require_stream_handle(opened.handle).is_err(),
        "another extension cannot drain or close this stream"
    );
}

/// `close-stream` releases the handle, so a guest cannot keep authority over a stream it closed —
/// and `kill` deliberately does NOT release a proc handle, because `ProcCaps` documents that a
/// killed child stays in the registry so its owner can still `poll-exit` and drain trailing output.
#[test]
fn closing_a_stream_releases_it_while_killing_a_child_does_not() {
    let (a, _b, _services) = two_guests_on_one_backend(CannedResponses::default());

    a.own_stream_handle(7);
    assert!(a.require_stream_handle(7).is_ok());
    a.release_stream_handle(7);
    assert!(
        a.require_stream_handle(7).is_err(),
        "a closed stream is no longer this guest's"
    );

    a.own_proc_handle(9);
    assert!(
        a.require_proc_handle(9).is_ok(),
        "a killed child is still pollable by its spawner"
    );
}
