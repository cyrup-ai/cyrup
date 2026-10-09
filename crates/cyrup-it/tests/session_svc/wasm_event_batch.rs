//! The pi v1.1.0 event batch, through an ASSEMBLED session and a LIVE wasm guest: what the demo
//! component's `event_probe` module observed, reported by its `/eventprobe` command.
//!
//! Timings (pi commit 36a686ee8, #10549): a tool's `tool_execution_end` carries how long its
//! `execute()` took (`durationMs`, `core/extensions/types.ts` @v1.1.0), a call that never ran
//! carries none, and the persisted tool-result row carries the same value.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_ext::host::LiveExtension;
use cyrup_ext::{EventKind, EventPatch};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_ext_sdk::example::event_probe::{
    BOUNDARY_MARKER, BOUNDARY_NOTE, FORCE_PROMPT, FORCED_PROMPT, OPTIONS_PROMPT, REPORT_COMMAND,
    REPORT_PREFIX,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use serde_json::{Value, json};
use std::sync::Mutex;
use tempfile::TempDir;

use crate::support::bins;

/// A native tool whose `execute()` takes 30 ms.
struct Slowpoke {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for Slowpoke {
    fn name(&self) -> &str {
        "slowpoke"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(ToolResult {
            content: vec![Content::text("slow")],
            ..ToolResult::default()
        })
    }
}

struct SlowpokeExt;

#[async_trait::async_trait]
impl NativeExtension for SlowpokeExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("slowpoke-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(Slowpoke {
            params: json!({ "type": "object", "properties": {}, "additionalProperties": true }),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

async fn session_with_probe(faux: Arc<FauxProvider>) -> (Arc<AgentSession>, Arc<LiveExtension>) {
    session_with_probe_and(faux, Vec::new()).await
}

async fn session_with_probe_and(
    faux: Arc<FauxProvider>,
    natives: Vec<Arc<dyn NativeExtension>>,
) -> (Arc<AgentSession>, Arc<LiveExtension>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Outlive the session (test-process-lifetime scratch dir), as the sibling `wasm_*` tests do.
    std::mem::forget(tmp);
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let mut builder = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(SlowpokeExt));
    for native in natives {
        builder = builder.with_native_extension(native);
    }
    let session = builder.build().await.unwrap().into_shared();
    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bins::component_bytes(),
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load the demo guest");
    session.bind_extensions().await;
    let mut names = session.active_tool_names();
    if !names.iter().any(|n| n == "slowpoke") {
        names.push("slowpoke".to_string());
        session.set_active_tools_by_name(&names).await;
    }
    (session, ext)
}

/// The guest's `/eventprobe` report.
async fn probe(session: &AgentSession, ext: &LiveExtension) -> Value {
    let _ = session.prompt(format!("/{REPORT_COMMAND}")).await.unwrap();
    session.wait_for_idle().await;
    let notes = ext.guest().notifications();
    let report = notes
        .iter()
        .rev()
        .find_map(|n| n.strip_prefix(REPORT_PREFIX))
        .unwrap_or_else(|| panic!("no probe report: {notes:?}"));
    serde_json::from_str(report).unwrap()
}

/// A guest's `tool_execution_end` handler receives `durationMs` for a call that ran and none for
/// one that did not, and the session persists the same value on the tool-result row; the assistant
/// row carries its response's duration too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_sees_how_long_a_tool_ran_and_the_rows_record_it() {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("slowpoke", json!({})),
                faux_tool_call("no_such_tool", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let (session, ext) = session_with_probe(faux).await;
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let report = probe(&session, &ext).await;
    let ends = report["toolEnds"].as_array().unwrap();
    let end_of = |name: &str| {
        ends.iter()
            .find(|e| e["toolName"] == name)
            .unwrap_or_else(|| panic!("no tool_execution_end for {name}: {report}"))
    };
    let took = end_of("slowpoke")["durationMs"]
        .as_u64()
        .unwrap_or_else(|| panic!("no durationMs crossed the boundary: {report}"));
    assert!((25..1000).contains(&took), "{took} ms: {report}");
    assert_eq!(
        end_of("no_such_tool")["durationMs"],
        Value::Null,
        "{report}"
    );

    let messages = session.messages().await;
    let row = |name: &str| {
        messages
            .iter()
            .find_map(|m| match m {
                Message::ToolResult {
                    tool_name,
                    duration_ms,
                    ..
                } if tool_name == name => Some(*duration_ms),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {name} row"))
    };
    assert_eq!(row("slowpoke"), Some(took));
    assert_eq!(row("no_such_tool"), None);
}

/// A guest's `session_compact_failed` handler receives pi's payload (SESS-050), and its
/// `agent_settled` handler receives `aborted` — `false` for a finished run, `true` for one the user
/// aborted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_sees_a_failed_compaction_and_whether_a_run_was_aborted() {
    let faux = Arc::new(FauxProvider::with_config(
        cyrup_provider::faux::FauxConfig {
            tokens_per_second: Some(5.0),
            ..Default::default()
        },
    ));
    let (session, ext) = session_with_probe(Arc::clone(&faux)).await;

    let _ = session.compact(None).await.expect_err("nothing to compact");

    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text(
            "a long and slowly streamed answer that the user will not wait for",
        )],
        StopReason::Stop,
    )]);
    let _ = session.prompt("go").await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    session.abort();
    session.wait_for_idle().await;

    // The probe's own command is a run too, and it settles after the report is taken.
    let report = probe(&session, &ext).await;
    assert_eq!(
        report["compactFailed"],
        json!([{
            "reason": "manual",
            "errorMessage": "Compaction failed: Nothing to compact (session too small)",
            "aborted": false,
            "willRetry": false,
            "fromExtension": false,
        }]),
        "{report}"
    );
    let settled = report["settled"].as_array().unwrap();
    assert_eq!(settled.first(), Some(&json!(true)), "{report}");
}

/// A host-side extension that runs before the guest and adds the `team` section through
/// `systemPromptOptions`.
struct TeamExt;

#[async_trait::async_trait]
impl NativeExtension for TeamExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("team-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::BeforeAgentStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::BeforeAgentStart { options, .. } = ev else {
            return HookOutcome::Noop;
        };
        let mut edited = options.clone();
        edited["sections"]["team"] = json!("TEAM_RULE");
        HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
            system: None,
            inject: Vec::new(),
            options: Some(edited),
        })
    }
}

/// EXT-084, the ledger's Verify on the guest tier: a section one extension adds through
/// `systemPromptOptions` is what the next reads in `systemPrompt` and `ctx.getSystemPrompt()` —
/// across the host/guest boundary (a native extension's section, re-rendered by the host) and
/// within one guest (its first handler's section, re-rendered through the `render-system-prompt`
/// import) — and the model is sent both; a handler that returns a prompt replaces everything,
/// while the transcript keeps the structured sections.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_edits_the_prompt_options_and_reads_every_extensions_edits() {
    let sent: Arc<Mutex<Vec<String>>> = Arc::default();
    let steps: Vec<FauxResponseStep> = (0..3)
        .map(|_| {
            let sent = Arc::clone(&sent);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                sent.lock()
                    .unwrap()
                    .push(cyrup_provider::get_current_system_prompt(
                        cyrup_provider::normalize_context(ctx).messages(),
                    ));
                faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    let (session, ext) = session_with_probe_and(faux, vec![Arc::new(TeamExt)]).await;

    let _ = session.prompt(OPTIONS_PROMPT).await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt(FORCE_PROMPT).await.unwrap();
    session.wait_for_idle().await;

    let report = probe(&session, &ext).await;
    assert_eq!(
        report["promptOptions"],
        json!([
            {
                "sections": ["team", "guest_rule"],
                "eventHas": ["guest_rule", "team"],
                "ctxHas": ["guest_rule", "team"],
            },
            {
                "sections": ["team", "guest_rule"],
                "eventHas": ["guest_rule", "team"],
                "ctxHas": ["guest_rule", "team"],
            },
        ]),
        "{report}"
    );
    let sent = sent.lock().unwrap().clone();
    assert!(
        sent[0].contains("<team>\nTEAM_RULE\n</team>")
            && sent[0].contains("<guest_rule>\nGUEST_RULE\n</guest_rule>"),
        "the model is sent both extensions' sections: {}",
        sent[0]
    );
    assert_eq!(
        sent[1], FORCED_PROMPT,
        "a returned prompt replaces everything"
    );

    let file = session.session_file().await.unwrap();
    cyrup_session::flush_session_writes();
    let text = std::fs::read_to_string(file).unwrap();
    assert!(
        text.contains("GUEST_RULE"),
        "the transcript stores the sections"
    );
    assert!(!text.contains(FORCED_PROMPT), "and never the forced prompt");
}

/// EXT-078, the ledger's Verify on the guest tier: a guest's `turn_end` handlers append a custom
/// message and — the second one having read it back through the rebuilt preview — ask to continue
/// on a tool-less final turn, which runs EXACTLY one more provider request; the draft lands in the
/// session file after the turn it answered and before the next turn's entries; the boundary names
/// the persisted assistant entry; and `agent_before_settle` reaches the guest before the run settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_continues_a_run_from_turn_end_and_its_draft_precedes_the_next_turn() {
    let sent: Arc<Mutex<Vec<String>>> = Arc::default();
    let replies = [format!("{BOUNDARY_MARKER} first"), "second".to_string()];
    let steps: Vec<FauxResponseStep> = replies
        .into_iter()
        .chain(std::iter::once("never".to_string()))
        .map(|reply| {
            let sent = Arc::clone(&sent);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                sent.lock()
                    .unwrap()
                    .push(serde_json::to_string(&ctx.messages).unwrap());
                faux_assistant_message(vec![faux_text(reply.clone())], StopReason::Stop)
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    let (session, ext) = session_with_probe(faux).await;
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "exactly one more request");
    assert!(sent[1].contains(BOUNDARY_NOTE), "{}", sent[1]);

    let file = session.session_file().await.unwrap();
    cyrup_session::flush_session_writes();
    let rows: Vec<Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let first_assistant = rows
        .iter()
        .position(|r| r["type"] == "message" && r["message"]["role"] == "assistant")
        .unwrap();
    assert_eq!(rows[first_assistant + 1]["type"], "custom_message");
    assert_eq!(rows[first_assistant + 1]["customType"], "boundary-probe");
    assert_eq!(rows[first_assistant + 2]["message"]["role"], "assistant");

    let report = probe(&session, &ext).await;
    let boundaries = report["boundaries"].as_array().unwrap();
    assert_eq!(
        boundaries[0]["messageEntryId"], rows[first_assistant]["id"],
        "{report}"
    );
    assert_eq!(boundaries[0]["outcome"], "completed");
    assert_eq!(
        boundaries[1],
        json!({"event": "turn_end", "previewedNote": true, "canContinue": true}),
        "the second handler previewed the first one's draft: {report}"
    );
    assert!(
        boundaries
            .iter()
            .any(|b| b["event"] == "agent_before_settle" && b["outcome"] == "completed"),
        "{report}"
    );
}
