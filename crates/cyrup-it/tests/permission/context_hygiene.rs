//! FULLY-WIRED PROOF of the `before_agent_start` context-hygiene layer (pi `index.ts:2134-2190`, port
//! doc §9). Each test drives a real `PermissionSystemExtension` (built through its production
//! constructor) at the SAME `on_event(BeforeAgentStart)` entry point the dispatcher drives at runtime,
//! with a live [`HostServices`] backend that records `set_active_tools` / `set_status` and exposes the
//! full registry via `all_tool_names`. Mirrors pi's own before-agent-start tests:
//!
//! - **system-prompt MUTATE**: a denied tool's "Available tools:" section + guideline bullet are
//!   stripped from the returned (mutated) system prompt.
//! - **active-tools shaping**: `setActiveTools` is called with a set that EXCLUDES the denied tool.
//! - **skill hiding + enforcement**: a `deny` skill is hidden from `<available_skills>` in the mutated
//!   prompt, yet its enforcement entry still BLOCKS a skill-read at `tool_call`.
//! - **yolo status pill**: set to `"yolo"` at session start when `yoloMode`, cleared at shutdown.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_core::ToolCallId;
use cyrup_ext::{
    EventPatch, ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_permission_system::{PermissionSystemExtension, EXTENSION_ID};
use serde_json::{json, Value};

/// Recorded fire-and-forget effects the shaping seam drives on the live backend.
#[derive(Default)]
struct Recorded {
    /// Each `set_active_tools(names)` call, in order (the `setActiveTools` analog).
    active_tools: Vec<Vec<String>>,
    /// Each `set_status(key, text)` call, in order (the yolo pill).
    statuses: Vec<(String, Option<String>)>,
}

/// A live [`HostServices`] backend exposing the full registry (`all_tool_names`) and RECORDING the
/// `set_active_tools` + `set_status` effects — the exact seams the shaping drives.
struct ShapingServices {
    all_tools: Vec<String>,
    rec: Arc<Mutex<Recorded>>,
}

impl HostServices for ShapingServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.all_tools.clone())
    }
    fn set_active_tools(&self, names: &[String]) {
        self.rec.lock().unwrap_or_else(|e| e.into_inner()).active_tools.push(names.to_vec());
    }
    fn set_status(&self, key: &str, text: Option<&str>) {
        self.rec
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .statuses
            .push((key.to_string(), text.map(str::to_string)));
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

async fn init(ext: &PermissionSystemExtension) {
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
}

fn ctx(cwd: &Path) -> HostCtx {
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf())
}

fn before_agent_start(system_prompt: &str) -> HostEvent {
    HostEvent::BeforeAgentStart {
        prompt: String::new(),
        images: Value::Null,
        system_prompt: system_prompt.to_string(),
        options: Value::Null,
        injected: Vec::new(),
    }
}

fn tool_call(name: &str, input: Value) -> HostEvent {
    HostEvent::ToolCall { call_id: ToolCallId::from("call-1"), name: name.to_string(), input }
}

fn mutate_system_prompt(o: &HookOutcome) -> Option<&str> {
    match o {
        HookOutcome::Mutate(EventPatch::SystemPromptAndInject { system, .. }) => system.as_deref(),
        _ => None,
    }
}

fn block_reason(o: &HookOutcome) -> Option<&str> {
    match o {
        HookOutcome::Block { reason } => reason.as_deref(),
        _ => None,
    }
}

/// Build an installed extension pointed at `agent_dir` (its own global policy), with a recording
/// backend exposing `all_tools`. Returns the extension + the shared recording state.
fn ext_with(
    agent_dir: &Path,
    global: &str,
    all_tools: &[&str],
) -> (PermissionSystemExtension, Arc<Mutex<Recorded>>) {
    write(&agent_dir.join("cyrup-permissions.jsonc"), global);
    let ext = PermissionSystemExtension::new(agent_dir.to_path_buf(), agent_dir.to_path_buf());
    let rec = Arc::new(Mutex::new(Recorded::default()));
    ext.set_host_services(Arc::new(ShapingServices {
        all_tools: all_tools.iter().map(|s| s.to_string()).collect(),
        rec: rec.clone(),
    }));
    (ext, rec)
}

// ================================================================================================
// (1) system-prompt MUTATE + active-tools shaping — a denied tool is stripped from the prompt AND
//     excluded from the active tool set.
// ================================================================================================

#[tokio::test]
async fn denied_tool_is_stripped_from_prompt_and_active_tool_set() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // `write` is denied at the tool level; `read`/`bash` are default-ask (exposed).
    let (ext, rec) = ext_with(agent_dir, r#"{ "tools": { "write": "deny" } }"#, &["read", "write", "bash"]);
    init(&ext).await;

    let system_prompt = "You are a test assistant.\n\nAvailable tools:\n- read\n- write\n- bash\n\nGuidelines:\n- use read to examine files instead of cat or sed.\n- use write only for new files or complete rewrites\n\nEnd:\nfin";
    let out = ext.on_event(&before_agent_start(system_prompt), &ctx(agent_dir)).await;

    // The handler RETURNED the sanitized prompt as a [mutate].
    let mutated = mutate_system_prompt(&out)
        .unwrap_or_else(|| panic!("before_agent_start must return a system-prompt mutate; got {out:?}"));

    // The whole "Available tools:" section is gone, and the denied `write` guideline bullet is stripped.
    assert!(!mutated.contains("Available tools:"), "tools section stripped:\n{mutated}");
    assert!(
        !mutated.contains("use write only for new files"),
        "the denied `write` guideline bullet must be stripped:\n{mutated}"
    );
    // The allowed `read` guideline survives; unrelated content is preserved.
    assert!(mutated.contains("use read to examine files"), "allowed guideline kept:\n{mutated}");
    assert!(mutated.contains("End:") && mutated.contains("You are a test assistant."));

    // `setActiveTools` was driven with a set that EXCLUDES the denied `write` and KEEPS read + bash.
    let rec = rec.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(rec.active_tools.len(), 1, "setActiveTools called exactly once");
    let active = &rec.active_tools[0];
    assert!(active.contains(&"read".to_string()) && active.contains(&"bash".to_string()));
    assert!(
        !active.contains(&"write".to_string()),
        "the active tool set must exclude the denied `write`; got {active:?}"
    );
}

// ================================================================================================
// (2) skill HIDING + ENFORCEMENT — a deny skill is hidden from <available_skills> in the mutated
//     prompt, yet its enforcement entry still BLOCKS a read of its file.
// ================================================================================================

#[tokio::test]
async fn deny_skill_hidden_from_prompt_but_still_gates_skill_read() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // `read` is allowed; the `secret` SKILL is denied. Both `deploy` (allow) and `secret` (deny) appear.
    let (ext, _rec) = ext_with(
        agent_dir,
        r#"{ "tools": { "read": "allow" }, "skills": { "deploy": "allow", "secret": "deny" } }"#,
        &["read"],
    );
    init(&ext).await;

    let cwd_str = agent_dir.to_string_lossy().into_owned();
    let deploy_file = format!("{cwd_str}/skills/deploy/SKILL.md");
    let secret_file = format!("{cwd_str}/skills/secret/SKILL.md");
    let system_prompt = format!(
        "intro\n<available_skills>\n  <skill>\n    <name>deploy</name>\n    <description>d</description>\n    <location>{deploy_file}</location>\n  </skill>\n  <skill>\n    <name>secret</name>\n    <description>s</description>\n    <location>{secret_file}</location>\n  </skill>\n</available_skills>\noutro"
    );

    let out = ext.on_event(&before_agent_start(&system_prompt), &ctx(agent_dir)).await;
    let mutated = mutate_system_prompt(&out)
        .unwrap_or_else(|| panic!("skill hiding must produce a mutate; got {out:?}"));

    // The denied skill is HIDDEN from the advertised list; the allowed one stays.
    assert!(mutated.contains("<name>deploy</name>"), "allowed skill advertised:\n{mutated}");
    assert!(
        !mutated.contains("<name>secret</name>"),
        "the deny skill must be hidden from <available_skills>:\n{mutated}"
    );

    // ...but its ENFORCEMENT entry survived the hiding: a read of the secret skill file is BLOCKED
    // (skill deny), even though the `read` TOOL is allowed.
    let read_secret = ext.on_event(&tool_call("read", json!({ "path": secret_file })), &ctx(agent_dir)).await;
    assert!(
        block_reason(&read_secret).is_some_and(|r| r.contains("not permitted to access this skill")),
        "the hidden deny-skill's enforcement entry must still block its read; got {read_secret:?}"
    );

    // The ALLOWED skill's file is still readable (proves we did not over-block).
    let read_deploy = ext.on_event(&tool_call("read", json!({ "path": deploy_file })), &ctx(agent_dir)).await;
    assert!(
        matches!(read_deploy, HookOutcome::Noop),
        "the allowed skill's file must remain readable; got {read_deploy:?}"
    );
}

// ================================================================================================
// (3) yolo STATUS pill — set to "yolo" at session start under yoloMode, cleared at shutdown.
// ================================================================================================

#[tokio::test]
async fn yolo_status_pill_set_on_start_and_cleared_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // Enable yolo via the extension config.json (the same source the live gate reads).
    write(&agent_dir.join("cyrup-permission-system/config.json"), r#"{ "yoloMode": true }"#);
    let (ext, rec) = ext_with(agent_dir, "{}", &["read"]);
    init(&ext).await;

    // Session start → the pill is set to "yolo".
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "new".into() }, &ctx(agent_dir)).await;
    {
        let rec = rec.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            rec.statuses.last(),
            Some(&(EXTENSION_ID.to_string(), Some("yolo".to_string()))),
            "yolo pill set on session start; statuses: {:?}",
            rec.statuses
        );
    }

    // Session shutdown → the pill is cleared (None).
    let _ = ext
        .on_event(&HostEvent::SessionShutdown { reason: "quit".into() }, &ctx(agent_dir))
        .await;
    {
        let rec = rec.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            rec.statuses.last(),
            Some(&(EXTENSION_ID.to_string(), None)),
            "yolo pill cleared on shutdown; statuses: {:?}",
            rec.statuses
        );
    }
}

// ================================================================================================
// (4) NO yolo → no pill value (the pill reflects the real config, never a fabricated status).
// ================================================================================================

#[tokio::test]
async fn no_yolo_config_syncs_a_cleared_pill() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // No yolo config → the pill syncs to None (cleared), never "yolo".
    let (ext, rec) = ext_with(agent_dir, "{}", &["read"]);
    init(&ext).await;

    let _ = ext.on_event(&HostEvent::SessionStart { reason: "new".into() }, &ctx(agent_dir)).await;
    let rec = rec.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        rec.statuses.last(),
        Some(&(EXTENSION_ID.to_string(), None)),
        "without yolo the pill is cleared, not set; statuses: {:?}",
        rec.statuses
    );
}
