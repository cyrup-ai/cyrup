//! The deciding gate's fail-closed behaviour and the decision record's scope trimming.

use std::sync::Arc;

use serde_json::{Value, json};

use cyrup_ext::{HookOutcome, NativeExtension};

use super::support::*;
use crate::dedup::DedupDetails;
use crate::extension::PermissionSystemExtension;
use crate::extension::paths::POLICY_FILE;

/// PERM-028. pi applies `getNonEmptyString` — which TRIMS (`common.ts:15-22`) — to
/// `target`/`command`/`path` and then falls through to a RAW `toolName ?? skillName`
/// (v0.8.0 `index.ts:581-592`). Cyrup filtered all five on a raw `!is_empty()`, so it kept the
/// padding and, worse, SELECTED a whitespace-only command that pi skips.
#[test]
fn permission_decision_scope_trims_the_first_three_and_not_the_last_two() {
    let padded = DedupDetails {
        command: Some("  git status  ".to_string()),
        tool_name: Some("bash".to_string()),
        ..DedupDetails::default()
    };
    assert_eq!(
        PermissionSystemExtension::permission_decision_scope(&padded),
        json!("git status"),
        "pi's `getNonEmptyString` trims the command"
    );

    // A whitespace-only command must FALL THROUGH, not be selected.
    let blank = DedupDetails {
        command: Some("   ".to_string()),
        tool_name: Some("bash".to_string()),
        ..DedupDetails::default()
    };
    assert_eq!(
        PermissionSystemExtension::permission_decision_scope(&blank),
        json!("bash")
    );

    // `toolName` is NOT run through `getNonEmptyString` upstream, so its padding survives.
    let raw_tool = DedupDetails {
        tool_name: Some("  bash  ".to_string()),
        ..DedupDetails::default()
    };
    assert_eq!(
        PermissionSystemExtension::permission_decision_scope(&raw_tool),
        json!("  bash  "),
        "pi falls through to a RAW `details.toolName`; trimming it here would be a NEW divergence"
    );

    // Nothing at all ⇒ pi returns `undefined`, cyrup's `null`.
    assert_eq!(
        PermissionSystemExtension::permission_decision_scope(&DedupDetails::default()),
        Value::Null
    );
}

/// pi `checkRequestedToolRegistration(toolName, pi.getAllTools())` (`index.ts:2218-2228`) runs
/// UNCONDITIONALLY — pi has no skip path. BEFORE this fix, when the live registry could not be
/// enumerated (no `HostServices` attached), cyrup silently SKIPPED the registry gate entirely,
/// letting ANY tool name through the allowlist with zero enforcement — this test fails against
/// that fail-open behavior.
#[tokio::test]
async fn registry_gate_fails_closed_with_no_attached_registry() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    // Global policy allows bash everywhere — a fail-OPEN gate would let this proceed (Noop).
    write_file(
        &agent_dir.join(POLICY_FILE),
        r#"{ "bash": { "*": "allow" } }"#,
    );
    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    init_ext(&ext).await;
    // Deliberately never call `set_host_services` — the registry cannot be enumerated.

    let outcome = ext
        .on_event(&bash_call("call-1"), &event_ctx(agent_dir))
        .await;
    assert!(
        matches!(outcome, HookOutcome::Block { .. }),
        "an unenumerable registry must fail CLOSED, never silently let the tool through"
    );
}

/// pi `canResolveAskPermissionRequest` (`yolo-mode.ts:21-23`), consulted via
/// `canRequestPermissionConfirmation` BEFORE any prompt/lock work (`index.ts:2263,2351,2452`):
/// `hasUI || isSubagent || yoloMode`. BEFORE this fix `prompt_decision` always attempted the
/// human-interaction lock + channel selection whenever a live backend was attached, even when
/// none of the three conditions held — this test fails against that behavior (it would hang/lock
/// against a live backend instead of failing closed immediately).
#[tokio::test]
async fn ask_fails_fast_without_ui_subagent_or_yolo() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    write_file(
        &agent_dir.join(POLICY_FILE),
        r#"{ "bash": { "*": "ask" } }"#,
    );
    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    init_ext(&ext).await;
    ext.set_host_services(Arc::new(FakeRegistry {
        names: vec!["bash".to_string()],
    }));

    // has_ui=false, no `CYRUP_SUBAGENT_CHILD` env, config.yolo_mode default false ⇒ the
    // pre-check must fail CLOSED immediately, never touching the lock/dialog machinery.
    let outcome = ext
        .on_event(&bash_call("call-1"), &event_ctx(agent_dir))
        .await;
    assert!(matches!(outcome, HookOutcome::Block { .. }));
}
