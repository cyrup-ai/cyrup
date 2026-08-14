//! AGENT-005, second clause: an extension must be able to OBSERVE and PATCH the usage a tool
//! reported for its own execution.
//!
//! Pi `2fd38684` ("allow observing and patching usage in tool_result hooks") wired both directions
//! through the extension seam:
//!
//! * READ — `ToolResultEventBase.usage?: Usage` (coding-agent/src/core/extensions/types.ts:919-921),
//!   populated by `runner.emitToolResult({ ..., usage: result.usage })` (agent-session.ts:490-516).
//! * WRITE — `ToolResultEventResult.usage?: Usage` (types.ts:1085-1090), returned from the reduce
//!   (`return { content, details, isError, usage: currentEvent.usage }`, runner.ts:924-931) and
//!   applied as `usage: hookResult.usage`.
//!
//! cyrup modelled `AfterToolCall.usage` / `AfterOverride.usage` on the agent-layer trait but the
//! extension event shape carried no `usage` in EITHER direction, so the only production implementer
//! of `Hooks::after_tool_call` — `cyrup-ext`'s bridge — could neither see nor change it.
//!
//! These run the real loop against the real `PolicyHooks` → `ExtHooks` → dispatcher chain.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExtensionId, StopReason, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink, Usage,
};
use cyrup_ext::{EventKind, EventPatch, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxProvider, FauxResponseStep,
};
use cyrup_provider::Provider;
use crate::{SessionBuilder, SessionConfig};
use tempfile::TempDir;

// ------------------------------------------------------------------------------ fixtures ----

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
    cfg.no_extensions = true;
    cfg
}

fn usage(input: u64, output: u64) -> Usage {
    Usage { input, output, total_tokens: input + output, ..Usage::default() }
}

/// A tool that reports usage for its OWN execution (Pi `AgentToolResult.usage`, types.ts:360-361 —
/// e.g. a sub-model summarizer that spends real tokens inside a tool).
struct BillingTool {
    reports: Option<Usage>,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for BillingTool {
    fn name(&self) -> &str {
        "billing"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("billed")],
            usage: self.reports.clone(),
            ..Default::default()
        })
    }
}

/// What the extension does with the usage it observes on a `tool_result`.
#[derive(Clone)]
enum Act {
    /// Observe only (`HookOutcome::Noop`) — the tool's own value must survive untouched.
    Observe,
    /// Replace it in full (Pi `ToolResultEventResult.usage`).
    Patch(Usage),
    /// Patch something ELSE, leaving `usage` alone — the omitted-key "keep" path.
    PatchContentOnly,
}

/// A native extension subscribing to `tool_result`; records every usage it observed.
struct UsageExt {
    seen: Arc<Mutex<Vec<Option<Usage>>>>,
    act: Act,
}

#[async_trait::async_trait]
impl NativeExtension for UsageExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("usage-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::ToolResult]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::ToolResult { usage, .. } = ev else { return HookOutcome::Noop };
        self.seen.lock().unwrap().push(usage.clone());
        match &self.act {
            Act::Observe => HookOutcome::Noop,
            Act::Patch(u) => HookOutcome::Mutate(EventPatch::ToolResult {
                content: None,
                details: None,
                is_error: None,
                usage: Some(u.clone()),
            }),
            Act::PatchContentOnly => HookOutcome::Mutate(EventPatch::ToolResult {
                content: Some(vec![Content::text("rewritten")]),
                details: None,
                is_error: None,
                usage: None,
            }),
        }
    }
}

fn faux_one_tool_turn() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call("billing".to_string(), serde_json::json!({}))],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    faux
}

/// Drive one tool turn; return `(usages the extension observed, the finalized tool result, stats)`.
async fn run_full(
    fx: &Fixture,
    reports: Option<Usage>,
    act: Act,
) -> (Vec<Option<Usage>>, cyrup_agent::ToolResultMessage, crate::SessionStats) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut cfg = base_config(fx);
    cfg.custom_tools =
        vec![Arc::new(BillingTool { reports, params: serde_json::json!({"type":"object"}) })
            as Arc<dyn Tool>];
    let session = SessionBuilder::new(faux_one_tool_turn() as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(UsageExt { seen: seen.clone(), act }))
        .build()
        .await
        .unwrap()
        .into_shared();
    let mut names = session.active_tool_names();
    names.push("billing".to_string());
    session.set_active_tools_by_name(&names).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let results: Vec<cyrup_agent::ToolResultMessage> = session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == "billing" => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1, "exactly one `billing` result");
    let seen = seen.lock().unwrap().clone();
    let stats = session.session_stats().await;
    (seen, results.into_iter().next().unwrap(), stats)
}

/// The two-value form used by the seam tests.
async fn run(
    fx: &Fixture,
    reports: Option<Usage>,
    act: Act,
) -> (Vec<Option<Usage>>, cyrup_agent::ToolResultMessage) {
    let (seen, result, _stats) = run_full(fx, reports, act).await;
    (seen, result)
}

// ------------------------------------------------------------------------------- the proof ----

/// READ DIRECTION. The usage the tool reported reaches the extension's `tool_result` handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_observes_the_usage_the_tool_reported() {
    let fx = fixture();
    let (seen, result) = run(&fx, Some(usage(11, 22)), Act::Observe).await;
    assert_eq!(seen, vec![Some(usage(11, 22))], "the handler saw the tool's own usage");
    // Observation is not mutation: a `Noop` handler leaves the value exactly as the tool set it.
    assert_eq!(result.usage, Some(usage(11, 22)));
}

/// The absent case, which is every ordinary tool: the handler sees `None`, not a zeroed `Usage`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_that_reports_no_usage_shows_the_handler_none() {
    let fx = fixture();
    let (seen, result) = run(&fx, None, Act::Observe).await;
    assert_eq!(seen, vec![None], "absent stays absent across the seam");
    assert_eq!(result.usage, None);
}

/// WRITE DIRECTION. A handler's `usage` REPLACES the tool's in full (Pi documents no deep merge,
/// types.ts:70-78) and the replacement is what lands on the transcript message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_patches_the_tool_result_usage() {
    let fx = fixture();
    let (seen, result) = run(&fx, Some(usage(11, 22)), Act::Patch(usage(700, 800))).await;
    assert_eq!(seen, vec![Some(usage(11, 22))], "it observed before replacing");
    assert_eq!(result.usage, Some(usage(700, 800)), "the handler's usage replaced the tool's");
}

/// A handler may ADD usage to a tool that reported none — the shape a metering extension needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_can_attach_usage_to_a_tool_that_reported_none() {
    let fx = fixture();
    let (_seen, result) = run(&fx, None, Act::Patch(usage(5, 6))).await;
    assert_eq!(result.usage, Some(usage(5, 6)));
}

/// A patch that omits `usage` KEEPS the tool's value (Pi `usage: afterResult.usage ?? result.usage`,
/// agent-loop.ts:738) — the regression a naive "replace the whole result" bridge would cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_patch_that_omits_usage_keeps_the_tools_value() {
    let fx = fixture();
    let (_seen, result) = run(&fx, Some(usage(11, 22)), Act::PatchContentOnly).await;
    assert_eq!(result.usage, Some(usage(11, 22)), "an omitted key keeps, never clears");
    let text = result.content.iter().find_map(|c| match c {
        Content::Text { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("rewritten"), "the patch that WAS made still applied");
}

// ------------------------------------------------------------------ session-total accounting ----

/// A tool's reported usage must reach the SESSION TOTALS, not just the transcript message.
/// Pi aggregates it in `getSessionStats` (`else if (message.role === "toolResult") { toolResults++;
/// if (message.usage) { addUsageToTotals(usageTotals, message.usage); } }`, agent-session.ts:
/// 3127-3133). cyrup counted the tool result but summed tokens only from assistant turns, so a tool
/// that spends real tokens was billed-but-invisible in `session_stats` and in the RPC state view.
///
/// Asserted as a DELTA against an identical run whose tool reports nothing: the faux provider's own
/// assistant turns contribute tokens too, and only the difference is attributable to the tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_reported_usage_reaches_the_session_stats_totals() {
    let fx = fixture();
    let (_, _, baseline) = run_full(&fx, None, Act::Observe).await;
    let (_seen, result, with_tool) = run_full(&fx, Some(usage(11, 22)), Act::Observe).await;

    assert_eq!(result.usage, Some(usage(11, 22)));
    assert_eq!(with_tool.tool_results, baseline.tool_results, "same shape of run");
    assert_eq!(
        with_tool.tokens.input - baseline.tokens.input,
        11,
        "the tool's input tokens joined the session totals"
    );
    assert_eq!(
        with_tool.tokens.output - baseline.tokens.output,
        22,
        "the tool's output tokens joined the session totals"
    );
}

/// The patched value — not the tool's original — is what gets billed, since the override replaced
/// the result before it was appended to the transcript.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_patched_usage_is_what_the_session_totals_count() {
    let fx = fixture();
    let (_, _, baseline) = run_full(&fx, None, Act::Observe).await;
    let (_, _, patched) = run_full(&fx, Some(usage(11, 22)), Act::Patch(usage(700, 800))).await;
    assert_eq!(patched.tokens.input - baseline.tokens.input, 700);
    assert_eq!(patched.tokens.output - baseline.tokens.output, 800);
}

/// The common case: no tool reports usage, so two identical runs total identically — i.e. the new
/// branch adds nothing when the field is absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_without_usage_contributes_nothing_to_the_totals() {
    let fx = fixture();
    let (_, _, a) = run_full(&fx, None, Act::Observe).await;
    let (_, _, b) = run_full(&fx, None, Act::Observe).await;
    assert_eq!(a.tool_results, 1);
    assert_eq!(a.tokens.input, b.tokens.input);
    assert_eq!(a.tokens.output, b.tokens.output);
    assert_eq!(a.tokens.cache_read, b.tokens.cache_read);
    assert_eq!(a.tokens.cache_write, b.tokens.cache_write);
}
