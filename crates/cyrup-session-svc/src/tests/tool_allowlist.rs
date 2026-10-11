//! `--tools` / `--exclude-tools` / `--no-tools` bound the tool REGISTRY, not only the initial
//! selection.
//!
//! pi's `_refreshToolRegistry` filters the base (built-in) definitions and every custom tool by
//! `_isAllowedTool` before the registry is built (`agent-session.ts:3497-3536` @v1.0.4), and
//! `setActiveToolsByName` only enables a registered tool (`:1500-1526`). A built-in the flags do
//! not allow is therefore not listed by `getAllTools()`, cannot be switched on by any later
//! `setActiveTools` caller, and is not callable from another tool.
//!
//! cyrup built `registry_tools` from every Availability-visible built-in and applied the flags to
//! the INITIAL selection only, so `--tools read,grep` started with two active tools and any caller
//! of `set_active_tools_by_name` (the permission system does it on every turn) got `bash` back.
//! The end-to-end proof through the permission system and a codemode script is
//! `cyrup::session_launch::tests::tools_allowlist_bounds_the_registry_the_permission_gate_and_codemode_reach`;
//! these tests pin the builder half without those extensions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{Content, Tool, ToolError, ToolResult, ToolUpdateSink};
use cyrup_provider::faux::FauxProvider;
use tempfile::TempDir;

use crate::{AgentSession, NoTools, SessionBuilder, SessionConfig};

/// A custom (SDK) tool with a name of the test's choosing.
struct Named {
    name: &'static str,
    params: serde_json::Value,
}

impl Named {
    fn arc(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            params: serde_json::json!({ "type": "object", "properties": {} }),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Named {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "A custom tool"
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("done")],
            ..Default::default()
        })
    }
}

async fn session_with(configure: impl FnOnce(&mut SessionConfig)) -> (AgentSession, TempDir) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    configure(&mut cfg);
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()), cfg)
        .build()
        .await
        .unwrap();
    (session, tmp)
}

fn registered(session: &AgentSession) -> Vec<String> {
    let mut names: Vec<String> = session.all_tools().into_iter().map(|t| t.name).collect();
    names.sort();
    names
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_string()).collect()
}

#[tokio::test]
async fn an_allowlist_registers_only_the_listed_builtins() {
    let (session, _tmp) = session_with(|cfg| cfg.tools = Some(names(&["read", "grep"]))).await;
    assert_eq!(registered(&session), ["grep", "read"]);

    session
        .set_active_tools_by_name(&names(&[
            "read", "bash", "edit", "write", "grep", "find", "ls",
        ]))
        .await;
    let mut active = session.active_tool_names();
    active.sort();
    assert_eq!(active, ["grep", "read"]);
}

#[tokio::test]
async fn a_denylist_removes_the_builtin_from_the_registry() {
    let (session, _tmp) = session_with(|cfg| cfg.exclude_tools = names(&["bash"])).await;
    let tools = registered(&session);
    assert!(!tools.iter().any(|n| n == "bash"), "{tools:?}");
    assert!(tools.iter().any(|n| n == "read"), "{tools:?}");

    session
        .set_active_tools_by_name(&names(&["bash", "read"]))
        .await;
    assert_eq!(session.active_tool_names(), ["read"]);
}

/// `noTools: "all"` is `allowedToolNames = []` (`sdk.ts:271`): nothing is registered, so nothing
/// can be re-enabled. `noTools: "builtin"` leaves `allowedToolNames` undefined and every built-in
/// ENABLE-able (the carve-out the plan-mode and permission companions rely on).
#[tokio::test]
async fn no_tools_all_registers_nothing_and_builtin_leaves_the_builtins_enableable() {
    let (session, _tmp) = session_with(|cfg| cfg.no_tools = Some(NoTools::All)).await;
    assert!(
        registered(&session).is_empty(),
        "{:?}",
        registered(&session)
    );
    session
        .set_active_tools_by_name(&names(&["read", "bash"]))
        .await;
    assert!(session.active_tool_names().is_empty());

    let (session, _tmp) = session_with(|cfg| cfg.no_tools = Some(NoTools::Builtin)).await;
    assert!(session.active_tool_names().is_empty());
    session
        .set_active_tools_by_name(&names(&["read", "bash"]))
        .await;
    let mut active = session.active_tool_names();
    active.sort();
    assert_eq!(active, ["bash", "read"]);
}

/// pi filters `allCustomTools` by `_isAllowedTool` in the same pass (`:3489-3497`), so an SDK tool
/// the allowlist does not name is not registered either, at build time or through
/// `register_custom_tools` afterwards.
#[tokio::test]
async fn a_custom_tool_outside_the_allowlist_is_not_registered() {
    let (session, _tmp) = session_with(|cfg| {
        cfg.tools = Some(names(&["read", "wanted"]));
        cfg.custom_tools = vec![Named::arc("wanted"), Named::arc("unwanted")];
    })
    .await;
    assert_eq!(registered(&session), ["read", "wanted"]);

    session.register_custom_tools(vec![
        Named::arc("late_unwanted"),
        Named::arc("late_unwanted_2"),
    ]);
    assert_eq!(registered(&session), ["read", "wanted"]);

    let (session, _tmp) = session_with(|cfg| {
        cfg.exclude_tools = names(&["denied"]);
        cfg.custom_tools = vec![Named::arc("kept"), Named::arc("denied")];
    })
    .await;
    let tools = registered(&session);
    assert!(tools.iter().any(|n| n == "kept"), "{tools:?}");
    assert!(!tools.iter().any(|n| n == "denied"), "{tools:?}");
}
