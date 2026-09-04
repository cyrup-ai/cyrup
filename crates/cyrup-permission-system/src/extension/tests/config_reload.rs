//! The `session_start` / `resources_discover` refresh: what a reload re-reads, re-applies and
//! re-reports.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use cyrup_ext::{HookOutcome, HostEvent, NativeExtension};

use crate::status;
use crate::types::PermissionState;

use super::support::*;
use crate::extension::paths::{CONFIG_DIR, CONFIG_FILE, POLICY_FILE, PROJECT_AGENT_SUBDIR};
use crate::extension::{PermissionSystemExtension, guard};
use crate::skill::SkillPromptEntry;

/// pi `resources_discover` reload branch (`index.ts:2103-2118`): re-reads `config.json` and
/// invalidates the agent-start cache. BEFORE this fix, `EventKind::ResourcesDiscover` was never
/// subscribed and `on_event` fell through to its catch-all `Noop` arm, so neither the config nor
/// the cached skill-enforcement entries ever refreshed — this test fails against that behavior.
#[test]
fn resources_discover_reloads_config_and_invalidates_skill_cache() {
    block_on(resources_discover_reloads_config_and_invalidates_skill_cache_body());
}

async fn resources_discover_reloads_config_and_invalidates_skill_cache_body() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    init_ext(&ext).await;

    // The constructor auto-materializes the default config (yolo_mode: false).
    assert!(
        !guard(&ext.config).yolo_mode,
        "default config starts with yolo off"
    );

    // Seed the agent-start skill cache as `before_agent_start` would.
    *guard(&ext.active_skill_entries) = vec![SkillPromptEntry {
        name: "demo".into(),
        state: PermissionState::Ask,
        normalized_location: "/skills/demo".into(),
        normalized_base_dir: "/skills".into(),
    }];

    // Flip yoloMode on disk directly (simulating an external edit to config.json betwen the
    // extension's construction and a later `resources_discover` reload).
    write_file(
        &agent_dir.join(CONFIG_DIR).join(CONFIG_FILE),
        r#"{ "yoloMode": true }"#,
    );

    let outcome = ext
        .on_event(
            &HostEvent::ResourcesDiscover {
                cwd: agent_dir.display().to_string(),
                reason: "reload".to_string(),
            },
            &event_ctx(agent_dir),
        )
        .await;
    assert!(matches!(outcome, HookOutcome::Noop));

    assert!(
        guard(&ext.config).yolo_mode,
        "resources_discover reload must re-read config.json"
    );
    assert!(
        guard(&ext.active_skill_entries).is_empty(),
        "resources_discover reload must invalidate the agent-start skill cache"
    );
}

/// pi `refreshSessionRuntimeState` (`index.ts:2077-2085`): every `session_start` unconditionally
/// re-derives `permissionManager`'s policy paths from the CURRENT session `ctx.cwd`. BEFORE this
/// fix `self.manager` was frozen at construction time and never re-derived on `session_start`, so
/// a session starting under a DIFFERENT project directory never picked up that project's policy
/// override — this test fails against that behavior.
#[tokio::test]
async fn session_start_rebuilds_manager_from_current_session_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    // Global policy: bash is allowed everywhere by default.
    write_file(
        &agent_dir.join(POLICY_FILE),
        r#"{ "bash": { "*": "allow" } }"#,
    );

    // The extension is CONSTRUCTED against `cwd1`, which has no project-level override.
    let cwd1 = dir.path().join("cwd1");
    std::fs::create_dir_all(&cwd1).unwrap();
    let ext = PermissionSystemExtension::new(agent_dir.clone(), cwd1);
    init_ext(&ext).await;
    ext.set_host_services(Arc::new(FakeRegistry {
        names: vec!["bash".to_string()],
    }));

    // A NEW session starts under `cwd2`, which HAS a project-scoped override denying bash.
    let cwd2 = dir.path().join("cwd2");
    std::fs::create_dir_all(&cwd2).unwrap();
    write_file(
        &PROJECT_AGENT_SUBDIR
            .iter()
            .fold(cwd2.clone(), |acc, seg| acc.join(seg))
            .join(POLICY_FILE),
        r#"{ "bash": { "*": "deny" } }"#,
    );

    let start_ctx = event_ctx(cwd2);
    let start_outcome = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &start_ctx,
        )
        .await;
    assert!(matches!(start_outcome, HookOutcome::Noop));

    // A bash call now, under `cwd2`, must be DENIED by the project override the rebuilt manager
    // picked up — proving the manager was rebuilt against the CURRENT session cwd, not left
    // stale against `cwd1`.
    let outcome = ext.on_event(&bash_call("call-1"), &start_ctx).await;
    assert!(
        matches!(outcome, HookOutcome::Block { .. }),
        "the cwd2 project-scoped deny must enforce once session_start rebuilds the manager"
    );
}

// ==========================================================================================
// PERM-013 / PERM-024 / PERM-026 / PERM-027 — the lifecycle refresh + agent-start cache.
// ==========================================================================================

/// PERM-024 (RED before the fix). pi's `before_agent_start` handler's SECOND statement is
/// `refreshExtensionConfig(ctx)` (v0.8.0 `index.ts:1877`), so a mid-session `config.json` edit
/// takes effect at the top of the very next turn. Cyrup refreshed only at `session_start` and
/// `resources_discover`.
#[test]
fn before_agent_start_re_reads_config_json() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
        write_file(&config_path, r#"{"yoloMode": false}"#);

        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let ctx = event_ctx(agent_dir.clone());
        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "startup".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        assert!(
            !ext.yolo_mode(),
            "control: the session started with yolo off"
        );

        write_file(&config_path, r#"{"yoloMode": true}"#);
        let _ = ext.on_event(&before_agent_start("SYSTEM"), &ctx).await;
        assert!(
            ext.yolo_mode(),
            "a mid-session config edit must be live at the top of the next turn (pi `:1877`)"
        );
    });
}

/// PERM-026 (RED before the fix). pi syncs the status pill from inside
/// `applyExtensionConfigSideEffects` (v0.8.0 `index.ts:1364-1366`), which EVERY refresh surface
/// reaches — including the `resources_discover` reload branch (`:1848`). Cyrup's sync lived only
/// in the `SessionStart` and `before_agent_start` arms, so a reload changed the live gating
/// behaviour while the pill kept the stale value.
#[test]
fn a_resources_discover_reload_re_syncs_the_yolo_pill() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
        write_file(&config_path, r#"{"yoloMode": false}"#);

        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let host = Arc::new(LifecycleRecorder::new());
        ext.set_host_services(host.clone());
        let ctx = event_ctx(agent_dir.clone());
        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "startup".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        assert_eq!(
            guard(&host.statuses).last().cloned(),
            Some(None),
            "control: yolo off paints no pill"
        );

        write_file(&config_path, r#"{"yoloMode": true}"#);
        let _ = ext
            .on_event(
                &HostEvent::ResourcesDiscover {
                    cwd: agent_dir.display().to_string(),
                    reason: "reload".to_string(),
                },
                &ctx,
            )
            .await;
        assert_eq!(
            guard(&host.statuses).last().cloned().flatten(),
            Some(status::YOLO_STATUS_VALUE.to_string()),
            "the reload must repaint the pill BEFORE any before_agent_start does"
        );
    });
}

/// PERM-027 (RED before the fix). pi writes a `lifecycle.reload` debug entry from BOTH reload
/// surfaces (v0.8.0 `index.ts:1834-1843` and `:1853-1857`) and from NEITHER on a startup
/// session, so an operator can tell a reload from a fresh start in the trail. Cyrup wrote none.
#[test]
fn reload_surfaces_write_lifecycle_reload_debug_entries() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().to_path_buf();
        write_file(
            &agent_dir.join(CONFIG_DIR).join(CONFIG_FILE),
            r#"{"debug": true}"#,
        );

        let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
        init_ext(&ext).await;
        let ctx = event_ctx(agent_dir.clone());

        // A STARTUP session writes no lifecycle line (pi gates on `event.reason === "reload"`).
        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "startup".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        assert_eq!(lifecycle_reload_entries(&agent_dir).len(), 0);

        let _ = ext
            .on_event(
                &HostEvent::SessionStart {
                    reason: "reload".to_string(),
                    previous_session_file: None,
                },
                &ctx,
            )
            .await;
        let _ = ext
            .on_event(
                &HostEvent::ResourcesDiscover {
                    cwd: agent_dir.display().to_string(),
                    reason: "reload".to_string(),
                },
                &ctx,
            )
            .await;

        let triggers: Vec<String> = lifecycle_reload_entries(&agent_dir)
            .into_iter()
            .filter_map(|e| e["triggeredBy"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            triggers,
            vec![
                "session_start".to_string(),
                "resources_discover".to_string()
            ],
            "both reload surfaces must name themselves in the trail"
        );
    });
}

/// Read every `lifecycle.reload` record out of the debug JSONL this extension writes.
fn lifecycle_reload_entries(agent_dir: &Path) -> Vec<Value> {
    let path = crate::logging::debug_path(&PermissionSystemExtension::logs_dir_for(agent_dir));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["event"] == json!("lifecycle.reload"))
        .collect()
}

/// PERM-003. pi threads `notifyWarning` into EVERY `PermissionManager` it builds
/// (`createPermissionManagerForCwd(cwd, notifyWarning)`, `index.ts:1595,2081,2109-2110`) and
/// into `refreshExtensionConfig` (`index.ts:1614`), so a policy or config file that exists but
/// does not parse reaches the human as a `warning` notification.
///
/// BEFORE this fix `PermissionManager::with_on_warning` had no caller outside this crate's own
/// unit tests and `refresh_config_and_manager` used the warning-discarding `ExtensionConfig::
/// load`, so both failures degraded in TOTAL SILENCE: a typo'd `cyrup-permissions.jsonc` fell
/// back to "ask everything", which looks exactly like a policy that genuinely says ask. This
/// test drives a real session lifecycle + tool call and asserts the messages actually arrive at
/// the host boundary.
#[test]
fn malformed_policy_and_config_files_notify_the_host() {
    block_on(malformed_policy_and_config_files_notify_the_host_body());
}

async fn malformed_policy_and_config_files_notify_the_host_body() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    // Present, but truncated mid-object: exists (so it is not the silent ENOENT case) and does
    // not parse.
    write_file(&agent_dir.join(POLICY_FILE), r#"{ "bash": { "*": "allow" "#);
    write_file(&agent_dir.join(CONFIG_DIR).join(CONFIG_FILE), "{ not json");

    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    init_ext(&ext).await;
    let host = Arc::new(NotifyRecorder::new());
    ext.set_host_services(host.clone());

    let ctx = event_ctx(agent_dir.clone());
    let start = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    assert!(matches!(start, HookOutcome::Noop));
    // A real tool call is what forces the policy layers to be read.
    let _ = ext.on_event(&bash_call("call-1"), &ctx).await;

    let warnings = host.warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("Failed to parse permission config at")
                && w.contains(POLICY_FILE)
                && w.ends_with("using ask fallback.")),
        "the unparseable policy file must reach the host as a warning; got {warnings:?}"
    );
    assert!(
        warnings.iter().any(
            |w| w.starts_with("Failed to parse permission-system config at")
                && w.ends_with("using default extension config.")
        ),
        "the unparseable extension config must reach the host as a warning; got {warnings:?}"
    );

    // pi `shownWarnings` (`index.ts:1573,1586-1592`): each distinct message is reported at most
    // once per session, so a reload storm cannot spam the user. Re-running the whole refresh +
    // tool-call cycle must not duplicate anything already shown.
    let before = warnings.len();
    let _ = ext.on_event(&bash_call("call-2"), &ctx).await;
    assert_eq!(
        host.warnings().len(),
        before,
        "warnings must be deduped within a session"
    );

    // ...and a NEW session re-arms them (pi `resetShownWarnings`, `index.ts:1819`), so a file
    // that is still broken is reported again rather than silently suppressed forever.
    //
    // PERM-021 — this asserts on the CONTENT of the delta, not on its size. The old
    // `warnings().len() > before` was satisfiable by the POLICY warning alone, so a regression
    // that stopped re-arming it while leaving the config channel alone would still have passed.
    // It also cannot be satisfied by the CONFIG warning: `WarningSink::reset` clears only
    // `shown`, while `last_config_warning` is cleared solely by a clean load or a successful
    // save — a suppression that is pi's own (`index.ts:1370-1374`'s
    // `result.warning !== lastConfigWarning` memo survives `resetShownWarnings`), so pi
    // likewise reports a still-broken `config.json` once per PROCESS. The sibling test below
    // covers the config channel through the clean-load-then-corrupt sequence that legitimately
    // clears that memo.
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    let _ = ext.on_event(&bash_call("call-3"), &ctx).await;
    let after = host.warnings();
    let delta = after.get(before..).unwrap_or_default();
    assert!(
        delta
            .iter()
            .any(|w| w.starts_with("Failed to parse permission config at")
                && w.contains(POLICY_FILE)),
        "a new session must re-report the still-broken POLICY file; delta was {delta:?}"
    );
    assert!(
        !delta
            .iter()
            .any(|w| w.starts_with("Failed to parse permission-system config at")),
        "the CONFIG warning is memoized per-process by `last_config_warning` (pi              `index.ts:1370-1374`); a re-report here would mean that memo stopped working"
    );
}

/// PERM-021's sibling: the CONFIG warning channel re-arms once `last_config_warning` is
/// legitimately cleared. pi clears it on a CLEAN load (`index.ts:1373-1374`
/// `else if (!result.warning) { lastConfigWarning = null; }`), so a session that loads a good
/// `config.json` and then finds a corrupt one reports the corruption — the case the count-based
/// assertion above could never distinguish from the policy warning firing twice.
///
/// Driven through [`block_on`] like every other lifecycle test in this module: the body is one
/// `async fn` so the three `.await`s below share a single current-thread runtime.
#[test]
fn a_config_warning_re_arms_after_a_clean_load_clears_the_memo() {
    block_on(a_config_warning_re_arms_after_a_clean_load_clears_the_memo_body());
}

async fn a_config_warning_re_arms_after_a_clean_load_clears_the_memo_body() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().to_path_buf();
    let config_path = agent_dir.join(CONFIG_DIR).join(CONFIG_FILE);
    // A VALID config: the load is clean, so `last_config_warning` is `None`.
    write_file(&config_path, "{ \"debug\": false }");

    let ext = PermissionSystemExtension::new(agent_dir.clone(), agent_dir.clone());
    init_ext(&ext).await;
    let host = Arc::new(NotifyRecorder::new());
    ext.set_host_services(host.clone());
    let ctx = event_ctx(agent_dir.clone());
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    assert!(
        !host
            .warnings()
            .iter()
            .any(|w| w.starts_with("Failed to parse permission-system config at")),
        "a valid config must not warn; got {:?}",
        host.warnings()
    );

    // Now corrupt it and reload. The memo is `None`, so the warning is NEW and must surface.
    write_file(&config_path, "{ not json");
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    assert!(
        host.warnings().iter().any(|w| w
            .starts_with("Failed to parse permission-system config at")
            && w.ends_with("using default extension config.")),
        "a corrupt config after a clean load must reach the host; got {:?}",
        host.warnings()
    );
}
// ------------------------------------------------------------ PERM-001: subagent env hints
