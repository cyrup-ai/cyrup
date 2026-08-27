//! Project-trust gating of the project-scoped policy scopes (pi #644,
//! `handlers/lifecycle.ts:54-60`, `:92-96`, `permission-session.ts:106-110`).
//!
//! The vulnerability these pin: an untrusted repository's checked-in
//! `<cwd>/.cyrup/agent/cyrup-permissions.jsonc` could ADD allow rules for anything the global
//! policy did not explicitly deny, turning an `ask` into a silent auto-allow before the human had
//! granted trust. The widening direction is what is asserted — a project ALLOW over a global
//! default of `ask` — because that is the direction that actually let something through.

use std::path::Path;
use std::sync::Arc;

use cyrup_ext::{HookOutcome, HostEvent, NativeExtension};

use super::support::*;
use crate::extension::PermissionSystemExtension;
use crate::extension::paths::{CONFIG_DIR, POLICY_FILE, PROJECT_AGENT_SUBDIR};

/// Build an extension whose GLOBAL policy is empty (so bash defaults to `ask`) and whose PROJECT
/// policy allows the `echo hi` that [`bash_call`] issues. Returns the extension and the session cwd.
async fn ext_with_project_allow(dir: &std::path::Path) -> (PermissionSystemExtension, std::path::PathBuf) {
    let agent_dir = dir.to_path_buf();
    // Global policy says nothing about bash, so the category default (`ask`) applies.
    write_file(&agent_dir.join(POLICY_FILE), r#"{}"#);

    let cwd = dir.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    // The untrusted repository's own file, widening the allow set.
    write_file(
        &PROJECT_AGENT_SUBDIR.iter().fold(cwd.clone(), |acc, seg| acc.join(seg)).join(POLICY_FILE),
        r#"{ "bash": { "echo *": "allow" } }"#,
    );

    let ext = PermissionSystemExtension::new(agent_dir, cwd.clone());
    init_ext(&ext).await;
    (ext, cwd)
}

async fn start_session(ext: &PermissionSystemExtension, ctx: &cyrup_ext::HostCtx) {
    ext.on_event(
        &HostEvent::SessionStart { reason: "startup".to_string(), previous_session_file: None },
        ctx,
    )
    .await;
}

/// The fix. With a backend attached and the host reporting the project UNTRUSTED, the project
/// scope is withheld, so the repository's `echo *: allow` never reaches the engine and the call
/// falls through to the global `ask` — which, with no UI, fails closed.
#[tokio::test]
async fn an_untrusted_project_cannot_widen_the_allow_set() {
    let dir = tempfile::tempdir().unwrap();
    let (ext, cwd) = ext_with_project_allow(dir.path()).await;
    ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));

    let ctx = event_ctx(cwd); // hand-built ctx => is_project_trusted = false
    start_session(&ext, &ctx).await;

    let outcome = ext.on_event(&bash_call("call-untrusted"), &ctx).await;
    assert!(
        matches!(outcome, HookOutcome::Block { .. }),
        "an untrusted project's own allow rule must NOT take effect"
    );
}

/// The other half: trust granted, the project scope loads exactly as before, and the repository's
/// allow rule applies. Without this, withholding everything unconditionally would also pass.
#[tokio::test]
async fn a_trusted_project_still_loads_its_own_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (ext, cwd) = ext_with_project_allow(dir.path()).await;
    ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));

    let ctx = trusted_event_ctx(cwd);
    start_session(&ext, &ctx).await;

    let outcome = ext.on_event(&bash_call("call-trusted"), &ctx).await;
    assert!(
        !matches!(outcome, HookOutcome::Block { .. }),
        "a trusted project's allow rule must still apply"
    );
}

/// The anti-narrowing guard, and the reason the gate keys on backend attachment rather than the
/// raw flag.
///
/// `HostCtxRich::default()` is `is_project_trusted = false`, and a host that attached no
/// `HostCtxSource` hands every dispatch that default (`cyrup-ext/src/native.rs:708-725`). Reading
/// the flag alone would withhold project policy from every such host — trading upstream's silent
/// widening for a silent narrowing. With no backend attached the answer is "trusted", i.e. the
/// scope is KEPT and behaviour is unchanged from before the gate existed.
#[tokio::test]
async fn with_no_backend_attached_the_project_scope_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (ext, cwd) = ext_with_project_allow(dir.path()).await;
    // Deliberately NO set_host_services.

    assert!(
        ext.project_trusted(&event_ctx(cwd.clone())),
        "with no backend attached the default is_project_trusted=false must NOT narrow the scope"
    );

    // And the flag IS honoured once a backend is attached — both ways.
    ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));
    assert!(!ext.project_trusted(&event_ctx(cwd.clone())), "untrusted must be honoured");
    assert!(ext.project_trusted(&trusted_event_ctx(cwd)), "trusted must be honoured");
}

/// pi `warnProjectUntrusted` (`handlers/lifecycle.ts:109-115`): the reduced scope is never silent.
/// `WarningSink` dedups per session, so the message appears exactly once across the session.
#[tokio::test]
async fn an_untrusted_session_announces_the_reduced_scope() {
    let dir = tempfile::tempdir().unwrap();
    let (ext, cwd) = ext_with_project_allow(dir.path()).await;
    let recorder = Arc::new(NotifyRecorder::new());
    ext.set_host_services(recorder.clone());

    let ctx = event_ctx(cwd);
    start_session(&ext, &ctx).await;

    let warnings = recorder.warnings();
    let announced: Vec<&String> =
        warnings.iter().filter(|w| w.contains("project is not trusted")).collect();
    assert_eq!(
        announced.len(),
        1,
        "an untrusted session must announce the reduced scope exactly once, got {warnings:?}"
    );
    assert!(
        announced[0].contains("Only global policy applies"),
        "the warning must say what is still in force: {:?}",
        announced[0]
    );
}

/// The `review`-stream entries for `event`, read back off the extension's own JSONL trail.
///
/// The trail lands at the DEFAULT logs dir — `<agent_dir>/cyrup-permission-system/logs` per
/// [`PermissionSystemExtension::logs_dir_for`] — which is inside the test's tempdir, so no
/// `CYRUP_PERMISSION_SYSTEM_LOGS_DIR` override is needed. The caller still has to hold the crate
/// env lock, because `logging::tests::logs_dir_env_var_overrides_the_default` sets that variable
/// PROCESS-WIDE while it runs and would otherwise divert this read to its own tempdir.
///
/// `review` is ungated (pi `logging.ts:99` is a bare `writeLine`, only `debug` early-returns on
/// `config.debug`), and the write is serialized under the logger's own lock rather than queued,
/// so there is nothing to flush before reading.
fn review_entries(agent_dir: &Path, event: &str) -> Vec<serde_json::Value> {
    let logs_dir = agent_dir.join(CONFIG_DIR).join(crate::logging::LOGS_DIR_NAME);
    let body = std::fs::read_to_string(crate::logging::debug_path(&logs_dir)).unwrap_or_default();
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| {
            entry.get("stream").and_then(serde_json::Value::as_str) == Some("review")
                && entry.get("event").and_then(serde_json::Value::as_str) == Some(event)
        })
        .collect()
}

/// pi `warnProjectUntrusted`'s FIRST half (`handlers/lifecycle.ts:113`):
/// `logger.review("project_trust.skipped", { cwd, phase })`.
///
/// This is the durable half — it survives a dropped notify sink, which is why pi writes it before
/// the notification and why it is worth pinning separately from
/// [`an_untrusted_session_announces_the_reduced_scope`]. Both `phase` values are covered: pi gates
/// the `resources_discover` reload on trust too (`:92-96`), so a reload in a still-untrusted
/// project records its own entry.
#[test]
fn an_untrusted_session_records_the_skip_in_the_review_trail() {
    with_config_env_lock(async {
        let dir = tempfile::tempdir().unwrap();
        let (ext, cwd) = ext_with_project_allow(dir.path()).await;
        ext.set_host_services(Arc::new(FakeRegistry { names: vec!["bash".to_string()] }));

        let ctx = event_ctx(cwd.clone());
        start_session(&ext, &ctx).await;
        ext.on_event(
            &HostEvent::ResourcesDiscover {
                cwd: cwd.display().to_string(),
                reason: "reload".to_string(),
            },
            &ctx,
        )
        .await;

        let entries = review_entries(dir.path(), "project_trust.skipped");
        let phases: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.get("phase").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(
            phases,
            vec!["session_start", "resources_discover"],
            "both trust-gated lifecycle arms must record their skip, in order"
        );
        for entry in &entries {
            assert_eq!(
                entry.get("cwd").and_then(serde_json::Value::as_str),
                Some(cwd.to_string_lossy().as_ref()),
                "the entry must name the cwd whose project scope was withheld"
            );
        }
    });
}
