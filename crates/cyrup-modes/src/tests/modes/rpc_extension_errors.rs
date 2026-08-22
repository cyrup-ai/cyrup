//! G2 — a CONTAINED extension fault is still observable: it must surface to the RPC client as an
//! `extension_error` line on stdout rather than being swallowed into a log the client cannot see.

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, fixture, parse_lines, type_of};
use crate::run_rpc;
use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};

// ----------------------------------------------------------------------------------------------
// G2 (CRITICAL) — a contained extension fault must surface to the RPC client as an `extension_error`
// event on stdout (Pi `onError: (err) => output({type:"extension_error", extensionPath, event,
// error})`, rpc-mode.ts:347-349). Pre-fix, `Dispatcher::add_error_listener` is never called by any
// mode, so `report()` fans out to zero listeners and the fault is swallowed into a `tracing::warn!`.
// ----------------------------------------------------------------------------------------------

/// Loads a native extension that panics on the `input` event, drives a prompt through the RPC loop,
/// and asserts the client sees an `extension_error` line carrying `{event:"input", extensionPath,
/// error}`. The panic is contained (the run still proceeds), so the ONLY observable of the fault is
/// the surfaced event — which pre-fix never reaches the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_contained_extension_fault_surfaces_as_extension_error_event() {
    use cyrup_core::ExtensionId;
    use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};

    struct FaultyInputExt {
        id: ExtensionId,
    }
    #[async_trait::async_trait]
    impl NativeExtension for FaultyInputExt {
        fn id(&self) -> ExtensionId {
            self.id.clone()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe(&[EventKind::Input]);
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            panic!("boom in the input handler");
        }
    }

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let runtime = build_runtime(&fx, faux).await;

    // Load the faulting extension into the very session the RPC loop drives.
    let session = runtime.session().await;
    session
        .services()
        .ext_host
        .load_native(Arc::new(FaultyInputExt { id: "faulty".into() }))
        .await
        .expect("load faulty ext");

    let input = concat!(r#"{"type":"prompt","id":"p1","message":"hi"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let err_ev = lines.iter().find(|l| type_of(l) == "extension_error").unwrap_or_else(|| {
        panic!(
            "a contained extension fault must surface as an `extension_error` event (Pi \
             rpc-mode.ts:347-349); none found in:\n{lines:#?}"
        )
    });
    assert_eq!(err_ev["event"], "input", "carries the Pi ExtensionError.event name: {err_ev}");
    assert!(err_ev["extensionPath"].is_string(), "carries extensionPath: {err_ev}");
    assert!(err_ev["error"].is_string(), "carries the error message: {err_ev}");
}
