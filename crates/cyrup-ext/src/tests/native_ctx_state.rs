//! EXT-005, native half — a native built-in's `HostCtx::rich()` must report the LIVE session state.
//!
//! `HostCtxRich` (native.rs) has always declared `is_idle` / `is_project_trusted` /
//! `context_usage` / `system_prompt` (Pi `ExtensionContext`, `coding-agent/src/core/extensions/
//! types.ts:329-346`), but `HostCtx::with_rich` had zero production callers: both
//! `load_native_inner` and `execute_native_command` built the ctx with `HostCtxRich::default()`.
//! So a native built-in reading `ctx.is_idle()` or `ctx.is_project_trusted()` did not get
//! "unavailable" — it got a confident `false`, i.e. a WRONG answer.
//!
//! These tests assert the values a handler actually OBSERVES, from both tiers.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{CancelToken, ExtensionId};
use crate::{
    CannedResponses, EventKind, ExtError, ExtensionHost, HookOutcome, HostConfig, HostCtx,
    HostEvent, InitApi, NativeExtension, RecordingServices,
};
use std::sync::{Arc, Mutex};

/// A native built-in that records exactly what its ctx told it, from both an event handler and a
/// command handler.
#[derive(Default)]
struct Prober {
    seen: Mutex<Vec<String>>,
}

impl Prober {
    fn snapshot(ctx: &HostCtx) -> String {
        format!(
            "idle={} trusted={} prompt={:?} model={:?} usage={}",
            ctx.is_idle(),
            ctx.is_project_trusted(),
            ctx.system_prompt(),
            ctx.model(),
            ctx.context_usage().is_some(),
        )
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl NativeExtension for Prober {
    fn id(&self) -> ExtensionId {
        "prober".into()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::AgentStart]);
        api.register_command("probe", Default::default());
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        if let Ok(mut g) = self.seen.lock() {
            g.push(format!("event:{}", Self::snapshot(ctx)));
        }
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Ok(Some(Self::snapshot(ctx)))
    }
}

fn services() -> Arc<RecordingServices> {
    Arc::new(RecordingServices::new(CannedResponses {
        // Every value below is the OPPOSITE of `HostCtxRich::default()`, so the assertions cannot
        // be satisfied by the old defaulted ctx.
        is_idle: true,
        is_project_trusted: true,
        system_prompt: Some("LIVE-SYSTEM-PROMPT".into()),
        current_model: Some("live-model".into()),
        ..Default::default()
    }))
}

#[tokio::test]
async fn a_native_event_handler_observes_the_live_ctx_state() {
    let host = ExtensionHost::with_wasm(HostConfig::default()).expect("host");
    let prober = Arc::new(Prober::default());
    host.load_native_with_services(prober.clone(), services()).await.expect("load native");

    let cancel = CancelToken::new();
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;

    let seen = prober.seen();
    assert_eq!(seen.len(), 1, "the handler ran once: {seen:?}");
    assert_eq!(
        seen[0],
        "event:idle=true trusted=true prompt=Some(\"LIVE-SYSTEM-PROMPT\") \
         model=Some(\"live-model\") usage=true",
        "a native EVENT handler must read the live backend, not HostCtxRich::default()"
    );
}

#[tokio::test]
async fn a_native_command_handler_observes_the_live_ctx_state() {
    let host = ExtensionHost::with_wasm(HostConfig::default()).expect("host");
    let prober = Arc::new(Prober::default());
    host.load_native_with_services(prober, services()).await.expect("load native");

    let cancel = CancelToken::new();
    let out = host
        .execute_native_command("probe", "", &cancel)
        .await
        .expect("routed")
        .expect("owned by the native")
        .expect("handler ok");
    assert_eq!(
        out.as_deref(),
        Some(
            "idle=true trusted=true prompt=Some(\"LIVE-SYSTEM-PROMPT\") \
             model=Some(\"live-model\") usage=true"
        ),
        "a native COMMAND handler must read the live backend"
    );
}

/// Without an injected backend nothing changes: the ctx keeps its defaults rather than inventing
/// values (the host that grants no capabilities must not claim a system prompt or a model).
#[tokio::test]
async fn a_native_handler_without_a_backend_keeps_the_defaults() {
    let host = ExtensionHost::with_wasm(HostConfig::default()).expect("host");
    let prober = Arc::new(Prober::default());
    host.load_native(prober.clone()).await.expect("load native");

    let cancel = CancelToken::new();
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;

    let seen = prober.seen();
    assert_eq!(seen[0], "event:idle=false trusted=false prompt=None model=None usage=false");
}
