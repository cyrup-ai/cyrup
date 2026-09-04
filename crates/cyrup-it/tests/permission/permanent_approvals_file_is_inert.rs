//! FULLY-WIRED REGRESSION PROOF that an on-disk `cyrup-permission-system-approvals.json` no longer
//! influences ANY permission decision — the observable consequence of upstream deleting
//! `PermanentApprovalStore` (`pi-permission-system` commit `a33ac2c`,
//! `feat(permissions)!: remove permanent approval store`, released as v0.8.0; CHANGELOG `### Removed`:
//! "Removed `PermanentApprovalStore` and the `pi-permission-system-approvals.json` persistence file
//! ... Cross-session persistent approvals are no longer written to disk.").
//!
//! WHAT ACTUALLY CHANGED. At v0.7.1 `applyPatternApprovalState` (`src/index.ts:850-874`) evaluated
//! FOUR rulesets — `[configRule, sessionApprovals.getRules(), permanentApprovals.getRules()]` — and
//! `evaluatePermission` is LAST-MATCH-WINS, so the on-disk store ranked last and could override both
//! the session store and the operator's own config rule. It was also TRI-state (unlike the allow-only
//! session store), so it could flip an `allow` to `deny` as well as an `ask` to `allow`. v0.8.0
//! `src/index.ts:557-579` evaluates `[configRule, sessionApprovals.getRules()]` and that whole
//! override tier is gone.
//!
//! WHY THIS IS THE RIGHT TEST. The store was already WRITE-dead on both sides: upstream's
//! `PermanentApprovalStore.approveAlways` had zero call sites in v0.7.1 `index.ts`, and cyrup
//! faithfully mirrored that ("Allow Always" has always gone to the session store, `extension.rs`
//! `apply_decision`). So "Allow Always does not persist across sessions" was ALREADY true before this
//! change and asserting it would leave a reverted build green. The only READ path is a hand-authored
//! or legacy approvals file, which is exactly what both red tests below plant on disk.
//!
//! Each test drives a real `PermissionSystemExtension` built through its PRODUCTION constructor
//! (`PermissionSystemExtension::new`, the one that derives every store path from `agent_dir`) through
//! the registered `before_tool_call` gate (`NativeExtension::on_event(ToolCall)`) — the same entry
//! point the dispatcher drives at runtime. The context is headless (`has_ui == false`) and this is not
//! a subagent, so an unresolved `ask` fail-CLOSES to `Block`: the outcome is a clean readout of the
//! resolved permission state.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use cyrup_permission_system::PermissionSystemExtension;
use serde_json::json;

/// The legacy on-disk approvals file `PermanentApprovalStore` used to read (cyrup's analog of pi's
/// `pi-permission-system-approvals.json`). Nothing reads it any more; the tests write it to prove
/// exactly that.
const APPROVALS_FILE: &str = "cyrup-permission-system-approvals.json";

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the unknown-tool gate (pi `index.ts:2218-2228`) checks BEFORE any permission check.
/// Without it `bash` reads as unregistered and the gate blocks before reaching the overlay at all,
/// which would make every assertion below vacuous.
struct RegistryServices;
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_string()])
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Build an installed extension over `global` policy, optionally planting `approvals` at the legacy
/// `<agent_dir>/cyrup-permission-system-approvals.json` path FIRST — so the store, if it still
/// existed, would lazily load it on the first decision.
async fn ext_with(
    agent_dir: &Path,
    global: &str,
    approvals: Option<&str>,
) -> PermissionSystemExtension {
    write(&agent_dir.join("cyrup-permissions.jsonc"), global);
    if let Some(body) = approvals {
        write(&agent_dir.join(APPROVALS_FILE), body);
    }
    let ext = PermissionSystemExtension::new(agent_dir.to_path_buf(), agent_dir.to_path_buf());
    ext.set_host_services(Arc::new(RegistryServices));
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
    ext
}

fn ctx(cwd: PathBuf) -> HostCtx {
    // A headless event-tier ctx — the exact shape the dispatcher hands `before_tool_call`.
    HostCtx::event(ExtMode::Print, false, cwd)
}

fn bash_call(call_id: &str, command: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: ToolCallId::from(call_id),
        name: "bash".to_string(),
        input: json!({ "command": command }),
    }
}

fn describe(outcome: &HookOutcome) -> String {
    format!("{outcome:?}")
}

// ================================================================================================
// (1) RED: an on-disk `allow` can no longer promote an `ask` to `allow`.
// ================================================================================================

/// Policy says `ask` for every bash command. A legacy approvals file grants `bash`/`git *` an
/// `allow`. At v0.7.1 that file ranked LAST in `[config, session, permanent]` and won, so the call
/// sailed through ungated with no prompt; at v0.8.0 the file is not consulted at all, the state stays
/// `ask`, and a headless context fail-CLOSES to `Block`.
#[tokio::test]
async fn on_disk_allow_no_longer_promotes_an_ask_to_allow() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(
        agent_dir,
        r#"{ "bash": { "*": "ask" } }"#,
        Some(r#"[{"tool":"bash","pattern":"git *","action":"allow"}]"#),
    )
    .await;

    let out = ext
        .on_event(
            &bash_call("call-1", "git status"),
            &ctx(agent_dir.to_path_buf()),
        )
        .await;
    assert!(
        matches!(out, HookOutcome::Block { .. }),
        "a hand-authored {APPROVALS_FILE} must NOT auto-allow a policy-`ask` command — the \
         permanent approval store was deleted upstream in v0.8.0 (a33ac2c); got {}",
        describe(&out)
    );
}

// ================================================================================================
// (2) RED: an on-disk `deny` can no longer override the operator's own `allow` rule.
// ================================================================================================

/// The mirror direction, and the one that shows the deleted tier was tri-state rather than merely an
/// extra allow-list: the operator's policy ALLOWS every bash command, and the legacy approvals file
/// denies `git *`. At v0.7.1 the file outranked the config rule and blocked the call; at v0.8.0 the
/// operator's rule is final and the call proceeds.
#[tokio::test]
async fn on_disk_deny_no_longer_overrides_a_config_allow() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(
        agent_dir,
        r#"{ "bash": { "*": "allow" } }"#,
        Some(r#"[{"tool":"bash","pattern":"git *","action":"deny"}]"#),
    )
    .await;

    let out = ext
        .on_event(
            &bash_call("call-2", "git status"),
            &ctx(agent_dir.to_path_buf()),
        )
        .await;
    assert!(
        matches!(out, HookOutcome::Noop),
        "a hand-authored {APPROVALS_FILE} must NOT override the operator's own `allow` rule; \
         got {}",
        describe(&out)
    );
}

// ================================================================================================
// (3) MIRROR (green before AND after): the overlay itself still works in both directions.
// ================================================================================================

/// Proves the removal is not over-broad — `apply_pattern_approval_state` still folds the config rule
/// in exactly as before. With NO approvals file present, an `allow` policy still allows and an `ask`
/// policy still fail-closes. Both assertions hold identically against a build that still carries the
/// permanent store (the file it would read does not exist), so this case isolates the deletion from
/// any collateral damage to the overlay.
#[tokio::test]
async fn config_rule_still_decides_when_no_approvals_file_exists() {
    let allow_dir = tempfile::tempdir().unwrap();
    let allowed = ext_with(allow_dir.path(), r#"{ "bash": { "*": "allow" } }"#, None).await;
    let allow_out = allowed
        .on_event(
            &bash_call("call-3", "git status"),
            &ctx(allow_dir.path().to_path_buf()),
        )
        .await;
    assert!(
        matches!(allow_out, HookOutcome::Noop),
        "a config `allow` must still proceed through the overlay; got {}",
        describe(&allow_out)
    );

    let ask_dir = tempfile::tempdir().unwrap();
    let asking = ext_with(ask_dir.path(), r#"{ "bash": { "*": "ask" } }"#, None).await;
    let ask_out = asking
        .on_event(
            &bash_call("call-4", "git status"),
            &ctx(ask_dir.path().to_path_buf()),
        )
        .await;
    assert!(
        matches!(ask_out, HookOutcome::Block { .. }),
        "a config `ask` with no reachable human must still fail-CLOSE; got {}",
        describe(&ask_out)
    );
}

// ================================================================================================
// (4) MIRROR (green before AND after): upstream's own v0.8.0 regression assertion.
// ================================================================================================

/// `v0.8.0:tests/approved-fixes.test.ts:259-292` asserts `existsSync(approvalsPath) === false` after
/// an "Allow Always". cyrup never wrote the file either (its `approve_always` writer was retained for
/// source fidelity and never wired), so this is green on both sides — it is recorded here as the
/// upstream-parity companion to (1) and (2), NOT as the proof of change.
#[tokio::test]
async fn no_approvals_file_is_ever_written() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    let ext = ext_with(agent_dir, r#"{ "bash": { "*": "ask" } }"#, None).await;

    let _ = ext
        .on_event(
            &bash_call("call-5", "git status"),
            &ctx(agent_dir.to_path_buf()),
        )
        .await;

    assert!(
        !agent_dir.join(APPROVALS_FILE).exists(),
        "the runtime must never create {APPROVALS_FILE}"
    );
}
