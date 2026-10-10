//! SUBA-139 / SUBA-153 — port of pi `test/unit/tool-activation.test.ts` @v0.76.1, against a fake
//! host whose `setActiveTools` keeps only registered names (upstream's harness, `:41-44`).
//!
//! Upstream's harness activates every tool on registration, the loader included. cyrup's loader is
//! `default_active() == false`, so the fake starts with the loader registered but inactive, which
//! is what a live cyrup session hands the `session_start` handler.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_core::{Tool, ToolDef, ToolReference};
use cyrup_ext::HostEvent;
use cyrup_ext::host::HostServices;
use cyrup_ext::native::{ExtMode, HostCtx, InitApi, NativeExtension};
use serde_json::{Value, json};

use super::*;
use crate::extension::SubagentsExtension;
use crate::registration::SubagentExtensionConfig;

// =================================================================================================
// Fake host
// =================================================================================================

struct FakeHost {
    registered: Vec<String>,
    active: Mutex<Vec<String>>,
    messages: Mutex<Vec<cyrup_session::AgentMessage>>,
    model: Mutex<Option<Value>>,
}

impl FakeHost {
    /// Upstream's runtime: `read` plus the extension's tools, `excluded` filtered out of both the
    /// registry and the active set (`createRuntime(messages, excluded, …)`).
    fn new(messages: Vec<cyrup_session::AgentMessage>, excluded: &[&str]) -> Arc<Self> {
        let keep = |name: &&&str| !excluded.contains(*name);
        let registered: Vec<String> = [
            "read",
            "subagent",
            "bg_wait",
            "subagent_supervisor",
            LOADER_NAME,
        ]
        .iter()
        .filter(keep)
        .map(|name| (*name).to_string())
        .collect();
        let active = ["read", "subagent", "bg_wait", "subagent_supervisor"]
            .iter()
            .filter(keep)
            .map(|name| (*name).to_string())
            .collect();
        Arc::new(Self {
            registered,
            active: Mutex::new(active),
            messages: Mutex::new(messages),
            model: Mutex::new(None),
        })
    }

    fn with_model(self: Arc<Self>, model: Option<Value>) -> Arc<Self> {
        *self.model.lock().unwrap() = model;
        self
    }

    fn active(&self) -> Vec<String> {
        self.active.lock().unwrap().clone()
    }

    fn has(&self, name: &str) -> bool {
        self.active().iter().any(|n| n == name)
    }

    fn select(&self, names: &[&str]) {
        *self.active.lock().unwrap() = names.iter().map(|n| (*n).to_string()).collect();
    }

    fn services(self: &Arc<Self>) -> Option<Arc<dyn HostServices>> {
        Some(Arc::clone(self) as Arc<dyn HostServices>)
    }
}

impl HostServices for FakeHost {
    fn active_tools(&self) -> Option<Vec<String>> {
        Some(self.active())
    }
    fn all_tool_names(&self) -> Option<Vec<String>> {
        Some(self.registered.clone())
    }
    fn set_active_tools(&self, names: &[String]) {
        let mut next: Vec<String> = Vec::new();
        for name in names {
            if self.registered.contains(name) && !next.contains(name) {
                next.push(name.clone());
            }
        }
        *self.active.lock().unwrap() = next;
    }
    fn session_context_messages(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<cyrup_session::AgentMessage>> + Send + '_>,
    > {
        let messages = self.messages.lock().unwrap().clone();
        Box::pin(async move { messages })
    }
    fn current_model_info(&self) -> Option<Value> {
        self.model.lock().unwrap().clone()
    }
}

// =================================================================================================
// Fixtures (upstream `:210-218`)
// =================================================================================================

/// pi `declared(added, removed)`.
fn declared(added: &[&str], removed: &[&str]) -> cyrup_session::AgentMessage {
    cyrup_session::AgentMessage::Core(cyrup_core::Message::System(cyrup_core::SystemMessage {
        tools_added: added
            .iter()
            .map(|name| ToolDef {
                name: (*name).to_string(),
                description: (*name).to_string(),
                parameters: json!({ "type": "object" }),
                ..ToolDef::default()
            })
            .collect(),
        tools_removed: removed
            .iter()
            .map(|name| ToolReference::new(*name))
            .collect(),
        timestamp: 1,
        ..cyrup_core::SystemMessage::default()
    }))
}

/// Upstream's legacy history: a user message and no tool-selection record.
fn legacy() -> cyrup_session::AgentMessage {
    cyrup_session::AgentMessage::Core(cyrup_core::Message::User {
        content: vec![cyrup_core::Content::text("continue")],
        timestamp: 1,
    })
}

/// pi `model(api, compat)`.
fn model(api: &str, compat: Option<Value>) -> Option<Value> {
    let mut model = json!({ "id": "m", "provider": "p", "api": api });
    if let Some(compat) = compat {
        model["compat"] = compat;
    }
    Some(model)
}

fn compatible() -> Option<Value> {
    model(
        "anthropic-messages",
        Some(
            json!({ "supportsMidConvoSystemMessages": true, "supportsMidConvoToolChanges": true }),
        ),
    )
}

fn incompatible() -> Option<Value> {
    model(
        "openai-completions",
        Some(json!({ "supportsMidConvoToolAdditions": true })),
    )
}

/// pi `startAgent(runtime)` (`:220-227`): emit `before_agent_start` with the active set as
/// `selectedTools` and return the list after the handler.
fn start_agent(state: &ToolActivationState, host: &Arc<FakeHost>) -> Vec<String> {
    let options = json!({ "selectedTools": host.active() });
    let after = state
        .on_before_agent_start(host.services(), &options)
        .unwrap_or(options);
    after["selectedTools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_string())
        .collect()
}

fn registered(mode: ToolActivationMode) -> ToolActivationState {
    let state = ToolActivationState::default();
    state.loader_registered(mode);
    state
}

/// Upstream's 14-row predicate table (`test :234-249`), verbatim.
fn upstream_predicate_table() -> Vec<(Option<Value>, bool)> {
    let system = || json!({ "supportsMidConvoSystemMessages": true });
    let with = |extra: Value| {
        let mut compat = system();
        for (key, value) in extra.as_object().unwrap() {
            compat[key] = value.clone();
        }
        Some(compat)
    };
    vec![
        (None, false),
        (model("anthropic-messages", None), false),
        (
            model(
                "anthropic-messages",
                with(json!({ "supportsMidConvoToolChanges": true })),
            ),
            true,
        ),
        (
            model(
                "anthropic-messages",
                Some(json!({
                    "supportsMidConvoSystemMessages": false,
                    "supportsMidConvoToolChanges": true
                })),
            ),
            false,
        ),
        (
            model(
                "anthropic-messages",
                Some(json!({ "supportsMidConvoToolChanges": true })),
            ),
            false,
        ),
        (model("anthropic-messages", Some(system())), false),
        (
            model(
                "openai-completions",
                with(json!({ "supportsMidConvoToolAdditions": true })),
            ),
            true,
        ),
        (
            model(
                "openai-completions",
                with(json!({ "supportsMidConvoToolChanges": true })),
            ),
            false,
        ),
        (
            model(
                "openai-responses",
                with(json!({ "supportsAdditionalTools": true })),
            ),
            true,
        ),
        (
            model(
                "openai-responses",
                with(json!({ "supportsToolSearch": true })),
            ),
            true,
        ),
        (model("openai-responses", Some(system())), false),
        (
            model(
                "openai-codex-responses",
                with(json!({ "supportsToolSearch": true })),
            ),
            true,
        ),
        (
            model(
                "azure-openai-responses",
                with(json!({ "supportsAdditionalTools": true })),
            ),
            true,
        ),
        (
            model(
                "google-generative-ai",
                with(json!({
                    "supportsMidConvoToolChanges": true,
                    "supportsMidConvoToolAdditions": true,
                    "supportsAdditionalTools": true
                })),
            ),
            false,
        ),
    ]
}

// =================================================================================================
// The predicate (upstream `:233-257`) and cyrup's gate
// =================================================================================================

/// Upstream's own predicate, on its own: every row of its 14-row table.
#[test]
fn upstreams_predicate_matches_its_fourteen_row_table() {
    let table = upstream_predicate_table();
    assert_eq!(table.len(), 14);
    for (case, expected) in &table {
        assert_eq!(
            compat_adds_tools_without_checkpoint(case.as_ref()),
            *expected,
            "{case:?}"
        );
    }
}

/// `[CYRUP-DELTA]` — the gate `auto` uses is upstream's predicate AND the adapter for that api
/// really sending the change natively. Since PROV-133 that is true for `anthropic-messages` and
/// still false everywhere else, so a row qualifies only if it does both.
///
/// This replaces `the_cyrup_gate_refuses_every_row_until_an_adapter_emits_native_tool_additions`,
/// which asserted that NO row qualified — the pin PROV-133 was required to break.
#[test]
fn the_cyrup_gate_needs_upstreams_flags_and_a_native_emitter() {
    for (case, upstream) in upstream_predicate_table() {
        let native = case
            .as_ref()
            .and_then(|m| m.get("api"))
            .and_then(Value::as_str)
            .is_some_and(cyrup_provider::api::emits_native_tool_additions);
        assert_eq!(
            adds_tools_without_checkpoint(case.as_ref()),
            upstream && native,
            "{case:?}"
        );
    }
}

/// A fresh `auto` session is eager — `subagent` active, no loader — on every row EXCEPT one whose
/// api emits tool additions natively and whose compat flags qualify. That exception is what
/// PROV-133 bought: on a mid-convo-capable Anthropic model the session starts lazy, with the
/// loader and without the eager `subagent` in the prompt.
///
/// This replaces `auto_starts_every_fresh_session_eager_while_no_adapter_emits_tool_additions`,
/// whose own doc comment said it "must change for that api's rows, which is the point".
#[tokio::test]
async fn auto_starts_lazy_only_where_the_adapter_emits_tool_additions_natively() {
    for (case, upstream) in upstream_predicate_table() {
        let native = case
            .as_ref()
            .and_then(|m| m.get("api"))
            .and_then(Value::as_str)
            .is_some_and(cyrup_provider::api::emits_native_tool_additions);
        let lazy = upstream && native;

        let host = FakeHost::new(Vec::new(), &[]).with_model(case.clone());
        let state = registered(ToolActivationMode::Auto);
        state.on_session_start_or_tree(host.services()).await;
        let selected = start_agent(&state, &host);

        if lazy {
            assert!(
                selected.iter().any(|n| n == LOADER_NAME),
                "a native-emitter row must start with the loader: {case:?}"
            );
            assert!(
                !selected.iter().any(|n| n == "subagent"),
                "a native-emitter row must not start eager: {case:?}"
            );
        } else {
            assert!(selected.iter().any(|n| n == "subagent"), "{case:?}");
            assert!(!selected.iter().any(|n| n == LOADER_NAME), "{case:?}");
            assert!(!host.has(LOADER_NAME), "{case:?}");
        }
    }
}

// =================================================================================================
// setSelection
// =================================================================================================

/// pi `[...new Set(next)]`: a name already active keeps its position (first occurrence wins); a
/// name being added goes to the end, `subagent` before the loader; unrelated tools keep their order.
#[test]
fn set_selection_keeps_existing_positions_and_appends_subagent_then_loader() {
    let host = FakeHost::new(Vec::new(), &[]);
    host.select(&[LOADER_NAME, "bg_wait", "read", "subagent"]);
    set_selection(host.as_ref(), true, true);
    assert_eq!(
        host.active(),
        vec![LOADER_NAME, "bg_wait", "read", "subagent"],
        "idempotent: nothing moves"
    );
    set_selection(host.as_ref(), false, true);
    assert_eq!(host.active(), vec![LOADER_NAME, "bg_wait", "read"]);
    set_selection(host.as_ref(), false, false);
    assert_eq!(host.active(), vec!["bg_wait", "read"]);
    set_selection(host.as_ref(), true, true);
    assert_eq!(
        host.active(),
        vec!["bg_wait", "read", "subagent", LOADER_NAME]
    );
}

// =================================================================================================
// Session start / tree (upstream `:102-152`, `:220-303`)
// =================================================================================================

/// Upstream `:102-121` — dynamic, fresh session: the loader and the support tools are active,
/// `subagent` is registered but inactive.
#[tokio::test]
async fn dynamic_starts_a_fresh_parent_with_the_loader_and_support_tools() {
    let host = FakeHost::new(Vec::new(), &[]);
    let state = registered(ToolActivationMode::Dynamic);
    state.on_session_start_or_tree(host.services()).await;
    assert!(!host.has("subagent"));
    for name in [LOADER_NAME, "bg_wait", "subagent_supervisor", "read"] {
        assert!(host.has(name), "{name}: {:?}", host.active());
    }
}

/// Upstream `:124-140` — cold and warm records restore across start and tree navigation.
#[tokio::test]
async fn dynamic_restores_cold_and_warm_records_across_start_and_tree() {
    let host = FakeHost::new(vec![declared(&["read"], &[])], &[]);
    let state = registered(ToolActivationMode::Dynamic);
    state.on_session_start_or_tree(host.services()).await;
    assert!(!host.has("subagent"));
    host.messages
        .lock()
        .unwrap()
        .push(declared(&["subagent"], &[]));
    state.on_session_start_or_tree(host.services()).await;
    assert!(host.has("subagent"));

    let warm = FakeHost::new(vec![declared(&["subagent"], &[])], &[]);
    state.on_session_start_or_tree(warm.services()).await;
    assert!(warm.has("subagent") && warm.has(LOADER_NAME));
}

/// Upstream `:142-152` — legacy history keeps `subagent` and gets the loader; an excluded loader
/// leaves `subagent` alone.
#[tokio::test]
async fn legacy_history_and_an_excluded_loader_keep_subagent_available() {
    let legacy_host = FakeHost::new(vec![legacy()], &[]);
    let state = registered(ToolActivationMode::Dynamic);
    state.on_session_start_or_tree(legacy_host.services()).await;
    assert!(legacy_host.has("subagent") && legacy_host.has(LOADER_NAME));

    let restricted = FakeHost::new(Vec::new(), &[LOADER_NAME]);
    let before = restricted.active();
    assert!(apply_recorded_selection(restricted.as_ref(), ToolActivationMode::Dynamic).await);
    assert_eq!(restricted.active(), before, "selection untouched");
    assert!(restricted.has("subagent") && !restricted.has(LOADER_NAME));
}

/// Upstream `:220-231` — auto with an incapable model (and with no model at all): `subagent`, no
/// loader; switching to a capable model mid-session changes nothing.
#[tokio::test]
async fn auto_with_an_incapable_model_starts_eager_and_a_model_switch_changes_nothing() {
    for model in [None, incompatible()] {
        let host = FakeHost::new(Vec::new(), &[]).with_model(model);
        let state = registered(ToolActivationMode::Auto);
        state.on_session_start_or_tree(host.services()).await;
        let first = start_agent(&state, &host);
        assert!(first.iter().any(|n| n == "subagent") && first.iter().any(|n| n == "read"));
        assert!(!first.iter().any(|n| n == LOADER_NAME));
        *host.model.lock().unwrap() = compatible();
        assert_eq!(start_agent(&state, &host), first);
    }
}

/// Upstream `:259-266` — dynamic always offers the loader, even to a recorded session without it.
#[tokio::test]
async fn dynamic_always_offers_the_loader_even_to_a_recorded_session_without_it() {
    for messages in [Vec::new(), vec![declared(&["read", "subagent"], &[])]] {
        let recorded = !messages.is_empty();
        let host = FakeHost::new(messages, &[]).with_model(incompatible());
        let state = registered(ToolActivationMode::Dynamic);
        state.on_session_start_or_tree(host.services()).await;
        assert!(start_agent(&state, &host).iter().any(|n| n == LOADER_NAME));
        assert_eq!(host.has("subagent"), recorded);
    }
}

/// Upstream `:279-303` — auto replays recorded sessions without adding or removing tools, whatever
/// the model, across resume, reload and tree navigation.
#[tokio::test]
async fn auto_replays_recorded_sessions_without_adding_or_removing_tools() {
    let cases: Vec<(&str, Vec<cyrup_session::AgentMessage>, &[&str])> = vec![
        (
            "cold",
            vec![declared(&["read", LOADER_NAME], &[])],
            &[LOADER_NAME],
        ),
        (
            "warm",
            vec![
                declared(&["read", LOADER_NAME], &[]),
                declared(&["subagent"], &[]),
            ],
            &[LOADER_NAME, "subagent"],
        ),
        (
            "eager",
            vec![declared(&["read", "subagent"], &[])],
            &["subagent"],
        ),
        (
            "loader removed",
            vec![
                declared(&["read", LOADER_NAME, "subagent"], &[]),
                declared(&[], &[LOADER_NAME]),
            ],
            &["subagent"],
        ),
        ("legacy", vec![legacy()], &[LOADER_NAME, "subagent"]),
    ];
    for model in [compatible(), incompatible()] {
        for (label, messages, expected) in &cases {
            let host = FakeHost::new(messages.clone(), &[]).with_model(model.clone());
            let state = registered(ToolActivationMode::Auto);
            for event in [
                "session_start resume",
                "session_start reload",
                "session_tree",
            ] {
                state.on_session_start_or_tree(host.services()).await;
                let selected = start_agent(&state, &host);
                for name in [LOADER_NAME, "subagent"] {
                    let want = expected.contains(&name);
                    assert_eq!(
                        selected.iter().any(|n| n == name),
                        want,
                        "{label}, {model:?}, {event}: {name}"
                    );
                    assert_eq!(
                        host.has(name),
                        want,
                        "{label}, {model:?}, {event}: active {name}"
                    );
                }
            }
        }
    }
}

/// Upstream `:88-94`, in cyrup's form: a host with no live dynamic-tool view keeps everything as
/// it is, and the loader is not put back at `before_agent_start`.
#[tokio::test]
async fn a_host_without_a_dynamic_tool_view_keeps_subagent_and_never_selects_the_loader() {
    struct NoView;
    impl HostServices for NoView {}
    let state = registered(ToolActivationMode::Dynamic);
    state
        .on_session_start_or_tree(Some(Arc::new(NoView) as Arc<dyn HostServices>))
        .await;
    let host = FakeHost::new(Vec::new(), &[]);
    let options = json!({ "selectedTools": host.active() });
    assert_eq!(state.on_before_agent_start(host.services(), &options), None);
    assert!(!host.has(LOADER_NAME));

    // No services at all is the same.
    let state = registered(ToolActivationMode::Dynamic);
    state.on_session_start_or_tree(None).await;
    assert_eq!(state.on_before_agent_start(host.services(), &options), None);
}

/// `[CYRUP-DELTA]` — a session that never emitted `session_start` (a host that skipped
/// `bind_extensions`) is left exactly as it was: no loader is added at `before_agent_start`.
#[tokio::test]
async fn before_agent_start_without_a_session_start_adds_no_loader() {
    let host = FakeHost::new(Vec::new(), &[]);
    let state = registered(ToolActivationMode::Dynamic);
    let options = json!({ "selectedTools": host.active() });
    assert_eq!(state.on_before_agent_start(host.services(), &options), None);
    assert!(host.has("subagent") && !host.has(LOADER_NAME));
}

// =================================================================================================
// before_agent_start (upstream `:154-181`)
// =================================================================================================

/// Upstream `:154-171` — prompt keywords do not activate delegation, and the handler re-adds the
/// loader to `selectedTools` and to the active set after something dropped it.
#[tokio::test]
async fn before_agent_start_puts_a_dropped_loader_back_without_activating_subagent() {
    let host = FakeHost::new(Vec::new(), &[]);
    let state = registered(ToolActivationMode::Dynamic);
    state.on_session_start_or_tree(host.services()).await;
    host.select(&["read"]);
    let edited = state
        .on_before_agent_start(host.services(), &json!({ "selectedTools": ["read"] }))
        .expect("the loader is put back");
    assert_eq!(edited["selectedTools"], json!(["read", LOADER_NAME]));
    assert!(host.has(LOADER_NAME));
    assert!(!host.has("subagent"));

    // pi `??=`: an absent list is seeded from the active set.
    let seeded = state
        .on_before_agent_start(host.services(), &json!({}))
        .expect("seeded");
    assert_eq!(seeded["selectedTools"], json!(["read", LOADER_NAME]));

    // Nothing to do when the loader is already selected.
    assert_eq!(
        state.on_before_agent_start(
            host.services(),
            &json!({ "selectedTools": ["read", LOADER_NAME] })
        ),
        None
    );
}

// =================================================================================================
// The loader tool (upstream `:102-121`, `:173-180`)
// =================================================================================================

fn executor_on(host: &Arc<FakeHost>) -> Arc<SubagentExecutor> {
    let executor = Arc::new(SubagentExecutor::new());
    executor.set_host_services(host.services().unwrap());
    executor
}

async fn call(tool: &SubagentsEnableTool, args: Value) -> ToolResult {
    tool.execute(
        ToolCallId::from("enable"),
        args,
        CancelToken::new(),
        Box::new(|_| {}),
    )
    .await
    .expect("the loader reports failures as is_error results")
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::test]
async fn the_loader_enables_subagent_keeps_other_tools_and_is_idempotent() {
    let host = FakeHost::new(Vec::new(), &[]);
    set_selection(host.as_ref(), false, true);
    let tool = SubagentsEnableTool::new(executor_on(&host));

    assert_eq!(tool.name(), LOADER_NAME);
    assert_eq!(tool.label(), Some("Enable Subagents"));
    assert_eq!(tool.exposure(), ToolExposure::ModelOnly);
    assert!(!tool.default_active());
    assert!(
        tool.description()
            .contains("authorized by the current request or applicable user/project instructions")
    );
    // #2492: a stray argument validates against the loader's schema.
    assert_eq!(
        cyrup_provider::validate::validate_tool_call(
            tool.parameters(),
            json!({ "action": "enable" })
        )
        .expect("stray args validate"),
        json!({ "action": "enable" })
    );

    let result = call(&tool, json!({ "action": "enable" })).await;
    assert!(!result.is_error, "{}", text(&result));
    assert_eq!(text(&result), LOADER_ENABLED_TEXT);
    assert_eq!(result.details, Some(json!({ "enabled": ["subagent"] })));
    assert!(host.has("subagent") && host.has("read"));
    let enabled = host.active();
    call(&tool, json!({})).await;
    assert_eq!(host.active(), enabled);
}

#[tokio::test]
async fn the_loader_reports_an_unavailable_subagent() {
    let host = FakeHost::new(Vec::new(), &["subagent"]);
    let tool = SubagentsEnableTool::new(executor_on(&host));
    let result = call(&tool, json!({})).await;
    assert!(result.is_error);
    assert_eq!(text(&result), "Cannot enable unavailable tools: subagent.");
    assert_eq!(result.details, Some(json!({ "unavailable": ["subagent"] })));
}

// =================================================================================================
// The extension wiring
// =================================================================================================

fn extension(mode: Option<ToolActivationMode>, root: &std::path::Path) -> SubagentsExtension {
    SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: crate::paths::Roots::sandboxed(root),
            tool_activation: mode,
            ..SubagentExtensionConfig::default()
        },
        root.to_path_buf(),
    )
}

async fn registered_tool_names(extension: SubagentsExtension) -> Vec<String> {
    let host = cyrup_ext::ExtensionHost::new(cyrup_ext::facade::HostConfig {
        mode: ExtMode::Json,
        has_ui: false,
        cwd: std::env::temp_dir(),
    });
    host.load_native(Arc::new(extension)).await.expect("load");
    host.registered_tools_filtered(&[], None, &std::collections::HashSet::new())
        .expect("tools")
        .iter()
        .map(|tool| tool.name().to_string())
        .collect()
}

/// Upstream `:268-277` and `fanout-child.ts`: `eager` registers no loader, and neither does a
/// `ChildSafe` child; `auto` and `dynamic` register it next to `subagent`.
#[tokio::test]
async fn the_loader_is_registered_except_under_eager_and_in_a_child() {
    let root = tempfile::tempdir().expect("tempdir");
    for (mode, want) in [
        (None, true),
        (Some(ToolActivationMode::Auto), true),
        (Some(ToolActivationMode::Dynamic), true),
        (Some(ToolActivationMode::Eager), false),
    ] {
        let names = registered_tool_names(extension(mode, root.path())).await;
        assert!(names.iter().any(|n| n == "subagent"), "{mode:?}: {names:?}");
        assert_eq!(
            names.iter().any(|n| n == LOADER_NAME),
            want,
            "{mode:?}: {names:?}"
        );
    }
    let child = SubagentsExtension::with_mode(
        SubagentExtensionConfig {
            roots: crate::paths::Roots::sandboxed(root.path()),
            tool_activation: Some(ToolActivationMode::Dynamic),
            ..SubagentExtensionConfig::default()
        },
        root.path().to_path_buf(),
        crate::extension::RegistrationMode::ChildSafe,
    );
    let names = registered_tool_names(child).await;
    assert!(!names.iter().any(|n| n == LOADER_NAME), "{names:?}");
}

#[tokio::test]
async fn the_extension_subscribes_to_session_tree() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut api = InitApi::new();
    extension(None, root.path())
        .init(&mut api)
        .await
        .expect("init");
    assert!(
        api.subscriptions()
            .contains(cyrup_ext::EventKind::SessionTree)
    );
}

fn before_agent_start(system_prompt: &str, selected: &[&str]) -> HostEvent {
    HostEvent::BeforeAgentStart {
        prompt: "delegate this complex task".to_string(),
        images: Value::Null,
        system_prompt: system_prompt.to_string(),
        options: json!({ "selectedTools": selected }),
        injected: Vec::new(),
    }
}

/// The real `on_event` path, and the merge: the loader edit and the advertised-catalog rewrite
/// (here, stripping a stale block) leave in ONE `Mutate`, so neither is dropped.
#[tokio::test]
async fn on_event_keeps_the_loader_edit_and_the_catalog_rewrite_in_one_mutate() {
    let root = tempfile::tempdir().expect("tempdir");
    let ext = extension(Some(ToolActivationMode::Dynamic), root.path());
    let host = FakeHost::new(Vec::new(), &[]);
    ext.executor().set_host_services(host.services().unwrap());
    ext.init(&mut InitApi::new()).await.expect("init");
    let ctx = HostCtx::event(ExtMode::Json, false, root.path().to_path_buf());
    ext.on_event(
        &HostEvent::SessionStart {
            reason: "startup".to_string(),
            previous_session_file: None,
        },
        &ctx,
    )
    .await;
    assert!(host.has(LOADER_NAME) && !host.has("subagent"));

    // Loader edit alone.
    host.select(&["read"]);
    let cyrup_ext::HookOutcome::Mutate(cyrup_ext::EventPatch::SystemPromptAndInject {
        system,
        options,
        ..
    }) = ext
        .on_event(&before_agent_start("base", &["read"]), &ctx)
        .await
    else {
        panic!("the loader edit must be returned");
    };
    assert_eq!(system, None);
    assert_eq!(
        options.expect("edited options")["selectedTools"],
        json!(["read", LOADER_NAME])
    );
    assert!(host.has(LOADER_NAME) && !host.has("subagent"));

    // Both edits at once.
    let stale = "base\n\n<advertised_subagents>\nstale\n</advertised_subagents>";
    let cyrup_ext::HookOutcome::Mutate(cyrup_ext::EventPatch::SystemPromptAndInject {
        system,
        options,
        ..
    }) = ext
        .on_event(&before_agent_start(stale, &["read", "subagent"]), &ctx)
        .await
    else {
        panic!("both edits must be returned");
    };
    assert_eq!(
        system.as_deref(),
        Some("base"),
        "the stale block is stripped"
    );
    assert_eq!(
        options.expect("the loader edit survives the catalog rewrite")["selectedTools"],
        json!(["read", "subagent", LOADER_NAME])
    );
}

// =================================================================================================
// config.toolActivation (upstream `:306-308`, `extension/config.ts:178-180`)
// =================================================================================================

#[test]
fn tool_activation_config_validates_like_upstream() {
    const MESSAGE: &str = r#"config.toolActivation must be "auto", "dynamic", or "eager""#;
    for bad in [json!("lazy"), Value::Null, json!(3), json!(["auto"])] {
        assert_eq!(
            SubagentExtensionConfig::validate_raw_config(&json!({ "toolActivation": bad })),
            Err(MESSAGE.to_string()),
            "{bad}"
        );
    }
    for (good, mode) in [
        ("auto", ToolActivationMode::Auto),
        ("dynamic", ToolActivationMode::Dynamic),
        ("eager", ToolActivationMode::Eager),
    ] {
        let raw = json!({ "toolActivation": good });
        assert_eq!(SubagentExtensionConfig::validate_raw_config(&raw), Ok(()));
        let typed: SubagentExtensionConfig = serde_json::from_value(raw).expect("typed parse");
        assert_eq!(typed.tool_activation, Some(mode));
    }
    assert_eq!(SubagentExtensionConfig::default().tool_activation, None);

    // Upstream's order: `artifactDir` first, then `toolActivation`, then `missions`.
    let both = json!({ "artifactDir": "nowhere", "toolActivation": "lazy" });
    assert!(
        SubagentExtensionConfig::validate_raw_config(&both)
            .unwrap_err()
            .starts_with("config.artifactDir")
    );
    let both = json!({ "toolActivation": "lazy", "missions": { "bogus": true } });
    assert_eq!(
        SubagentExtensionConfig::validate_raw_config(&both),
        Err(MESSAGE.to_string())
    );

    assert_eq!(
        SubagentExtensionConfig::invalid_config_disposition(&json!({ "toolActivation": "lazy" })),
        crate::registration::InvalidConfigDisposition::Refuse(vec!["toolActivation"])
    );
}
