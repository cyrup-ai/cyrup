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
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{
    CannedResponses, EventKind, ExtError, ExtensionHost, HookOutcome, HostConfig, HostCtx,
    HostEvent, InitApi, NativeExtension, RecordingServices,
};
use cyrup_core::{CancelToken, ExtensionId};
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
    host.load_native_with_services(prober.clone(), services())
        .await
        .expect("load native");

    let cancel = CancelToken::new();
    host.dispatcher()
        .dispatch_notify(&HostEvent::AgentStart, &cancel)
        .await;

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
    host.load_native_with_services(prober, services())
        .await
        .expect("load native");

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
    host.dispatcher()
        .dispatch_notify(&HostEvent::AgentStart, &cancel)
        .await;

    let seen = prober.seen();
    assert_eq!(
        seen[0],
        "event:idle=false trusted=false prompt=None model=None usage=false"
    );
}

// ---------------------------------------------------------------------------
// EXT-061 — `ctx.getSystemPromptOptions()`, the native half.
// ---------------------------------------------------------------------------

/// COVERAGE, NOT A REGRESSION PROOF (this pass's rule 8): `HostCtx::system_prompt_options` is a NEW
/// accessor, so no version of this test can go red against the previous HEAD — it did not exist to
/// call. It is written to pin the three behaviours the item's Verify line asks for, each of which a
/// later edit could silently take away.
///
/// (1) The COMMAND tier reads the attached bag. pi declares `getSystemPromptOptions()` on
/// `ExtensionCommandContext` (`extensions/types.ts:355` @v0.83.0), one tier up from
/// `getSystemPrompt()` on the base context (`:346`).
#[tokio::test]
async fn a_native_command_handler_reads_the_attached_system_prompt_options_bag() {
    let bag = serde_json::json!({"cwd": "/live", "selectedTools": ["read", "bash"]});
    let svc = Arc::new(RecordingServices::new(CannedResponses {
        system_prompt_options: Some(bag.clone()),
        ..Default::default()
    }));
    let ctx = crate::HostCtx::command(
        crate::ExtMode::Tui,
        true,
        std::path::PathBuf::from("/fallback"),
    )
    .with_rich(crate::native::rich_from_services(svc.as_ref()));

    assert_eq!(ctx.system_prompt_options().expect("command tier"), bag);
}

/// (2) With no bag attached the answer is pi's OWN no-backend default — `() => ({ cwd: this.cwd })`
/// (`core/extensions/runner.ts:287`, re-bound at `:350` @v0.83.0) — not `{}` and not an error. This
/// is the assertion that keeps the capability from being the shape EXT-066 found: declared in the
/// world, dead at the backend.
#[tokio::test]
async fn a_native_command_handler_with_no_bag_reads_pis_cwd_only_default() {
    let ctx = crate::HostCtx::command(crate::ExtMode::Tui, true, std::path::PathBuf::from("/proj"));
    assert_eq!(
        ctx.system_prompt_options().expect("command tier"),
        serde_json::json!({"cwd": "/proj"})
    );
}

/// (3) An EVENT-tier read is refused with the observable deadlock-guard error, never a silent empty
/// bag — the tier gate is upstream's own placement, not a cyrup restriction.
#[tokio::test]
async fn an_event_tier_native_handler_is_refused_the_options_bag() {
    let ctx = crate::HostCtx::event(crate::ExtMode::Tui, true, std::path::PathBuf::from("/proj"));
    assert!(
        matches!(ctx.system_prompt_options(), Err(ExtError::Deadlock)),
        "pi puts getSystemPromptOptions on ExtensionCommandContext (types.ts:355), so an event \
         handler must be refused rather than handed a bag"
    );
}
