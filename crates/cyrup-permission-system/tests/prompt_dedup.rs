//! FULLY-WIRED REGRESSION PROOF that the prompt-dedup cache covers **every** ask surface, not just
//! the main check — pi keeps the cache inside `promptPermission` itself
//! (`pi-permission-system` v0.7.1 `src/index.ts:1798-1815` lookup, `:1890-1892` store), so all three
//! call sites that route through it are deduplicated identically:
//!
//! - skill-read (`index.ts:2282-2292`, `source: "skill_read"`),
//! - external-directory (`index.ts:2369-2378`, `source: "tool_call"`),
//! - the main `ask` check (`index.ts:2469`).
//!
//! A re-emitted IDENTICAL `tool_call` (same `toolCallId` + same fingerprint) must therefore render
//! ZERO additional prompts on ANY of them — upstream's own `tests/edit-decision-deduplication-red.
//! test.ts` is the regression proof for that invariant.
//!
//! BEFORE the fix, cyrup's cache lookup/store lived in `resolve_ask` (the main check) rather than in
//! `prompt_decision` (the `promptPermission` port), so `resolve_skill_read` and
//! `resolve_external_directory` called the prompting core directly and bypassed the cache entirely:
//! a re-emitted identical `tool_call` opened a SECOND modal dialog for the same skill-file read /
//! out-of-workdir path. Both tests below fail against that behavior with `2` prompts observed.
//!
//! Each test drives a real `PermissionSystemExtension` through the registered `before_tool_call` gate
//! (`NativeExtension::on_event(ToolCall)`) with a scripted [`AskChannel`] that COUNTS prompts — the
//! same seam `tests/forwarding_persist.rs` uses. "Allow Once" is deliberate: it persists nothing, so
//! the only thing that can collapse the second prompt is the dedup cache.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use cyrup_permission_system::{
    AskChannel, AskOutcome, ExtensionConfig, ManagerPaths, PermissionDecisionState,
    PermissionPromptDecision, PermissionSystemExtension, PromptOpts,
};
use serde_json::{json, Value};

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the unknown-tool gate (pi `index.ts:2218-2228`) checks BEFORE any permission check.
/// Without it `read` reads as unregistered against the default empty registry and the gate blocks
/// before ever reaching the prompting core.
struct RegistryServices;
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["read".to_string()])
    }
}

/// A scripted channel answering "Allow Once" (pi `permission-dialog.ts` `APPROVE_ONCE_OPTION`) and
/// COUNTING how many dialogs it serviced. `Once` persists nothing — no session rule, no store
/// overlay — so the prompt count is a clean readout of the dedup cache alone.
struct CountingOnceChannel {
    prompts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AskChannel for CountingOnceChannel {
    async fn confirm(&self, _title: &str, _message: &str, _opts: PromptOpts) -> AskOutcome {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        AskOutcome::Decided(PermissionPromptDecision {
            approved: true,
            state: PermissionDecisionState::Once,
            denial_reason: None,
        })
    }
}

/// `prompt_decision`'s fail-fast pre-check (pi `canRequestPermissionConfirmation`,
/// `index.ts:2263,2351,2452`) is `hasUI || isSubagent || yoloMode`, and the channel it then selects is
/// the injected `ask_channel` only when `has_ui` is false. Marking this process child-shaped is what
/// routes the prompt to [`CountingOnceChannel`] instead of a live `LocalAskChannel`. Set ONCE and
/// never unset: both tests in this binary need it and may run concurrently.
fn ensure_subagent_child() {
    static SET: Once = Once::new();
    SET.call_once(|| {
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("CYRUP_SUBAGENT_CHILD", "1");
        }
    });
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Build an installed extension over `global` policy with the counting channel injected. Returns the
/// extension and its prompt counter.
async fn ext_with_counting_channel(
    agent_dir: &Path,
    global: &str,
) -> (PermissionSystemExtension, Arc<AtomicUsize>) {
    let policy_path = agent_dir.join("cyrup-permissions.jsonc");
    write(&policy_path, global);
    let paths = ManagerPaths {
        global_config_path: policy_path,
        agents_dir: agent_dir.join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: agent_dir.join("settings.json"),
        global_mcp_config_path: agent_dir.join("mcp.json"),
        mcp_server_names_override: None,
    };
    let prompts = Arc::new(AtomicUsize::new(0));
    let ext = PermissionSystemExtension::from_parts(
        paths,
        agent_dir.join("cyrup-permission-system-approvals.json"),
        ExtensionConfig::default(),
        Arc::new(CountingOnceChannel { prompts: Arc::clone(&prompts) }),
    );
    ext.set_host_services(Arc::new(RegistryServices));
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
    (ext, prompts)
}

fn ctx(cwd: &Path) -> HostCtx {
    // A headless event-tier ctx — the exact shape the dispatcher hands `before_tool_call`.
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf())
}

/// The SAME `tool_call` twice means the SAME `call_id` — that is the whole point: pi's cache key is
/// `requestId (= toolCallId) \0 sha256(fingerprint)` (`index.ts:728-737`).
fn read_call(call_id: &str, path: &str) -> HostEvent {
    HostEvent::ToolCall {
        call_id: ToolCallId::from(call_id),
        name: "read".to_string(),
        input: json!({ "path": path }),
    }
}

// ================================================================================================
// (1) SKILL-READ ask surface (pi `index.ts:2282-2292`, `source: "skill_read"`).
// ================================================================================================

#[tokio::test]
async fn reemitted_skill_read_reuses_the_cached_decision_with_no_second_prompt() {
    ensure_subagent_child();
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // The `read` TOOL is denied, so a Noop can ONLY come from the skill-read bypass; the `deploy`
    // SKILL is `ask`, so that bypass has to prompt.
    let (ext, prompts) = ext_with_counting_channel(
        agent_dir,
        r#"{ "tools": { "read": "deny" }, "skills": { "deploy": "ask" } }"#,
    )
    .await;

    let cwd = agent_dir.to_path_buf();
    let skill_file = format!("{}/skills/deploy/SKILL.md", cwd.to_string_lossy());
    // before_agent_start caches the skill-enforcement entries the read gate resolves against
    // (pi `resolveSkillPromptEntries`, `index.ts:2175`).
    let system_prompt = format!(
        "<available_skills>\n  <skill>\n    <name>deploy</name>\n    <description>d</description>\n    <location>{skill_file}</location>\n  </skill>\n</available_skills>"
    );
    let _ = ext
        .on_event(
            &HostEvent::BeforeAgentStart {
                prompt: String::new(),
                images: Value::Null,
                system_prompt,
                options: Value::Null,
                injected: Vec::new(),
            },
            &ctx(&cwd),
        )
        .await;

    let call = read_call("call-skill-1", &skill_file);

    let first = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(first, HookOutcome::Noop),
        "the human allowed the skill read once → it must proceed; got {first:?}"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "the first skill-read ask surfaced one dialog");

    // The IDENTICAL tool_call, re-emitted. pi reuses the cached decision (collapsed to Allow-Once)
    // and renders ZERO additional prompts.
    let second = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(second, HookOutcome::Noop),
        "the reused decision must still allow the read; got {second:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "a re-emitted IDENTICAL skill-read tool_call must reuse the cached decision, \
         never open a second dialog"
    );
}

// ================================================================================================
// (2) EXTERNAL-DIRECTORY ask surface (pi `index.ts:2369-2378`, `source: "tool_call"`).
// ================================================================================================

#[tokio::test]
async fn reemitted_external_directory_read_reuses_the_cached_decision_with_no_second_prompt() {
    ensure_subagent_child();
    let cwd_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let agent_dir = cwd_dir.path();
    // `read` itself is allowed, so the ONLY gate that prompts is the external-directory guard; an
    // approved-Once falls through to the (allowed) main check.
    let (ext, prompts) =
        ext_with_counting_channel(agent_dir, r#"{ "read": "allow", "external_directory": "ask" }"#)
            .await;

    let cwd = agent_dir.to_path_buf();
    let outside_path = outside_dir.path().join("secret.txt").to_string_lossy().into_owned();
    let call = read_call("call-ext-1", &outside_path);

    let first = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(first, HookOutcome::Noop),
        "the human allowed the out-of-workdir read once → it must proceed; got {first:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "the first external-directory ask surfaced one dialog"
    );

    // The IDENTICAL tool_call, re-emitted. "Allow Once" persisted NOTHING (no session rule), so the
    // dedup cache is the only thing that can collapse this — exactly pi's behavior.
    let second = ext.on_event(&call, &ctx(&cwd)).await;
    assert!(
        matches!(second, HookOutcome::Noop),
        "the reused decision must still allow the read; got {second:?}"
    );
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        1,
        "a re-emitted IDENTICAL out-of-workdir tool_call must reuse the cached decision, \
         never open a second dialog"
    );
}
