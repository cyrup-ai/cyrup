//! UW-3 — a watchdog review longer than the extension dispatcher's per-handler budget still
//! completes, in BOTH places it is awaited inside an `AgentEnd` handler: the main session's
//! watchdog (`extension/host/native_impl.rs`) and an armed subagent child's
//! (`prompt_runtime.rs` → `register_child.rs`).
//!
//! # Why the existing test could not see this
//!
//! `watchdog_model_turn_integration.rs` scripts the review with an INSTANT provider, so the review
//! finished long before `cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET` (5 s). The dispatcher drops any
//! handler future still running at 5 s — and the review's own bound is `agentEndTimeoutMs`, 30 s. A
//! real model call routinely takes longer than 5 s. Here the review response is DELAYED past the
//! budget (`REVIEW_DELAY_MS`), and the finding it carries must still arrive.
//!
//! Killing mutation for both tests: the review awaited without its declared
//! `SanctionedWaitKind::ModelReview` wait (`handle_agent_end_in_handler` → `handle_agent_end`) —
//! the handler is dropped at 5 s and no warning ever reaches the runtime.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext::native::{ExtMode, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::{FauxResponse, FauxToolCall};

/// Past the 5 s dispatch budget, well inside the review's own 30 s bound.
const REVIEW_DELAY_MS: u64 = 6_500;

const SLOW_SUMMARY: &str = "UW3_SLOW_REVIEW_SUMMARY: found only after the budget elapsed";

const _: () = assert!(
    REVIEW_DELAY_MS as u128 > cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET.as_millis(),
    "the review must outlast the dispatch budget, or this file tests nothing"
);

/// The main turn (writes a file, then answers), then a SLOW review turn that warns.
fn script() -> Vec<FauxResponse> {
    vec![
        FauxResponse::tool_call(
            "write",
            serde_json::json!({
                "path": "watchdog-probe.txt",
                "content": "written by the scripted turn\n",
            }),
        ),
        FauxResponse::text("wrote the probe file"),
        FauxResponse {
            tool_calls: vec![FauxToolCall::new(
                "watchdog_warn",
                serde_json::json!({
                    "severity": "blocker",
                    "summary": SLOW_SUMMARY,
                    "evidence": "the review took longer than the dispatch budget",
                    "recommendedAction": "declare the wait",
                }),
            )],
            delay_ms: Some(REVIEW_DELAY_MS),
            ..FauxResponse::default()
        },
        FauxResponse::text("review complete"),
    ]
}

/// The MAIN session's watchdog: its boundary review, armed through `/subagents-watchdog session
/// on`, outlives the budget and its finding lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_main_watchdogs_slow_review_is_not_cut_by_the_dispatch_budget() {
    let home = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            async_by_default: false,
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension.clone() as Arc<dyn NativeExtension>],
        responses: script(),
        queue_responses: true,
        ..HarnessOptions::default()
    })
    .await
    .expect("a real session with the real SubagentsExtension");
    let ctx =
        cyrup_ext::native::HostCtx::command(ExtMode::Json, false, work_dir.path().to_path_buf());
    extension
        .execute_command("subagents-watchdog", "session on", &ctx)
        .await
        .expect("armed");

    let started = std::time::Instant::now();
    harness
        .run("please write the probe file")
        .await
        .expect("the turn completes");

    let snapshot = extension.watchdog().get_snapshot(None);
    let warning = snapshot.last_warning.as_ref().unwrap_or_else(|| {
        panic!(
            "the slow review's finding never arrived — the handler was dropped at the budget \
             ({:?} elapsed); snapshot: {snapshot:#?}",
            started.elapsed()
        )
    });
    assert_eq!(warning.summary, SLOW_SUMMARY);
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(REVIEW_DELAY_MS),
        "the review really did take longer than the budget"
    );
}

/// An ARMED CHILD's watchdog: the same slow review, driven through the child's own prompt runtime
/// exactly as a spawned subagent builds it — from its env, with the encoded child config the parent
/// hands over. Its finding lands, which means the handler survived to emit its terminal status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_armed_childs_slow_review_is_not_cut_by_the_dispatch_budget() {
    let config = cyrup_ext_subagents::watchdog::child_status::ChildWatchdogConfig {
        enabled: true,
        run_id: Some("uw3-child-run".to_string()),
        agent: Some("worker".to_string()),
        child_index: None,
        watchdog_tail_timeout_ms: 120_000,
        agent_end_timeout_ms: 30_000,
        max_warnings: None,
        model: None,
        thinking: None,
        lsp: cyrup_ext_subagents::watchdog::types::WatchdogLspConfig {
            enabled: false,
            timeout_ms: 1_000,
            max_files: 5,
            max_diagnostics: 7,
        },
        auto_follow_blockers: false,
        auto_follow_max_attempts: None,
        stalemate_repeats: 2,
    };
    let encoded =
        cyrup_ext_subagents::watchdog::child_status::encode_child_watchdog_config(Some(&config))
            .expect("encodes");
    let runtime = Arc::new(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_from_env(&move |key: &str| {
            (key == cyrup_ext_subagents::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV)
                .then(|| encoded.clone())
        })
        .expect("builds")
        .expect("an armed child gets a prompt runtime"),
    );
    let watchdog = Arc::clone(runtime.watchdog().expect("the child watchdog is armed"));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![runtime as Arc<dyn NativeExtension>],
        responses: script(),
        queue_responses: true,
        ..HarnessOptions::default()
    })
    .await
    .expect("a real session with the child's prompt runtime");

    harness
        .run("please write the probe file")
        .await
        .expect("the turn completes");

    let snapshot = watchdog.runtime().get_snapshot(None);
    let warning = snapshot.last_warning.as_ref().unwrap_or_else(|| {
        panic!("the child's slow review was dropped at the budget; snapshot: {snapshot:#?}")
    });
    assert_eq!(warning.summary, SLOW_SUMMARY);
}
