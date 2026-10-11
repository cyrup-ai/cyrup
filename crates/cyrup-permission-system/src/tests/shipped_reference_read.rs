//! The codemode script reference, which the `codemode` tool description sends the model to `read`,
//! lives under the agent directory, outside the project. Under an armed policy the external-directory
//! guard answered that read with an approval prompt (a block when nobody can answer, `-p`) or, with
//! `external_directory: deny`, a "Hard stop" that told the model not to retry. Pi has no permission
//! system, so the conflict is the delta's own. These tests pin that exactly the shipped page is
//! readable and nothing around it is.
//!
//! The same holds for the files this process spills for the model (`Full output: <path>` of a cut
//! `bash` or `codemode` result, `Image saved to <path>`), which the result tells the model to read
//! and which sit in the temp directory: exactly the recorded paths are readable, nothing near them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension};
use serde_json::{Value, json};

use crate::{AskChannel, ExtensionConfig, ManagerPaths, PermissionSystemExtension};

struct Registry;
impl HostServices for Registry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["read".to_owned(), "write".to_owned()])
    }
}

/// An extension over `policy` in `agent_dir`, headless: an `ask` has nobody to ask and blocks.
async fn extension(agent_dir: &Path, policy: &str) -> PermissionSystemExtension {
    let policy_path = agent_dir.join("cyrup-permissions.jsonc");
    std::fs::write(&policy_path, policy).unwrap();
    let paths = ManagerPaths {
        global_config_path: policy_path,
        agents_dir: agent_dir.join("agents"),
        project_global_config_path: None,
        project_agents_dir: None,
        legacy_global_settings_path: agent_dir.join("settings.json"),
        global_mcp_config_path: agent_dir.join("mcp.json"),
        mcp_server_names_override: None,
    };
    let ext = PermissionSystemExtension::from_parts(
        paths,
        ExtensionConfig::default(),
        Arc::new(crate::NoOpAskChannel) as Arc<dyn AskChannel>,
    );
    ext.set_host_services(Arc::new(Registry));
    ext.init(&mut InitApi::new()).await.unwrap();
    ext
}

async fn call(
    ext: &PermissionSystemExtension,
    cwd: &Path,
    id: &str,
    tool: &str,
    input: Value,
) -> HookOutcome {
    let event = HostEvent::ToolCall {
        call_id: ToolCallId::from(id),
        name: tool.to_owned(),
        input,
    };
    ext.on_event(
        &event,
        &HostCtx::event(ExtMode::Print, false, cwd.to_path_buf()),
    )
    .await
}

fn blocked_for(outcome: &HookOutcome) -> String {
    match outcome {
        HookOutcome::Block { reason, .. } => reason.clone().unwrap_or_default(),
        other => panic!("expected a block, got {other:?}"),
    }
}

#[tokio::test]
async fn the_shipped_codemode_page_is_readable_under_every_external_directory_policy() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let page = agent_dir.path().join("docs").join("codemode.md");
    for external in ["ask", "deny"] {
        let ext = extension(
            agent_dir.path(),
            &json!({ "tools": { "read": "allow" }, "external_directory": external }).to_string(),
        )
        .await;
        let outcome = call(
            &ext,
            project.path(),
            "page",
            "read",
            json!({ "path": page }),
        )
        .await;
        assert!(
            matches!(outcome, HookOutcome::Noop),
            "external_directory {external}: {outcome:?}"
        );
        // Spelled with a `..` segment that lands on the same file, it is the same file.
        let detour = agent_dir
            .path()
            .join("docs")
            .join("..")
            .join("docs")
            .join("codemode.md");
        let outcome = call(
            &ext,
            project.path(),
            "detour",
            "read",
            json!({ "path": detour }),
        )
        .await;
        assert!(
            matches!(outcome, HookOutcome::Noop),
            "external_directory {external}, detour: {outcome:?}"
        );
    }
}

#[tokio::test]
async fn only_the_page_itself_and_only_a_read_of_it_skips_the_external_directory_guard() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let docs = agent_dir.path().join("docs");
    let ext = extension(
        agent_dir.path(),
        &json!({ "tools": { "read": "allow", "write": "allow" }, "external_directory": "deny" })
            .to_string(),
    )
    .await;
    let outside = [
        // Beside the page, under the same directory.
        ("sibling", "read", docs.join("other.md")),
        // The agent directory holds the credentials: a `..` out of docs/ is not the page.
        ("auth", "read", docs.join("..").join("auth.json")),
        ("settings", "read", agent_dir.path().join("settings.json")),
        // The page's name elsewhere is a different file.
        (
            "elsewhere",
            "read",
            project.path().join("..").join("codemode.md"),
        ),
        // Only a read is exempt; changing the page meets the guard.
        ("write", "write", docs.join("codemode.md")),
    ];
    for (id, tool, path) in outside {
        let outcome = call(&ext, project.path(), id, tool, json!({ "path": path })).await;
        let reason = blocked_for(&outcome);
        assert!(
            reason.contains("external directory permission denial"),
            "{id}: {reason}"
        );
    }
}

#[tokio::test]
async fn the_read_policy_still_decides_a_read_of_the_shipped_page() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let ext = extension(
        agent_dir.path(),
        &json!({ "tools": { "read": "deny" }, "external_directory": "allow" }).to_string(),
    )
    .await;
    let outcome = call(
        &ext,
        project.path(),
        "page",
        "read",
        json!({ "path": agent_dir.path().join("docs").join("codemode.md") }),
    )
    .await;
    assert!(
        blocked_for(&outcome).contains("read"),
        "a denied read tool stays denied: {outcome:?}"
    );
}

/// A spill file as the writers leave one: created, then recorded, in a directory outside the
/// project. The name carries the test's tag so no two tests record the same path.
fn spilled(dir: &Path, tag: &str) -> std::path::PathBuf {
    let path = dir.join(format!("pi-codemode-{tag}.txt"));
    std::fs::write(&path, "the full output\n").unwrap();
    cyrup_core::spilled_files::record(&path);
    path
}

#[tokio::test]
async fn a_file_this_process_spilled_is_readable_under_every_external_directory_policy() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let spill_dir = tempfile::tempdir().unwrap();
    let spill = spilled(spill_dir.path(), "readable");
    for external in ["ask", "deny"] {
        let ext = extension(
            agent_dir.path(),
            &json!({ "tools": { "read": "allow" }, "external_directory": external }).to_string(),
        )
        .await;
        let outcome = call(
            &ext,
            project.path(),
            "spill",
            "read",
            json!({ "path": spill }),
        )
        .await;
        assert!(
            matches!(outcome, HookOutcome::Noop),
            "external_directory {external}: {outcome:?}"
        );
    }
}

#[tokio::test]
async fn only_the_recorded_path_and_only_a_read_of_it_skips_the_guard_for_a_spill() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let spill_dir = tempfile::tempdir().unwrap();
    let spill = spilled(spill_dir.path(), "exact");
    // Next to the spill file, with a name that looks like one, but nobody recorded it.
    let lookalike = spill_dir.path().join("pi-codemode-lookalike.txt");
    std::fs::write(&lookalike, "x").unwrap();
    let ext = extension(
        agent_dir.path(),
        &json!({ "tools": { "read": "allow", "write": "allow" }, "external_directory": "deny" })
            .to_string(),
    )
    .await;
    let guarded = [
        ("lookalike", "read", lookalike),
        // Only a read is exempt; changing the spill file meets the guard.
        ("write", "write", spill.clone()),
        // Another directory's file of the same name is not the recorded file.
        (
            "elsewhere",
            "read",
            agent_dir.path().join(spill.file_name().unwrap()),
        ),
    ];
    for (id, tool, path) in guarded {
        let outcome = call(&ext, project.path(), id, tool, json!({ "path": path })).await;
        let reason = blocked_for(&outcome);
        assert!(
            reason.contains("external directory permission denial"),
            "{id}: {reason}"
        );
    }
}

#[tokio::test]
async fn the_read_policy_still_decides_a_read_of_a_spill_file() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let spill_dir = tempfile::tempdir().unwrap();
    let spill = spilled(spill_dir.path(), "read-policy");
    let ext = extension(
        agent_dir.path(),
        &json!({ "tools": { "read": "deny" }, "external_directory": "allow" }).to_string(),
    )
    .await;
    let outcome = call(
        &ext,
        project.path(),
        "spill",
        "read",
        json!({ "path": spill }),
    )
    .await;
    assert!(
        blocked_for(&outcome).contains("read"),
        "a denied read tool stays denied: {outcome:?}"
    );
}

/// The temp directory is shared. If something swaps the spill file for a link after the path was
/// recorded, the read would follow it to a file nobody vouched for.
#[cfg(unix)]
#[tokio::test]
async fn a_recorded_path_that_became_a_link_is_guarded_again() {
    let agent_dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let spill_dir = tempfile::tempdir().unwrap();
    let spill = spilled(spill_dir.path(), "swapped");
    let target = spill_dir.path().join("somewhere-else.txt");
    std::fs::write(&target, "private").unwrap();
    std::fs::remove_file(&spill).unwrap();
    std::os::unix::fs::symlink(&target, &spill).unwrap();
    let ext = extension(
        agent_dir.path(),
        &json!({ "tools": { "read": "allow" }, "external_directory": "deny" }).to_string(),
    )
    .await;
    let outcome = call(
        &ext,
        project.path(),
        "link",
        "read",
        json!({ "path": spill }),
    )
    .await;
    let reason = blocked_for(&outcome);
    assert!(
        reason.contains("external directory permission denial"),
        "{reason}"
    );
}
