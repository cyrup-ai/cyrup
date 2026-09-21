//! VL-S6's **advertise-vs-dispatch** seam for the seven inspector/project verbs.
//!
//! The schema (`schema.rs`), the model-facing action list (`text.rs`'s `SUBAGENT_ACTIONS`) and
//! `route_action`'s two guard arms are three separate places naming the same seven strings, and
//! the enum in the middle — [`InspectorAction`]/[`ProjectPaneAction`] and their `from_wire` — is
//! what joins them. Nothing crossed that join: every unit test in `inspectors/actions.rs`,
//! `inspectors/herdr/actions.rs` and `inspectors/herdr/project_panes.rs` enters BELOW it already
//! holding the enum, so deleting any single `from_wire` arm left the whole suite green while the
//! schema kept telling the model the verb existed. The model would then call it and get
//! `Unknown action: project.close. … Valid: … project.close …` — a refusal that lists the verb
//! it is refusing — which is the standing bar's *"a verb that is advertised"* failure exactly.
//!
//! This file is the join, asserted three ways:
//!
//! 1. the seven strings are advertised, in BOTH lists;
//! 2. `as_str` → `from_wire` round-trips for every variant, so no arm may be dropped or aliased;
//! 3. every one of the seven, driven through the REAL [`cyrup_core::Tool::execute`], lands on its
//!    OWN answer and never on the unknown-action fallback.
//!
//! Row 3 is the one that catches a deleted `from_wire` arm at the place the model would feel it:
//! `routing.rs`'s guard is `from_wire(..).is_some()`, so an arm that stops resolving does not
//! change any signature, does not fail to compile, and silently moves the verb to the default.
//!
//! In-crate rather than in `cyrup-it` for the reason `lane_actions_tests.rs` gives: `cyrup-it` is
//! `required-features = ["it"]` and off the `cargo test --workspace` merge gate, so a
//! reachability test living only there would gate nothing.
//!
//! **Nothing here needs herdr or ghostty installed**, and that is deliberate: with neither
//! present, `inspector.*` refuses at `resolve_target` and `project.*` answers from the binding
//! file (or produces upstream's install sentence). Every one of those is a real answer from the
//! real arm, which is all row 3 claims.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult};

use crate::extension::executor::SubagentExecutor;
use crate::extension::testsupport::tool_text;
use crate::extension::tool::SubagentTool;
use crate::inspectors::types::{InspectorAction, ProjectPaneAction};

/// The seven wire strings, spelled out here rather than derived from the enums — a test that
/// derived them would agree with a renamed arm instead of catching it.
const VL_S6_VERBS: [&str; 7] = [
    "inspector.open",
    "inspector.command",
    "inspector.status",
    "inspector.close",
    "project.open",
    "project.status",
    "project.close",
];

/// The first words of `route_action`'s default arm — `text.rs`'s `unknown_action_message`, whose
/// body is `Unknown action: {action}. Use subagent({ action: "status" }) … Valid: …`. An
/// advertised verb whose `from_wire` arm was deleted lands here, because the guard that would
/// have dispatched it is `from_wire(..).is_some()`.
const UNKNOWN_ACTION_PREFIX: &str = "Unknown action:";

async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
    tool.execute(
        ToolCallId::from("t"),
        params,
        CancelToken::new(),
        Box::new(|_u: cyrup_core::ToolUpdate| {}),
    )
    .await
}

fn text_of(reply: &Result<ToolResult, ToolError>) -> String {
    match reply {
        Ok(result) => tool_text(result),
        Err(error) => error.to_string(),
    }
}

/// Every one of the seven is advertised in BOTH model-facing lists.
///
/// `subagent_actions()` is the did-you-mean / unknown-action list and the schema's `enum` is what
/// the model's tool definition carries; a verb in one and not the other is already a divergence,
/// and a verb in neither is unreachable by name.
#[test]
fn the_seven_verbs_are_advertised_in_both_lists() {
    let advertised = crate::extension::tool::text::subagent_actions();
    let schema = crate::extension::tool::schema::subagent_tool_parameters();
    let enumerated: Vec<String> = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("the action property carries an enum")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();

    for verb in VL_S6_VERBS {
        assert!(
            advertised.contains(&verb),
            "{verb} is missing from SUBAGENT_ACTIONS"
        );
        assert!(
            enumerated.iter().any(|value| value == verb),
            "{verb} is missing from the tool schema's action enum"
        );
    }
}

/// `as_str` → `from_wire` is a bijection over both enums, and neither accepts the other's verbs.
///
/// *Gutted by*: deleting any one arm of either `from_wire` (that variant's round trip becomes
/// `None`); making an arm return the wrong variant (the equality fails).
#[test]
fn every_verb_round_trips_through_the_contract() {
    for verb in [
        InspectorAction::Open,
        InspectorAction::Command,
        InspectorAction::Status,
        InspectorAction::Close,
    ] {
        assert_eq!(
            InspectorAction::from_wire(verb.as_str()),
            Some(verb),
            "{} does not round-trip",
            verb.as_str()
        );
        assert_eq!(
            ProjectPaneAction::from_wire(verb.as_str()),
            None,
            "{} must not resolve as a project verb",
            verb.as_str()
        );
    }
    for verb in [
        ProjectPaneAction::Open,
        ProjectPaneAction::Status,
        ProjectPaneAction::Close,
    ] {
        assert_eq!(
            ProjectPaneAction::from_wire(verb.as_str()),
            Some(verb),
            "{} does not round-trip",
            verb.as_str()
        );
        assert_eq!(
            InspectorAction::from_wire(verb.as_str()),
            None,
            "{} must not resolve as an inspector verb",
            verb.as_str()
        );
    }
    assert_eq!(InspectorAction::from_wire("inspector.reopen"), None);
    assert_eq!(ProjectPaneAction::from_wire("project.reopen"), None);
}

/// **The seam, through the live dispatch.** Each verb is called with deliberately underspecified
/// params, so each must land on its OWN refusal or answer — never on `unknown subagent action`.
///
/// What each one answers with nothing installed:
///
/// * `inspector.*` — `resolve_target` refuses (`Inspector actions require id or dir.`), which is
///   upstream's own sentence and proves the inspector arm ran.
/// * `project.status` / `project.close` — answered from the binding file with **zero** herdr
///   calls, so they are real answers on a box with no herdr.
/// * `project.open` — upstream's install sentence, through `UnavailableHerdrClient`.
///
/// *Gutted by*: deleting any single `from_wire` arm. The two guard arms in `routing.rs` are
/// `ProjectPaneAction::from_wire(..).is_some()` and `InspectorAction::from_wire(..).is_some()`,
/// so a deleted arm compiles, keeps both lists advertising the verb, and drops it to the default
/// — which answers [`UNKNOWN_ACTION_PREFIX`] and names the verb in its own `Valid:` list.
#[tokio::test]
async fn every_verb_reaches_its_own_arm_through_the_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    let tool = SubagentTool::new(executor, dir.path().to_path_buf());

    for verb in VL_S6_VERBS {
        let text = text_of(&dispatch(&tool, serde_json::json!({ "action": verb })).await);
        assert!(
            !text.starts_with(UNKNOWN_ACTION_PREFIX),
            "{verb} is advertised but lands on the unknown-action arm: {text}"
        );
        assert!(
            !text.is_empty(),
            "{verb} answered with nothing at all, which is not an arm running"
        );
    }
}

// =================================================================================================
// The in-session project-pane map, through the tool
// =================================================================================================

/// Seed a project-pane binding for `root`, as literal JSON.
///
/// Literal rather than through the (private) writer on purpose, for the reason
/// `lane_actions_tests.rs` gives about its manifest: it also pins the on-disk spelling this verb
/// reads, and the file is shared with pi.
fn seed_project_pane_binding(root: &std::path::Path, pane_id: &str) {
    let path = crate::inspectors::herdr::project_pane_binding_path(root);
    std::fs::create_dir_all(path.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "kind": "herdr-project-pane",
            "projectRoot": root,
            "paneId": pane_id,
            "openedAt": "2026-09-21T00:00:00.000Z",
            "herdrVersion": "0.9.1",
            "command": "cyrup",
        }))
        .expect("serialize"),
    )
    .expect("write the binding");
}

/// A herdr seam that answers `pane get` with a live, idle, owning pane and `--version` with
/// 0.9.1 — the only two calls `ProjectPaneManager::status` makes.
///
/// herdr is not installed in this container, and `project.status` on a root that HAS a binding
/// does talk to herdr (`ProjectPaneManager::inspect_pane`, `project_panes.rs:893`), so the insert
/// half of the map needs this fake. The remove half below needs no seam at all.
struct LivePaneHerdr {
    pane_id: String,
    cwd: String,
}

#[async_trait::async_trait]
impl crate::inspectors::plugins::HerdrClient for LivePaneHerdr {
    async fn run(
        &self,
        args: &[&str],
    ) -> Result<serde_json::Value, crate::inspectors::types::HerdrErrorCode> {
        if args == ["--version"] {
            return Ok(serde_json::Value::String("0.9.1".into()));
        }
        Ok(serde_json::json!({ "pane": {
            "pane_id": self.pane_id,
            "terminal_id": "t-1",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "focused": false,
            "cwd": self.cwd,
            "agent_status": "idle",
            "revision": 3,
        }}))
    }
}

/// **The INSERT half.** `project.status` remembers into the executor's own map, and both readers
/// of that map see it.
///
/// The map is `SubagentExecutor::herdr_project_panes`, handed over by
/// [`SubagentExecutor::herdr_project_pane_map`] — the exact expression `routing.rs`'s `project.*`
/// arm passes as `ProjectPaneDeps::panes`. Its two readers are
/// `herdr_project_pane_snapshots()` → `FleetState::herdr_project_panes` →
/// `tui::fleet_status::project_pane_entries` (the human's roster) and
/// `open_herdr_project_pane_count()` → the herdr pane label's `" · N panes"` suffix.
///
/// *Gutted by*: `remember`'s early return on `None` being made unconditional; `panes` on
/// [`ProjectPaneDeps`] going back to a value the handler cannot write through.
#[tokio::test]
async fn project_status_remembers_into_the_executors_own_map() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
    seed_project_pane_binding(&root, "w1:p7");

    let executor = Arc::new(SubagentExecutor::new());
    assert!(executor.herdr_project_pane_snapshots().is_empty());

    let client = LivePaneHerdr {
        pane_id: "w1:p7".to_owned(),
        cwd: root.to_string_lossy().into_owned(),
    };
    let reply = crate::inspectors::herdr::handle_herdr_project_pane_action(
        ProjectPaneAction::Status,
        &crate::inspectors::herdr::ProjectPaneParams::default(),
        crate::inspectors::herdr::ProjectPaneDeps {
            cwd: root.clone(),
            client: &client,
            panes: Some(executor.herdr_project_pane_map()),
        },
    )
    .await;
    assert!(reply.is_ok(), "status answered: {}", text_of(&reply));

    let snapshots = executor.herdr_project_pane_snapshots();
    assert_eq!(
        snapshots.len(),
        1,
        "the roster reads this map and must see the pane: {snapshots:?}"
    );
    assert_eq!(snapshots[0].pane_id, "w1:p7");
    assert_eq!(snapshots[0].project_root, root);
    assert_eq!(
        executor.open_herdr_project_pane_count(),
        1,
        "the pane label's ` · N panes` suffix reads the same map"
    );
}

/// **The REMOVE half, through the REAL tool** — and therefore the proof that `routing.rs` passes
/// the live map rather than `None`.
///
/// The map is filled by the production `SessionStart` call
/// ([`SubagentExecutor::restore_herdr_project_panes`], `native_impl.rs`'s `SessionStart` arm),
/// which reads binding files and calls herdr not at all. The binding is then removed, which is
/// what another cyrup process closing that pane leaves behind. `project.close` in THIS session
/// must reconcile its own map: upstream's `Absent` answer is not an error, and `:706` prunes the
/// root index on that path too.
///
/// Reachable with **no herdr installed**, which is the point: this is the one project-pane path
/// that reaches `panes` without a herdr call, so it pins the thread itself.
///
/// *Gutted by*: `panes: None` in `routing.rs`'s `project.*` arm — the shape this batch replaced.
/// With it, the roster keeps showing a pane that is gone until the session restarts. Also gutted
/// by deleting the `panes.remove(..)` in the handler's `Close` arm.
#[tokio::test]
async fn project_close_through_the_tool_forgets_the_pane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
    seed_project_pane_binding(&root, "w1:p7");

    let executor = Arc::new(SubagentExecutor::new());
    // `native_impl.rs`'s `SessionStart` arm, verbatim.
    executor.restore_herdr_project_panes(&root);
    assert_eq!(
        executor.herdr_project_pane_snapshots().len(),
        1,
        "the session-start restore is what puts the pane in the map"
    );

    // Another process closed the pane and removed the binding.
    std::fs::remove_file(crate::inspectors::herdr::project_pane_binding_path(&root))
        .expect("remove the binding");

    let tool = SubagentTool::new(Arc::clone(&executor), root.clone());
    let text = text_of(&dispatch(&tool, serde_json::json!({ "action": "project.close" })).await);
    assert!(
        !text.starts_with(UNKNOWN_ACTION_PREFIX),
        "project.close must reach its arm: {text}"
    );
    assert!(
        executor.herdr_project_pane_snapshots().is_empty(),
        "project.close must forget the pane, or the roster keeps showing a closed one; got {:?}",
        executor.herdr_project_pane_snapshots()
    );
    assert_eq!(executor.open_herdr_project_pane_count(), 0);
}
