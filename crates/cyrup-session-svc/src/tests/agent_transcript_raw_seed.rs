//! SESS-043 — the agent transcript must be re-seeded from pi's **raw** `AgentMessage` context, not
//! from the `convertToLlm`-flattened one.
//!
//! pi assigns the raw list at all three re-seed sites — `agent-session.ts:1874-1875` (manual
//! `compact`), `:2155-2156` (`_runAutoCompaction`) and `:3067-3068` (`navigateToEntry`) @v0.83.0 —
//! each `this.agent.state.messages = sessionContext.messages`, where `buildSessionContext`
//! (`session-manager.ts:460-468`) is `buildContextEntries(...).flatMap(sessionEntryToContextMessages)`
//! (`:383-408`) with every role intact. `convertToLlm`
//! (`coding-agent/src/core/messages.ts:148-195`) is applied later, once, at the request boundary.
//!
//! SEAM-112 adds the FOURTH and last one: the build/resume seed, pi `sdk.ts:190` + `:374`
//! (`const existingSession = sessionManager.buildSessionContext();` … `agent.state.messages =
//! existingSession.messages;`). It is the same `buildSessionContext` call, so it carries the same
//! raw roles — and it was the one site cyrup still fed from the flattened twin after SESS-043
//! converted the other three (`session.rs:1748`, `:2219`, `:5072`).
//!
//! cyrup folded `build_context().messages` — the already-flattened projection — through
//! `core_message_to_agent`, whose target enum had no `bashExecution` / `branchSummary` /
//! `compactionSummary` arms at all. The transcript therefore had a different LENGTH and different
//! roles from pi's: an `excludeFromContext` (`!!`) bash message is DROPPED by the flattening and a
//! summary is REWRITTEN into `COMPACTION_SUMMARY_PREFIX … SUFFIX` wrapper prose.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::hooks::coding_agent_convert_to_llm;
use crate::{SessionBuilder, SessionConfig};
use cyrup_core::{Content, Message, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session::agent_message::AgentMessage as Raw;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Compaction settings that force even a small session to compact (keep nothing, reserve nothing).
fn aggressive_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        serde_json::json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

fn text_of(m: &Message) -> String {
    match m {
        Message::User { content, .. } => content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

/// The manual `/compact` re-seed (`agent-session.ts:1874-1875`).
///
/// **Pre-fix this test is red on its first assertion.** The transcript was
/// `build_context().messages.iter().map(core_message_to_agent)`, so the retained compaction summary
/// arrived as a `user` message whose text is `COMPACTION_SUMMARY_PREFIX + summary + SUFFIX` — there
/// was no `AgentMessage::App` variant for it to land in, and `cyrup_agent::AgentMessage` could not
/// express the `compactionSummary` role at all.
#[tokio::test]
async fn compaction_reseeds_the_transcript_from_pi_s_raw_context() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    let _ = session.compact(None).await.expect("compaction succeeds");

    let raw = session.raw_context_messages().await;
    assert!(
        raw.iter().any(|m| matches!(m, Raw::CompactionSummary(_))),
        "a compacted context leads with a compactionSummary — without one the two projections \
         cannot differ and this test would be vacuous (raw = {raw:?})"
    );

    let transcript = session.agent_messages().await;

    // (1) The role survived the seeding. Pre-fix: zero `App` messages exist anywhere, because the
    //     variant did not exist and the summary had already been flattened to a `user` turn.
    let summaries: Vec<_> = transcript
        .iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::App { role, payload } if role == "compactionSummary" => {
                Some(payload.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "pi's transcript holds the compactionSummary AS a compactionSummary; got {transcript:?}"
    );
    // (2) …carrying pi's raw `summary` field, NOT the wrapper prose the LLM projection adds.
    let summary = summaries[0]["summary"].as_str().expect("pi's `summary` field");
    assert!(
        !summary.starts_with("The conversation history before this point was compacted"),
        "the raw projection stores the bare summary; the wrapper belongs to convertToLlm only \
         (got {summary:?})"
    );

    // (3) Length and role sequence match the raw projection element-for-element — pi's assignment
    //     is an identity, so `messages.slice(0, -1)` arithmetic indexes the same turns.
    assert_eq!(
        transcript.len(),
        raw.len(),
        "the transcript IS `sessionContext.messages`; flattening changes the length"
    );

    // (4) …and the request the model actually receives is unchanged: applying pi's `convertToLlm`
    //     to the transcript reproduces `build_context()` exactly. This is the no-regression half —
    //     the flattening now happens once, at the boundary, instead of at seeding time.
    let at_boundary = coding_agent_convert_to_llm(&transcript);
    let flattened = session.messages().await;
    assert_eq!(
        at_boundary.len(),
        flattened.len(),
        "convertToLlm(transcript) must equal build_context().messages"
    );
    let wrapped = at_boundary.iter().map(text_of).collect::<Vec<_>>();
    assert!(
        wrapped
            .iter()
            .any(|t| t.starts_with("The conversation history before this point was compacted")),
        "the wrapper prose reappears at the LLM boundary, where pi puts it: {wrapped:?}"
    );
}

/// SEAM-112 — the **build/resume** seed (pi `sdk.ts:190` + `:374`), the fourth and last site.
///
/// pi resumes with `const existingSession = sessionManager.buildSessionContext();` (sdk.ts:190) and
/// then `agent.state.messages = existingSession.messages;` (sdk.ts:374) — the SAME raw projection
/// the three re-seed sites use, roles intact (`session-manager.ts:461-469` composed with
/// `:383-407`, which returns `custom` / `branchSummary` / `compactionSummary` untouched).
///
/// **RED before the fix on assertions (1) and (2).** `builder.rs` seeded from
/// `manager.build_context()` folded through `core_message_to_agent`, i.e. the
/// `convertToLlm`-flattened twin, whose target enum has no `bashExecution` / `compactionSummary`
/// role at all: the bash execution arrived as a plain `user` turn holding the STRINGIFIED wire
/// object (`cyrup-session/src/agent_message.rs:200`'s `custom_to_message`) and the retained summary
/// as a `user` turn holding `COMPACTION_SUMMARY_PREFIX … SUFFIX`. This is the resume half of the
/// defect `compaction_reseeds_the_transcript_from_pi_s_raw_context` pins for `/compact`; before
/// SEAM-112 the two paths disagreed with each other about what a transcript even contains.
///
/// The `!!` execution is placed AFTER the compaction on purpose: with `keepRecentTokens: 0` an
/// earlier one would be summarized away, and it is the message whose ROLE the two projections
/// disagree about most visibly.
#[tokio::test]
async fn resuming_a_session_seeds_the_transcript_from_pi_s_raw_context() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("CONTEXT SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("TURN PREFIX SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("EXTRA SUMMARY")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider.clone(), base_config(&fx))
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("build");
    let file = session.session_file().await.expect("a persisted session to resume from");

    let _ = session.prompt("tell me one").await.expect("prompt 1");
    session.wait_for_idle().await;
    let _ = session.prompt("tell me two").await.expect("prompt 2");
    session.wait_for_idle().await;
    let _ = session.compact(None).await.expect("compaction succeeds");
    // `!!` — present in pi's RAW context and dropped only by `convertToLlm` (`messages.ts:152-156`).
    let _ = session
        .execute_bash(
            "echo excluded-from-context",
            crate::BashOptions { exclude_from_context: true, id: None, operations: None },
            None,
        )
        .await
        .expect("immediate bash");
    drop(session);

    let mut resume_cfg = base_config(&fx);
    resume_cfg.target = crate::SessionTarget::Resume(file);
    let resumed = SessionBuilder::new(provider, resume_cfg)
        .cli_settings(aggressive_compaction_settings())
        .build()
        .await
        .expect("resume");

    let raw = resumed.raw_context_messages().await;
    assert!(
        raw.iter().any(|m| matches!(m, Raw::CompactionSummary(_))),
        "the resumed branch must lead with a compactionSummary, else the two projections cannot \
         differ and this test is vacuous (raw = {raw:?})"
    );
    assert!(
        raw.iter()
            .any(|m| matches!(m, Raw::Custom(c) if c.custom_type == "bashExecution")),
        "the `!!` execution must be in the RAW context — pi's `sessionEntryToContextMessages` \
         projects a `custom_message` entry as `createCustomMessage(entry.customType, …)` \
         (session-manager.ts:396-400) and drops it only at the request boundary (raw = {raw:?})"
    );

    let transcript = resumed.agent_messages().await;
    // (1) The `!!` execution survived the seed with its ROLE. pi's `sessionEntryToContextMessages`
    //     returns `createCustomMessage(entry.customType, …)` for a `custom_message` entry
    //     (`session-manager.ts:396-400`), so `bashExecution` rides the `custom` arm — the same
    //     shape `record_bash_result` produces for a LIVE execution, which is why
    //     `a_live_bash_execution_renders_like_a_reseeded_one…` above can compare the two.
    //     Pre-fix the role was gone: `build_context()` had already flattened it to a `user` turn.
    assert!(
        transcript.iter().any(|m| matches!(
            m,
            cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "bashExecution"
        )),
        "a resumed transcript holds the excluded bash execution as a bashExecution, exactly as \
         `sessionEntryToContextMessages` returns it (sdk.ts:190,374); got {transcript:?}"
    );
    // (2) …and the compaction summary is a compactionSummary carrying pi's bare `summary` field,
    //     not the `COMPACTION_SUMMARY_PREFIX … SUFFIX` prose the LLM projection adds.
    let summaries: Vec<_> = transcript
        .iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::App { role, payload } if role == "compactionSummary" => {
                Some(payload.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(summaries.len(), 1, "one retained compactionSummary; got {transcript:?}");
    let summary = summaries[0]["summary"].as_str().expect("pi's `summary` field");
    assert!(
        !summary.starts_with("The conversation history before this point was compacted"),
        "the raw projection stores the bare summary; the wrapper belongs to convertToLlm only \
         (got {summary:?})"
    );

    // (3) The seed is pi's IDENTITY assignment (sdk.ts:374): element-for-element the raw
    //     projection, so `messages.slice(0, -1)` arithmetic indexes the same turns on a resumed
    //     session as on a live one.
    assert_eq!(
        transcript.len(),
        raw.len(),
        "`agent.state.messages = existingSession.messages` is an identity (sdk.ts:374); \
         got {transcript:?}"
    );

    // (4) The flattening now happens ONCE, at the request boundary, exactly where pi puts it: the
    //     wrapper prose reappears there, and the `!!` output is dropped there
    //     (`convertToLlm`'s `case \"bashExecution\"`, messages.ts:152-156).
    let at_boundary = coding_agent_convert_to_llm(&transcript);
    let rendered: Vec<String> = at_boundary.iter().map(text_of).collect();
    assert!(
        rendered
            .iter()
            .any(|t| t.starts_with("The conversation history before this point was compacted")),
        "the wrapper prose belongs at the LLM boundary, not in the transcript: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|t| t.contains("excluded-from-context")),
        "`!!` output must never reach the model, on a resumed session either: {rendered:?}"
    );
}

/// `coding_agent_convert_to_llm` is pi's `convertToLlm` (`messages.ts:148-195`), which renders the
/// declaration-merged roles the BASE `defaultConvertToLlm` drops.
///
/// **Coverage, not proof, for the `App` arms:** `AgentMessage::App` is new in this change, so no
/// pre-fix version of this assertion can be written. The `custom` arm below IS a red-before claim:
/// the boundary used to resolve to [`cyrup_agent::default_convert_to_llm`], which returns `None`
/// for `Custom`, so an extension-injected message never reached the model — while the same message
/// DID reach it after any compaction, because `build_context()` had already flattened it to a
/// `user` turn. The two paths disagreed; pi has one.
#[test]
fn the_llm_boundary_renders_pi_s_declaration_merged_roles() {
    let custom = cyrup_agent::AgentMessage::Custom {
        kind: "ext.note".into(),
        payload: serde_json::Value::String("remember the deploy freeze".into()),
        timestamp: Some(11),
    };
    // The pre-fix boundary. Kept as an explicit witness of what changed.
    assert!(
        cyrup_agent::default_convert_to_llm(std::slice::from_ref(&custom)).is_empty(),
        "the BASE convertToLlm drops custom roles (agent/src/harness/messages.ts:120)"
    );

    let branch_summary = serde_json::json!({
        "role": "branchSummary",
        "summary": "explored the retry path",
        "fromId": "e1",
        "timestamp": 12,
    });
    let serde_json::Value::Object(branch_payload) = branch_summary else { unreachable!() };
    let app = cyrup_agent::AgentMessage::App {
        role: "branchSummary".into(),
        payload: branch_payload,
    };

    let out = coding_agent_convert_to_llm(&[custom, app]);
    assert_eq!(out.len(), 2, "both roles render to exactly one user message each: {out:?}");
    assert_eq!(text_of(&out[0]), "remember the deploy freeze", "pi's `case \"custom\"` (:162-168)");
    let branch = text_of(&out[1]);
    assert!(
        branch.starts_with("The following is a summary of a branch that this conversation came back from:"),
        "pi's BRANCH_SUMMARY_PREFIX (messages.ts:20-24) is applied here, not at seeding: {branch:?}"
    );
    assert!(branch.contains("explored the retry path"));
}

/// SESS-043 follow-up (found in adversarial review of this change) — a **live** `!` execution and a
/// **re-seeded** one must render identically at the LLM boundary, and `!!` must be honoured on both.
///
/// `record_bash_result` (`session.rs`) appends a live execution as
/// `Custom { kind: "bashExecution", payload: <BashExecutionMessage body> }` — cyrup overloads that
/// arm's `kind` with pi's role, exactly as the session file does
/// (`append_custom_message("bashExecution", …)` reloads as `Raw::BashExecution`). After a compaction
/// the SAME execution comes back through `raw_message_to_agent` as `App { role: "bashExecution" }`.
///
/// **RED before the fix in `coding_agent_convert_to_llm`'s `Custom` arm.** Making the boundary the
/// coding-agent `convertToLlm` reached that arm for the first time (the base
/// `default_convert_to_llm` had dropped every `Custom`), and it routed straight to
/// `custom_to_message`, whose catch-all stringifies its input. Pre-fix this test failed twice:
/// case (a) produced one message holding the raw JSON object instead of
/// `bash_execution_to_text`, and case (b) — the `!!` case, pi's `case "bashExecution"` returning
/// `undefined` (`messages.ts:152-156` @v0.83.0) — produced a message at all, leaking output the
/// user had explicitly excluded from context.
#[test]
fn a_live_bash_execution_renders_like_a_reseeded_one_and_honours_exclude_from_context() {
    use cyrup_agent::AgentMessage as Agent;
    use cyrup_session::agent_message::BashExecutionMessage;

    let body = |exclude: bool| {
        serde_json::json!({
            "command": "ls -la",
            "output": "a.txt\nb.txt",
            "exitCode": 0,
            "cancelled": false,
            "truncated": false,
            "fullOutputPath": serde_json::Value::Null,
            "excludeFromContext": exclude,
        })
    };
    let reseeded = |exclude: bool| {
        crate::event::raw_message_to_agent(&Raw::BashExecution(BashExecutionMessage {
            command: "ls -la".into(),
            output: "a.txt\nb.txt".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 7,
            exclude_from_context: Some(exclude),
        }))
    };

    // (a) `!` — rendered as pi's `bashExecutionToText`, byte-identical on both paths.
    let live = Agent::Custom {
        kind: "bashExecution".into(),
        payload: body(false),
        timestamp: Some(7),
    };
    let live_out = coding_agent_convert_to_llm(std::slice::from_ref(&live));
    assert_eq!(live_out.len(), 1, "one user turn, as pi's `case \"bashExecution\"` returns");
    assert_eq!(
        text_of(&live_out[0]),
        text_of(&coding_agent_convert_to_llm(&[reseeded(false)])[0]),
        "the live and post-compaction renderings of one execution must not disagree"
    );
    assert!(
        !text_of(&live_out[0]).contains("excludeFromContext"),
        "the wire object must not be stringified into the request: {:?}",
        text_of(&live_out[0])
    );

    // (b) `!!` — dropped from the REQUEST on both paths (`messages.ts:152-156`).
    let excluded = Agent::Custom {
        kind: "bashExecution".into(),
        payload: body(true),
        timestamp: Some(7),
    };
    assert!(
        coding_agent_convert_to_llm(std::slice::from_ref(&excluded)).is_empty(),
        "`!!` output must never reach the model, on the live turn or after a compaction"
    );
    assert!(coding_agent_convert_to_llm(&[reseeded(true)]).is_empty());

    // (c) A genuine extension `custom` message is untouched by the role routing above.
    let ext = Agent::Custom {
        kind: "ext.note".into(),
        payload: serde_json::Value::String("keep me".into()),
        timestamp: Some(8),
    };
    let out = coding_agent_convert_to_llm(std::slice::from_ref(&ext));
    assert_eq!(out.len(), 1);
    assert_eq!(text_of(&out[0]), "keep me");
}

/// The raw→transcript projection keeps every role pi's `AgentMessage` union has.
///
/// **Coverage, not proof:** `raw_message_to_agent` is new in this change, so it cannot be red
/// against HEAD~. What it pins is the mapping contract the two red-before assertions above depend
/// on: `custom` takes the typed arm every other cyrup producer uses, and the other three ride
/// through `App` as their pi wire object with `role` intact.
#[test]
fn raw_message_to_agent_preserves_every_pi_role() {
    use cyrup_agent::AgentMessage as Agent;
    use cyrup_session::agent_message::{
        BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomRoleMessage,
    };

    let bash = Raw::BashExecution(BashExecutionMessage {
        command: "ls".into(),
        output: "a.txt".into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 1,
        // pi's raw context KEEPS an excluded bash message; only convertToLlm drops it.
        exclude_from_context: Some(true),
    });
    match crate::event::raw_message_to_agent(&bash) {
        Agent::App { role, payload } => {
            assert_eq!(role, "bashExecution");
            assert_eq!(payload["command"], "ls");
            assert_eq!(payload["excludeFromContext"], true);
        }
        other => panic!("bashExecution must survive as an App message, got {other:?}"),
    }
    // …and it still disappears at the boundary, exactly as pi's `case "bashExecution"` does.
    assert!(
        coding_agent_convert_to_llm(&[crate::event::raw_message_to_agent(&bash)]).is_empty(),
        "`!!` bash output is excluded from the REQUEST, never from the transcript"
    );

    let custom = Raw::Custom(CustomRoleMessage {
        custom_type: "ext.note".into(),
        content: serde_json::Value::String("hi".into()),
        display: true,
        details: None,
        timestamp: 2,
    });
    match crate::event::raw_message_to_agent(&custom) {
        Agent::Custom { kind, payload, timestamp } => {
            assert_eq!(kind, "ext.note");
            assert_eq!(payload, serde_json::Value::String("hi".into()));
            assert_eq!(timestamp, Some(2));
        }
        other => panic!("custom keeps the typed arm, got {other:?}"),
    }

    let branch = Raw::BranchSummary(BranchSummaryMessage {
        summary: "s".into(),
        from_id: "e1".into(),
        timestamp: 3,
    });
    assert!(matches!(
        crate::event::raw_message_to_agent(&branch),
        Agent::App { ref role, .. } if role == "branchSummary"
    ));

    let compaction = Raw::CompactionSummary(CompactionSummaryMessage {
        summary: "s".into(),
        tokens_before: 9,
        timestamp: 4,
    });
    assert!(matches!(
        crate::event::raw_message_to_agent(&compaction),
        Agent::App { ref role, .. } if role == "compactionSummary"
    ));

    let core = Raw::Core(Message::User { content: vec![Content::text("hello")], timestamp: 5 });
    assert!(matches!(
        crate::event::raw_message_to_agent(&core),
        Agent::User { ref timestamp, .. } if *timestamp == Some(5)
    ));
}
