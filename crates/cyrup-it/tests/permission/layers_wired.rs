//! FULLY-WIRED PROOF that the four supplementary policy LAYERS the pi `tool_call` handler runs (pi
//! `index.ts:2208-2499`) actually ENFORCE through the registered `before_tool_call` gate
//! (`NativeExtension::on_event(ToolCall)`) — the SAME entry point the dispatcher drives at runtime.
//! Each test drives a real `PermissionSystemExtension` (built through its production constructors)
//! with a real `HostEvent` + `HostCtx`, mirroring pi's own handler tests:
//!
//! - **agent + projectAgent layers** (pi `resolveAgentName`, `index.ts:2033-2047,2417`): an
//!   agent-scoped deny rule blocks the NAMED persona and does NOT block a different persona.
//! - **registry / unknown-tool block** (pi `index.ts:2218-2228`): a tool absent from the full
//!   registry (`HostServices::all_tool_names`) is blocked before any permission check.
//! - **skill-read bypass** (pi `index.ts:2230-2303`): a `read` whose path lands on an allowed skill
//!   proceeds even though the `read` TOOL is denied.
//! - **external-directory guard** (pi `index.ts:2310-2414`): a `read` targeting a path OUTSIDE the
//!   working directory is blocked by the `external_directory` policy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use cyrup_permission_system::PermissionSystemExtension;
use serde_json::{json, Value};

/// A scripted [`HostServices`] whose ONLY override is [`HostServices::all_tool_names`] — the full
/// registry the registry / unknown-tool gate checks against (pi `pi.getAllTools()`).
struct RegistryServices {
    names: Vec<String>,
}
impl HostServices for RegistryServices {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.names.clone())
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// Build an installed extension pointed at `agent_dir` with `global` as its global policy, and `cwd`
/// as the session working directory (used verbatim by the gate's `HostCtx`).
fn ext_with_global(agent_dir: &Path, global: &str) -> PermissionSystemExtension {
    write(&agent_dir.join("cyrup-permissions.jsonc"), global);
    PermissionSystemExtension::new(agent_dir.to_path_buf(), agent_dir.to_path_buf())
}

async fn init(ext: &PermissionSystemExtension) {
    let mut api = InitApi::new();
    ext.init(&mut api).await.unwrap();
}

fn ctx(cwd: PathBuf) -> HostCtx {
    // A headless event-tier ctx — the exact shape the dispatcher hands `before_tool_call`.
    HostCtx::event(ExtMode::Print, false, cwd)
}

fn tool_call(name: &str, input: Value) -> HostEvent {
    HostEvent::ToolCall { call_id: ToolCallId::from("call-1"), name: name.to_string(), input }
}

fn block_reason(o: &HookOutcome) -> Option<&str> {
    match o {
        HookOutcome::Block { reason } => reason.as_deref(),
        _ => None,
    }
}

// ================================================================================================
// (1) agent + projectAgent LAYERS — a named-persona deny rule enforces for that persona only.
// ================================================================================================

#[tokio::test]
async fn agent_scoped_rule_enforces_for_named_agent_only() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // Global allows every bash command; the `coder` persona's OWN frontmatter denies `secret *`.
    write(&agent_dir.join("agents/coder.md"), "---\npermission:\n  bash:\n    \"secret *\": deny\n---\nbody");

    let cwd = agent_dir.to_path_buf();
    let ev = tool_call("bash", json!({ "command": "secret leak" }));

    // Named persona `coder`: the agent-layer deny ENFORCES.
    let coder = ext_with_global(agent_dir, r#"{ "bash": { "*": "allow" } }"#)
        .with_agent_name(Some("coder".to_string()));
    coder.set_host_services(Arc::new(RegistryServices { names: vec!["bash".into()] }));
    init(&coder).await;
    let coder_out = coder.on_event(&ev, &ctx(cwd.clone())).await;
    assert!(
        block_reason(&coder_out).is_some_and(|r| r.contains("Agent 'coder'")
            && r.contains("is not permitted to run 'bash'")),
        "agent-scoped deny must block the named persona; got {coder_out:?}"
    );

    // A DIFFERENT persona `writer` (no `writer.md`): the agent layer is empty → global allow wins.
    let writer = ext_with_global(agent_dir, r#"{ "bash": { "*": "allow" } }"#)
        .with_agent_name(Some("writer".to_string()));
    writer.set_host_services(Arc::new(RegistryServices { names: vec!["bash".into()] }));
    init(&writer).await;
    let writer_out = writer.on_event(&ev, &ctx(cwd)).await;
    assert!(
        matches!(writer_out, HookOutcome::Noop),
        "the same command must NOT be blocked for a different persona; got {writer_out:?}"
    );
}

// ================================================================================================
// (2) REGISTRY / unknown-tool block — a tool absent from the full registry is blocked pre-policy.
// ================================================================================================

#[tokio::test]
async fn unknown_tool_is_blocked_before_permission_check() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // Even a permissive global policy cannot save an UNREGISTERED tool — the registry gate runs first.
    let ext = ext_with_global(agent_dir, r#"{ "defaultPolicy": { "tools": "allow", "bash": "allow" } }"#);
    ext.set_host_services(Arc::new(RegistryServices { names: vec!["bash".into(), "read".into()] }));
    init(&ext).await;
    let cwd = agent_dir.to_path_buf();

    // An unregistered tool → blocked with the unknown-tool reason (pi `formatUnknownToolReason`).
    let unknown = ext.on_event(&tool_call("frobnicate", json!({})), &ctx(cwd.clone())).await;
    assert!(
        block_reason(&unknown)
            .is_some_and(|r| r.contains("Tool 'frobnicate' is not registered in this runtime")),
        "an unregistered tool must be blocked before permission checks; got {unknown:?}"
    );

    // A registered tool (`bash`) sails past the registry gate (allowed by the permissive policy).
    let known = ext.on_event(&tool_call("bash", json!({ "command": "ls" })), &ctx(cwd)).await;
    assert!(
        matches!(known, HookOutcome::Noop),
        "a registered tool must pass the registry gate; got {known:?}"
    );
}

// ================================================================================================
// (3) SKILL-READ bypass — an allowed skill's file is readable even when the `read` tool is denied.
// ================================================================================================

#[tokio::test]
async fn allowed_skill_read_bypasses_read_tool_deny() {
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path();
    // The `read` TOOL is denied, but the `deploy` SKILL is allowed.
    let ext = ext_with_global(
        agent_dir,
        r#"{ "tools": { "read": "deny" }, "skills": { "deploy": "allow" } }"#,
    );
    ext.set_host_services(Arc::new(RegistryServices { names: vec!["read".into()] }));
    init(&ext).await;
    let cwd = agent_dir.to_path_buf();
    let cwd_str = cwd.to_string_lossy().into_owned();
    let skill_file = format!("{cwd_str}/skills/deploy/SKILL.md");

    // before_agent_start: the companion parses the `<available_skills>` block + resolves each state.
    let system_prompt = format!(
        "<available_skills>\n  <skill>\n    <name>deploy</name>\n    <description>d</description>\n    <location>{skill_file}</location>\n  </skill>\n</available_skills>"
    );
    let bas = HostEvent::BeforeAgentStart {
        prompt: String::new(),
        images: Value::Null,
        system_prompt,
        options: Value::Null,
        injected: Vec::new(),
    };
    let _ = ext.on_event(&bas, &ctx(cwd.clone())).await;

    // Reading the allowed skill's file PROCEEDS despite the read-tool deny (the bypass).
    let skill_read = ext.on_event(&tool_call("read", json!({ "path": skill_file })), &ctx(cwd.clone())).await;
    assert!(
        matches!(skill_read, HookOutcome::Noop),
        "an allowed skill's file must be readable despite the read-tool deny; got {skill_read:?}"
    );

    // A NON-skill read is still governed by the (denied) read tool.
    let other = format!("{cwd_str}/notes/other.txt");
    let plain_read = ext.on_event(&tool_call("read", json!({ "path": other })), &ctx(cwd)).await;
    assert!(
        block_reason(&plain_read).is_some_and(|r| r.contains("is not permitted to run 'read'")),
        "a non-skill read must still hit the read-tool deny; got {plain_read:?}"
    );
}

// ================================================================================================
// (4) EXTERNAL-DIRECTORY guard — a read outside the working directory is blocked.
// ================================================================================================

#[tokio::test]
async fn read_outside_working_directory_is_guarded() {
    let cwd_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let agent_dir = cwd_dir.path();
    // `read` is allowed generally, but external-directory access is denied.
    let ext = ext_with_global(agent_dir, r#"{ "read": "allow", "external_directory": "deny" }"#);
    ext.set_host_services(Arc::new(RegistryServices { names: vec!["read".into()] }));
    init(&ext).await;
    let cwd = agent_dir.to_path_buf();

    // A path OUTSIDE the working directory → blocked by the external-directory guard.
    let outside_path = outside_dir.path().join("secret.txt").to_string_lossy().into_owned();
    let out = ext.on_event(&tool_call("read", json!({ "path": outside_path })), &ctx(cwd.clone())).await;
    assert!(
        block_reason(&out).is_some_and(|r| r.contains("outside working directory")),
        "a read outside the working directory must be guarded; got {out:?}"
    );

    // A path INSIDE the working directory → the guard does not fire; the (allowed) read proceeds.
    let inside_path = cwd.join("inside.txt").to_string_lossy().into_owned();
    let inside = ext.on_event(&tool_call("read", json!({ "path": inside_path })), &ctx(cwd)).await;
    assert!(
        matches!(inside, HookOutcome::Noop),
        "a read inside the working directory must proceed; got {inside:?}"
    );
}

#[tokio::test]
async fn non_builtin_filesystem_tool_outside_working_directory_is_guarded() {
    // pi `isLikelyFilesystemToolName` parity: a NON-builtin filesystem-like tool name (`read_file`,
    // not in the 6-name PATH_BEARING_TOOLS set, no `edits` key) targeting a path outside the working
    // directory must STILL be caught by the external-directory guard. Before the heuristic was
    // wired, `get_path_bearing_tool_path` returned `None` for such a name and the whole guard was
    // skipped, letting the path reach OUTSIDE the working directory ungated.
    let cwd_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let agent_dir = cwd_dir.path();
    let ext = ext_with_global(agent_dir, r#"{ "read_file": "allow", "external_directory": "deny" }"#);
    ext.set_host_services(Arc::new(RegistryServices { names: vec!["read_file".into()] }));
    init(&ext).await;
    let cwd = agent_dir.to_path_buf();

    let outside_path = outside_dir.path().join("secret.txt").to_string_lossy().into_owned();
    let out = ext.on_event(&tool_call("read_file", json!({ "path": outside_path })), &ctx(cwd)).await;
    assert!(
        block_reason(&out).is_some_and(|r| r.contains("outside working directory")),
        "a non-builtin FS tool (read_file) outside the working directory must be guarded; got {out:?}"
    );
}
