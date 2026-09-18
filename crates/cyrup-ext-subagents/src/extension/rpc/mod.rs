//! PB-8 — the **subagent RPC bridge**: the inter-extension surface through which a host, an
//! editor, or a sibling extension drives subagents programmatically.
//!
//! Ports pi `src/extension/rpc.ts` (848 lines @v0.68.0), registered upstream from
//! `src/extension/index.ts:759-764` and announced from its `session_start` handler at `:1186`.
//! cyrup registers the identical surface from
//! [`crate::extension::SubagentsExtension`]'s own `NativeExtension::init`
//! (`extension/host/native_impl.rs`), in the [`RegistrationMode::Full`] arm only.
//!
//! [`RegistrationMode::Full`]: crate::extension::RegistrationMode::Full
//!
//! # The client contract — the one page an integrator has to read
//!
//! Three topics, on the host-owned inter-extension bus ([`cyrup_ext::bus::SharedBus`]):
//!
//! | topic | direction | payload |
//! |---|---|---|
//! | [`SUBAGENT_RPC_REQUEST_EVENT`] | client → subagents | `{version: 1, requestId, method, params?, source?}` |
//! | `subagents:rpc:v1:reply:<requestId>` | subagents → client | `{version: 1, requestId, method?, success, data \| error}` |
//! | [`SUBAGENT_RPC_READY_EVENT`] | subagents → everyone | the [`ping::ping_data`] document, once per session start |
//!
//! **Subscribe to the FULL reply topic before you emit.** [`cyrup_ext::bus::SharedBus`] has no
//! prefix matching — `subscribers_for` is an exact compare (`cyrup-ext/src/bus.rs:119-130`) — so a
//! client subscribes to `subagents:rpc:v1:reply:<the id it is about to use>`, not to the prefix.
//! Both halves are public and can be driven per request: `SharedBus::subscribe`
//! (`cyrup-ext/src/bus.rs:46-53`) and `ExtensionHost::bus()` (`cyrup-ext/src/facade.rs:1859-1861`).
//! Use [`subagent_rpc_reply_event`] to spell the topic so a caller never builds it by hand.
//!
//! **Delivery is deferred, but not slow.** `SharedBus::emit` only QUEUES (`bus.rs:80-93`, with the
//! CYRUP-DELTA stated there — a WASM guest cannot fan out while it holds its own store), and
//! `BusFanout::drain_bus` (`facade.rs:2719-2760`) loops up to 64 rounds per drain, each round
//! draining the whole queue and re-checking. The consequence is the good one: a reply emitted from
//! inside `on_bus_event` is picked up by the NEXT ROUND OF THE SAME DRAIN, so a client subscribed
//! to the reply topic is reached inside the very same `deliver_bus_events(..)` call that carried
//! the request. There is no pump to arrange and nothing to sleep on.
//!
//! The price of that is stated where it is paid: the handler BODY runs on the host's drain, under
//! the single `DrainLatch`, where upstream's listener is fire-and-forget. See the
//! `[CYRUP-DELTA, mechanism]` on `SubagentsExtension::on_bus_event`
//! (`extension/host/native_impl.rs`) for what that blocks.
//!
//! # Guarantees
//!
//! * **Exactly one reply per request.** Upstream's `try`/`catch` (`rpc.ts:824-837`) has no silent
//!   return, and neither does this. `NativeExtension::on_bus_event` returns `Err` only when the
//!   reply itself could not be emitted — returning `Err` for a bad REQUEST would be contained and
//!   logged by `drain_bus` (`facade.rs:2740-2750`) and would hang the caller forever.
//! * **`requestId` is validated first** (`rpc.ts:773`), and a `\r`/`\n` is rejected: the reply
//!   topic is built from it. An unusable id still gets a reply, on the literal
//!   `subagents:rpc:v1:reply:unknown` (`rpc.ts:789-795`).
//! * **`ping` answers with no session** (`rpc.ts:709`), so a client can discover the surface before
//!   a session exists. **Every other method fails closed** with `no_active_session` (`:710`).
//! * **The tool's own gates are not bypassed.** Every method but `ping` and the in-memory `status`
//!   tier dispatches through the SAME [`crate::extension::SubagentTool`] instance the model uses —
//!   the one `init` registered, captured at registration — so the authority consult
//!   (`extension/tool/routing.rs:1729-1766`), the child-safe refusals and the single-dispatch
//!   guard all apply to an RPC caller too. See [`Self::dispatch`](SubagentRpcBridge::dispatch).
//! * **A `ChildSafe` registration answers no RPC at all**: it subscribes to no bus topic, so a
//!   fanout child never exposes the orchestrator's spawn/stop/manage surface.
//!
//! # `[CYRUP-DELTA, mechanism]` — `stop` is NOT re-implemented here
//!
//! Upstream's `stopAsyncRun` (`rpc.ts:561-701`) is 140 lines re-implementing the stop path inline:
//! resolve location, read status, session gate, child resolution, the workflow in-process branch,
//! reconcile, deliver. cyrup already ported that whole body one level down, behind
//! `route_control_action`'s `"stop"` arm (`extension/tool/routing.rs:1804-1809` →
//! [`crate::extension::SubagentExecutor::control_stop`]). Routing the RPC through the tool arm
//! reaches the identical machinery AND keeps the authority consult, which upstream's inline copy
//! bypasses. That is STRICTER than upstream and it is the correct direction: an RPC caller must
//! not be a way around a `Forbid`/`Confirm` authority policy.

pub(crate) mod envelope;
pub(crate) mod fleet;
pub(crate) mod params;
pub(crate) mod ping;

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Map, Value};

use cyrup_core::{CancelToken, Tool, ToolCallId};

use crate::extension::SubagentExecutor;
use crate::extension::tool::SubagentTool;

use envelope::{
    SubagentRpcError, SubagentRpcErrorCode, SubagentRpcRequest, error_reply, error_reply_topic,
    parse_request, success_reply,
};
use fleet::{FleetKeyState, build_fleet_status};
use params::{
    assert_record_params, has_status_target, manage_params, normalize_status_params,
    normalize_target_params, resume_params, spawn_params, steer_params,
};

/// pi `SUBAGENT_RPC_PROTOCOL_VERSION` (`rpc.ts:29`).
pub const SUBAGENT_RPC_PROTOCOL_VERSION: u32 = 1;
/// pi `SUBAGENT_RPC_REQUEST_EVENT` (`rpc.ts:30`) — the ONE topic this extension subscribes to.
pub const SUBAGENT_RPC_REQUEST_EVENT: &str = "subagents:rpc:v1:request";
/// pi `SUBAGENT_RPC_READY_EVENT` (`rpc.ts:31`).
pub const SUBAGENT_RPC_READY_EVENT: &str = "subagents:rpc:v1:ready";
/// pi `SUBAGENT_RPC_REPLY_EVENT_PREFIX` (`rpc.ts:32`). Never subscribe to this — see the module
/// doc; subscribe to [`subagent_rpc_reply_event`]'s full topic.
pub const SUBAGENT_RPC_REPLY_EVENT_PREFIX: &str = "subagents:rpc:v1:reply:";

/// pi `SUBAGENT_RPC_METHODS` (`rpc.ts:34`) — EIGHT, in upstream's order, which is the order a
/// `ping` reply advertises them in.
pub const SUBAGENT_RPC_METHODS: [&str; 8] = [
    "ping",
    "status",
    "manage",
    "spawn",
    "steer",
    "interrupt",
    "stop",
    "resume",
];

/// pi `SUBAGENT_RPC_MANAGEMENT_ACTIONS` (`rpc.ts:65-73`) — SEVEN of the nine `schedule.*` verbs
/// cyrup dispatches (`background/scheduled_runs/tool.rs:95-108` also knows `schedule.create` and
/// `schedule.run-due`). The narrowing is upstream's own; see [`params::manage_params`].
pub const SUBAGENT_RPC_MANAGEMENT_ACTIONS: [&str; 7] = [
    "schedule.list",
    "schedule.show",
    "schedule.history",
    "schedule.pause",
    "schedule.resume",
    "schedule.run",
    "schedule.delete",
];

/// pi `subagentRpcReplyEvent(requestId)` (`rpc.ts:334-336`) — the exact topic a client must have
/// subscribed to BEFORE it emitted its request.
#[must_use]
pub fn subagent_rpc_reply_event(request_id: &str) -> String {
    format!("{SUBAGENT_RPC_REPLY_EVENT_PREFIX}{request_id}")
}

/// pi `SubagentRpcMethod` (`rpc.ts:35`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentRpcMethod {
    Ping,
    Status,
    Manage,
    Spawn,
    Steer,
    Interrupt,
    Stop,
    Resume,
}

impl SubagentRpcMethod {
    /// The wire spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Status => "status",
            Self::Manage => "manage",
            Self::Spawn => "spawn",
            Self::Steer => "steer",
            Self::Interrupt => "interrupt",
            Self::Stop => "stop",
            Self::Resume => "resume",
        }
    }

    /// `SUBAGENT_RPC_METHODS.includes(raw.method)` (`rpc.ts:777`).
    pub(crate) fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ping" => Some(Self::Ping),
            "status" => Some(Self::Status),
            "manage" => Some(Self::Manage),
            "spawn" => Some(Self::Spawn),
            "steer" => Some(Self::Steer),
            "interrupt" => Some(Self::Interrupt),
            "stop" => Some(Self::Stop),
            "resume" => Some(Self::Resume),
            _ => None,
        }
    }
}

/// Everything one dispatch needs, gathered by the caller so this module owns no state but the
/// fleet key map.
///
/// `tool` is the `Arc<SubagentTool>` `init` REGISTERED — captured at registration, not rebuilt.
/// That equivalence is upstream's: `options.execute` is `executor.executePublic`
/// (`extension/index.ts:762`), the identical seam the registered `ToolDefinition.execute` calls at
/// `:776`. Rebuilding one here (through `SubagentsExtension::subagent_tool`) would give the RPC
/// path its own `DispatchGuard` and a default description — a parallel universe with a separate
/// single-dispatch budget.
pub(crate) struct RpcDeps<'a> {
    pub(crate) tool: &'a SubagentTool,
    pub(crate) executor: &'a SubagentExecutor,
    pub(crate) cwd: &'a Path,
}

/// pi's `registerSubagentRpcBridge` closure state (`rpc.ts:817-848`): ONE listener, ONE fleet key
/// map, for the life of the extension.
///
/// The map is behind a [`Mutex`] rather than being `&mut`-threaded because bus delivery is a
/// `&self` trait method (`cyrup_ext::native::NativeExtension::on_bus_event`) — upstream's
/// single-threaded closure capture has no direct analogue. The lock is held only across the
/// synchronous [`build_fleet_status`] call and is never held across an `await`.
#[derive(Debug, Default)]
pub(crate) struct SubagentRpcBridge {
    fleet_keys: Mutex<FleetKeyState>,
}

impl SubagentRpcBridge {
    /// pi `const fleetKeys: FleetKeyState = { sessionId: null, next: 0, keys: new Map() }`
    /// (`rpc.ts:821`).
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The listener body (`rpc.ts:822-838`): parse, handle, reply. Returns the reply TOPIC and the
    /// reply ENVELOPE, so the caller only has to emit.
    ///
    /// There is no error path out of this function, by construction — a fault becomes a reply,
    /// exactly as upstream's `catch` at `:834-837` does.
    pub(crate) async fn dispatch(&self, raw: &Value, deps: &RpcDeps<'_>) -> (String, Value) {
        let request = match parse_request(raw) {
            Ok(request) => request,
            // `:835-836` — a request that did not even parse still gets a reply, on the
            // `safeReplyRequestId` topic.
            Err(error) => return (error_reply_topic(raw), error_reply(raw, &error)),
        };
        tracing::debug!(
            target: "cyrup_ext_subagents::rpc",
            request_id = %request.request_id,
            method = request.method.as_str(),
            source = ?request.source,
            "subagent rpc request"
        );
        let topic = subagent_rpc_reply_event(&request.request_id);
        match self.handle_request(&request, deps).await {
            Ok(data) => (topic, success_reply(&request, data)),
            Err(error) => (topic, error_reply(raw, &error)),
        }
    }

    /// pi `handleRequest` (`rpc.ts:703-769`). The dispatch ORDER is upstream's, and the first two
    /// lines are the load-bearing ones: `ping` answers without a context, everything else fails
    /// closed without one.
    async fn handle_request(
        &self,
        request: &SubagentRpcRequest,
        deps: &RpcDeps<'_>,
    ) -> Result<Value, SubagentRpcError> {
        // `:709` — BEFORE the context check.
        if request.method == SubagentRpcMethod::Ping {
            return Ok(ping::ping_data(deps.executor, deps.cwd));
        }
        // `:710`. cyrup's analogue of upstream's `ExtensionContext` is the late-bound capability
        // backend: with no backend there is no session identity, no way to emit, and no way to
        // scope an answer, so the surface refuses rather than degrading into an unfiltered one.
        let Some(services) = deps.executor.host_services() else {
            return Err(SubagentRpcError::new(
                SubagentRpcErrorCode::NoActiveSession,
                "No active extension context for subagent RPC.",
            ));
        };

        match request.method {
            // Answered above.
            SubagentRpcMethod::Ping => Ok(ping::ping_data(deps.executor, deps.cwd)),
            // `:712-714`.
            SubagentRpcMethod::Manage => {
                let params = manage_params(request.params.as_ref())?;
                execute_checked(deps, request, params).await
            }
            // `:715-717`.
            SubagentRpcMethod::Spawn => {
                let params = spawn_params(request.params.as_ref())?;
                execute_checked(deps, request, params).await
            }
            // `:718-755`.
            SubagentRpcMethod::Status => {
                self.handle_status(request, deps, services.session_id().as_deref())
                    .await
            }
            // `:756-758`.
            SubagentRpcMethod::Steer => {
                let params = steer_params(request.params.as_ref())?;
                execute_checked(deps, request, params).await
            }
            // `:759-761`.
            SubagentRpcMethod::Interrupt => {
                let mut params = normalize_target_params(request.params.as_ref(), "interrupt")?;
                params.insert("action".to_string(), Value::from("interrupt"));
                execute_checked(deps, request, params).await
            }
            // `:762-764`. See this module's doc for why this does NOT re-implement
            // `stopAsyncRun`. `childId` is threaded through on top of the target whitelist because
            // cyrup's `control_stop` accepts a child-scoped stop (SUBA-087) and upstream's own
            // `stopAsyncRun` reads the same key (`rpc.ts:567`); its length/newline gate answers
            // with upstream's sentence from `background::control::validate_stop_child_id`.
            SubagentRpcMethod::Stop => {
                let input = assert_record_params(request.params.as_ref(), "stop")?;
                let mut params = normalize_target_params(request.params.as_ref(), "stop")?;
                params.insert("action".to_string(), Value::from("stop"));
                if let Some(child_id) = input.get("childId") {
                    params.insert("childId".to_string(), child_id.clone());
                }
                execute_checked(deps, request, params).await
            }
            // `:765-767`.
            SubagentRpcMethod::Resume => {
                let params = resume_params(request.params.as_ref())?;
                execute_checked(deps, request, params).await
            }
        }
    }

    /// pi's `status` arm (`rpc.ts:718-755`) — the TWO-TIER projection.
    ///
    /// `fleet` and `asyncSnapshot` ride on BOTH tiers (`:733-734` and `:748-753`). Wiring the
    /// snapshot into only the in-memory fast path would leave the executor-backed answer — the one
    /// every targeted `status` takes — without it.
    ///
    /// # `[CYRUP-DELTA, mechanism]` — the "in-memory" tier is a MATERIALIZED projection
    ///
    /// Upstream's fast path reads `options.state` (`:727-734`), a long-lived object the extension
    /// mutates in place, so both tiers share one live state and the fast one touches no disk.
    /// cyrup has no such cache: [`SubagentExecutor::fleet_state`] BUILDS the projection per call
    /// (`extension/executor/status.rs:200-330`). With `include_history: false` — what both tiers
    /// pass — the on-disk async root is not listed, but each TRACKED job still has its nested
    /// children read from disk (`status.rs:262`'s `read_nested_children`), so the fast tier costs
    /// one bounded read per live background run rather than nothing.
    ///
    /// What it does NOT do is MUTATE. Every read on this path is a read: the capacity number comes
    /// from [`active_async_capacity`]'s counting variant, not from the sweeping one, and is not
    /// measured at all when the fleet gate is closed. That matters because `status` is the one
    /// method an external bus client can drive in a loop without side effects, and upstream's is
    /// free of them.
    async fn handle_status(
        &self,
        request: &SubagentRpcRequest,
        deps: &RpcDeps<'_>,
        session_id: Option<&str>,
    ) -> Result<Value, SubagentRpcError> {
        let status_params = normalize_status_params(request.params.as_ref())?;
        // `:721`.
        if !has_status_target(&status_params) {
            // `:722-726` — upstream may THROW resolving the session and swallows it, letting the
            // executor produce the canonical error. cyrup's session identity is an `Option`
            // (`HostServices::session_id`), so there is nothing to catch: `None` simply fails the
            // gate below and the request takes the executor tier, which is the same outcome.
            let state = deps.executor.fleet_state(deps.cwd, false, false).await;
            // `:727`.
            if can_use_in_memory_status(&state, session_id) {
                let (fleet, snapshot) = self
                    .fleet_and_snapshot(deps, Some(&state), session_id)
                    .await;
                // `:730-735`.
                return Ok(serde_json::json!({
                    "text": in_memory_status_summary(&fleet),
                    "details": { "mode": "management", "results": [] },
                    "fleet": fleet,
                    "asyncSnapshot": snapshot,
                }));
            }
        }
        // `:738-744` — the executor tier, through the tool, so the child-safe gate and the
        // `id`-first precedence are the tool's own.
        let mut params = status_params;
        params.insert("action".to_string(), Value::from("status"));
        let status = execute_checked(deps, request, params).await?;
        // `:745-754` — the projection is taken AFTER the executor call, because that call
        // reconciles: a fleet built before it would describe the tree the status read repaired.
        let state = deps.executor.fleet_state(deps.cwd, false, false).await;
        let (fleet, snapshot) = self
            .fleet_and_snapshot(deps, Some(&state), session_id)
            .await;
        let mut merged = match status {
            Value::Object(map) => map,
            other => {
                let mut map = Map::new();
                map.insert("text".to_string(), other);
                map
            }
        };
        merged.insert("fleet".to_string(), fleet);
        merged.insert("asyncSnapshot".to_string(), snapshot);
        Ok(Value::Object(merged))
    }

    /// The two public projections a `status` reply carries, built from one materialized
    /// [`crate::tui::fleet_state::FleetState`] and one clock reading.
    ///
    /// **This is where UW-21 closes.** `build_async_status_snapshot_for_state`
    /// (`background/async_status_snapshot/state.rs:73-83`) had ZERO production callers — 1 928
    /// tested lines reachable only from their own unit tests. This call is pi's own
    /// `buildAsyncStatusSnapshotForState(options.state, sessionId)` (`rpc.ts:729` and `:753`), and
    /// it runs on BOTH status tiers exactly as upstream runs it on both.
    ///
    /// Its `:31` session gate (`state.rs:47-53`) is STRICT in all three arms and is NOT relaxed to
    /// make this easier: a session mismatch answers with an empty snapshot, and that is the
    /// answer.
    async fn fleet_and_snapshot(
        &self,
        deps: &RpcDeps<'_>,
        state: Option<&crate::tui::fleet_state::FleetState>,
        session_id: Option<&str>,
    ) -> (Value, Value) {
        // Measured only when [`build_fleet_status`]'s fail-closed gate will actually USE it: a
        // closed gate answers with `empty_fleet()`'s hardcoded `{used: 0, limit: 0}`
        // (`fleet.rs`'s `empty_fleet`), so listing the session's slot pool first would be I/O for
        // a number thrown away — on a path an external bus client can drive in a loop.
        let capacity = if fleet::fleet_gate_open(state, session_id) {
            active_async_capacity(deps.executor, session_id).await
        } else {
            crate::background::active_async_capacity::ActiveAsyncCapacitySnapshot {
                used: 0,
                limit: 0,
            }
        };
        let fleet = {
            let mut keys = self
                .fleet_keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            build_fleet_status(state, &mut keys, session_id, capacity)
        };
        let parsed_session = crate::identity::SessionId::parse_opt(session_id);
        let snapshot =
            crate::background::async_status_snapshot::build_async_status_snapshot_for_state(
                state,
                parsed_session.as_ref(),
                &crate::background::async_status_snapshot::AsyncStatusSnapshotOptions {
                    // One clock reading for the whole reply, as upstream's `generatedAt` option
                    // exists for (`async_status_snapshot/types.rs:288-289`).
                    generated_at: Some(crate::time::now_epoch_millis()),
                    ..Default::default()
                },
            );
        (
            fleet,
            serde_json::to_value(&snapshot).unwrap_or_else(|_| Value::Null),
        )
    }
}

/// pi `canUseInMemoryStatus` (`rpc.ts:415-424`).
///
/// # `[CYRUP-DELTA, unrepresentable]` — three of upstream's five clauses
///
/// Upstream conjoins five terms: the state exists, the session id is truthy,
/// `state.currentSessionId === sessionId`, `state.statusProjectionSessionId === sessionId`, and
/// `foregroundControls`/`asyncJobs` are both `instanceof Map`.
///
/// * `statusProjectionSessionId` has NO cyrup field — there is no separate projection-session
///   marker on [`crate::tui::fleet_state::FleetState`], because the projection here is
///   materialized fresh by `SubagentExecutor::fleet_state` on every call rather than being a
///   long-lived cache that could belong to a different session than the state around it.
/// * The two `instanceof Map` clauses are a JavaScript shape check on a field that could have been
///   assigned anything. `foreground_controls` and `tracked_jobs` are `Vec`s by TYPE here, so the
///   check is discharged by the compiler.
///
/// What survives is clauses 1-3 exactly — which is the whole of the session gate.
fn can_use_in_memory_status(
    state: &crate::tui::fleet_state::FleetState,
    session_id: Option<&str>,
) -> bool {
    session_id.is_some_and(|id| state.current_session_id.as_deref() == Some(id))
}

/// pi `inMemoryStatusSummary` (`rpc.ts:426-429`), including its singular/plural noun.
fn in_memory_status_summary(fleet: &Value) -> String {
    let total = fleet
        .get("totalActive")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let noun = if total == 1 { "child" } else { "children" };
    format!("In-memory subagent status: {total} active {noun}.")
}

/// pi's `state.activeAsyncCapacity` (`rpc.ts:301`), with the `{used: 0, limit: limit ?? 0}`
/// fallback `extension/executor/reports.rs:92-120` already uses for the doctor report on BOTH the
/// no-session and the `Err` arms (upstream `doctor.ts:193`).
///
/// # A `status` read does not RECONCILE
///
/// Upstream's `state.activeAsyncCapacity` is a plain FIELD on its live `SubagentState`
/// (`rpc.ts:301`); reading it costs nothing and changes nothing. cyrup's capacity lives in the
/// per-session slot pool on disk, and that module offers two reads:
/// `get_active_async_capacity_snapshot` is a bare alias of `reconcile_active_async_capacity`
/// (`background/active_async_capacity/sweep.rs:105-119`) which DELETES slots it can prove
/// released, while [`capacity::snapshot_for`] (`sweep.rs:25-48`) *"counts, does not reconcile"*.
///
/// The RPC `status` surface is a READ an external bus client can drive in a loop, so it takes the
/// counting one. Sweeping belongs to the admission path, which is where
/// `reconcile_active_async_capacity` is called from — exactly upstream's split, where the
/// reconciliation runs at spawn and the field is merely read here. `[CYRUP-DELTA, mechanism]`:
/// upstream reads a field, cyrup lists one directory; the reading is bounded and, unlike the
/// sweeping variant it replaces, has no side effect.
async fn active_async_capacity(
    executor: &SubagentExecutor,
    session_id: Option<&str>,
) -> crate::background::active_async_capacity::ActiveAsyncCapacitySnapshot {
    use crate::background::active_async_capacity as capacity;

    let cfg = executor.config_snapshot().await;
    let limit =
        capacity::resolve_max_active_async_runs_per_session(cfg.max_active_async_runs_per_session);
    let fallback = capacity::ActiveAsyncCapacitySnapshot {
        used: 0,
        limit: limit.unwrap_or(0),
    };
    let Some(session) = crate::identity::SessionId::parse_opt(session_id) else {
        return fallback;
    };
    capacity::snapshot_for(
        &session,
        limit,
        &SubagentExecutor::capacity_options(&cfg, executor.live_workflow_run_ids()),
    )
    .await
    .unwrap_or(fallback)
}

/// pi `executeChecked` (`rpc.ts:472-484`) — run the normalized object through the tool and flatten
/// the result.
///
/// Upstream builds a fresh `AbortController` per dispatch (`:480`); cyrup's equivalent is a fresh
/// [`CancelToken`]. The call id is upstream's own `rpc-<method>-<requestId>` (`:481`), which is
/// what makes an RPC-driven dispatch identifiable in a transcript.
///
/// # `[CYRUP-DELTA, mechanism]` — `failIfToolError` is the `Err` arm
///
/// Upstream's `AgentToolResult` carries an `isError` flag and `failIfToolError` (`:380-383`) turns
/// it into an `execution_failed` throw. cyrup's [`Tool::execute`] returns
/// `Result<ToolResult, ToolError>`: a refusal IS the `Err`, so the flag and the re-throw collapse
/// into one arm. [`crate::extension::SubagentTool`]'s one deliberate `Ok`-refusal — the
/// authority-declined branch (`extension/tool/routing.rs:1755-1764`) — stays a SUCCESS reply
/// carrying the decline text, which is upstream's own choice for that branch (`:4421`: a user
/// declining is a choice, not a failure).
async fn execute_checked(
    deps: &RpcDeps<'_>,
    request: &SubagentRpcRequest,
    params: Map<String, Value>,
) -> Result<Value, SubagentRpcError> {
    let call_id = ToolCallId::from(format!(
        "rpc-{}-{}",
        request.method.as_str(),
        request.request_id
    ));
    let result = deps
        .tool
        .execute(
            call_id,
            Value::Object(params),
            CancelToken::new(),
            Box::new(|_update| {}),
        )
        .await
        .map_err(|error| {
            // `:382` — upstream's fallback sentence when the failing result carried no text.
            let message = if error.message.trim().is_empty() {
                "Subagent RPC execution failed.".to_string()
            } else {
                error.message
            };
            SubagentRpcError::new(SubagentRpcErrorCode::ExecutionFailed, message)
        })?;
    // `:372-378` — `{text, details?}`.
    let text = result
        .content
        .iter()
        .filter_map(|part| match part {
            cyrup_core::Content::Text { text, .. } => Some(text.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut data = Map::new();
    data.insert("text".to_string(), Value::from(text));
    if let Some(details) = result.details {
        data.insert("details".to_string(), details);
    }
    Ok(Value::Object(data))
}

/// The reply a request gets when the bridge is subscribed but no registered tool was captured —
/// structurally unreachable (the subscription and the capture are the SAME arm of `init`), and
/// still answered rather than dropped, because the module's first guarantee is one reply per
/// request and a caller blocked on a reply topic cannot tell "impossible" from "ignored".
pub(crate) fn unregistered_reply(raw: &Value) -> (String, Value) {
    (
        error_reply_topic(raw),
        error_reply(
            raw,
            &SubagentRpcError::new(
                SubagentRpcErrorCode::NoActiveSession,
                "The subagent RPC surface is not registered in this process.",
            ),
        ),
    )
}
