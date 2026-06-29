//! Host-side typed aggregation + first-registration-wins getters + rich native ctx (gap-08 #4/#6/#7).
//! Native built-ins (no wasm) answer `project_trust`/`resources_discover` via `handled(json)`; the
//! facade folds them into typed decisions. Also covers the first-wins tool getter and the rich
//! `HostCtx` fields.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_core::{CancelToken, ExtensionId, Tool, ToolCallId, ToolError, ToolResult};
use cyrup_ext::{
    EventKind, ExtMode, ExtensionHost, HandledValue, HookOutcome, HostConfig, HostCtx, HostCtxRich,
    InitApi, NativeExtension, ProjectTrustDecision,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: std::path::PathBuf::from(".") }
}

/// A native extension that answers `project_trust` and/or `resources_discover` with a fixed payload.
struct DiscoveryExt {
    id: ExtensionId,
    trust: Option<Value>,
    resources: Option<Value>,
}

#[async_trait::async_trait]
impl NativeExtension for DiscoveryExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), cyrup_ext::ExtError> {
        let mut kinds = Vec::new();
        if self.trust.is_some() {
            kinds.push(EventKind::ProjectTrust);
        }
        if self.resources.is_some() {
            kinds.push(EventKind::ResourcesDiscover);
        }
        api.subscribe(&kinds);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEventAlias, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEventAlias::ProjectTrust => match &self.trust {
                Some(v) => HookOutcome::Handled(HandledValue(v.clone())),
                None => HookOutcome::Noop,
            },
            HostEventAlias::ResourcesDiscover => match &self.resources {
                Some(v) => HookOutcome::Handled(HandledValue(v.clone())),
                None => HookOutcome::Noop,
            },
            _ => HookOutcome::Noop,
        }
    }
}

use cyrup_ext::HostEvent as HostEventAlias;

#[tokio::test]
async fn project_trust_first_decision_wins() {
    let host = ExtensionHost::new(cfg());
    // First ext abstains (no `trusted`), second decides trusted+remember, third would also decide.
    host.load_native(Arc::new(DiscoveryExt {
        id: "abstain".into(),
        trust: Some(json!({ "note": "no decision" })),
        resources: None,
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(DiscoveryExt {
        id: "decider".into(),
        trust: Some(json!({ "trusted": true, "remember": true })),
        resources: None,
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(DiscoveryExt {
        id: "late".into(),
        trust: Some(json!({ "trusted": false })),
        resources: None,
    }))
    .await
    .unwrap();

    let decision = host.aggregate_project_trust(&CancelToken::new()).await;
    assert_eq!(
        decision,
        Some(ProjectTrustDecision { trusted: true, remember: true, by: "decider".into() }),
        "the first extension that returns a parseable decision wins"
    );
}

#[tokio::test]
async fn project_trust_no_decision_is_none() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(DiscoveryExt {
        id: "abstain".into(),
        trust: Some(json!({ "note": "no decision" })),
        resources: None,
    }))
    .await
    .unwrap();
    assert_eq!(host.aggregate_project_trust(&CancelToken::new()).await, None);
}

#[tokio::test]
async fn resources_discover_aggregates_union_with_attribution() {
    let host = ExtensionHost::new(cfg());
    host.load_native(Arc::new(DiscoveryExt {
        id: "a".into(),
        trust: None,
        resources: Some(json!({ "skillPaths": ["/s/a"], "themePaths": ["/t/x"] })),
    }))
    .await
    .unwrap();
    host.load_native(Arc::new(DiscoveryExt {
        id: "b".into(),
        trust: None,
        // Overlapping theme path `/t/x` is de-duplicated; new skill + prompt added.
        resources: Some(json!({ "skillPaths": ["/s/b"], "promptPaths": ["/p/b"], "themePaths": ["/t/x"] })),
    }))
    .await
    .unwrap();

    let agg = host.aggregate_resources(&CancelToken::new()).await;
    assert_eq!(agg.skill_paths, vec!["/s/a".to_string(), "/s/b".to_string()]);
    assert_eq!(agg.prompt_paths, vec!["/p/b".to_string()]);
    assert_eq!(agg.theme_paths, vec!["/t/x".to_string()], "duplicate theme path de-duped");
    // Attribution: a contributed (1 skill, 0 prompt, 1 theme); b contributed (1 skill, 1 prompt, 0 new theme).
    assert_eq!(agg.by_extension.len(), 2);
    assert_eq!(agg.by_extension[0], (ExtensionId::from("a"), 1, 0, 1));
    assert_eq!(agg.by_extension[1], (ExtensionId::from("b"), 1, 1, 0));
}

// ---------------------------------------------------------------------------
// getAllRegisteredTools first-registration-wins (gap-08 #7).
// ---------------------------------------------------------------------------
struct NamedTool {
    name: String,
    schema: Value,
}
#[async_trait::async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![], details: None, terminate: false })
    }
}

#[test]
fn all_registered_tool_names_is_first_registration_wins() {
    use cyrup_ext::ExtensionRegistry;
    let reg = ExtensionRegistry::new();
    let schema = json!({ "type": "object" });
    // ext A registers `alpha` then `beta`; ext B overrides `alpha` (last-wins for execution) and
    // adds `gamma`. The GETTER order must stay first-registration-wins: alpha, beta, gamma.
    reg.register_tool("a".into(), Arc::new(NamedTool { name: "alpha".into(), schema: schema.clone() }))
        .unwrap();
    reg.register_tool("a".into(), Arc::new(NamedTool { name: "beta".into(), schema: schema.clone() }))
        .unwrap();
    reg.register_tool("b".into(), Arc::new(NamedTool { name: "alpha".into(), schema: schema.clone() }))
        .unwrap();
    reg.register_tool("b".into(), Arc::new(NamedTool { name: "gamma".into(), schema: schema.clone() }))
        .unwrap();

    let names = reg.all_registered_tool_names().unwrap();
    assert_eq!(
        names,
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        "first-registration-wins order (the alpha override keeps alpha's original position)"
    );
    // tool_info mirrors the same first-wins order + carries source/parameters.
    let info = reg.tool_info().unwrap();
    assert_eq!(info.len(), 3);
    assert_eq!(info[0]["name"], json!("alpha"));
    assert_eq!(info[0]["source"], json!("extension"));
}

// ---------------------------------------------------------------------------
// Rich native HostCtx fields (gap-08 #6).
// ---------------------------------------------------------------------------
#[test]
fn host_ctx_rich_fields() {
    let ctx = HostCtx::command(ExtMode::Tui, true, std::path::PathBuf::from(".")).with_rich(
        HostCtxRich {
            model: Some("claude-x".into()),
            is_idle: true,
            is_project_trusted: true,
            context_usage: Some(json!({ "tokens": 1234 })),
            system_prompt: Some("you are a helpful agent".into()),
        },
    );
    assert_eq!(ctx.model(), Some("claude-x"));
    assert!(ctx.is_idle());
    assert!(ctx.is_project_trusted());
    assert_eq!(ctx.context_usage().and_then(|u| u.get("tokens")), Some(&json!(1234)));
    assert_eq!(ctx.system_prompt(), Some("you are a helpful agent"));
    // A default-constructed ctx carries empty rich fields.
    let plain = HostCtx::event(ExtMode::Tui, true, std::path::PathBuf::from("."));
    assert_eq!(plain.model(), None);
    assert!(!plain.is_project_trusted());
}
