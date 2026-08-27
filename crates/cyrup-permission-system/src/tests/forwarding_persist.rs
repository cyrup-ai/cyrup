//! FULLY-WIRED PROOF (R-PERM-030, port doc §8.2) that a FORWARDED "Allow Always" decision persists
//! into the CHILD's session approval store "just like a local ask" — through the SAME `apply_decision`
//! path a local dialog "Allow Always" uses (pi `persistPatternApprovalDecision` → `sessionApprovals.
//! approveAlways`, `index.ts:905`; the parent-side `processForwardedPermissionRequests` never writes a
//! store, `index.ts:1357-1504`, because the forwarded request carries only a message, no tool/subject —
//! so the store write is the child's, keyed on the tool/subject the child's own gate knows).
//!
//! A scripted `AskChannel` standing in for the [`cyrup_permission_system::ForwardingAskChannel`]'s
//! returned decision (the parent human picking "Allow Always" → response `state:"always"` →
//! `PermissionDecisionState::Always`) is installed on a child-shaped extension. Two identical `bash`
//! `ask`-tier calls (distinct tool_call_ids ⇒ a dedup MISS on the second) are driven through the real
//! `before_tool_call` gate: the FIRST forwards (channel invoked once) and the always-decision persists
//! a session rule for `(bash, "echo hi")`; the SECOND auto-ALLOWS via the store overlay with NO second
//! forward.
//!
//! Formerly `tests/forwarding_persist.rs`, an integration binary of its own. It owned a process
//! because it MUTATED process env — `unsafe { std::env::set_var("CYRUP_SUBAGENT_CHILD", "1") }` —
//! and [`super`]'s doc barred that from this directory. It no longer mutates anything: the anchor is
//! a THREAD-LOCAL [`crate::envx`] pin, so the module is an ordinary unit-test module.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, NativeExtension};

use crate::{
    AskChannel, AskOutcome, CHILD_ENV_VAR, ExtensionConfig, ManagerPaths, PermissionDecisionState,
    PermissionPromptDecision, PermissionSystemExtension, PromptOpts,
};

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the registry / unknown-tool gate (`gate::check_requested_tool_registration`, pi
/// `index.ts:2218-2228`) checks BEFORE any permission check. Without this attached, `bash` reads as
/// unregistered against the default empty registry and the gate blocks before ever reaching the
/// scripted [`AlwaysChannel`] below (mirrors `tests/layers_wired.rs`'s identical stand-in).
struct RegistryServices;
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_string()])
    }
}

/// A scripted channel: returns an "Allow Always" decision (the shape a `ForwardingAskChannel` returns
/// when the parent human picks "Allow Always") and counts how many forwards it serviced.
struct AlwaysChannel {
    invocations: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AskChannel for AlwaysChannel {
    async fn confirm(&self, _title: &str, _message: &str, _opts: PromptOpts) -> AskOutcome {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        AskOutcome::Decided(PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Always,
            denial_reason: None,
        })
    }
}

fn bash_call(call_id: &str, command: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: cyrup_core::ToolCallId::from(call_id),
        name: "bash".to_string(),
        input: serde_json::json!({ "command": command }),
    }
}

/// Driven on a CURRENT-THREAD runtime with [`CHILD_ENV_VAR`] pinned in this SYNCHRONOUS frame: the
/// pin is a thread-local [`crate::envx`] overlay, so the body must never be resumed on a worker
/// thread that cannot see it. Nothing here needs a second worker — the scripted channel resolves
/// immediately and no task is spawned.
#[test]
fn forwarded_allow_always_persists_a_child_session_rule() {
    let _pin = crate::envx::pin(CHILD_ENV_VAR, Some("1"));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(forwarded_allow_always_persists_a_child_session_rule_body());
}

async fn forwarded_allow_always_persists_a_child_session_rule_body() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    // Default is ASK; make bash explicitly ASK so both calls hit the forwarding channel unless a
    // session rule already promotes them to allow.
    std::fs::write(agent_dir.path().join("cyrup-permissions.jsonc"), r#"{ "bash": { "*": "ask" } }"#)
        .unwrap();

    let paths = ManagerPaths {
        global_config_path: agent_dir.path().join("cyrup-permissions.jsonc"),
        agents_dir: agent_dir.path().join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: agent_dir.path().join("settings.json"),
        global_mcp_config_path: agent_dir.path().join("mcp.json"),
        mcp_server_names_override: None,
    };
    let invocations = Arc::new(AtomicUsize::new(0));
    let ext = PermissionSystemExtension::from_parts(
        paths,
        ExtensionConfig::default(),
        Arc::new(AlwaysChannel { invocations: invocations.clone() }),
    );
    ext.set_host_services(Arc::new(RegistryServices));

    // A headless child ctx (has_ui=false) ⇒ `prompt_decision`'s live-vs-fail-closed gate
    // (`ctx.has_ui || is_subagent_child() || yolo_mode`, pi `confirmPermission`'s `hasUI` vs
    // `isSubagentExecutionContext` split, `index.ts:1506-1519`) only reaches `self.ask_channel` (the
    // scripted forwarding stand-in) when this process is CHILD-shaped. The caller's `envx::pin`
    // supplies that shape for THIS THREAD only, so no sibling test can observe it.
    let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.path().to_path_buf());

    // FIRST identical bash call: forwards (channel invoked once) and persists an always session rule.
    let first = ext.on_event(&bash_call("call-1", "echo hi"), &ctx).await;
    assert!(matches!(first, HookOutcome::Noop), "the forwarded Allow-Always must let the call proceed");
    assert_eq!(invocations.load(Ordering::SeqCst), 1, "the first call forwarded exactly once");

    // SECOND identical bash call, DIFFERENT tool_call_id ⇒ a dedup MISS. It must auto-ALLOW via the
    // persisted session rule, WITHOUT a second forward.
    let second = ext.on_event(&bash_call("call-2", "echo hi"), &ctx).await;
    assert!(matches!(second, HookOutcome::Noop), "the second identical call must auto-allow");
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "the always-decision persisted a child session rule ⇒ the second call did NOT forward again"
    );
}
