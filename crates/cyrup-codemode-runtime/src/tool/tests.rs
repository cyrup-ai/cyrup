//! The tool as the registry and the loadout see it: its metadata, its identity, and how it presents
//! the other tools per `codemode.mode` (pi `createCodemodeToolDefinition`,
//! `prepareCodemodeLoadout`, `isCodemodeTool` @v1.0.1; upstream's session-level
//! `presents callable tools per codemode.mode`, `test/agent-session-codemode.test.ts:118-159`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_config::CodemodeMode;
use cyrup_core::{ConstrainedSampling, ConstrainedSamplingConfig, Tool, ToolExposure, ToolLoadout};
use serde_json::json;

use super::{
    CODEMODE_TOOL_NAME, CodemodeHostSlot, CodemodeTool, CodemodeToolOptions,
    UnavailableSandboxFactory, codemode_schema, is_codemode_tool,
};
use crate::testkit::{FakeModels, RecordingHost, StubTool};

fn options(host: &CodemodeHostSlot) -> CodemodeToolOptions {
    CodemodeToolOptions::new(host.clone(), Arc::new(UnavailableSandboxFactory))
}

fn codemode(host: &CodemodeHostSlot) -> Arc<dyn Tool> {
    Arc::new(CodemodeTool::new(options(host)))
}

fn echo() -> Arc<dyn Tool> {
    StubTool::new("echo", "Echo text back.\n\nSecond paragraph.").arc()
}

fn stats() -> Arc<dyn Tool> {
    StubTool::new("stats", "Return structured stats")
        .output_schema(json!({
            "type": "object",
            "properties": { "files": { "type": "number" } },
            "required": ["files"]
        }))
        .arc()
}

fn read() -> Arc<dyn Tool> {
    StubTool::new("read", "Read a file.").arc()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn description_of(loadout: &ToolLoadout, name: &str) -> String {
    loadout
        .executable()
        .iter()
        .find(|tool| tool.name() == name)
        .map(|tool| tool.description().to_owned())
        .unwrap_or_default()
}

fn host_slot(mode: CodemodeMode) -> CodemodeHostSlot {
    let slot = CodemodeHostSlot::new();
    let host = RecordingHost::default();
    *host.mode.lock().unwrap() = mode;
    slot.bind(Arc::new(host));
    slot
}

/// What the registry reads off the tool, before any session exists (`tool.ts:361-382`).
#[test]
fn the_tool_registers_inactive_model_only_with_a_grammar_and_a_prompt_contribution() {
    let tool = CodemodeTool::new(options(&CodemodeHostSlot::new()));
    assert_eq!(tool.name(), "codemode");
    assert_eq!(tool.label(), Some("codemode"));
    assert_eq!(
        tool.exposure(),
        ToolExposure::ModelOnly,
        "scripts must not start other scripts"
    );
    assert!(!tool.default_active(), "registered inactive");
    assert_eq!(
        tool.prompt_snippet(),
        Some("Run JavaScript that calls other tools")
    );
    assert_eq!(
        tool.prompt_guidelines(),
        vec![
            "Use codemode to batch independent tool calls (Promise.allSettled), chain them, or filter large output, instead of many separate calls."
        ]
    );
    assert_eq!(
        tool.parameters(),
        &json!({
            "type": "object",
            "properties": { "code": { "type": "string", "description": "Raw JavaScript source." } },
            "required": ["code"]
        })
    );
    match tool
        .constrained_sampling()
        .and_then(ConstrainedSampling::config)
    {
        Some(ConstrainedSamplingConfig::Grammar { variants }) => {
            assert_eq!(
                variants.openai_lark.as_deref(),
                Some(cyrup_codemode::source::CODEMODE_SOURCE_GRAMMAR)
            );
            assert_eq!(variants.openai_regex, None);
        }
        other => panic!("expected a grammar declaration, got {other:?}"),
    }
}

/// Before activation the description is the helper list alone (`createCodemodeDescription([], …)`).
#[test]
fn the_registered_description_lists_no_tools() {
    let tool = CodemodeTool::new(options(&CodemodeHostSlot::new()));
    assert!(
        tool.description()
            .starts_with("Run JavaScript that calls other tools.")
    );
    assert!(!tool.description().contains("Nested tools:"));
    assert!(
        tool.description().contains("`models`"),
        "models: true by default"
    );

    let mut no_models = options(&CodemodeHostSlot::new());
    no_models.models = false;
    assert!(
        !CodemodeTool::new(no_models)
            .description()
            .contains("`models`")
    );
}

/// `isCodemodeTool` compares identity, so another extension's `codemode` tool with an equal-looking
/// schema is not this tool, and a wrapper that passes the schema through still is.
#[test]
fn an_impostor_with_the_same_name_and_an_equal_schema_is_not_the_codemode_tool() {
    let ours = CodemodeTool::new(options(&CodemodeHostSlot::new()));
    assert!(is_codemode_tool(&ours));

    let impostor = StubTool::new(CODEMODE_TOOL_NAME, "Mine.")
        .parameters(codemode_schema().clone())
        .arc();
    assert_eq!(impostor.parameters(), codemode_schema(), "equal by value");
    assert!(!is_codemode_tool(impostor.as_ref()), "not by identity");

    let renamed = StubTool::new("other", "x").parameters(codemode_schema().clone());
    assert!(!is_codemode_tool(&renamed));

    // The loadout's description wrapper delegates `parameters()` by reference.
    let slot = host_slot(CodemodeMode::On);
    let loadout = ToolLoadout::resolve(&names(&["echo", "codemode"]), &[echo(), codemode(&slot)]);
    let wrapped = loadout
        .executable()
        .iter()
        .find(|tool| tool.name() == "codemode")
        .unwrap();
    assert!(is_codemode_tool(wrapped.as_ref()));
}

/// `on`: declared tools say how scripts call them and are not listed again in codemode.
#[test]
fn on_mode_describes_declared_tools_and_lists_only_the_non_direct_callables() {
    let slot = host_slot(CodemodeMode::On);
    let mcp = StubTool::new("mcp__docs__search", "Search docs.")
        .exposure(ToolExposure::Codemode)
        .arc();
    let loadout = ToolLoadout::resolve(
        &names(&["read", "echo", "codemode"]),
        &[read(), echo(), stats(), mcp, codemode(&slot)],
    );

    let echo_description = description_of(&loadout, "echo");
    assert!(echo_description.contains("Codemode: `tools.echo(args)` resolves to"));
    assert!(!echo_description.contains("codemode tool declaration:"));

    let codemode_description = description_of(&loadout, "codemode");
    assert!(
        !codemode_description.contains("### `echo`"),
        "direct tools are not listed"
    );
    assert!(
        !codemode_description.contains("### `stats`"),
        "an inactive direct tool is not callable"
    );
    assert!(
        codemode_description.contains("### `mcp__docs__search`"),
        "a `codemode`-exposure tool is listed"
    );

    assert!(loadout.hidden_declarations().is_empty());
    let advertised = loadout.advertised();
    assert!(
        advertised.contains("read")
            && advertised.contains("echo")
            && advertised.contains("codemode")
    );
}

/// `only`: codemode lists echo, which stays active but is left out of requests.
#[test]
fn only_mode_lists_every_callable_tool_and_leaves_active_direct_tools_out_of_requests() {
    let slot = host_slot(CodemodeMode::Only);
    let loadout = ToolLoadout::resolve(
        &names(&["read", "echo", "codemode"]),
        &[read(), echo(), stats(), codemode(&slot)],
    );

    assert!(
        !description_of(&loadout, "echo").contains("Codemode: `tools.echo"),
        "no per-tool note in `only`"
    );
    let codemode_description = description_of(&loadout, "codemode");
    assert!(codemode_description.contains("### `echo`"));
    assert!(codemode_description.contains("### `read`"));
    assert!(!codemode_description.contains("### `stats`"));

    let advertised = loadout.advertised();
    assert!(advertised.contains("codemode"));
    assert!(!advertised.contains("echo") && !advertised.contains("read"));
    // They stay active and executable, only their declarations are hidden.
    let executable: Vec<&str> = loadout
        .executable()
        .iter()
        .map(|tool| tool.name())
        .collect();
    assert_eq!(executable, ["read", "echo", "codemode"]);
    assert_eq!(
        loadout
            .hidden_declarations()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        names(&["echo", "read"])
    );
}

/// Without codemode, tools keep their plain descriptions.
#[test]
fn without_codemode_tools_keep_their_plain_descriptions() {
    let slot = host_slot(CodemodeMode::On);
    let loadout = ToolLoadout::resolve(&names(&["echo"]), &[echo(), codemode(&slot)]);
    assert_eq!(
        description_of(&loadout, "echo"),
        "Echo text back.\n\nSecond paragraph."
    );
}

/// Listing by exposure, not by the active set: `tool_search` activating a deferred tool does not
/// change the codemode description, in either mode (`tool.ts:324-330`).
#[test]
fn the_description_stays_identical_when_tool_search_activates_a_deferred_tool() {
    for mode in [CodemodeMode::On, CodemodeMode::Only] {
        let slot = host_slot(mode);
        let deferred = || {
            StubTool::new("mcp__github__issue", "Open an issue.")
                .exposure(ToolExposure::Deferred)
                .namespace("mcp__github", Some("GitHub server"))
                .arc()
        };
        let registry = vec![read(), echo(), deferred(), codemode(&slot)];
        let before = ToolLoadout::resolve(&names(&["read", "echo", "codemode"]), &registry);
        let after = ToolLoadout::resolve(
            &names(&["read", "echo", "codemode", "mcp__github__issue"]),
            &registry,
        );
        let before_description = description_of(&before, "codemode");
        assert_eq!(
            before_description,
            description_of(&after, "codemode"),
            "mode {mode:?}"
        );
        assert!(
            !before_description.contains("mcp__github"),
            "a deferred tool is never listed ({mode:?})"
        );
    }
}

/// Deferred tools are callable (the loadout's callable view) but never listed.
#[test]
fn deferred_tools_are_never_listed_in_either_mode() {
    for mode in [CodemodeMode::On, CodemodeMode::Only] {
        let slot = host_slot(mode);
        let deferred = StubTool::new("hidden_search", "Search hidden things.")
            .exposure(ToolExposure::Deferred)
            .arc();
        let loadout = ToolLoadout::resolve(&names(&["codemode"]), &[deferred, codemode(&slot)]);
        assert!(!description_of(&loadout, "codemode").contains("hidden_search"));
    }
}

/// `codemode` is `model-only`: it is never in its own callable view, so it never lists itself and
/// a script cannot call it.
#[test]
fn the_codemode_tool_is_not_callable_from_itself() {
    let slot = host_slot(CodemodeMode::Only);
    let loadout = ToolLoadout::resolve(&names(&["echo", "codemode"]), &[echo(), codemode(&slot)]);
    let description = description_of(&loadout, "codemode");
    assert!(!description.contains("### `codemode`"));
    assert!(description.contains("### `echo`"));
}

/// The settings reach the description through the host: `inlineBudget: 0` lists only namespaces.
#[test]
fn the_host_budget_limits_the_listed_declarations_and_an_override_wins() {
    let slot = CodemodeHostSlot::new();
    let host = RecordingHost::default();
    *host.inline_budget.lock().unwrap() = 0.0;
    slot.bind(Arc::new(host));
    let mcp = || {
        StubTool::new("mcp__docs__search", "Search docs.")
            .exposure(ToolExposure::Codemode)
            .namespace("mcp__docs", None)
            .arc()
    };

    let listed = |opts: CodemodeToolOptions| {
        let tool: Arc<dyn Tool> = Arc::new(CodemodeTool::new(opts));
        let loadout = ToolLoadout::resolve(&names(&["codemode"]), &[mcp(), tool]);
        description_of(&loadout, "codemode")
    };
    let zero = listed(options(&slot));
    assert!(zero.contains("## mcp__docs (tools not listed)"));
    assert!(!zero.contains("### `mcp__docs__search`"));

    let mut overridden = options(&slot);
    overridden.inline_budget = Some(3000.0);
    assert!(listed(overridden).contains("### `mcp__docs__search`"));
}

/// `models` is declared only for a host with model access (`models: options.models === true`
/// plus a registry behind it): "Without it, `models` is not declared."
#[test]
fn models_is_declared_only_when_the_host_has_model_access() {
    let declared = |host: Option<RecordingHost>| {
        let slot = CodemodeHostSlot::new();
        if let Some(host) = host {
            slot.bind(Arc::new(host));
        }
        let loadout = ToolLoadout::resolve(&names(&["codemode"]), &[codemode(&slot)]);
        description_of(&loadout, "codemode").contains("`models`: classifiers and image generation")
    };
    assert!(!declared(None), "no session context");
    assert!(
        !declared(Some(RecordingHost::default())),
        "a host without model access"
    );

    let with_models = RecordingHost::default();
    *with_models.models.lock().unwrap() = Some(Arc::new(FakeModels::new(
        Vec::new(),
        Arc::new(|_, _| unreachable!("never called")),
        Arc::new(|_, _| unreachable!("never called")),
    )));
    assert!(declared(Some(with_models)));
}
