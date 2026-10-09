//! SUBA-139 / SUBA-153 end-to-end: the `subagents_enable` loader on a real, fully wired session
//! (pi `src/extension/tool-activation.ts` @v0.76.1).
//!
//! The unit port of upstream's `tool-activation.test.ts` runs against a fake host. What only a real
//! session proves is the host half: that the selection `session_start` writes through
//! `HostServices::set_active_tools` shapes the FIRST model request, and that calling the loader
//! puts `subagent` on the NEXT request of the same run (the per-turn drain,
//! `AgentSession::next_turn_tools`). Each test reads the tool list the scripted provider was
//! actually handed (`harness.faux().contexts[i].tools`).
//!
//! Fixture-free like `wait_tool_registration_integration`: the loader spawns nothing, and the
//! `subagent` call below is a `list`, which reads agent definitions and launches no child.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::{SubagentExtensionConfig, ToolActivationMode};
use cyrup_test_support::harness::{Harness, HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;

const LOADER: &str = "subagents_enable";

struct Run {
    harness: Harness,
    _home: tempfile::TempDir,
    _work: tempfile::TempDir,
}

async fn session(
    mode: Option<ToolActivationMode>,
    responses: Vec<FauxResponse>,
    options: HarnessOptions,
) -> Run {
    let home = tempfile::tempdir().expect("home tempdir");
    let work = tempfile::tempdir().expect("work tempdir");
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            tool_activation: mode,
            ..SubagentExtensionConfig::default()
        },
        work.path().to_path_buf(),
    ));
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses,
        ..options
    })
    .await
    .expect("harness builds a real session with the subagents extension loaded");
    // The host's `bindExtensions` (`print-mode.ts` / `interactive-mode.ts`): emits `session_start`,
    // where the loader selection is decided. The harness does not bind on its own.
    harness.session().bind_extensions().await;
    Run {
        harness,
        _home: home,
        _work: work,
    }
}

/// The tool names the scripted provider was handed on request `index`.
fn request_tools(harness: &Harness, index: usize) -> Vec<String> {
    harness.faux().contexts[index]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

fn has(tools: &[String], name: &str) -> bool {
    tools.iter().any(|tool| tool == name)
}

/// SUBA-139's Verify line: a fresh `dynamic` session offers the loader and not `subagent`; calling
/// the loader puts `subagent` on the next prompt's first request, and the call dispatches. Before
/// the port the first request carried `subagent` and no loader, and the scripted
/// `subagents_enable` call reached no tool.
///
/// Across two prompts rather than inside one run: the harness holds its `AgentSession` by value,
/// never `into_shared`, and on such an unbound session `PolicyHooks::prepare_next_turn` degrades to
/// the plain delegate (`cyrup-session-svc/src/hooks.rs`), so the per-turn drain the bound runtime
/// performs inside a run does not happen here. The post-run / pre-prompt drain does, and that is
/// the path this exercises.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_offers_the_loader_first_and_subagent_after_it_is_called() {
    let run = session(
        Some(ToolActivationMode::Dynamic),
        vec![
            FauxResponse::tool_call(LOADER, serde_json::json!({ "action": "enable" })),
            FauxResponse::text("enabled"),
            FauxResponse::tool_call("subagent", serde_json::json!({ "action": "list" })),
            FauxResponse::text("done"),
        ],
        HarnessOptions {
            queue_responses: true,
            ..HarnessOptions::default()
        },
    )
    .await;
    let events = run
        .harness
        .run("delegate if authorized")
        .await
        .expect("the first run completes");

    let first = request_tools(&run.harness, 0);
    assert!(
        has(&first, LOADER),
        "first request offers the loader: {first:?}"
    );
    assert!(!has(&first, "subagent"), "and not subagent: {first:?}");
    assert!(
        has(&first, "bg_wait"),
        "support tools stay active: {first:?}"
    );

    let ends = tool_ends(&events);
    let (name, result, is_error) = ends[0];
    assert_eq!(name, LOADER);
    assert!(!is_error, "{result:#}");
    assert!(
        result.to_string().contains("Enabled: subagent."),
        "the loader's own success text: {result:#}"
    );

    let events = run
        .harness
        .run("now list the agents")
        .await
        .expect("the second run completes");
    let next = request_tools(&run.harness, 2);
    assert!(
        has(&next, "subagent"),
        "the next prompt carries subagent: {next:?}"
    );
    assert!(has(&next, LOADER), "{next:?}");
    let ends = tool_ends(&events);
    assert_eq!(
        ends[0].0, "subagent",
        "the subagent call dispatches: {events:#?}"
    );
}

fn tool_ends(
    events: &[cyrup_session_svc::AgentSessionEvent],
) -> Vec<(&str, &serde_json::Value, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => Some((tool_name.as_str(), result, *is_error)),
            _ => None,
        })
        .collect()
}

/// The default (`auto`) on the faux model: `subagent` from the first request, no loader.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_on_the_faux_model_starts_with_subagent_and_no_loader() {
    let run = session(
        None,
        vec![FauxResponse::text("ok")],
        HarnessOptions::default(),
    )
    .await;
    run.harness.run("hi").await.expect("run");
    let first = request_tools(&run.harness, 0);
    assert!(has(&first, "subagent") && !has(&first, LOADER), "{first:?}");
}

/// SUBA-153's `[CYRUP-DELTA]`, pinned on a real session: an `anthropic-messages` model whose
/// compat flags satisfy upstream's predicate still starts eager, because no cyrup adapter emits
/// native mid-conversation tool additions yet (`cyrup_provider::api::emits_native_tool_additions`,
/// PROV-133). Upstream would offer the loader here; when PROV-133 lands this test flips.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_on_a_capable_anthropic_model_stays_eager_until_the_adapter_emits_tool_additions() {
    let mut model = cyrup_test_support::response::faux_model();
    model.api = cyrup_provider::known_api::ANTHROPIC_MESSAGES.into();
    model.compat = Some(cyrup_provider::api::compat::ModelCompat {
        supports_mid_convo_system_messages: Some(true),
        supports_mid_convo_tool_changes: Some(true),
        ..Default::default()
    });
    let run = session(
        Some(ToolActivationMode::Auto),
        vec![FauxResponse::text("ok")],
        HarnessOptions {
            model: Some(model),
            ..HarnessOptions::default()
        },
    )
    .await;
    run.harness.run("hi").await.expect("run");
    let first = request_tools(&run.harness, 0);
    assert!(has(&first, "subagent") && !has(&first, LOADER), "{first:?}");
}

/// `eager`: no loader is registered at all (upstream `:86-87`, test `:268-277`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eager_registers_no_loader() {
    let run = session(
        Some(ToolActivationMode::Eager),
        vec![FauxResponse::text("ok")],
        HarnessOptions::default(),
    )
    .await;
    run.harness.run("hi").await.expect("run");
    let first = request_tools(&run.harness, 0);
    assert!(has(&first, "subagent") && !has(&first, LOADER), "{first:?}");
    let all: Vec<String> = run
        .harness
        .session()
        .all_tools()
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert!(!has(&all, LOADER), "{all:?}");
}
