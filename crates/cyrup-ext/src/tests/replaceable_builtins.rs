//! Replaceable built-in extensions: pi's `omitReplacedExtensions`
//! (`packages/coding-agent/src/core/resource-loader.ts:116-153` @v1.0.1) and the
//! `{ name, factory, replaceable: true, builtin: true }` entries of `builtInExtensions`
//! (`src/extensions/index.ts:9-14`).
//!
//! Upstream's rule: a replaceable extension is left out of the loaded set when any tool, command or
//! flag name it registered is also registered by an extension that is NOT replaceable; two
//! replaceable extensions never leave each other out. The tests drive the real seams, in both load
//! orders (cyrup loads natives first, then guests; pi's order differs and the outcome must not):
//! [`ExtensionHost::load_native`] for natives and [`ExtensionRegistry::register_guest_tool`], the
//! function the guest `registration.register-tool` import calls, for a guest.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cyrup_agent::AgentEvent;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use serde_json::{Value, json};

use crate::{
    ClaimKind, CommandDescriptor, EventKind, ExtError, ExtMode, ExtensionConflict, ExtensionHost,
    HookOutcome, HostConfig, HostCtx, HostEvent, InitApi, NativeExtension, OmittedExtension,
    ToolDescriptor,
};

fn host() -> ExtensionHost {
    ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: std::path::PathBuf::from("."),
    })
}

/// A tool whose output names its owner, so "which implementation ran" is observable.
struct Marker {
    name: String,
    marker: String,
    schema: Value,
}

#[async_trait::async_trait]
impl Tool for Marker {
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
            content: vec![Content::text(self.marker.clone())],
            ..ToolResult::default()
        })
    }
}

/// What an extension registers in `init`, and whether it declares itself replaceable.
#[derive(Clone, Default)]
struct Plan {
    replaceable: bool,
    tools: Vec<&'static str>,
    commands: Vec<&'static str>,
    flags: Vec<&'static str>,
}

struct Ext {
    id: &'static str,
    plan: Plan,
    /// How many times `init` ran.
    inits: Arc<AtomicUsize>,
    /// Every `agent_start` this extension was handed.
    events: Arc<AtomicUsize>,
}

impl Ext {
    fn new(id: &'static str, plan: Plan) -> Arc<Self> {
        Arc::new(Self {
            id,
            plan,
            inits: Arc::new(AtomicUsize::new(0)),
            events: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[async_trait::async_trait]
impl NativeExtension for Ext {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(self.id)
    }
    fn replaceable(&self) -> bool {
        self.plan.replaceable
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        self.inits.fetch_add(1, Ordering::SeqCst);
        api.subscribe(&[EventKind::AgentStart]);
        for tool in &self.plan.tools {
            api.register_tool(Arc::new(Marker {
                name: (*tool).to_owned(),
                marker: format!("{} ran {tool}", self.id),
                schema: json!({ "type": "object" }),
            }));
        }
        for command in &self.plan.commands {
            api.register_command(
                *command,
                CommandDescriptor {
                    description: format!("{} owns {command}", self.id),
                    completions: Vec::new(),
                },
            );
        }
        for flag in &self.plan.flags {
            api.register_flag(*flag, json!({ "type": "boolean", "default": false }));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        self.events.fetch_add(1, Ordering::SeqCst);
        HookOutcome::Noop
    }
}

fn builtin(tools: &[&'static str], commands: &[&'static str], flags: &[&'static str]) -> Plan {
    Plan {
        replaceable: true,
        tools: tools.to_vec(),
        commands: commands.to_vec(),
        flags: flags.to_vec(),
    }
}

fn ordinary(tools: &[&'static str], commands: &[&'static str], flags: &[&'static str]) -> Plan {
    Plan {
        replaceable: false,
        ..builtin(tools, commands, flags)
    }
}

async fn output(host: &ExtensionHost, name: &str) -> String {
    let tool = host
        .registry()
        .tool(name)
        .unwrap()
        .unwrap_or_else(|| panic!("`{name}` is not registered"));
    let result = tool
        .execute(
            ToolCallId::from("call-1"),
            json!({}),
            CancelToken::new(),
            Box::new(|_| {}) as ToolUpdateSink,
        )
        .await
        .unwrap();
    result
        .content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

async fn agent_start(host: &ExtensionHost) {
    host.subscriber()
        .on_event(&AgentEvent::AgentStart, CancelToken::new())
        .await;
}

fn ids(host: &ExtensionHost) -> Vec<String> {
    host.loaded_ids().iter().map(ToString::to_string).collect()
}

fn omitted(extension: &str, kind: ClaimKind, name: &str, by: &str) -> OmittedExtension {
    OmittedExtension {
        extension: ExtensionId::from(extension),
        kind,
        name: name.to_owned(),
        by: ExtensionId::from(by),
    }
}

// --------------------------------------------------------------------------------------- tools --

/// An extension that registers `codemode` after the built-in took over cleanly: the built-in is
/// not loaded, the other extension's tool is the one registered and callable, and nothing is
/// reported as a collision.
#[tokio::test]
async fn an_extension_registering_the_name_replaces_the_builtin_loaded_before_it() {
    let host = host();
    let builtin_ext = Ext::new("codemode", builtin(&["codemode"], &[], &[]));
    host.load_native(builtin_ext.clone()).await.unwrap();
    assert_eq!(output(&host, "codemode").await, "codemode ran codemode");

    let mine = Ext::new("mine", ordinary(&["codemode"], &[], &[]));
    host.load_native(mine.clone()).await.unwrap();

    assert_eq!(output(&host, "codemode").await, "mine ran codemode");
    assert_eq!(ids(&host), ["mine"]);
    assert_eq!(
        host.dispatcher().len(),
        1,
        "the built-in's handlers are gone"
    );
    assert_eq!(
        host.omitted_extensions(),
        [omitted("codemode", ClaimKind::Tool, "codemode", "mine")]
    );
    assert_eq!(host.extension_conflicts(), Vec::<ExtensionConflict>::new());
    // The active set hands the agent the replacement, once.
    let active = host.active_tools(&[]).unwrap();
    assert_eq!(active.iter().filter(|t| t.name() == "codemode").count(), 1);
    // The factory ran, as upstream's does; its handlers are not called.
    assert_eq!(builtin_ext.inits.load(Ordering::SeqCst), 1);
    agent_start(&host).await;
    assert_eq!(builtin_ext.events.load(Ordering::SeqCst), 0);
    assert_eq!(mine.events.load(Ordering::SeqCst), 1);
}

/// The other load order: the extension is already loaded when the built-in registers the same name.
#[tokio::test]
async fn a_builtin_loaded_after_an_extension_that_registered_its_name_is_left_out() {
    let host = host();
    let mine = Ext::new("mine", ordinary(&["codemode"], &[], &[]));
    host.load_native(mine.clone()).await.unwrap();
    let builtin_ext = Ext::new("codemode", builtin(&["codemode", "extra"], &[], &[]));
    host.load_native(builtin_ext.clone()).await.unwrap();

    assert_eq!(output(&host, "codemode").await, "mine ran codemode");
    assert!(
        host.registry().tool("extra").unwrap().is_none(),
        "a built-in that is left out registers nothing at all"
    );
    assert_eq!(ids(&host), ["mine"]);
    assert_eq!(host.dispatcher().len(), 1);
    assert_eq!(
        host.omitted_extensions(),
        [omitted("codemode", ClaimKind::Tool, "codemode", "mine")]
    );
    assert_eq!(host.extension_conflicts(), Vec::<ExtensionConflict>::new());
    assert_eq!(builtin_ext.inits.load(Ordering::SeqCst), 1);
    agent_start(&host).await;
    assert_eq!(builtin_ext.events.load(Ordering::SeqCst), 0);
}

/// An extension registering an unrelated tool leaves the built-in alone: both load.
#[tokio::test]
async fn an_extension_registering_an_unrelated_tool_leaves_the_builtin_loaded() {
    for builtin_first in [true, false] {
        let host = host();
        let builtin_ext = Ext::new("codemode", builtin(&["codemode"], &["cm"], &["cm-flag"]));
        let mine = Ext::new(
            "mine",
            ordinary(&["other"], &["other-cmd"], &["other-flag"]),
        );
        if builtin_first {
            host.load_native(builtin_ext.clone()).await.unwrap();
            host.load_native(mine.clone()).await.unwrap();
        } else {
            host.load_native(mine.clone()).await.unwrap();
            host.load_native(builtin_ext.clone()).await.unwrap();
        }
        assert_eq!(output(&host, "codemode").await, "codemode ran codemode");
        assert_eq!(output(&host, "other").await, "mine ran other");
        assert_eq!(host.dispatcher().len(), 2);
        assert_eq!(host.loaded_ids().len(), 2);
        assert!(host.omitted_extensions().is_empty());
        assert!(host.extension_conflicts().is_empty());
        agent_start(&host).await;
        assert_eq!(builtin_ext.events.load(Ordering::SeqCst), 1);
        assert_eq!(mine.events.load(Ordering::SeqCst), 1);
    }
}

/// A guest registers its tool through `ExtensionRegistry::register_guest_tool`, the function the
/// `registration.register-tool` import calls. The built-in is left out the same way, and its
/// handlers go before the next tool refresh hands the agent the active set.
#[tokio::test]
async fn a_guest_descriptor_with_the_name_replaces_the_builtin() {
    let host = host();
    let builtin_ext = Ext::new("codemode", builtin(&["codemode"], &[], &[]));
    host.load_native(builtin_ext.clone()).await.unwrap();

    host.registry()
        .register_guest_tool(
            "guest".into(),
            ToolDescriptor {
                prepare_arguments: false,
                render_shell: None,
                constrained_sampling: None,
                exposure: cyrup_core::ToolExposure::Direct,
                namespace: None,
                default_active: true,
                output_schema: None,
                annotations: None,
                prepare_loadout: false,
                name: "codemode".to_owned(),
                label: "Codemode".to_owned(),
                description: "the guest's own".to_owned(),
                parameters: json!({ "type": "object" }),
                execution_mode: None,
                prompt_snippet: None,
                prompt_guidelines: Vec::new(),
                has_renderer: false,
            },
        )
        .unwrap();

    assert!(host.registry().has_guest_tool("codemode").unwrap());
    assert!(
        host.registry().tool("codemode").unwrap().is_none(),
        "the built-in's executable tool is gone"
    );
    assert_eq!(
        host.omitted_extensions(),
        [omitted("codemode", ClaimKind::Tool, "codemode", "guest")]
    );
    assert!(host.extension_conflicts().is_empty());
    // The late door: the next refresh drops the built-in's handlers and id.
    assert!(host.refresh_tools().unwrap());
    assert_eq!(host.dispatcher().len(), 0);
    assert!(ids(&host).is_empty());
    agent_start(&host).await;
    assert_eq!(builtin_ext.events.load(Ordering::SeqCst), 0);
}

// ----------------------------------------------------------------------- commands and flags ----

/// Upstream's namespaces are tools, commands and flags: `/mcp` replaces the built-in MCP support.
#[tokio::test]
async fn a_command_of_the_same_name_replaces_the_builtin() {
    let host = host();
    host.load_native(Ext::new("mcp", builtin(&["mcp_tool"], &["mcp"], &[])))
        .await
        .unwrap();
    host.load_native(Ext::new("third-party-mcp", ordinary(&[], &["mcp"], &[])))
        .await
        .unwrap();
    assert_eq!(ids(&host), ["third-party-mcp"]);
    assert!(host.registry().tool("mcp_tool").unwrap().is_none());
    assert_eq!(
        host.registry().command_owner("mcp").unwrap(),
        Some(ExtensionId::from("third-party-mcp"))
    );
    assert_eq!(
        host.omitted_extensions(),
        [omitted("mcp", ClaimKind::Command, "mcp", "third-party-mcp")]
    );
    assert!(
        host.omitted_extensions()[0]
            .warning()
            .contains("registers command `/mcp`, so built-in extension `mcp` was not loaded")
    );
}

/// The third namespace: a flag.
#[tokio::test]
async fn a_flag_of_the_same_name_replaces_the_builtin() {
    let host = host();
    host.load_native(Ext::new("mcp", builtin(&[], &[], &["mcp-servers"])))
        .await
        .unwrap();
    host.load_native(Ext::new("flagger", ordinary(&[], &[], &["mcp-servers"])))
        .await
        .unwrap();
    assert_eq!(ids(&host), ["flagger"]);
    assert_eq!(
        host.omitted_extensions(),
        [omitted("mcp", ClaimKind::Flag, "mcp-servers", "flagger")]
    );
    assert!(host.extension_conflicts().is_empty());
}

/// A command shared by two ordinary extensions is not a collision (`name:N` suffixes), and neither
/// is one shared with a built-in that is NOT replaceable.
#[tokio::test]
async fn names_shared_without_a_replaceable_extension_are_not_replacements() {
    let host = host();
    host.load_native(Ext::new("a", ordinary(&[], &["deploy"], &[])))
        .await
        .unwrap();
    host.load_native(Ext::new("b", ordinary(&[], &["deploy"], &[])))
        .await
        .unwrap();
    assert_eq!(ids(&host), ["a", "b"]);
    assert!(host.omitted_extensions().is_empty());
}

// ------------------------------------------------------------------------------ the whole rule --

/// Upstream drops the extension, not the one name: every registration of the built-in goes.
#[tokio::test]
async fn a_builtin_left_out_registers_nothing_else() {
    let host = host();
    host.load_native(Ext::new(
        "tool-search",
        builtin(&["tool_search", "codemode"], &["ts"], &["ts-flag"]),
    ))
    .await
    .unwrap();
    host.load_native(Ext::new("mine", ordinary(&["codemode"], &[], &[])))
        .await
        .unwrap();
    assert!(host.registry().tool("tool_search").unwrap().is_none());
    assert!(!host.registry().has_command("ts").unwrap());
    assert!(host.registry().get_flag("ts-flag").unwrap().is_none());
    assert!(host.registry().flag_declarations().unwrap().is_empty());
}

/// `taken` is built from the NON-replaceable extensions alone, so two replaceable built-ins that
/// share a name are both kept and meet the ordinary first-wins conflict.
#[tokio::test]
async fn two_replaceable_builtins_do_not_replace_each_other() {
    let host = host();
    host.load_native(Ext::new("one", builtin(&["shared"], &[], &[])))
        .await
        .unwrap();
    host.load_native(Ext::new("two", builtin(&["shared"], &[], &[])))
        .await
        .unwrap();
    assert_eq!(ids(&host), ["one", "two"]);
    assert!(host.omitted_extensions().is_empty());
    assert_eq!(
        host.extension_conflicts(),
        [ExtensionConflict {
            path: "two".into(),
            message: "Tool \"shared\" conflicts with one".to_owned(),
        }]
    );
    assert_eq!(output(&host, "shared").await, "one ran shared");
}

/// A replaceable built-in is a built-in only by what it declares: a native that does not say so
/// keeps today's first-wins rule and its fatal conflict record.
#[tokio::test]
async fn an_extension_that_is_not_replaceable_still_conflicts() {
    let host = host();
    host.load_native(Ext::new("first", ordinary(&["shared"], &[], &[])))
        .await
        .unwrap();
    host.load_native(Ext::new("second", ordinary(&["shared"], &[], &[])))
        .await
        .unwrap();
    assert_eq!(ids(&host), ["first", "second"]);
    assert_eq!(host.extension_conflicts().len(), 1);
    assert!(host.omitted_extensions().is_empty());
}

/// The left-out built-in's record keeps the id usable: a later extension may take it, as a name
/// never loaded.
#[tokio::test]
async fn the_id_of_a_left_out_builtin_is_free_again() {
    let host = host();
    host.load_native(Ext::new("codemode", builtin(&["codemode"], &[], &[])))
        .await
        .unwrap();
    host.load_native(Ext::new("mine", ordinary(&["codemode"], &[], &[])))
        .await
        .unwrap();
    assert!(!ids(&host).contains(&"codemode".to_owned()));
    host.load_native(Ext::new("codemode", ordinary(&["fresh"], &[], &[])))
        .await
        .unwrap();
    assert_eq!(output(&host, "fresh").await, "codemode ran fresh");
}
