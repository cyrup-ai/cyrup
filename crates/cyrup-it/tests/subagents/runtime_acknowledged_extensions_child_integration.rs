//! SUBA-063, the CHILD half: a cooperating extension inside a subagent child acknowledges itself on
//! the inter-extension bus, and the child's prompt runtime writes the validated capture to the file
//! the parent named — pi `registerRuntimeExtensionAcknowledgements`
//! (`runs/shared/subagent-prompt-runtime.ts:116-140` @v0.64.0).
//!
//! Everything here is the production child path: the runtime is built by
//! `prompt_runtime_from_env` from the same variable the parent writes into every child's
//! environment (`CYRUP_SUBAGENT_RUNTIME_ACKNOWLEDGED_EXTENSIONS`), it runs inside a real session
//! (the harness), and the acknowledgement travels over the session's real `SharedBus` — emitted by
//! a native through `HostServices::emit_event`, fanned out by the dispatcher's bus drain, received
//! by the runtime's `on_bus_event`, and flushed at the session's real `agent_end`. The parent half
//! (reading this file back onto the result) is proved across the process boundary in
//! `exec_run_sync_integration.rs` and `background_runner_main_integration.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_core::ExtensionId;
use cyrup_ext::event::{EventKind, HostEvent};
use cyrup_ext::host::HostServices;
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome};
use cyrup_ext_subagents::exec::runtime_acknowledged_extensions::{
    RUNTIME_EXTENSION_ACK_EVENT, RUNTIME_EXTENSION_ACK_PATH_ENV,
};
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;

/// A third-party extension that cooperates with the protocol: at `agent_start` it emits
/// `subagent:acknowledge-extension` with its own id — and, being sloppy, one id the protocol must
/// refuse.
#[derive(Default)]
struct CooperatingExtension {
    services: Mutex<Option<Arc<dyn HostServices>>>,
}

#[async_trait::async_trait]
impl NativeExtension for CooperatingExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("it-cooperating-extension")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentStart]);
        Ok(())
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::AgentStart)
            && let Some(services) = self.services.lock().unwrap().clone()
        {
            services.emit_event(
                RUNTIME_EXTENSION_ACK_EVENT,
                &serde_json::json!({ "id": "acme.cooperating@1.0" }),
            );
            services.emit_event(
                RUNTIME_EXTENSION_ACK_EVENT,
                &serde_json::json!({ "id": "../not/an/id" }),
            );
            services.emit_event(
                RUNTIME_EXTENSION_ACK_EVENT,
                &serde_json::json!({ "id": "acme.cooperating@1.0" }),
            );
        }
        HookOutcome::Noop
    }
}

/// The capture lands at the parent-named path, validated and de-duplicated, with mode `0600`.
///
/// Gutted by, each alone: dropping `api.subscribe_bus(RUNTIME_EXTENSION_ACK_EVENT)` from the
/// runtime's `init` (the runtime never hears the acknowledgement, so finalize REMOVES the file);
/// dropping the `finalize()` call from `on_event` (nothing is ever written).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cooperating_child_extensions_acknowledgement_is_written_where_the_parent_asked() {
    let scratch = tempfile::tempdir().unwrap();
    let capture = scratch.path().join("runtime-acknowledged-extensions.json");
    let raw = capture.display().to_string();
    let runtime = Arc::new(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_from_env(&move |key: &str| {
            (key == RUNTIME_EXTENSION_ACK_PATH_ENV).then(|| raw.clone())
        })
        .expect("builds")
        .expect("a child asked for acknowledgements has a prompt runtime"),
    );
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![
            runtime as Arc<dyn NativeExtension>,
            Arc::new(CooperatingExtension::default()) as Arc<dyn NativeExtension>,
        ],
        responses: vec![FauxResponse::text("done")],
        ..HarnessOptions::default()
    })
    .await
    .expect("a real session with the child runtime and a cooperating extension");

    harness.run("work").await.expect("the turn completes");

    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&capture).unwrap_or_else(|e| {
            panic!(
                "the child runtime must write its capture at agent_end ({}): {e}",
                capture.display()
            )
        }))
        .expect("the capture is JSON");
    assert_eq!(
        written,
        serde_json::json!({
            "version": 1,
            "source": "child-runtime",
            "ids": ["acme.cooperating@1.0"],
            "omitted": 0,
        })
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&capture).unwrap().permissions().mode() & 0o777,
            0o600,
            "the capture is owner-only, as pi's `{{ mode: 0o600 }}`"
        );
    }
}
