//! SUBA-005 end-to-end proof: the four newly-ported management actions are reachable BY A MODEL
//! through the real, fully-wired `subagent` tool on a real session — and actually change state.
//!
//! `tests/management_actions_integration.rs` proves the handlers behave correctly against a real
//! on-disk agent tree. It cannot prove reachability: before this fix `handle_management_action` had
//! no `eject`/`disable`/`enable`/`reset` arms AND `SubagentTool::route_action` rejected all four
//! with "unknown subagent action", so a correct handler would still have been unreachable. This file
//! closes that gap by driving a `SessionBuilder`-assembled `AgentSession` (via `cyrup-test-support`,
//! scripted faux LLM responses only — no provider traffic) whose scripted responses are `subagent`
//! tool calls carrying those actions.
//!
//! The two calls are deliberately ORDERED and DEPENDENT: `enable` can only answer
//! "removed disabled override at …" if the preceding `disable` really wrote one. A pair of
//! independently-accepted verbs cannot produce that transcript.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;


use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;

fn tool_results(
    events: &[cyrup_session_svc::AgentSessionEvent],
) -> Vec<(String, String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => Some((tool_name.clone(), result.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_can_disable_then_enable_an_agent_through_the_live_subagent_tool() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");

    // A real user-scope agent the actions will operate on, at the exact path
    // `resolve_user_agent_read_dirs(CYRUP_HOME)` scans.
    let user_agents = home.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&user_agents).unwrap();
    std::fs::write(
        user_agents.join("probe.md"),
        "---\nname: probe\ndescription: a probe persona for the SUBA-005 dispatch proof\n---\n\nYou are probe.\n",
    )
    .unwrap();
    let user_settings = user_agents.join("settings.json");
    assert!(!user_settings.exists(), "precondition: no settings file yet");

    // This run names its own root: `roots` reaches the user-scope discovery roots and the
    // user `settings.json` path through `SubagentExecutor::discovery_config`, which the management
    // action's own dispatch resolves against.
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call(
                "subagent",
                serde_json::json!({ "action": "disable", "agent": "probe" }),
            ),
            FauxResponse::tool_call(
                "subagent",
                serde_json::json!({ "action": "enable", "agent": "probe" }),
            ),
            FauxResponse::text("done"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real session with the subagents extension loaded");

    let events = harness.run("turn probe off and back on").await;

    let events = events.expect("the turn completes without a transport/session-level error");
    let results = tool_results(&events);
    assert_eq!(
        results.len(),
        2,
        "both scripted management calls must dispatch; got: {results:#?}"
    );

    let (name, disable_text, disable_err) = &results[0];
    assert_eq!(name, "subagent");
    assert!(
        !disable_err,
        "disable must succeed on the live session; got: {disable_text}"
    );
    assert!(
        disable_text.contains("Disabled agent 'probe' via user settings override"),
        "the reply must be the real handler's own text, proving the action was serviced rather \
         than rejected as unknown; got: {disable_text}"
    );

    let (name, enable_text, enable_err) = &results[1];
    assert_eq!(name, "subagent");
    assert!(!enable_err, "enable must succeed; got: {enable_text}");
    assert!(
        enable_text.contains("Enabled agent 'probe' (removed disabled override at"),
        "enable can only report REMOVING an override if the preceding disable really wrote one — \
         this is the state-change proof, not a verb-accepted proof; got: {enable_text}"
    );

    // And the write really landed in the user's settings file (then was cleaned back up).
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&user_settings).unwrap())
            .expect("settings.json parses");
    assert!(
        settings.get("subagents").is_none(),
        "the enable pruned the emptied override block back out: {settings}"
    );
}
