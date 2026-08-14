//! The PRODUCER half of DRIFT-001/AGENT-004: who actually sets `addedToolNames`.
//!
//! Upstream no tool ever sets the field. Pi wraps every tool that lands in the session's
//! `_toolRegistry` — the built-ins and the extension-registered ones alike
//! (`wrapRegisteredTools(allCustomTools, runner)` + `wrapRegisteredTools(baseToolDefinitions…)`,
//! coding-agent/src/core/agent-session.ts:2506-2515) — and the wrapper
//! (`core/extensions/wrapper.ts:22-35`) snapshots `runner.getActiveTools()` on both sides of
//! `execute`, folding the difference onto the result:
//!
//! ```text
//! const activeBefore = runner.getActiveTools();
//! const result = await execute(...);
//! const activeAfter = runner.getActiveTools();
//! if (!activeBefore.every((name) => activeAfter.includes(name))) return result;   // not additive
//! const addedToolNames = activeAfter.filter((name) => !beforeNames.has(name));
//! if (addedToolNames.length === 0) return result;
//! return { ...result, addedToolNames: [...new Set([...(result.addedToolNames ?? []), ...added])] };
//! ```
//!
//! Before this, cyrup's extension-tool execute path returned the guest/native result verbatim with
//! `added_tool_names` at its `Default` (empty) and nothing anywhere computed it — so in a REAL run
//! the anchor was always absent and only a hand-written Rust `Tool` could produce one.
//!
//! These tests drive REAL runs through the extension seam and read the anchor off the finalized
//! transcript message, never off a tool's return value.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_core::{
    CancelToken, Content, ExtensionId, StopReason, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_ext::{ExtError, HostCtx, HookOutcome, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxProvider, FauxResponseStep,
};
use cyrup_provider::Provider;
use crate::{AgentSession, SessionBuilder, SessionConfig};
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

fn empty_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

type SessionSlot = Arc<OnceLock<Weak<AgentSession>>>;

/// What the extension's tool does to the active set while it runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Widen {
    /// Purely additive: `[…current…, "late"]` — Pi's `dynamic-tools.ts` shape.
    AddLate,
    /// Drops a currently-active tool while adding one — NOT additive, so upstream bails out.
    SwapForLate,
    /// Re-asserts the current set verbatim: nothing added, nothing removed.
    Nothing,
}

/// An extension-registered tool. It NEVER touches `added_tool_names` — that is the whole point.
struct ExtLoaderTool {
    slot: SessionSlot,
    what: Widen,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for ExtLoaderTool {
    fn name(&self) -> &str {
        "ext_loader"
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
        let session =
            self.slot.get().and_then(Weak::upgrade).ok_or_else(|| ToolError::new("no session"))?;
        let mut names = session.active_tool_names();
        match self.what {
            Widen::AddLate => names.push("late".to_string()),
            Widen::SwapForLate => {
                // Remove SOMETHING that was active, then add: the change is no longer additive.
                names.retain(|n| n != "read");
                names.push("late".to_string());
            }
            Widen::Nothing => {}
        }
        session.set_active_tools_by_name(&names).await;
        Ok(ToolResult { content: vec![Content::text("loaded")], ..Default::default() })
    }
}

/// The tool `ext_loader` activates. Registered by the same extension, deactivated before the run.
struct LateTool {
    ran: Arc<AtomicUsize>,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for LateTool {
    fn name(&self) -> &str {
        "late"
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
        self.ran.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult { content: vec![Content::text("late-ran")], ..Default::default() })
    }
}

/// A native extension that registers both tools at `init` — the seam that produced `ToolResult`s
/// with `added_tool_names` permanently empty before the wrapper existed.
struct LoaderExt {
    slot: SessionSlot,
    what: Widen,
    late_ran: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for LoaderExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("loader-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(ExtLoaderTool {
            slot: self.slot.clone(),
            what: self.what,
            params: empty_schema(),
        }));
        api.register_tool(Arc::new(LateTool {
            ran: self.late_ran.clone(),
            params: empty_schema(),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// The tool names offered to the model, one entry per provider request.
type OfferedTools = Arc<Mutex<Vec<Vec<String>>>>;

#[derive(Clone)]
enum Reply {
    Call(&'static str),
    Text(&'static str),
}

fn faux_script(offered: &OfferedTools, script: Vec<Reply>) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = script
        .into_iter()
        .map(|reply| {
            let cap = offered.clone();
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                cap.lock().unwrap().push(ctx.tools.iter().map(|t| t.name.clone()).collect());
                match reply {
                    Reply::Call(name) => faux_assistant_message(
                        vec![faux_tool_call(name.to_string(), serde_json::json!({}))],
                        StopReason::ToolUse,
                    ),
                    Reply::Text(t) => {
                        faux_assistant_message(vec![faux_text(t.to_string())], StopReason::Stop)
                    }
                }
            })
        })
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

/// A bound session whose EXTENSION registered `ext_loader` + `late`, with `late` deactivated so the
/// widening below is a genuine addition.
async fn session_with_ext(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    what: Widen,
) -> (Arc<AgentSession>, Arc<AtomicUsize>) {
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let late_ran = Arc::new(AtomicUsize::new(0));
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(fx))
        .with_native_extension(Arc::new(LoaderExt {
            slot: slot.clone(),
            what,
            late_ran: late_ran.clone(),
        }))
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));
    // Start from a known active set: the built-ins plus `ext_loader`, with `late` left
    // registered-but-inactive — the state Pi's `dynamic-tools.ts` example leaves a tool in.
    let mut names: Vec<String> =
        session.active_tool_names().into_iter().filter(|n| n != "late").collect();
    names.push("ext_loader".to_string());
    session.set_active_tools_by_name(&names).await;
    let active = session.active_tool_names();
    assert!(active.iter().any(|n| n == "ext_loader"), "the extension tool is active: {active:?}");
    assert!(active.iter().any(|n| n == "read"), "a built-in is active too: {active:?}");
    assert!(!active.iter().any(|n| n == "late"), "`late` is inactive: {active:?}");
    (session, late_ran)
}

/// Every anchor the run produced, as `(tool_name, added_tool_names)`.
async fn anchors(session: &Arc<AgentSession>) -> Vec<(String, Vec<String>)> {
    session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if !t.added_tool_names.is_empty() => {
                Some((t.tool_name, t.added_tool_names))
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------- the proof ----

/// THE PRODUCER PROOF. An extension-registered tool that widens the active set gets the anchor
/// derived FOR it by the host, without the tool touching the field — and the newly-anchored tool
/// really is callable on the next turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_derives_added_tool_names_for_an_extension_tool_that_widens_the_active_set() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_script(
        &offered,
        vec![Reply::Call("ext_loader"), Reply::Call("late"), Reply::Text("done")],
    );
    let (session, late_ran) = session_with_ext(&fx, faux, Widen::AddLate).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    // The anchor exists, is on the tool that widened the set, and names exactly the new tool —
    // even though `ExtLoaderTool::execute` returns `..Default::default()` for that field.
    assert_eq!(
        anchors(&session).await,
        vec![("ext_loader".to_string(), vec!["late".to_string()])],
        "the host-side wrapper derived the anchor from the active-set diff"
    );

    // And the derivation is not cosmetic: the anchored tool was genuinely callable afterwards.
    let turns = offered.lock().unwrap().clone();
    assert!(turns.len() >= 2, "the run drove at least two turns: {turns:?}");
    assert!(!turns[0].iter().any(|t| t == "late"), "turn 1 did not offer `late`: {:?}", turns[0]);
    assert!(turns[1].iter().any(|t| t == "late"), "turn 2 offered `late`: {:?}", turns[1]);
    assert_eq!(late_ran.load(Ordering::SeqCst), 1, "`late` actually executed");
}

/// Pi's bail-out: a change that REMOVES a previously-active tool invalidates the model's cached
/// definitions wholesale, so the wrapper records nothing at all — not even the additions that came
/// with it (`if (!activeBefore.every((name) => activeAfter.includes(name))) return result`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_additive_change_records_no_anchor_even_though_a_tool_was_added() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_script(&offered, vec![Reply::Call("ext_loader"), Reply::Text("done")]);
    let (session, _) = session_with_ext(&fx, faux, Widen::SwapForLate).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    assert!(anchors(&session).await.is_empty(), "a removal suppresses the anchor entirely");
    // The swap really did happen — otherwise this test would pass for the wrong reason.
    let after = session.active_tool_names();
    assert!(after.iter().any(|n| n == "late"), "`late` was added: {after:?}");
    assert!(!after.iter().any(|n| n == "read"), "`read` was removed: {after:?}");
}

/// An ordinary tool call changes nothing, so no result carries an anchor. This is the shape of
/// EVERY turn of every ordinary run, and the reason adding the field costs no bytes on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unchanged_active_set_records_no_anchor() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_script(&offered, vec![Reply::Call("ext_loader"), Reply::Text("done")]);
    let (session, _) = session_with_ext(&fx, faux, Widen::Nothing).await;

    let session_file = session.session_file().await.expect("the session persists to a file");
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    assert!(anchors(&session).await.is_empty(), "no anchor when nothing changed");
    let raw = std::fs::read_to_string(&session_file).unwrap();
    assert!(!raw.contains("addedToolNames"), "and no key on disk either:\n{raw}");
}
