//! EXT-004 — a tool registered AFTER `init` must actually reach the agent.
//!
//! Pi's `api.registerTool()` ends with `runtime.refreshTools()` on EVERY registration
//! (`coding-agent/src/core/extensions/loader.ts:249-256`), bound to `_refreshToolRegistry()`
//! (`agent-session.ts:2452-2546`), which rebuilds the tool registry from
//! `runner.getAllRegisteredTools()` and auto-activates names that were not there before
//! (`:2537-2543`). Registering from inside a live handler is a documented Pi pattern —
//! `coding-agent/examples/extensions/dynamic-tools.ts` registers from a `session_start` handler.
//!
//! cyrup minted the executable `Arc<dyn Tool>` for a guest descriptor exactly ONCE, immediately
//! after `init` (`ExtensionHost::load_wasm`). A descriptor that arrived later landed in the
//! registry's descriptor table and stopped there: `active_tools()` never surfaced it, so the agent
//! had no way to call it. This test does not assert that registration returned `Ok` — it drives the
//! guest's registration through a real `session_start` dispatch and then EXECUTES the tool across
//! the wasm boundary, asserting the guest's own code ran.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::fixture;

use cyrup_core::{CancelToken, Content};
use cyrup_ext::{DenyServices, HostEvent};
use serde_json::json;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_from_a_session_start_handler_is_executable() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    // After `init` the late tool does not exist yet — nothing has registered it.
    let before = host.active_tools(&[]).expect("active tools");
    assert!(
        !before.iter().any(|t| t.name() == "demo_late"),
        "demo_late must not exist before session_start: {:?}",
        before
            .iter()
            .map(|t| t.name().to_string())
            .collect::<Vec<_>>()
    );

    let cancel = CancelToken::new();

    // Drive the REAL lifecycle event. The guest's `session_start` handler calls
    // `ctx.register_tool(...)` across the `registration.register-tool` import.
    host.dispatcher()
        .dispatch_notify(
            &HostEvent::SessionStart {
                reason: "startup".into(),
                previous_session_file: None,
            },
            &cancel,
        )
        .await;

    // The host must now surface it as an EXECUTABLE tool, not just a descriptor.
    let active = host.active_tools(&[]).expect("active tools");
    let late = active
        .iter()
        .find(|t| t.name() == "demo_late")
        .unwrap_or_else(|| {
            panic!(
                "a tool registered from session_start never reached the agent's tool set: {:?}",
                active
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect::<Vec<_>>()
            )
        })
        .clone();

    assert_eq!(
        late.description(),
        "Registered after init, from a session_start handler (demo).",
        "the surfaced tool carries the guest's descriptor"
    );

    // The load-bearing assertion: the agent can CALL it, and the GUEST's code runs.
    let sink: cyrup_core::ToolUpdateSink = Box::new(|_| {});
    let result = late
        .execute(
            "late-1".into(),
            json!({ "text": "hi" }),
            cancel.clone(),
            sink,
        )
        .await
        .expect("the late-registered guest tool executes across the wasm boundary");
    match result.content.first() {
        Some(Content::Text { text, .. }) => {
            assert_eq!(
                text, "late: hi",
                "the GUEST's late-registered executor produced the result"
            )
        }
        other => panic!("unexpected late tool result content: {other:?}"),
    }

    // Idempotence: a second refresh must not duplicate or re-wrap it.
    let again = host.active_tools(&[]).expect("active tools");
    assert_eq!(
        again.iter().filter(|t| t.name() == "demo_late").count(),
        1,
        "the late tool is surfaced exactly once"
    );
}

/// A guest tool descriptor is bound to the instance that OWNS it. `load_wasm` used to wrap every
/// descriptor in the shared registry under the id of whichever extension loaded last, so a second
/// extension re-wrapped the first one's tools onto its own instance. Two loads of the same demo
/// component (distinct ids) must leave `demo_echo` executing against its real owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_extension_does_not_steal_the_first_ones_tools() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo-a".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load a");
    host.load_wasm("demo-b".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load b");

    let cancel = CancelToken::new();
    let active = host.active_tools(&[]).expect("active tools");
    let echo = active
        .iter()
        .find(|t| t.name() == "demo_echo")
        .expect("demo_echo surfaced")
        .clone();
    let sink: cyrup_core::ToolUpdateSink = Box::new(|_| {});
    let result = echo
        .execute("tc".into(), json!({ "text": "ok" }), cancel, sink)
        .await
        .expect("demo_echo executes against a live owner");
    match result.content.first() {
        Some(Content::Text { text, .. }) => assert_eq!(text, "echo: ok"),
        other => panic!("unexpected demo_echo result: {other:?}"),
    }
}

/// #2835, the SECOND materialization path. `AgentSession::refresh_extension_tools` — the drain that
/// carries a post-`init` registration onto the LIVE agent — calls `active_tools_filtered(&[], …)`
/// with the session's allowlist, and it is the ONLY path a late tool takes.
///
/// Fixing just the builder's path would leave the whole defect reachable through this one: a tool
/// registered from a `session_start` handler, which is exactly what pi's #2835 regression fixture
/// does, would merge straight into `DynamicToolState` and onto the agent with the allowlist never
/// consulted — and every builder-level test would still pass.
///
/// Driven through a REAL guest registration rather than a stub, so the thing being filtered is a
/// genuinely late-arriving descriptor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_registered_tool_is_filtered_too() {
    let bytes = std::fs::read(fixture::component()).expect("read fixture component bytes");
    let host = cyrup_ext::ExtensionHost::with_wasm(fixture::cfg()).expect("host with wasm runtime");
    host.load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    host.dispatcher()
        .dispatch_notify(
            &HostEvent::SessionStart {
                reason: "startup".into(),
                previous_session_file: None,
            },
            &CancelToken::new(),
        )
        .await;

    // Unrestricted: the late tool is there. This is the precondition — without it the assertion
    // below would pass for the wrong reason.
    assert!(
        host.active_tools(&[])
            .expect("active tools")
            .iter()
            .any(|t| t.name() == "demo_late"),
        "precondition: the late tool must exist before we assert it is filtered"
    );

    // A session pinned to `tools: ["read"]`, exactly as `refresh_extension_tools` would ask.
    let allow: std::collections::HashSet<String> = ["read".to_string()].into();
    let filtered = host
        .active_tools_filtered(&[], Some(&allow), &std::collections::HashSet::new())
        .expect("active tools");
    assert!(
        !filtered.iter().any(|t| t.name() == "demo_late"),
        "a tool registered AFTER init must still be bounded by the session's allowlist; got {:?}",
        filtered
            .iter()
            .map(|t| t.name().to_string())
            .collect::<Vec<_>>()
    );

    // And the allowlist that DOES name it lets it through, so this is a filter and not a blanket
    // refusal of late registrations.
    let allow: std::collections::HashSet<String> = ["demo_late".to_string()].into();
    assert!(
        host.active_tools_filtered(&[], Some(&allow), &std::collections::HashSet::new())
            .expect("active tools")
            .iter()
            .any(|t| t.name() == "demo_late")
    );

    // `excludeTools` reaches it too.
    let exclude: std::collections::HashSet<String> = ["demo_late".to_string()].into();
    assert!(
        !host
            .active_tools_filtered(&[], None, &exclude)
            .expect("active tools")
            .iter()
            .any(|t| t.name() == "demo_late")
    );
}
