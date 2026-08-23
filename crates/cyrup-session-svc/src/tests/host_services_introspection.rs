//! EXT-037 / EXT-038 — the guest INTROSPECTION reads: `getSystemPromptOptions()` over the live
//! bag, and `getAllTools()` over the whole merged registry (with and without an attached
//! [`crate::host_services::SessionCatalog`] to supply extension provenance).
//!
//! One of the five files the inline `mod tests` in `host_services.rs` became when that file was
//! split into `src/host_services/`; this is the section its `EXT-037 / EXT-038: guest
//! introspection` banner opened. Shares [`super::host_services_core::svc_with`] with its siblings.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, Tool};
use cyrup_ext::host::HostServices;
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use serde_json::{json, Value};

use crate::host_services::SessionCatalog;
use crate::tools::DynamicToolState;

use super::host_services_core::svc_with;

/// A tool double carrying the two fields pi's `ToolInfo` reads off the definition and cyrup's
/// internal [`crate::tools::ToolInfo`] does not: `description` + `promptGuidelines`.
struct CatalogTool {
    name: &'static str,
    params: Value,
    guidelines: Vec<&'static str>,
}

impl CatalogTool {
    fn new(name: &'static str, guidelines: Vec<&'static str>) -> Self {
        Self { name, params: json!({"type": "object", "properties": {}}), guidelines }
    }
}

#[async_trait::async_trait]
impl Tool for CatalogTool {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "described"
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.guidelines.clone()
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult::default())
    }
}

/// A [`SessionCatalog`] double standing in for the live `AgentSession` (which a unit test here
/// cannot build): pi's three-source command concatenation, plus the extension registry's
/// per-tool `sourceInfo`. `pub(super)` because `host_services_oauth.rs` — the other half of the
/// same original `mod tests` — drives `commands()` through it too.
pub(super) struct FakeCatalog;

impl SessionCatalog for FakeCatalog {
    fn commands(&self) -> Vec<Value> {
        vec![
            json!({"name": "deploy", "description": "first", "source": "extension"}),
            json!({"name": "deploy:2", "description": "second", "source": "extension"}),
            json!({"name": "review", "description": "a template", "source": "prompt"}),
            json!({"name": "skill:pdf", "description": "a skill", "source": "skill"}),
        ]
    }

    fn extension_tool_source_info(&self) -> std::collections::HashMap<String, Value> {
        std::collections::HashMap::from([(
            "ext_tool".to_string(),
            json!({"path": "demo-ext", "source": "demo-ext", "scope": "temporary", "origin": "top-level"}),
        )])
    }
}

fn dynamic_tools_with(tools: Vec<Arc<dyn Tool>>) -> Arc<Mutex<DynamicToolState>> {
    let contributions = tools
        .iter()
        .map(|t| (t.name().to_string(), crate::builder::tool_contribution(t)))
        .collect();
    let rebuilder = crate::tools::PromptRebuilder::new(
        cyrup_session::prompt::PromptInputs::default(),
        contributions,
    );
    Arc::new(Mutex::new(DynamicToolState::new(tools.clone(), tools, rebuilder)))
}

/// EXT-061 — `system_prompt_options()` is the BAG behind `system_prompt()`, in pi's
/// `BuildSystemPromptOptions` shape (`core/system-prompt.ts:8-25` @v0.83.0), sourced from the
/// SAME `PromptRebuilder` the next prompt rebuild consumes (pi's `_baseSystemPromptOptions`,
/// `core/agent-session.ts:1044-1053`, handed back at `:2436`).
///
/// COVERAGE, NOT A REGRESSION PROOF (rule 8): the trait method is new this pass, so no form of
/// this test could have failed against the previous HEAD. What it pins is the two properties a
/// later edit can quietly break — that the unattached backend answers `None` (which is what
/// routes the WIT import to pi's `{cwd}` default instead of a fabricated bag), and that the
/// attached one reports the LIVE active set rather than the cleared `selected_tools` the
/// rebuild base carries.
#[test]
fn system_prompt_options_reports_the_live_bag_behind_the_system_prompt() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // Unattached ⇒ `None`. The import layer, not this backend, supplies pi's `{cwd}` default.
    assert!(svc.system_prompt_options().is_none(), "no dynamic-tool view attached ⇒ no live bag");

    let read: Arc<dyn Tool> = Arc::new(CatalogTool::new("read", vec!["read: prefer read"]));
    let bash: Arc<dyn Tool> = Arc::new(CatalogTool::new("bash", vec![]));
    svc.attach_dynamic_tools(dynamic_tools_with(vec![read, bash]));

    let bag = svc.system_prompt_options().expect("a live dynamic-tool view answers");
    assert_eq!(
        bag["selectedTools"],
        json!(["read", "bash"]),
        "pi's bag carries `selectedTools: validToolNames` — the ACTIVE set, not the rebuild \
         base's cleared field: {bag}"
    );
    assert!(bag.get("cwd").is_some(), "`cwd` is the one REQUIRED key of pi's bag: {bag}");
    assert_eq!(
        bag["promptGuidelines"],
        json!(["read: prefer read"]),
        "each active tool's guidelines, in active order (agent-session.ts:1031-1034): {bag}"
    );
    // pi omits `customPrompt`/`appendSystemPrompt` when unset rather than emitting null.
    assert!(bag.get("customPrompt").is_none(), "an unset optional is OMITTED, not null: {bag}");
    assert!(bag.get("appendSystemPrompt").is_none(), "an unset optional is OMITTED, not null: {bag}");
}

/// EXT-038 — `all_tools()` must report the WHOLE merged registry (built-ins included) in pi's
/// `ToolInfo` shape, not the extension-only view `registry.tool_info()` gives. Guards the
/// functional half: a plan-mode extension reads this before calling `setActiveTools`, and the
/// write IS honoured, so an extension-only read silently strips read/write/edit/bash.
#[test]
fn all_tools_reports_the_whole_merged_registry_in_pis_toolinfo_shape() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);

    // Unattached: `None`, which is what keeps the cyrup-ext registry fallback reachable.
    assert!(svc.all_tools().is_none(), "no dynamic-tool view attached ⇒ no live answer");

    let builtin: Arc<dyn Tool> = Arc::new(CatalogTool::new("read", vec!["read: prefer read"]));
    let ext: Arc<dyn Tool> = Arc::new(CatalogTool::new("ext_tool", vec![]));
    svc.attach_dynamic_tools(dynamic_tools_with(vec![builtin, ext]));
    svc.attach_session_catalog(Arc::new(FakeCatalog));

    let rows = svc.all_tools().expect("a live dynamic-tool view answers");
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"read"), "the BUILT-IN must appear — the whole point of EXT-038: {names:?}");
    assert!(names.contains(&"ext_tool"), "the extension tool must still appear: {names:?}");

    let read = rows.iter().find(|r| r["name"] == json!("read")).expect("read row");
    // pi's `ToolInfo` is EXACTLY these five keys (`extensions/types.ts:1552-1554` @v0.83.0) —
    // no `source` discriminator (EXT-060).
    let keys: Vec<&str> = read.as_object().expect("object").keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["description", "name", "parameters", "promptGuidelines", "sourceInfo"],
        "pi's ToolInfo keys and no others"
    );
    assert_eq!(read["description"], json!("described"));
    assert_eq!(read["promptGuidelines"], json!(["read: prefer read"]), "guidelines must survive");
    assert_eq!(
        read["sourceInfo"],
        json!({"path": "<builtin:read>", "source": "builtin", "scope": "temporary", "origin": "top-level"}),
        "a tool the extension registry does not own gets pi's synthetic builtin SourceInfo"
    );

    let ext_row = rows.iter().find(|r| r["name"] == json!("ext_tool")).expect("ext row");
    assert_eq!(
        ext_row["sourceInfo"]["source"],
        json!("demo-ext"),
        "an extension-contributed tool keeps the REGISTRY's sourceInfo, not the builtin synthetic"
    );
}

/// EXT-038 — with no catalog attached the merged set is still reported (the built-ins are what
/// matter); only the extension provenance degrades to the synthetic form.
#[test]
fn all_tools_without_a_catalog_still_reports_builtins() {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let svc = svc_with(provider);
    let builtin: Arc<dyn Tool> = Arc::new(CatalogTool::new("bash", vec![]));
    svc.attach_dynamic_tools(dynamic_tools_with(vec![builtin]));

    let rows = svc.all_tools().expect("a live dynamic-tool view answers");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], json!("bash"));
    assert_eq!(rows[0]["sourceInfo"]["source"], json!("builtin"));
}

