//! Backlog #28 — extension tool/flag NAME COLLISIONS: detection, and the precedence that resolves
//! them.
//!
//! Ground truth, read at the ported tag `pi v0.83.0` (`git -C pi show v0.83.0:<path>`):
//!
//! * `packages/coding-agent/src/core/extensions/runner.ts:450-460 getAllRegisteredTools()` —
//!   `for (const ext of this.extensions) { for (const tool of ext.tools.values()) { if
//!   (!toolsByName.has(tool.definition.name)) toolsByName.set(...) } }`. Load order, **first hit
//!   wins**. This is not merely a listing: `agent-session.ts:2463-2487 _refreshToolRegistry` builds
//!   `allCustomTools` from exactly this array and writes it into the definition registry, so the
//!   first-registered extension supplies the definition that RUNS.
//! * `runner.ts:463-471 getToolDefinition(name)` — same load-order loop, `return` on first hit.
//! * `runner.ts:473-483 getFlags()` — `if (!allFlags.has(name))`, same rule for flags.
//! * `packages/coding-agent/src/core/resource-loader.ts:1059-1094 detectExtensionConflicts()` —
//!   walks the loaded extensions tracking `toolOwners`/`flagOwners` and emits
//!   `{path, message: 'Tool "<n>" conflicts with <owner>'}` /
//!   `{path, message: 'Flag "--<n>" conflicts with <owner>'}` for every later claimant.
//! * `resource-loader.ts:625-632 addExtensionConflictDiagnostics()` — pushes those onto
//!   `extensionsResult.errors`, with the comment "Keep all extensions loaded. Conflicts are reported
//!   as diagnostics, and precedence is handled by load order."
//! * `main.ts:735-738` maps every `getExtensions().errors` entry to
//!   `{type:"error", message: 'Failed to load extension "<path>": <error>'}` and `main.ts:843-848`
//!   exits 1 on any error diagnostic — so a collision is FATAL upstream.
//!
//! cyrup before this fix: `grep -i conflict crates/cyrup-ext/src` returned zero hits, and
//! `registry.rs` documented its tool map as "last insert wins", so the SECOND extension to claim a
//! name both silently displaced the first AND produced no diagnostic — precedence inverted vs pi.
//!
//! Everything here drives the real seams: `ExtensionHost::load_native` (how every cyrup built-in —
//! subagents / intercom / permission-system — registers its tools) and
//! `ExtensionHost::discover_and_load` (the session builder's disk-extension pass,
//! `cyrup-session-svc/src/builder.rs:927`, whose `errors` become `StartupDiagnostics::extensions`
//! and then `AgentSessionRuntime::diagnostics`). `ExtensionRegistry::register_flag` is the function
//! the guest `registration.register-flag` import calls (`host/live.rs:107`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{
    ExtError, ExtMode, ExtensionConflict, ExtensionHost, HookOutcome, HostConfig, HostCtx,
    HostEvent, InitApi, NativeExtension,
};
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: std::path::PathBuf::from("."),
    }
}

/// A tool whose `execute` echoes a caller-chosen marker, so "which implementation ran" is an
/// observable rather than an inference from a pointer identity.
struct MarkerTool {
    name: String,
    marker: String,
    schema: Value,
}

impl MarkerTool {
    fn new(name: &str, marker: &str) -> Self {
        Self {
            name: name.to_string(),
            marker: marker.to_string(),
            schema: json!({ "type": "object" }),
        }
    }
}

#[async_trait::async_trait]
impl Tool for MarkerTool {
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
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::Text {
                text: self.marker.clone().into(),
                text_signature: None,
            }],
            ..Default::default()
        })
    }
}

/// A native built-in that registers `(name, marker)` tools — the same shape
/// `cyrup-ext-subagents` / `cyrup-intercom` use.
struct ToolExt {
    id: ExtensionId,
    tools: Vec<(String, String)>,
}

impl ToolExt {
    fn loaded(id: &str, tools: &[(&str, &str)]) -> Arc<dyn NativeExtension> {
        Arc::new(Self {
            id: id.into(),
            tools: tools
                .iter()
                .map(|(n, m)| ((*n).to_string(), (*m).to_string()))
                .collect(),
        })
    }
}

#[async_trait::async_trait]
impl NativeExtension for ToolExt {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for (name, marker) in &self.tools {
            api.register_tool(Arc::new(MarkerTool::new(name, marker)));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// Run a tool and return the text it emitted.
async fn run(tool: &Arc<dyn Tool>) -> String {
    let out = tool
        .execute(
            ToolCallId::from("call-1"),
            json!({}),
            CancelToken::new(),
            Box::new(|_| {}) as ToolUpdateSink,
        )
        .await
        .expect("tool executed");
    out.content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// The headline: precedence. pi runner.ts:450-471 — FIRST extension in load order wins.
// ---------------------------------------------------------------------------

/// Two extensions claim `shared`. The one loaded FIRST must be the one the agent executes
/// (`getAllRegisteredTools` feeds `_refreshToolRegistry`, so list order and execution agree).
#[tokio::test]
async fn first_loaded_extension_wins_the_tool_name_and_is_the_one_that_executes() {
    let host = ExtensionHost::new(cfg());
    host.load_native(ToolExt::loaded("alpha", &[("shared", "alpha-ran")]))
        .await
        .unwrap();
    host.load_native(ToolExt::loaded("beta", &[("shared", "beta-ran")]))
        .await
        .unwrap();

    // The execution seam: `ExtensionRegistry::tool(name)` is what `ExtensionHost` resolves a call
    // through (pi `getToolDefinition`, runner.ts:463-471).
    let resolved = host
        .registry()
        .tool("shared")
        .unwrap()
        .expect("`shared` is registered");
    assert_eq!(
        run(&resolved).await,
        "alpha-ran",
        "pi runner.ts:463-471 returns the FIRST extension's tool in load order; cyrup's last-wins \
         map ran beta's instead"
    );

    // And the agent-visible active set carries that same single winner.
    let active = host.active_tools(&[]).unwrap();
    let shared: Vec<&Arc<dyn Tool>> = active.iter().filter(|t| t.name() == "shared").collect();
    assert_eq!(shared.len(), 1, "one name, one tool");
    assert_eq!(
        run(shared[0]).await,
        "alpha-ran",
        "the merged active set must hand the agent the first-registered implementation"
    );
}

/// Same rule for a guest (WASM) descriptor claiming a name a native already owns: the descriptor is
/// rejected, so no `WasmTool` is ever materialized over the top of the live tool.
#[test]
fn a_guest_descriptor_cannot_displace_an_already_owned_tool_name() {
    use crate::{ExtensionRegistry, ToolDescriptor};
    let reg = ExtensionRegistry::new();
    reg.register_tool(
        "alpha".into(),
        Arc::new(MarkerTool::new("shared", "alpha-ran")),
    )
    .unwrap();
    reg.register_guest_tool(
        "beta".into(),
        ToolDescriptor {
            prepare_arguments: false,
            render_shell: None,
            constrained_sampling: None,
            name: "shared".to_string(),
            label: "Shared".to_string(),
            description: "guest".to_string(),
            parameters: json!({ "type": "object" }),
            execution_mode: None,
            prompt_snippet: None,
            prompt_guidelines: vec![],
            has_renderer: false,
        },
    )
    .unwrap();

    assert!(
        !reg.has_guest_tool("shared").unwrap(),
        "pi has ONE tools namespace per extension list; a later extension's descriptor must not \
         take a name the first extension already owns"
    );
    assert_eq!(
        reg.conflicts().unwrap(),
        vec![ExtensionConflict {
            path: "beta".into(),
            message: "Tool \"shared\" conflicts with alpha".to_string(),
        }]
    );
}

/// pi's flag rule, `runner.ts:473-483 getFlags()`: `if (!allFlags.has(name))` — first extension's
/// spec is the one the CLI reconciles `--flag` against. `register_flag` is the function the guest
/// `registration.register-flag` import calls (host/live.rs:107).
#[test]
fn first_extension_wins_a_flag_name() {
    let host = ExtensionHost::new(cfg());
    let reg = host.registry();
    reg.register_flag(
        "alpha".into(),
        "persona",
        json!({ "type": "string", "owner": "alpha" }),
    )
    .unwrap();
    reg.register_flag(
        "beta".into(),
        "persona",
        json!({ "type": "boolean", "owner": "beta" }),
    )
    .unwrap();

    assert_eq!(
        reg.get_flag("persona").unwrap(),
        Some(json!({ "type": "string", "owner": "alpha" })),
        "pi runner.ts:477 keeps the FIRST extension's flag spec"
    );
    assert_eq!(
        host.extension_conflicts(),
        vec![ExtensionConflict {
            path: "beta".into(),
            // pi resource-loader.ts:1085 prefixes the flag name with `--`.
            message: "Flag \"--persona\" conflicts with alpha".to_string(),
        }]
    );
}

// ---------------------------------------------------------------------------
// Detection: pi resource-loader.ts:1059-1094 + :625-632 — conflicts become load `errors`.
// ---------------------------------------------------------------------------

/// The diagnostic itself, from the native-load path every cyrup built-in uses.
#[tokio::test]
async fn a_tool_collision_produces_pis_conflict_diagnostic() {
    let host = ExtensionHost::new(cfg());
    host.load_native(ToolExt::loaded("alpha", &[("shared", "alpha-ran")]))
        .await
        .unwrap();
    host.load_native(ToolExt::loaded(
        "beta",
        &[("shared", "beta-ran"), ("beta-only", "b")],
    ))
    .await
    .unwrap();

    assert_eq!(
        host.extension_conflicts(),
        vec![ExtensionConflict {
            path: "beta".into(),
            message: "Tool \"shared\" conflicts with alpha".to_string(),
        }],
        "pi resource-loader.ts:1067-1077 records ONE entry, on the losing extension, naming the \
         owner — and only for the colliding name"
    );
}

/// The production surfacing seam. `discover_and_load` is what `cyrup-session-svc/src/builder.rs:927`
/// calls; its `errors` become `StartupDiagnostics::extensions` and then, via
/// `AgentSessionRuntime::diagnostics`, the `Failed to load extension "<path>": <err>` error the bin
/// exits 1 on. pi folds conflicts into that exact array (`addExtensionConflictDiagnostics`,
/// resource-loader.ts:625-632), so a collision must reach it with no caller opting in.
#[cfg(feature = "wasm-host")]
#[tokio::test]
async fn discover_and_load_reports_conflicts_in_its_errors_array() {
    use crate::DiscoveryRoots;
    use crate::host::{DenyServices, HostServices};

    let host = ExtensionHost::new(cfg());
    host.load_native(ToolExt::loaded("alpha", &[("shared", "alpha-ran")]))
        .await
        .unwrap();
    host.load_native(ToolExt::loaded("beta", &[("shared", "beta-ran")]))
        .await
        .unwrap();

    // An empty root set: nothing on disk to discover, so every error below is a CONFLICT error.
    let roots = DiscoveryRoots {
        project_cwd: None,
        agent_dir: None,
        configured: vec![],
        disabled: vec![],
    };
    let services: Arc<dyn HostServices> = Arc::new(DenyServices);
    let result = host.discover_and_load(&roots, true, services).await;

    assert!(
        result.loaded.is_empty(),
        "no disk extensions in this fixture"
    );
    let messages: Vec<(String, String, bool)> = result
        .errors
        .iter()
        .map(|e| (e.path.display().to_string(), e.error.clone(), e.fatal))
        .collect();
    assert_eq!(
        messages,
        vec![(
            "beta".to_string(),
            "Tool \"shared\" conflicts with alpha".to_string(),
            // pi's bin exits 1 on it (main.ts:843-848), so it is a FATAL diagnostic, not the
            // project-trust skip class.
            true
        )],
        "conflicts must ride the same `errors` array pi uses (resource-loader.ts:625-632)"
    );
}

// ---------------------------------------------------------------------------
// MIRROR CASES — these must stay GREEN when the fix is reverted. They pin the behavior the fix must
// NOT change, so the failures above cannot be dismissed as "the assertions are vacuous".
// ---------------------------------------------------------------------------

/// MIRROR: distinct names from two extensions both survive and both execute. Nothing about
/// precedence is in play, so this passes with last-wins and with first-wins alike.
#[tokio::test]
async fn mirror_distinct_tool_names_from_two_extensions_all_resolve() {
    let host = ExtensionHost::new(cfg());
    host.load_native(ToolExt::loaded("alpha", &[("a-tool", "alpha-ran")]))
        .await
        .unwrap();
    host.load_native(ToolExt::loaded("beta", &[("b-tool", "beta-ran")]))
        .await
        .unwrap();

    assert_eq!(
        run(&host.registry().tool("a-tool").unwrap().unwrap()).await,
        "alpha-ran"
    );
    assert_eq!(
        run(&host.registry().tool("b-tool").unwrap().unwrap()).await,
        "beta-ran"
    );
    assert_eq!(
        host.registry().all_registered_tool_names().unwrap(),
        vec!["a-tool".to_string(), "b-tool".to_string()]
    );
    assert!(
        host.extension_conflicts().is_empty(),
        "no name is claimed twice, so pi's detectExtensionConflicts emits nothing"
    );
}

/// MIRROR: within ONE extension, re-registering a name still REPLACES — pi's per-extension map is a
/// plain `extension.tools.set(tool.name, …)` (loader.ts:245-252), and the same-owner re-registration
/// is exactly how the guest-descriptor→`WasmTool` materialization pass and hot-reload behave. This
/// passes under last-wins too, so it holds the "same owner overwrites" half fixed while the
/// cross-extension half moves.
#[tokio::test]
async fn mirror_same_extension_re_registering_a_name_still_overwrites() {
    let host = ExtensionHost::new(cfg());
    host.load_native(ToolExt::loaded(
        "alpha",
        &[("shared", "first"), ("shared", "second")],
    ))
    .await
    .unwrap();

    assert_eq!(
        run(&host.registry().tool("shared").unwrap().unwrap()).await,
        "second",
        "one extension overwriting its OWN tool is pi's Map.set, not a conflict"
    );
    assert_eq!(
        host.registry().all_registered_tool_names().unwrap(),
        vec!["shared".to_string()],
        "the overwrite keeps the original position (one entry, not two)"
    );
    assert!(
        host.extension_conflicts().is_empty(),
        "pi's detectExtensionConflicts skips `existingOwner === ext.path` (resource-loader.ts:1069)"
    );
}

/// MIRROR: the owner-less host-side `set_flag` (what `apply_extension_flag_values`' own tests drive)
/// is untouched — it still writes unconditionally and raises no conflict.
#[test]
fn mirror_ownerless_set_flag_is_unchanged() {
    let host = ExtensionHost::new(cfg());
    host.registry()
        .set_flag("persona", json!({ "type": "string" }))
        .unwrap();
    host.registry()
        .set_flag("persona", json!({ "type": "boolean" }))
        .unwrap();
    assert_eq!(
        host.registry().get_flag("persona").unwrap(),
        Some(json!({ "type": "boolean" }))
    );
    assert!(host.extension_conflicts().is_empty());
}
