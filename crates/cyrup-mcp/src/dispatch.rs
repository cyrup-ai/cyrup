//! MCP-214's executor — the object that turns every registered MCP tool from a stub answering
//! `MCP not initialized` into a call against a live server.
//!
//! # The gap this closes
//!
//! [`crate::registration`] registers the whole model-visible surface from disk caches, with **no
//! server contacted**: one [`crate::registration::DirectTool`] per cached MCP tool or resource and
//! one [`crate::registration::ProxyTool`] for the `mcp` gateway. Both hold the pass's shared
//! [`crate::registration::ToolDispatch`] slot and both answer `MCP not initialized` for as long as
//! that slot is empty. This module supplies the one implementor of
//! [`crate::registration::McpToolDispatch`] that fills it; the commit tail of `startInitialization`
//! installs it once [`crate::state::McpState`] exists.
//!
//! Upstream has no counterpart type. `pi-mcp-adapter` registers closures that read `index.ts`'s
//! module-scoped `state` / `initPromise` / `currentOwner` slots directly (`index.ts:906`, and
//! `direct-tools.ts:380`'s `createDirectToolExecutor`); cyrup's tools are objects registered through
//! a host, so the closures' captured environment becomes this struct's one field.
//!
//! # Why the router is borrowed rather than re-derived
//!
//! [`crate::proxy::McpTool`] already carries the complete, tested port of `index.ts:849`'s
//! `execute`: the args coercion, the "gateway params were nested inside `args`" rescue, the
//! bounded init gate with its three envelopes, the generation fence, and the nine-arm router in
//! upstream's relative order. Nothing registers that tool ([`crate::registration::register_surface`]
//! registers [`crate::registration::ProxyTool`]), but its body is the gateway's behaviour and it has
//! the test suite to prove it.
//!
//! So [`McpDispatch::call_proxy`] **drives that body** — it builds the gate this generation's phase
//! implies, hands it to a throwaway [`crate::proxy::McpTool`] and calls its
//! [`cyrup_core::Tool::execute`]. A second hand-written nine-arm match would be a fork of the one
//! function MCP-163 calls this section's only `critical`, and forks of resolution state machines
//! drift silently: the symptom is a bare tool name reaching the wrong server's same-named tool.
//!
//! [`McpDispatch::call_direct`] cannot borrow a body the same way — there is no Rust port of
//! `direct-tools.ts:380` — so it routes through [`crate::proxy::execute_call`] with the server
//! pinned and the direct origin supplied, which is what that executor's eight phases reduce to once
//! the server is known. See [`McpDispatch::call_direct`] for the phase-by-phase correspondence and
//! for the one honest divergence it carries.

use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::{json, Map as JsonMap, Value};

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::config::McpSettings;
use crate::extension::{InitTask, McpExtension};
use crate::owner::McpRuntimeOwner;
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::results::text_result;
use crate::proxy::{
    execute_call, ApprovalOrigin, InitPhase, McpTool, ProxyCtx, ProxyInitGate, INIT_WAIT_TIMEOUT_MS,
};
use crate::registration::{DirectToolSpec, McpToolDispatch};

/// One re-read of `proxy_ctx` while the commit tail runs.
///
/// The build's future and the driver that commits it settle on the same wakeup: the driver awaits
/// the memoised [`InitTask`], and so does [`join_build`]. Whichever the runtime schedules first,
/// the commit's write of the context is a handful of synchronous statements away — it takes the
/// state slot, records the human-wait ctx and installs the runtime env with no await between them.
/// Re-reading closes that scheduling window instead of reporting `not_initialized` for a build that
/// in fact succeeded a microsecond ago.
const COMMIT_SETTLE_STEP: Duration = Duration::from_millis(5);

/// How many times [`committed_ctx`] re-reads before it concludes the commit is not coming — 200 ms
/// against a window that is one scheduler turn wide, so a stale generation's shutdown path (which
/// *does* await) is the only realistic way to exhaust it.
const COMMIT_SETTLE_STEPS: u32 = 40;

/// The late-bound executor behind every tool [`crate::registration`] registers — the port of
/// upstream's `() => state` / `() => initPromise` / `currentOwner` closures.
///
/// One instance serves every call of every generation, because it captures **nothing** about a
/// generation and reads all three slots live.
pub struct McpDispatch {
    /// The extension whose generation slots every call re-reads.
    ///
    /// `Weak`, and it is load-bearing three ways:
    ///
    /// 1. the extension owns the [`crate::registration::ToolDispatch`] that owns this, so a strong
    ///    edge would be a reference cycle that never drops;
    /// 2. [`crate::registration::ToolDispatch::install`] is a `OnceLock::set`, so **one** instance
    ///    must serve every generation that reuses the same slot — which it can do only by reading
    ///    `proxy_ctx` / `owner` at call time rather than capturing them at install time;
    /// 3. a captured [`ProxyCtx`] would pin a dead generation's [`crate::state::McpState`] for the
    ///    life of the process and route calls into it after a session replacement — a fenced-off
    ///    manager, its child processes and its lifecycle, all still reachable.
    extension: Weak<McpExtension>,
}

impl McpDispatch {
    /// Build the executor over a handle to the extension that owns it.
    ///
    /// The caller is the commit tail, which has the `Arc` and downgrades it:
    /// `McpDispatch::new(Arc::downgrade(self))`.
    #[must_use]
    pub const fn new(extension: Weak<McpExtension>) -> Self {
        Self { extension }
    }

    /// `index.ts:906`'s init gate, as the four states a call can find the generation in.
    ///
    /// The owner is read **once, before the wait** (`const executeOwner = currentOwner;`) and
    /// travels with the outcome: fencing on a re-read would fence a call that began under
    /// generation N against generation N+1's owner, which is active, and so would let the stale
    /// call through — the precise write the fence exists to refuse.
    async fn gate(&self) -> Gate {
        let Some(extension) = self.extension.upgrade() else {
            return Gate { owner: None, outcome: GateOutcome::NotInitialized };
        };
        let owner = extension.owner();
        if let Some(ctx) = extension.proxy_ctx() {
            return Gate { owner, outcome: GateOutcome::Ready(ctx) };
        }
        let Some(task) = extension.init_task() else {
            return Gate { owner, outcome: GateOutcome::NotInitialized };
        };
        let waited = tokio::time::timeout(
            Duration::from_millis(INIT_WAIT_TIMEOUT_MS),
            join_build(&extension, &task),
        )
        .await;
        let outcome = match waited {
            Err(_) => GateOutcome::TimedOut,
            Ok(Settled::Ready(ctx)) => GateOutcome::Ready(ctx),
            Ok(Settled::Failed(message)) => GateOutcome::Failed(message),
            Ok(Settled::Gone) => GateOutcome::NotInitialized,
        };
        Gate { owner, outcome }
    }

    /// The gateway router, primed with the phase this generation is in.
    ///
    /// The returned tool is a router and nothing else: its description and render kind are never
    /// read, because the only method called on it is [`cyrup_core::Tool::execute`] — the surface the
    /// model sees is [`crate::registration::ProxyTool`]'s, registered from the regenerated
    /// description, and it is that tool's `execute` which delegates here.
    ///
    /// A build in flight seeds [`crate::proxy::InitPhase::Pending`] and spawns the publisher that
    /// resolves it, so the bounded wait, its timer and its three envelopes stay where they are
    /// tested — inside [`crate::proxy::ProxyInitGate`] and `McpTool::execute` — and the args
    /// coercion still runs **before** the wait, as upstream orders it. A settled phase leaves no
    /// sender alive, which is correct: the gate reads the retained value and never awaits a change.
    fn gateway_router(&self) -> McpTool {
        let (sender, receiver) = tokio::sync::watch::channel(InitPhase::NotInitialized);
        let gate = Arc::new(ProxyInitGate::new(receiver));
        if let Some(extension) = self.extension.upgrade() {
            // `const executeOwner = currentOwner;` — published before the phase, so the fence and
            // the wait agree on which generation the call belongs to.
            gate.set_owner(extension.owner());
            if let Some(ctx) = extension.proxy_ctx() {
                let _ = sender.send(InitPhase::Ready(ctx));
            } else if let Some(task) = extension.init_task() {
                let _ = sender.send(InitPhase::Pending);
                tokio::spawn(async move {
                    let phase = match join_build(&extension, &task).await {
                        Settled::Ready(ctx) => InitPhase::Ready(ctx),
                        Settled::Failed(message) => InitPhase::Failed(message),
                        // Upstream's `!state` after the await: the build resolved, and the commit
                        // tail dropped it as stale rather than committing it.
                        Settled::Gone => InitPhase::NotInitialized,
                    };
                    let _ = sender.send(phase);
                });
            }
        }
        McpTool::new(String::new(), &McpSettings::default(), gate)
    }
}

/// What one call found when it read the generation's slots, plus the owner it must fence against.
struct Gate {
    /// `currentOwner`, captured before the wait. `None` only outside a session — no generation has
    /// started, or the extension itself is gone.
    owner: Option<Arc<McpRuntimeOwner>>,
    outcome: GateOutcome,
}

/// The init gate's four answers — upstream's `state` slot crossed with its `initPromise` slot,
/// with the bounded wait already resolved.
enum GateOutcome {
    /// A committed context: the call proceeds.
    Ready(Arc<ProxyCtx>),
    /// The wait hit [`INIT_WAIT_TIMEOUT_MS`] with the build still running.
    TimedOut,
    /// The build rejected. Carries the message the `init_failed` envelope reports.
    Failed(String),
    /// Nothing is committed and nothing is coming.
    NotInitialized,
}

/// What joining the in-flight build resolved to, once the commit tail has had its turn.
enum Settled {
    Ready(Arc<ProxyCtx>),
    Failed(String),
    /// The build resolved, but no context was committed — the generation was superseded and the
    /// commit tail shut the new state down instead of installing it.
    Gone,
}

/// Join the memoised build rather than starting a rival one, then read what it committed.
///
/// Cloning the `Shared` out of the `Arc` clones the *inner* handle and leaves the outer `Arc`'s
/// identity intact, which is what lets a call attach to a build without disturbing
/// `startInitialization`'s third staleness check (`Arc::ptr_eq` on that same outer `Arc`).
async fn join_build(extension: &Arc<McpExtension>, task: &Arc<InitTask>) -> Settled {
    match (**task).clone().await {
        Ok(_) => match committed_ctx(extension).await {
            Some(ctx) => Settled::Ready(ctx),
            None => Settled::Gone,
        },
        Err(error) => Settled::Failed(error.to_string()),
    }
}

/// The committed context, re-read across the commit tail's scheduling window (see
/// [`COMMIT_SETTLE_STEP`]).
async fn committed_ctx(extension: &Arc<McpExtension>) -> Option<Arc<ProxyCtx>> {
    for _ in 0..COMMIT_SETTLE_STEPS {
        if let Some(ctx) = extension.proxy_ctx() {
            return Some(ctx);
        }
        tokio::time::sleep(COMMIT_SETTLE_STEP).await;
    }
    extension.proxy_ctx()
}

/// `index.ts:911` — the timeout envelope. No `mode` key, and it carries the budget it gave up on.
fn init_timeout_result() -> ToolResult {
    let mut map = JsonMap::new();
    map.insert(
        "error".to_string(),
        Value::String(McpErrorCode::InitTimeout.as_str().to_string()),
    );
    map.insert("timeoutMs".to_string(), json!(INIT_WAIT_TIMEOUT_MS));
    text_result("MCP initialization is still in progress. Try again shortly.", map)
}

/// `index.ts:917` — the rejected-build envelope, reporting the message rather than throwing it.
fn init_failed_result(message: &str) -> ToolResult {
    let mut map = JsonMap::new();
    map.insert(
        "error".to_string(),
        Value::String(McpErrorCode::InitFailed.as_str().to_string()),
    );
    map.insert("message".to_string(), Value::String(message.to_string()));
    text_result(format!("MCP initialization failed: {message}"), map)
}

/// The answer a tool gives with nothing committed — byte-identical to the one
/// `registration`'s uninstalled-slot arm gives, so "the dispatcher is not installed" and "the
/// dispatcher found no state" are indistinguishable to the model, exactly as upstream's single
/// `!state` branch makes them.
fn not_initialized_result() -> ToolResult {
    let mut map = JsonMap::new();
    map.insert(
        "error".to_string(),
        Value::String(McpErrorCode::NotInitialized.as_str().to_string()),
    );
    text_result("MCP not initialized", map)
}

#[async_trait::async_trait]
impl McpToolDispatch for McpDispatch {
    /// `direct-tools.ts:380` `createDirectToolExecutor` — one registered per-tool call.
    ///
    /// Upstream's executor is not ported as its own function, deliberately: it would fork the lazy
    /// connect ladder, the five-point single-shot auto-auth latch, the approval gate and the output
    /// guard away from [`crate::proxy::execute_call`], which already implements all four. Pinning
    /// the server and supplying the direct origin reduces that executor to a call into it, phase by
    /// phase:
    ///
    /// * **Phase 1** — a hint is always given, so the tool resolves against
    ///   `state.tool_metadata[server]` and the disabled check runs on the named server.
    ///   `direct-tools.ts:388`'s start.
    /// * **Phase 2** (no hint: the ambiguity gate and the two ordered scans) — unreachable here.
    /// * **Phase 3** — a tool that is not in metadata yet (cold cache, lazy server) lazily connects,
    ///   runs the auto-auth ladder under the single-shot latch, re-resolves, and otherwise returns
    ///   the `needs-auth` / `server_backoff` envelopes. `direct-tools.ts:388-421`.
    /// * **Phases 4-5** (prefix discovery, native-tool fallthrough) — both gate on "no server
    ///   name", so neither is reached.
    /// * **Phase 6 onward** — the approval gate with the origin below, the in-flight accounting,
    ///   then `resources/read` when the metadata carries a `resourceUri` and `tools/call`
    ///   otherwise, through the output guard. `direct-tools.ts:481-545`.
    ///
    /// **The one divergence, stated rather than hidden:** every result
    /// [`crate::proxy::execute_call`] builds carries `details.mode = "call"`, which upstream's
    /// direct executor does not emit. It is additive and inert — `error-signal.ts`'s
    /// `toolErrorOverride` branches on `details.error` alone, and the two codes it re-flags
    /// (`tool_error`, `call_failed`) are produced byte-identically.
    ///
    /// `call_id` and `on_update` are unused because no phase of this path streams or reads a call
    /// id; the trait fixes the signature, so they keep their names with an underscore rather than
    /// being dropped.
    async fn call_direct(
        &self,
        spec: &DirectToolSpec,
        _call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let gate = self.gate().await;
        let ctx = match gate.outcome {
            GateOutcome::Ready(ctx) => ctx,
            GateOutcome::TimedOut => return Ok(init_timeout_result()),
            GateOutcome::Failed(message) => {
                // An owner abort rethrows rather than reporting — the session is going away, and a
                // successful result describing that would be recorded in a transcript nobody keeps.
                if gate.owner.is_some_and(|owner| owner.token().is_cancelled()) {
                    return Err(ToolError::new(message));
                }
                return Ok(init_failed_result(&message));
            }
            GateOutcome::NotInitialized => return Ok(not_initialized_result()),
        };

        // The generation fence — a call that outlived its generation aborts rather than writing
        // into a restarted session.
        if let Some(owner) = gate.owner.as_ref() {
            owner.throw_if_inactive().map_err(|error| ToolError::new(error.to_string()))?;
        }

        execute_call(
            &ctx,
            // The CATALOG name, which is the model-facing prefixed one — NOT `original_name`.
            //
            // This parameter is `execute_call`'s **lookup key**, not its wire name. Phase 1 resolves
            // it with `get_single_tool_match`, whose comparison is `tool.name == tool_name`
            // (`proxy/results.rs`, upstream `proxy-modes.ts:39`), and `ToolMetadata::name` is
            // `format_tool_name(tool.name, server, effective_prefix)` — the same expression
            // `resolve_direct_tools` builds `prefixed_name` from, so the two are equal by
            // construction in every prefix mode, `ToolPrefix::None` included. The un-prefixing back
            // to the server's own name happens once, downstream, where the invocation reads
            // `tool_meta.original_name` (`proxy/call.rs`).
            //
            // Passing `original_name` here un-prefixed it a second time, so under the DEFAULT
            // `ToolPrefix::Server` every direct tool looked up `echo` in a catalog holding
            // `fixture_echo` and answered `tool_not_found` with `suggestions: ["fixture_echo"]` —
            // the model's own tool, next to the name it just failed to find.
            &spec.prefixed_name,
            // The provider always sends an object for a tool call; anything else carries no
            // arguments rather than an argument that is not a record.
            if params.is_object() { Some(&params) } else { None },
            // The server hint that pins phases 1 and 3 and skips 2, 4 and 5.
            Some(&spec.server_name),
            &cancel,
            // `direct-tools.ts:440` `spec.resourceUri ? "resource" : "direct"`.
            Some(ApprovalOrigin::for_direct_tool(spec.resource_uri.as_ref())),
        )
        .await
        .map_err(|error| ToolError::new(error.to_string()))
    }

    /// `index.ts:849` — one `mcp({...})` gateway call, routed through the nine modes.
    ///
    /// The whole body is [`crate::proxy::McpTool`]'s: this supplies the generation's owner and
    /// phase, and that tool's `execute` does the args coercion, the nested-params rescue, the
    /// bounded init wait, the fence and the nine-arm dispatch. See the module docs for why the
    /// router is borrowed rather than written twice.
    async fn call_proxy(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.gateway_router().execute(call_id, params, cancel, on_update).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn dangling() -> McpDispatch {
        McpDispatch::new(Weak::new())
    }

    fn spec() -> DirectToolSpec {
        DirectToolSpec {
            server_name: "linear".to_string(),
            original_name: "create_issue".to_string(),
            prefixed_name: "linear_create_issue".to_string(),
            description: "create an issue".to_string(),
            input_schema: None,
            resource_uri: None,
        }
    }

    fn error_code(result: &ToolResult) -> Option<String> {
        result
            .details
            .as_ref()?
            .get("error")?
            .as_str()
            .map(str::to_string)
    }

    fn text(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    // ---- The honest "not initialized" answer ----------------------------------------------------

    /// With no extension to read, a direct tool answers exactly what its uninstalled-slot arm
    /// answers — a successful result carrying `details.error`, never an `Err`.
    #[tokio::test]
    async fn a_direct_call_with_no_extension_reports_not_initialized() {
        let result = dangling()
            .call_direct(
                &spec(),
                ToolCallId::from("call-1"),
                json!({}),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a not-initialized answer is a successful result");
        assert_eq!(error_code(&result).as_deref(), Some("not_initialized"));
        assert_eq!(text(&result), "MCP not initialized");
        // The three init envelopes carry no `mode` key.
        assert!(result.details.as_ref().and_then(|d| d.get("mode")).is_none());
    }

    /// The gateway's answer is the same, and it comes from the same gate the nine modes sit behind.
    #[tokio::test]
    async fn a_gateway_call_with_no_extension_reports_not_initialized() {
        let result = dangling()
            .call_proxy(
                ToolCallId::from("call-2"),
                json!({ "search": "issue" }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a not-initialized answer is a successful result");
        assert_eq!(error_code(&result).as_deref(), Some("not_initialized"));
        assert_eq!(text(&result), "MCP not initialized");
    }

    /// Args coercion runs **before** the gate, so a malformed `args` is rejected even with nothing
    /// initialized — upstream's order, and the proof that `call_proxy` really is running
    /// `McpTool::execute`'s preamble rather than a second copy of it.
    #[tokio::test]
    async fn the_gateway_rejects_malformed_args_before_it_consults_the_gate() {
        let error = dangling()
            .call_proxy(
                ToolCallId::from("call-3"),
                json!({ "tool": "create_issue", "args": "{not json" }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("a malformed `args` throws rather than reporting a code");
        assert!(
            error.to_string().starts_with("Invalid args JSON:"),
            "unexpected error: {error}"
        );
    }

    // ---- The two envelopes `call_direct` builds for itself ---------------------------------------

    /// `init_timeout` carries the budget it gave up on, so the model can decide whether to retry.
    #[test]
    fn the_timeout_envelope_reports_the_budget() {
        let result = init_timeout_result();
        assert_eq!(error_code(&result).as_deref(), Some("init_timeout"));
        assert_eq!(
            result.details.as_ref().and_then(|d| d.get("timeoutMs")),
            Some(&json!(INIT_WAIT_TIMEOUT_MS))
        );
        assert_eq!(text(&result), "MCP initialization is still in progress. Try again shortly.");
    }

    /// `init_failed` reports the rejection message in both the text and `details.message`.
    #[test]
    fn the_failure_envelope_carries_the_message_twice() {
        let result = init_failed_result("config parse failed");
        assert_eq!(error_code(&result).as_deref(), Some("init_failed"));
        assert_eq!(
            result.details.as_ref().and_then(|d| d.get("message")),
            Some(&json!("config parse failed"))
        );
        assert_eq!(text(&result), "MCP initialization failed: config parse failed");
    }
}
