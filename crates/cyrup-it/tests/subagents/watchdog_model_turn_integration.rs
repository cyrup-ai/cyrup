//! UW-4 reachability: the main-session watchdog review runs a REAL nested model turn and a
//! `watchdog_warn` call it makes reaches the runtime — through the production wiring, end to end.
//!
//! # Why this file exists
//!
//! `watchdog/` is eighteen modules and ~18k lines, fully ported and fully wired, and until UW-4 the
//! one thing it could not do was produce a finding: the bound `WatchdogReviewAgent` was
//! `NoTurnReviewAgent`, whose `run` returned `Ok(Vec::new())`. Every unit test in
//! `watchdog::review` drives that stub or a `#[cfg(test)]` double, so all of them passed over a
//! machine that could never warn. `/subagents-watchdog status` said "real model review" the whole
//! time.
//!
//! So the assertion that matters here is not "the review ran". It is that a string which exists
//! ONLY inside a scripted model response came out the other end as a watchdog warning — which can
//! only happen if a real [`cyrup_agent::Agent`] was built, streamed, called a tool named
//! `watchdog_warn`, and that call reached `WatchdogWarnTool::execute` and the runtime's emission
//! guard.
//!
//! # What is production here, and what is scripted
//!
//! Production: [`cyrup_ext_subagents::extension::SubagentsExtension`] (built through its real
//! constructor, which calls `register_main_watchdog`, which binds the real
//! `review::ModelTurnReviewAgent`), a real `SessionBuilder`-assembled `AgentSession` with the real
//! `LiveHostServices`, the real `/subagents-watchdog` command handler, the real `write` built-in,
//! the real agent-end boundary, the real turn-delta formatter, the real emission guard and the real
//! warning-injection sink.
//!
//! Scripted: only the LLM. Both the OUTER session turn and the NESTED review turn consume the same
//! `ScriptedProvider` queue, which is what `HostServices::registered_provider` (GAP-2) makes
//! possible — the nested turn asks the live session for its own provider and gets the harness's,
//! so this whole file runs offline.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext::native::{ExtMode, NativeExtension};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::watchdog::register_main::WatchdogServicesFn;
use cyrup_ext_subagents::watchdog::review::ModelTurnReviewAgent;
use cyrup_test_support::harness::{Harness, HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::{FauxResponse, FauxToolCall};

/// The scripted strings the model "discovers". They appear NOWHERE else in the workspace, so an
/// assertion on them is an assertion that the model turn happened.
const SCRIPTED_SUMMARY: &str = "UW4_SCRIPTED_SUMMARY: the turn dropped the caller's constraint";
const SCRIPTED_EVIDENCE: &str = "UW4_SCRIPTED_EVIDENCE: watchdog-probe.txt was written blind";
const SCRIPTED_ACTION: &str = "UW4_SCRIPTED_ACTION: restate the constraint before writing";

/// A `watchdog_warn` call as the review model would emit it.
fn watchdog_warn_call() -> FauxResponse {
    FauxResponse {
        tool_calls: vec![FauxToolCall::new(
            "watchdog_warn",
            serde_json::json!({
                "severity": "blocker",
                "summary": SCRIPTED_SUMMARY,
                "evidence": SCRIPTED_EVIDENCE,
                "recommendedAction": SCRIPTED_ACTION,
            }),
        )],
        ..FauxResponse::default()
    }
}

/// Build the harness with the real extension, arm the watchdog through its real slash command, and
/// drive one turn whose scripted response WRITES a file (which is what makes the agent-end boundary
/// review at all — `review_changes_only` is on, so a turn that edited nothing is skipped).
///
/// `review_script` is appended after the main turn's two responses and is what the NESTED review
/// agent consumes.
///
/// The two tempdirs come back with the harness rather than being leaked, because the REVIEWER's
/// cwd is `work_dir` — a test that asserts the reviewer wrote nothing has to be able to name it.
async fn armed_harness(
    review_script: Vec<FauxResponse>,
) -> (Harness, Arc<SubagentsExtension>, TempDirs) {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("extension cwd");
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            async_by_default: false,
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));
    let mut responses = vec![
        // Turn 1: the assistant writes a file. A successful `write` tool result is what
        // `event_indicates_repo_edit` keys on, and it is the ONLY way to reach the review in a
        // non-git working tree.
        FauxResponse::tool_call(
            "write",
            serde_json::json!({
                "path": "watchdog-probe.txt",
                "content": "written by the scripted turn\n",
            }),
        ),
        // Turn 2: the assistant's follow-up, which ends the run and reaches `agent_end`.
        FauxResponse::text("wrote the probe file"),
    ];
    responses.extend(review_script);

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension.clone() as Arc<dyn NativeExtension>],
        responses,
        // The QUEUE-consuming faux flavour, so the outer turn and the nested review turn take
        // successive entries instead of cycling the same one forever.
        queue_responses: true,
        ..HarnessOptions::default()
    })
    .await
    .expect("a real, fully-wired AgentSession with the real SubagentsExtension");

    // The watchdog is default-off (`settings.ts:72`). Arm it exactly as an operator does —
    // `/subagents-watchdog session on` — through the extension's real command handler, and AFTER
    // the session start that would otherwise clear the override (`runtime.ts:187-193`).
    let ctx =
        cyrup_ext::native::HostCtx::command(ExtMode::Json, false, work_dir.path().to_path_buf());
    extension
        .execute_command("subagents-watchdog", "session on", &ctx)
        .await
        .expect("the watchdog command is registered and handles `session on`");
    assert_eq!(
        extension.watchdog().get_snapshot(None).session_override,
        Some(true),
        "the slash command must actually arm the runtime, or nothing below proves anything"
    );
    // The temp dirs stay alive for the whole test by being RETURNED, not by being leaked: the
    // extension's cwd is the reviewer's cwd, and an assertion about what the reviewer did or did
    // not write has to resolve against it.
    (
        harness,
        extension,
        TempDirs {
            work_dir,
            _home: home,
        },
    )
}

/// The tempdirs `armed_harness` owns. `work_dir` is the extension's cwd, which is what
/// `register_main_watchdog` hands `ModelTurnReviewAgent::new` — so it is the directory a reviewer's
/// relative tool path would resolve against.
struct TempDirs {
    work_dir: tempfile::TempDir,
    _home: tempfile::TempDir,
}

/// **UW-4, the strong one.** A real nested model turn calls `watchdog_warn`, and the warning it
/// carries — scripted strings that exist nowhere else — arrives at the runtime.
///
/// Over the gutted implementation (`NoTurnReviewAgent`'s `Ok(Vec::new())`) no turn happens, no tool
/// is called, `WatchdogWarnTool::execute` never runs, the emission guard never sees a warning and
/// `last_warning` stays `None` — so assertions (1) and (2) fail. It also fails if the review runs a
/// turn but never surfaces the warn tool, surfaces it under another name, or synthesises the result
/// text instead of routing through `WatchdogWarnTool::execute`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_review_turn_emits_a_watchdog_warn_finding_through_the_production_path() {
    let (harness, extension, dirs) = armed_harness(vec![
        // The REVIEW turn: the nested agent calls `watchdog_warn`.
        watchdog_warn_call(),
        // Its follow-up, after it reads the tool result back.
        FauxResponse::text("review complete"),
    ])
    .await;

    harness
        .run("please write the probe file")
        .await
        .expect("the turn completes without a transport/session-level error");

    let snapshot = extension.watchdog().get_snapshot(None);

    // (1) A finding reached the runtime at all.
    let warning = snapshot.last_warning.as_ref().unwrap_or_else(|| {
        panic!("no watchdog warning reached the runtime; snapshot: {snapshot:#?}")
    });

    // (2) It carries the SCRIPTED strings — the part that can only have come through a model turn.
    assert_eq!(warning.summary, SCRIPTED_SUMMARY);
    assert_eq!(warning.evidence, SCRIPTED_EVIDENCE);
    assert_eq!(warning.recommended_action, SCRIPTED_ACTION);

    // (3) `/subagents-watchdog status`'s "real model review" is now TRUE rather than aspirational.
    assert_eq!(snapshot.review_description, "real model review");

    // (4) The review itself succeeded — a resolution/transport failure would have been recorded.
    assert_eq!(
        snapshot.failed_reviews, 0,
        "the review must have completed cleanly; snapshot: {snapshot:#?}"
    );

    // (5) The NESTED turn was built the way `review.ts:304-342 @v0.68.0` builds it. The faux
    // provider records every request context it was handed, so this reads the actual wire request
    // the review agent made: the watchdog system prompt, and EXACTLY the EXPOSE list
    // (`WATCHDOG_REVIEW_READ_ONLY_TOOL_NAMES` + the bound `watchdog_warn`) — no `bash`, no `edit`,
    // no `write`, no `subagent`.
    let review_request = review_context(&harness)
        .unwrap_or_else(|| panic!("no nested review request reached the provider"));
    let mut tool_names: Vec<String> = review_request
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    tool_names.sort();
    assert_eq!(
        tool_names,
        vec!["find", "grep", "ls", "read", "watchdog_warn"],
        "the review's tool list is the read-only four plus the bound warn tool"
    );

    // (6) The CONTROL for the negative test below: a `write` this session performs really does
    // land in `harness.cwd()`, so an assertion that a file is ABSENT from these two directories is
    // an assertion that can fail.
    assert!(
        harness.cwd().join("watchdog-probe.txt").exists(),
        "the scripted turn's own write must be visible in the session cwd, or the negative \
         assertions in this file prove nothing; cwd: {}",
        harness.cwd().display()
    );
    assert!(
        !dirs.work_dir.path().join("watchdog-probe.txt").exists(),
        "the session's write belongs to the session cwd, not the extension cwd"
    );
}

/// The first request context whose system prompt is the WATCHDOG's — i.e. the nested review turn,
/// as distinct from the outer session's turns.
fn review_context(harness: &Harness) -> Option<cyrup_provider::Context> {
    harness.faux().contexts.into_iter().find(|ctx| {
        ctx.system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("You are the main-session subagent watchdog"))
    })
}

/// Every request context the nested review turn produced, in order.
fn review_contexts(harness: &Harness) -> Vec<cyrup_provider::Context> {
    harness
        .faux()
        .contexts
        .into_iter()
        .filter(|ctx| {
            ctx.system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("You are the main-session subagent watchdog"))
        })
        .collect()
}

/// **UW-4's negative half.** The reviewer is read-only in fact, not just in its system prompt: the
/// review turn names `write`, both layers of the tool policy refuse it, nothing is written, the run
/// is not derailed, and the review goes on to report its finding.
///
/// The two layers are asserted separately because they fire at different points and only one of
/// them can be reached by scripting a model response:
///
/// - **Layer one, the tool LIST** (`review.ts:305 @v0.68.0`): asserted off the real wire request —
///   the reviewer was never offered `write`, so the loop answers "Tool write not found". That is
///   the refusal this scripted turn actually receives, and `cyrup-agent` guarantees it comes
///   first, because preflight locates the tool before running `before_tool_call`
///   (`crates/cyrup-agent/src/agent/run/tools/preflight.rs:17,80`).
/// - **Layer two, EXECUTION TIME** (`:338-340`): asserted against the policy the production
///   `ModelTurnReviewAgent` installs on every turn it runs, with upstream's exact sentence. It
///   cannot be reached by a scripted response *precisely because* layer one always wins here, so
///   it is reached through the production agent instead of being assumed. Upstream's second layer
///   exists for harness-supplied tools; here it guards a future widening of
///   `cyrup_tools::read_only_tools` — at which point layer one would stop catching `write` and
///   this is the only thing left standing between a reviewer and a mutation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_review_that_names_a_write_tool_is_refused_at_execution_time() {
    let probe = "watchdog-review-must-not-write.txt";
    let (harness, extension, dirs) = armed_harness(vec![
        // The REVIEW turn names `write` — which is in neither the EXPOSE list nor the PERMIT list.
        FauxResponse::tool_call(
            "write",
            serde_json::json!({ "path": probe, "content": "the reviewer must never write\n" }),
        ),
        // Having been refused, it reports a real finding instead.
        watchdog_warn_call(),
        FauxResponse::text("review complete"),
    ])
    .await;

    harness
        .run("please write the probe file")
        .await
        .expect("the turn completes");

    let snapshot = extension.watchdog().get_snapshot(None);
    // The refusal did not derail the review: it went on to warn, so the turn genuinely continued
    // past the blocked call rather than dying on it.
    let warning = snapshot
        .last_warning
        .as_ref()
        .unwrap_or_else(|| panic!("the review must survive a blocked tool call; {snapshot:#?}"));
    assert_eq!(warning.summary, SCRIPTED_SUMMARY);

    // THE assertion, read off the real wire request the nested agent made: the reviewer was never
    // given `write` (nor `edit`, `bash` or `subagent`), the refusal came back to it as an ERROR tool
    // result, and it then kept going. `read`/`grep`/`find`/`ls` are present, so this is a statement
    // about the FILTER and not about an empty tool list.
    let contexts = review_contexts(&harness);
    let last = contexts
        .last()
        .unwrap_or_else(|| panic!("no nested review request reached the provider"));
    let offered: Vec<&str> = last.tools.iter().map(|t| t.name.as_str()).collect();
    for forbidden in ["write", "edit", "bash", "subagent"] {
        assert!(
            !offered.contains(&forbidden),
            "the reviewer must never be offered '{forbidden}'; offered: {offered:?}"
        );
    }
    assert!(offered.contains(&"read") && offered.contains(&"watchdog_warn"));
    let wire = serde_json::to_string(&contexts).expect("the captured contexts serialize");
    assert!(
        wire.contains(
            r#""toolName":"write","content":[{"type":"text","text":"Tool write not found"}]"#
        ),
        "the reviewer's `write` must come back as an error tool result; wire: {wire}"
    );

    // LAYER TWO — the execution-time refusal (`review.ts:338-340 @v0.68.0`), read off the policy
    // the PRODUCTION review agent installs on every turn, with upstream's exact sentence. Gutting
    // `ModelTurnReviewAgent::tool_call_block_reason` to `|_| None` fails here.
    let installed = ModelTurnReviewAgent::new(
        dirs.work_dir.path().to_path_buf(),
        Arc::new(|| None) as WatchdogServicesFn,
    )
    .tool_call_block_reason();
    assert_eq!(
        installed("write").as_deref(),
        Some("Watchdog reviews are read-only; tool 'write' is not allowed."),
        "the reviewer's execution-time policy must refuse `write` with upstream's own sentence"
    );
    assert_eq!(
        installed("read"),
        None,
        "and it must still permit the read-only four"
    );

    // And nothing was written — in the directory a reviewer's relative path actually resolves
    // against. The reviewer's cwd is the EXTENSION's cwd (`register_main_watchdog` hands it to
    // `ModelTurnReviewAgent::new`); the session's cwd is the harness temp dir. Neither may contain
    // the probe. The session's OWN write landing in `harness.cwd()` is what proves these two paths
    // can see a file at all — asserted in the positive test above.
    for dir in [dirs.work_dir.path(), harness.cwd()] {
        assert!(
            !dir.join(probe).exists(),
            "the watchdog review must never write a file; found {}",
            dir.join(probe).display()
        );
    }
    assert!(
        harness.cwd().join("watchdog-probe.txt").exists(),
        "control: the session's own scripted write must be present, or the absence of the \
         reviewer's probe means nothing"
    );
}
