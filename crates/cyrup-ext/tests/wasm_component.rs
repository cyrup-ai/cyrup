//! LIVE guest COMPONENT end-to-end (arch-08b, the headline proof). Builds the `cyrup-ext-sdk`
//! bundled demo extension to a `wasm32-wasip2` COMPONENT, loads it through the host (`load_wasm`:
//! link host imports -> instantiate -> run `init`), and dispatches REAL events into the running
//! `.wasm` guest, asserting the guest handled them: a `tool_call` permission gate blocks `bash`,
//! a notify hook records a UI notification, and a dynamically-registered guest tool executes
//! (streaming an update) across the boundary.
//!
//! The fixture is built by invoking `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`
//! (wasm32-wasip2 emits a component directly — no cargo-component/wasm-tools needed). Set
//! `CYRUP_EXT_FIXTURE_COMPONENT` to a prebuilt component to skip the nested build.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{CancelToken, Content};
use cyrup_ext::{
    DenyServices, EventKind, ExtMode, Extension, ExtensionHost, HostConfig, HostEvent, NotifyKind,
    Reduced,
};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Build (or locate) the demo guest component.
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // Build into a dedicated target dir so this nested build never contends with the outer
    // `cargo test --workspace` target lock.
    let build_dir = std::env::temp_dir().join("cyrup-ext-fixture-target");
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&build_dir)
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");

    let wasm = build_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_guest_component_blocks_notifies_and_runs_a_tool() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");

    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, Arc::new(DenyServices))
        .await
        .expect("load + init the live wasm extension");

    // init declared subscriptions: the gate (tool_call) + the notify hook (agent_start).
    assert!(ext.subscriptions().contains(EventKind::ToolCall), "guest subscribed to tool_call");
    assert!(ext.subscriptions().contains(EventKind::AgentStart), "guest subscribed to agent_start");

    let cancel = CancelToken::new();

    // 1) tool_call(bash) -> the GUEST blocks it with a reason (R-08-010), end to end across wasm.
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "tc1".into(),
                name: "bash".into(),
                input: json!({ "command": "rm -rf /" }),
            },
            &cancel,
        )
        .await;
    match reduced {
        Reduced::Blocked { reason, by } => {
            assert_eq!(by.to_string(), "demo");
            assert!(
                reason.as_deref().unwrap_or("").contains("bash is disabled"),
                "got reason {reason:?}"
            );
        }
        other => panic!("expected the guest to block bash, got {other:?}"),
    }

    // The guest's `ctx.ui().notify(...)` ran inside the gate — observable host-side.
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("blocked a bash call")),
        "guest UI notification recorded: {:?}",
        ext.guest().notifications()
    );
    // ...and it carried Pi's `type: "error"` severity across the WIT boundary (types.ts:135): the
    // gate used `notify_with(.., NotifyKind::Error)`, recorded host-side with the kind intact.
    assert!(
        ext.guest()
            .notifications_with_kind()
            .iter()
            .any(|(m, k)| m.contains("blocked a bash call") && *k == NotifyKind::Error),
        "blocked-bash notification recorded with error severity: {:?}",
        ext.guest().notifications_with_kind()
    );

    // 2) tool_call(read) -> NOT blocked: the (possibly folded) event passes.
    let reduced = host
        .dispatcher()
        .dispatch_block_mutate(
            HostEvent::ToolCall {
                call_id: "tc2".into(),
                name: "read".into(),
                input: json!({ "path": "x" }),
            },
            &cancel,
        )
        .await;
    assert!(matches!(reduced, Reduced::Pass(_)), "non-bash tool passes the guest gate");

    // 3) notify-only: agent_start -> the guest records another notification.
    host.dispatcher().dispatch_notify(&HostEvent::AgentStart, &cancel).await;
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("demo extension active")),
        "guest agent_start notification recorded"
    );

    // 4) the guest-registered `demo_echo` tool is in the active set and EXECUTES across the boundary,
    //    streaming an onUpdate chunk (R-08-013/015).
    let active = host.active_tools(&[]).expect("active tools");
    let echo = active.iter().find(|t| t.name() == "demo_echo").expect("guest tool surfaced");

    let updates: Arc<Mutex<Vec<cyrup_core::ToolUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_updates = updates.clone();
    let sink: cyrup_core::ToolUpdateSink = Box::new(move |u| sink_updates.lock().unwrap().push(u));

    let result = echo
        .execute("tc3".into(), json!({ "text": "hello" }), cancel.clone(), sink)
        .await
        .expect("guest tool executes");
    match result.content.first() {
        Some(Content::Text { text, .. }) => assert_eq!(text, "echo: hello"),
        other => panic!("unexpected guest tool result content: {other:?}"),
    }
    assert!(
        !updates.lock().unwrap().is_empty(),
        "guest tool streamed an onUpdate chunk across the boundary"
    );

    // 5) a guest slash command EXECUTES across the boundary at command tier and returns text
    //    (R-08-016), and its dynamic argument completer answers a prefix query.
    let out = ext.execute_command("greet", "world", &cancel).await.expect("guest command runs");
    assert_eq!(out.as_deref(), Some("hello, world!"));
    assert!(
        ext.guest().notifications().iter().any(|n| n.contains("greet command ran")),
        "command handler's ctx.ui().notify ran"
    );
    // The command addressed a KEYED status segment then cleared it (Pi `setStatus(key, text)` /
    // `setStatus(key, undefined)`, types.ts:141) — both calls recorded with their key across the WIT.
    let statuses = ext.guest().statuses();
    assert!(
        statuses.iter().any(|(k, t)| k == "greet" && t.as_deref() == Some("greeting…")),
        "keyed status set recorded: {statuses:?}"
    );
    assert!(
        statuses.iter().any(|(k, t)| k == "greet" && t.is_none()),
        "keyed status clear (text=None) recorded: {statuses:?}"
    );
    // Replaying the keyed segment resolves to cleared (the clear was last).
    assert_eq!(ext.guest().status_for("greet"), None, "greet status segment resolves cleared");
    let comps = ext.argument_completions("greet", "te").await.expect("completions");
    assert_eq!(comps, vec!["team".to_string()], "dynamic getArgumentCompletions filtered by prefix");

    // 6) a guest message renderer renders a call across the boundary (R-08-020).
    let widget = ext
        .render_call("demo", &json!({ "x": 1 }))
        .await
        .expect("render call")
        .expect("renderer produced a widget");
    assert_eq!(widget["widget"], json!("text"));
    assert!(widget["text"].as_str().unwrap_or("").contains("demo call"));

    // 7) the exec capability is DENIED for this DenyServices-backed guest (the untrusted analog): the
    //    guest's `/execdemo` calls `pi.exec("echo",["hi"])` across the WIT boundary, the host router
    //    routes it to `DenyServices::exec` which returns "exec capability not granted", and the guest
    //    surfaces that denial (Pi's ambient exec is unavailable to an ungranted extension).
    let out = ext.execute_command("execdemo", "", &cancel).await.expect("execdemo runs");
    assert_eq!(
        out.as_deref(),
        Some("exec denied: exec capability not granted"),
        "the deny-all backend refuses the guest's exec"
    );
    assert!(
        ext.guest()
            .notifications()
            .iter()
            .any(|n| n.contains("exec denied: exec capability not granted")),
        "the guest observed the exec denial across the boundary: {:?}",
        ext.guest().notifications()
    );

    // 8) the http-client capability is likewise DENIED for this DenyServices-backed guest (the
    //    untrusted analog, arch-08 §3.2 draft / pi-mcp-adapter-port.md §3.2): the guest's `/httpdemo`
    //    calls `ctx.http_request(...)` across the WIT boundary, the host router routes it to
    //    `DenyServices::http_request` which returns "http-client capability not granted", and the
    //    guest surfaces that denial — the SAME trust gate as `exec`, no separate allowlist.
    let out = ext.execute_command("httpdemo", "http://127.0.0.1:1/unused", &cancel).await.expect("httpdemo runs");
    assert_eq!(
        out.as_deref(),
        Some("http denied: http-client capability not granted"),
        "the deny-all backend refuses the guest's http_request"
    );
    assert!(
        ext.guest()
            .notifications()
            .iter()
            .any(|n| n.contains("http denied: http-client capability not granted")),
        "the guest observed the http-client denial across the boundary: {:?}",
        ext.guest().notifications()
    );

    // The streaming half is denied the same way (denial happens at `request-stream`, before any
    // poll — never a hang/panic on the ungranted path).
    let out =
        ext.execute_command("httpstreamdemo", "http://127.0.0.1:1/unused", &cancel).await.expect("httpstreamdemo runs");
    assert_eq!(
        out.as_deref(),
        Some("http stream denied: http-client capability not granted"),
        "the deny-all backend refuses the guest's http_request_stream"
    );

    // an unknown command surfaces an error (never a host crash).
    let err = ext.execute_command("nope", "", &cancel).await.unwrap_err();
    assert!(err.to_string().contains("no such command"), "unknown command surfaced: {err}");
}
