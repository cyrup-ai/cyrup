//! EXT-107 — the GUEST tier of pi `pi.registerToolRenderer(resolver)`, against the real
//! `wasm32-wasip2` demo component.
//!
//! pi's resolver is a closure, which cannot cross the component boundary, so the guest declares a
//! pattern and its idiom through `registration.register-tool-renderer`, and the host places it in
//! the SAME load-order chain a native's callback joins. The demo declares `demo_family_*` as a
//! FILL-IN (pi's MCP idiom, `next() ?? mine`) and `demo_claim_*` as a CLAIM
//! (`matches ? mine : next()`) — and registers no tool under either name.
//!
//! Each test asks the HOST to resolve and render by tool name; none touches the guest handle.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::fixture;

use cyrup_core::ExtensionId;
use cyrup_ext::{
    DenyServices, ExtError, ExtensionHost, HookOutcome, HostCtx, HostEvent, InitApi,
    NativeExtension, RenderOptions, ToolRendererMode,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// A native extension beside the guest: an exact-name renderer, or a claim on the guest's family.
struct Native {
    exact: Option<&'static str>,
    claim: Option<&'static str>,
}

#[async_trait::async_trait]
impl NativeExtension for Native {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("native")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        if let Some(name) = self.exact {
            api.register_tool_renderer(name);
        }
        if let Some(pattern) = self.claim {
            api.register_tool_renderer_pattern(pattern, ToolRendererMode::Claim)?;
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn render_call(&self, key: &str, _call: &Value) -> Option<Value> {
        Some(Value::String(format!("native call[{key}]")))
    }
    fn render_result(&self, key: &str, _result: &Value) -> Option<Value> {
        Some(Value::String(format!("native result[{key}]")))
    }
}

async fn load_guest(host: &ExtensionHost) {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init");
}

/// The rendered text, whichever shape it came back in: a guest returns a `text` widget, a native
/// a bare string.
async fn call_text(host: &ExtensionHost, name: &str) -> Option<String> {
    host.render_tool_call(name, &json!({ "q": 1 }), &RenderOptions::default())
        .await
        .map(|v| match v {
            Value::String(s) => s,
            other => other["text"].as_str().unwrap_or_default().to_string(),
        })
}

async fn result_text(host: &ExtensionHost, name: &str) -> Option<String> {
    host.render_tool_result(name, &json!({ "r": 2 }), &RenderOptions::default())
        .await
        .map(|v| match v {
            Value::String(s) => s,
            other => other["text"].as_str().unwrap_or_default().to_string(),
        })
}

/// Clause 1 (guest) — the guest's family draws calls to a name NO extension registered, through
/// the pre-check and both surfaces; a name outside every family still draws nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_family_draws_a_tool_no_extension_registered() {
    let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    load_guest(&host).await;

    assert!(host.has_tool_renderer("demo_family_ghost"));
    let call = call_text(&host, "demo_family_ghost").await.unwrap();
    assert!(call.starts_with("guest-family call:"), "{call}");
    let result = result_text(&host, "demo_family_ghost").await.unwrap();
    assert!(result.starts_with("guest-family result:"), "{result}");

    assert!(!host.has_tool_renderer("unclaimed_ghost"));
    assert_eq!(call_text(&host, "unclaimed_ghost").await, None);
}

/// Clause 2 (guest) — a tool with its OWN renderer keeps it: the guest's family is a fill-in, so
/// it defers to the registered renderer whichever side of it the owner loaded on, and still draws
/// the owner's unregistered sibling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_renderer_still_wins_over_the_guest_family() {
    for native_first in [true, false] {
        let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
        let native = Arc::new(Native {
            exact: Some("demo_family_owned"),
            claim: None,
        });
        if native_first {
            host.load_native(native).await.unwrap();
            load_guest(&host).await;
        } else {
            load_guest(&host).await;
            host.load_native(native).await.unwrap();
        }
        assert_eq!(
            call_text(&host, "demo_family_owned").await.as_deref(),
            Some("native call[demo_family_owned]"),
            "native loaded first = {native_first}"
        );
        assert_eq!(
            result_text(&host, "demo_family_owned").await.as_deref(),
            Some("native result[demo_family_owned]")
        );
        let sibling = call_text(&host, "demo_family_ghost").await.unwrap();
        assert!(sibling.starts_with("guest-family call:"), "{sibling}");
    }
}

/// Clause 3 (guest) — the guest and a native both CLAIM `demo_claim_*`; whichever loaded first
/// draws, and reversing the load order reverses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_and_a_native_claim_resolve_in_load_order() {
    let native = || {
        Arc::new(Native {
            exact: None,
            claim: Some("demo_claim_*"),
        })
    };

    let guest_first = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    load_guest(&guest_first).await;
    guest_first.load_native(native()).await.unwrap();
    let drawn = call_text(&guest_first, "demo_claim_x").await.unwrap();
    assert!(drawn.starts_with("guest-claim call:"), "{drawn}");

    let native_first = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    native_first.load_native(native()).await.unwrap();
    load_guest(&native_first).await;
    assert_eq!(
        call_text(&native_first, "demo_claim_x").await.as_deref(),
        Some("native call[demo_claim_x]")
    );
}

/// Clause 4 (guest) — purging the guest drops its declarations with it; a native's claim beside it
/// keeps drawing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purging_the_guest_removes_its_declarations() {
    let host = ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    load_guest(&host).await;
    host.load_native(Arc::new(Native {
        exact: None,
        claim: Some("native_*"),
    }))
    .await
    .unwrap();
    assert!(host.has_tool_renderer("demo_family_ghost"));
    host.registry()
        .purge_owner(&ExtensionId::from("demo"))
        .unwrap();
    assert!(!host.has_tool_renderer("demo_family_ghost"));
    assert!(!host.has_tool_renderer("demo_claim_x"));
    assert_eq!(
        call_text(&host, "native_x").await.as_deref(),
        Some("native call[native_x]")
    );
}
