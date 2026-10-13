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
//! [`crate::registration::McpToolDispatch`] that fills it; `startInitialization` installs it as the
//! build starts, so a call that lands before [`crate::state::McpState`] exists joins the build
//! through [`McpDispatch::gate`] instead of finding an empty slot.
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

use serde_json::{Map as JsonMap, Value};

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::config::McpSettings;
use crate::extension::{Build, BuildState, McpExtension};
use crate::owner::McpRuntimeOwner;
use crate::proxy::error_vocab::McpErrorCode;
use crate::proxy::results::text_result;
use crate::proxy::{
    ApprovalOrigin, InitPhase, McpTool, OriginatingCall, ProxyCtx, ProxyInitGate, execute_call,
};
use crate::registration::{DirectToolSpec, McpToolDispatch};

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

    /// The init gate of a direct tool, as the states a call can find the generation in.
    ///
    /// The owner is read **once, before the wait** (`const executeOwner = currentOwner;`) and
    /// travels with the outcome: fencing on a re-read would fence a call that began under
    /// generation N against generation N+1's owner, which is active, and so would let the stale
    /// call through — the precise write the fence exists to refuse.
    ///
    /// A build in flight is waited for, with no bound of its own, and the call's own `cancel` ends
    /// the wait (MCP-631). Upstream's direct tool waits the same way — `await
    /// ensureSessionRuntime(ctx)` before its executor runs (`index.ts:491` @v5.2.0), with no bound —
    /// but observes no signal while it does; the gateway's wait is the bounded one
    /// ([`ProxyInitGate`]).
    ///
    /// What it waits for is its own `server`, not the whole build (MCP-635): once that server has
    /// settled in the startup pass, the call goes through the route the pass built over the
    /// generation's state ([`crate::startup::StartupPass::route_for`]), while slower servers are
    /// still connecting. pi's built-in extension holds no call on another server: it registers a
    /// server's tools from that server's connection once ready (`registerTools`,
    /// `extensions/mcp/index.ts:368` @v1.0.4) and calls through it (`tools.ts:281-283`). A server
    /// whose startup connect failed, or a build that settles first, gets the build's own answer.
    ///
    /// A build superseded by a second build of the same generation is not the call's answer
    /// (MCP-641): the call waits on the generation's current build instead, as often as that
    /// happens. Only a call whose generation is over — its owner gone or replaced — is answered
    /// `not_initialized`, as before.
    async fn gate(&self, server: &str, cancel: &CancelToken) -> Gate {
        let Some(extension) = self.extension.upgrade() else {
            return Gate {
                owner: None,
                outcome: GateOutcome::NotInitialized,
                route: None,
            };
        };
        let owner = extension.owner();
        loop {
            if let Some(ctx) = extension.proxy_ctx() {
                return Gate {
                    owner,
                    outcome: GateOutcome::Ready(ctx),
                    route: None,
                };
            }
            let Some(build) = extension.init_task() else {
                return Gate {
                    owner,
                    outcome: GateOutcome::NotInitialized,
                    route: None,
                };
            };
            let settled = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    return Gate {
                        owner,
                        outcome: GateOutcome::Aborted,
                        route: None,
                    };
                }
                settled = settle_for(&extension, &build, server) => settled,
            };
            let (outcome, route) = match settled {
                Settled::Ready(ctx) => (GateOutcome::Ready(ctx), None),
                Settled::Routed(ctx) => (GateOutcome::Ready(ctx), Some(build)),
                Settled::Failed(message) => (GateOutcome::Failed(message), None),
                Settled::Gone if superseded_within(&extension, &build, owner.as_ref()) => continue,
                Settled::Gone => (GateOutcome::NotInitialized, None),
            };
            return Gate {
                owner,
                outcome,
                route,
            };
        }
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
            } else if let Some(mut build) = extension.init_task() {
                let _ = sender.send(InitPhase::Pending);
                let owner = extension.owner();
                tokio::spawn(async move {
                    let phase = loop {
                        match settle(&extension, &build).await {
                            Settled::Ready(ctx) | Settled::Routed(ctx) => {
                                break InitPhase::Ready(ctx);
                            }
                            Settled::Failed(message) => break InitPhase::Failed(message),
                            // MCP-641 — superseded inside this generation: the wait goes on with
                            // the build that replaced it, still within the gateway's own bound.
                            Settled::Gone
                                if superseded_within(&extension, &build, owner.as_ref()) =>
                            {
                                if let Some(ctx) = extension.proxy_ctx() {
                                    break InitPhase::Ready(ctx);
                                }
                                match extension.init_task() {
                                    Some(next) => build = next,
                                    None => break InitPhase::NotInitialized,
                                }
                            }
                            // Upstream's `!state` after the await: the build resolved, and the
                            // commit tail dropped it as stale rather than committing it.
                            Settled::Gone => break InitPhase::NotInitialized,
                        }
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
    /// The build whose route the call was given before its commit (MCP-635), if it was: a second
    /// build of the generation may yet supersede it and close its servers (MCP-641).
    route: Option<Arc<Build>>,
}

/// The init gate's four answers — upstream's `state` slot crossed with its `initPromise` slot,
/// with the bounded wait already resolved.
enum GateOutcome {
    /// A committed context: the call proceeds.
    Ready(Arc<ProxyCtx>),
    /// The call's own token was cancelled while it waited for the build.
    Aborted,
    /// The build rejected. Carries the message the `init_failed` envelope reports.
    Failed(String),
    /// Nothing is committed and nothing is coming.
    NotInitialized,
}

/// What joining the in-flight build resolved to, once the commit tail has had its turn.
enum Settled {
    Ready(Arc<ProxyCtx>),
    /// The route to the call's own server, built over the build's state before its commit.
    Routed(Arc<ProxyCtx>),
    Failed(String),
    /// The build resolved, but no context was committed — the generation was superseded and the
    /// commit tail shut the new state down instead of installing it.
    Gone,
}

/// What the build this call joined came to, once its driver has settled it.
///
/// The commit installs the proxy context before it settles the build, so a [`BuildState::Committed`]
/// build's context is there to read; a superseded build leaves none.
async fn settle(extension: &Arc<McpExtension>, build: &Arc<Build>) -> Settled {
    match build.settled().await {
        BuildState::Committed => extension.proxy_ctx().map_or(Settled::Gone, Settled::Ready),
        BuildState::Failed(message) => Settled::Failed(message),
        BuildState::Superseded | BuildState::Running => Settled::Gone,
    }
}

/// What a direct call to `server` that joined the build in flight comes to: the route to its own
/// server once that server has settled (MCP-635), or else what the build came to.
///
/// The build's answer wins a tie, so a call that finds both settled at once takes the committed
/// context rather than the pass's.
async fn settle_for(extension: &Arc<McpExtension>, build: &Arc<Build>, server: &str) -> Settled {
    let own = async {
        match build.startup().route_for(server).await {
            Some(ctx) => ctx,
            // The build's answer is the call's: wait for it alone.
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        settled = settle(extension, build) => settled,
        ctx = own => Settled::Routed(ctx),
    }
}

/// Whether `build` was superseded by a second build of the same generation, which is still
/// running under `owner` (MCP-641) — so a call that joined `build` has a build to go to instead.
fn superseded_within(
    extension: &Arc<McpExtension>,
    build: &Arc<Build>,
    owner: Option<&Arc<McpRuntimeOwner>>,
) -> bool {
    let same_generation = match (owner, extension.owner()) {
        (Some(captured), Some(current)) => Arc::ptr_eq(captured, &current) && current.is_active(),
        _ => false,
    };
    let replaced = extension.proxy_ctx().is_some()
        || extension
            .init_task()
            .is_some_and(|current| !Arc::ptr_eq(&current, build));
    same_generation && build.is_superseded() && replaced
}

/// Whether a call's result says it did not get an answer from its server.
fn unanswered(result: &Result<ToolResult, crate::errors::McpError>) -> bool {
    match result {
        Err(_) => true,
        Ok(result) => {
            result.is_error
                || result
                    .details
                    .as_ref()
                    .and_then(|details| details.get("error"))
                    .is_some()
        }
    }
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
    /// `call_id` is pi's `toolCallId`, which `direct-tools.ts:139` @v5.0.0 passes to the executor
    /// and `:321` sends to the server as `_meta["pi-mcp-adapter/toolCallId"]`
    /// ([`crate::proxy::OriginatingCall`], MCP-609). `on_update` is unused because no phase of this
    /// path streams; the trait fixes the signature, so it keeps its name with an underscore rather
    /// than being dropped.
    async fn call_direct(
        &self,
        spec: &DirectToolSpec,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // `throwIfAborted(signal)` (`direct-tools.ts:140` @v5.2.0): a call cancelled before it
        // began does not wait for anything.
        crate::abort::throw_if_aborted(&cancel, None)
            .map_err(|error| ToolError::new(error.to_string()))?;
        loop {
            let gate = self.gate(&spec.server_name, &cancel).await;
            let ctx = match gate.outcome {
                GateOutcome::Ready(ctx) => ctx,
                // [CYRUP-DELTA, upward] The call's own cancel ends its wait for the build; upstream's
                // `await ensureSessionRuntime(ctx)` observes no signal.
                GateOutcome::Aborted => {
                    return Err(ToolError::new(crate::abort::ABORTED_FALLBACK_REASON));
                }
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
                owner
                    .throw_if_inactive()
                    .map_err(|error| ToolError::new(error.to_string()))?;
            }

            let result = execute_call(
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
                if params.is_object() {
                    Some(&params)
                } else {
                    None
                },
                // The server hint that pins phases 1 and 3 and skips 2, 4 and 5.
                Some(&spec.server_name),
                &cancel,
                // `direct-tools.ts:440` `spec.resourceUri ? "resource" : "direct"`.
                Some(ApprovalOrigin::for_direct_tool(spec.resource_uri.as_ref())),
                OriginatingCall::of(&call_id).as_ref(),
            )
            .await;
            // MCP-641 — the call went through its server's route before the commit, and a second build
            // of this generation superseded that build while the call ran: its servers were closed
            // under the call, which therefore has no answer. It goes again through the generation's
            // current build. Only then: a call answered, failed for any other reason, cancelled, or
            // whose generation is over keeps the result it got. The cost, chosen over losing the
            // call: a request the closed server had already read may have run there, and runs
            // again on the winner.
            if let Some(route) = gate.route.as_ref()
                && !cancel.is_cancelled()
                && unanswered(&result)
                && let Some(extension) = self.extension.upgrade()
                && superseded_within(&extension, route, gate.owner.as_ref())
            {
                tracing::debug!(
                    "MCP: {} went through a build that was superseded; calling again through the \
                 current one",
                    spec.prefixed_name
                );
                continue;
            }
            return result.map_err(|error| ToolError::new(error.to_string()));
        }
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
        self.gateway_router()
            .execute(call_id, params, cancel, on_update)
            .await
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use serde_json::json;

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
            lazy: false,
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
        assert!(
            result
                .details
                .as_ref()
                .and_then(|d| d.get("mode"))
                .is_none()
        );
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

    // ---- The envelope `call_direct` builds for itself --------------------------------------------

    /// `init_failed` reports the rejection message in both the text and `details.message`.
    #[test]
    fn the_failure_envelope_carries_the_message_twice() {
        let result = init_failed_result("config parse failed");
        assert_eq!(error_code(&result).as_deref(), Some("init_failed"));
        assert_eq!(
            result.details.as_ref().and_then(|d| d.get("message")),
            Some(&json!("config parse failed"))
        );
        assert_eq!(
            text(&result),
            "MCP initialization failed: config parse failed"
        );
    }
}
