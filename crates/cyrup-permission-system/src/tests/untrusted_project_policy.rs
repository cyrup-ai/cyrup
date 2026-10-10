//! A project the user has not trusted may tighten the permission policy and may not loosen it.
//!
//! The policy engine's own "untrusted" layers could always add denies, but upstream lets them turn an
//! `ask` into an `allow` as well, which let any repository approve its own `bash` calls for whoever
//! ran `cyrup -p` in it. The host says whether the project is trusted on every event's context
//! (`ctx.isProjectTrusted`); these tests drive the extension with both answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use cyrup_core::ToolCallId;
use cyrup_ext::{
    ExtMode, HookOutcome, HostCtx, HostCtxRich, HostEvent, HostServices, InitApi, NativeExtension,
};
use serde_json::json;

use crate::{AskChannel, ExtensionConfig, ManagerPaths, PermissionSystemExtension};

struct Registry;
impl HostServices for Registry {
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(vec!["bash".to_owned()])
    }
}

/// The project the session runs in.
fn project_dir(dir: &Path) -> std::path::PathBuf {
    dir.join("work")
}

/// An extension over a global policy that asks about every `bash` call and a project policy that
/// allows `echo`, headless.
async fn extension(dir: &Path) -> PermissionSystemExtension {
    let global = dir.join("cyrup-permissions.jsonc");
    std::fs::write(&global, r#"{ "bash": { "*": "ask" } }"#).unwrap();
    // Where a session's rebuild of the manager looks for it, so it is still there afterwards.
    let project = project_dir(dir).join(".cyrup/agent/cyrup-permissions.jsonc");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    std::fs::write(&project, r#"{ "bash": { "echo *": "allow" } }"#).unwrap();
    let ext = PermissionSystemExtension::from_parts(
        ManagerPaths {
            global_config_path: global,
            agents_dir: dir.join("agents"),
            project_global_config_path: Some(project),
            project_agents_dir: None,
            legacy_global_settings_path: dir.join("settings.json"),
            global_mcp_config_path: dir.join("mcp.json"),
            mcp_server_names_override: Some(Vec::new()),
        },
        ExtensionConfig::default(),
        Arc::new(crate::NoOpAskChannel) as Arc<dyn AskChannel>,
    );
    ext.set_host_services(Arc::new(Registry));
    ext.init(&mut InitApi::new()).await.unwrap();
    ext
}

fn ctx(cwd: &Path, project_trusted: bool) -> HostCtx {
    HostCtx::event(ExtMode::Print, false, cwd.to_path_buf()).with_rich(HostCtxRich {
        is_project_trusted: project_trusted,
        ..HostCtxRich::default()
    })
}

async fn echo(ext: &PermissionSystemExtension, ctx: &HostCtx) -> HookOutcome {
    ext.on_event(
        &HostEvent::ToolCall {
            call_id: ToolCallId::from("c1"),
            name: "bash".to_owned(),
            input: json!({ "command": "echo hi" }),
        },
        ctx,
    )
    .await
}

#[tokio::test]
async fn the_project_policys_allow_applies_only_while_the_host_says_the_project_is_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let ext = extension(dir.path()).await;

    let work = project_dir(dir.path());
    let trusted = echo(&ext, &ctx(&work, true)).await;
    assert!(matches!(trusted, HookOutcome::Noop), "{trusted:?}");

    let untrusted = echo(&ext, &ctx(&work, false)).await;
    match untrusted {
        HookOutcome::Block { reason, .. } => assert!(
            reason
                .unwrap_or_default()
                .contains("requires approval, but no interactive UI is available"),
            "the user's ask stands, and with nobody to ask it blocks"
        ),
        other => panic!("an untrusted project approved its own call: {other:?}"),
    }

    // Trusting the project later (`/trust`, then a reload) is seen by the very next call.
    let trusted_again = echo(&ext, &ctx(&work, true)).await;
    assert!(
        matches!(trusted_again, HookOutcome::Noop),
        "{trusted_again:?}"
    );
}
