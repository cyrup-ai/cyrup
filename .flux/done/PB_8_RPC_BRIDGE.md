---
stage: qa
status: completed
updated: 2026-09-18
---

# PB-8 — the subagent RPC bridge

OBJECTIVE: port `pi-subagents/src/extension/rpc.ts` (848 lines @ **v0.68.0**) — the event-bus RPC
surface through which an editor, host, or sibling extension drives subagents programmatically.
Today **nothing outside cyrup's own tool surface can delegate at all**: `grep -rn 'subagents:rpc'
crates/` returns **0**.

## Definition of done

A sibling extension (or the host) emits `subagents:rpc:v1:request` on the inter-extension bus and
receives a `subagents:rpc:v1:reply:<requestId>` envelope; `cyrup-ext-subagents` emits
`subagents:rpc:v1:ready` when it comes up. The bridge is registered from production `init`, not from
a test. At least one test drives the **production** path: real `NativeExtension::init` →
`SharedBus::emit` → host fan-out → `on_bus_event` → reply on the bus.

**No `allow(dead_code)`. No "landed unwired". No disclosure in place of completion.** This row exists
precisely because this programme has shipped tested machinery with no production caller five times
(UW-1, UW-4, UW-5, UW-8, UW-15, UW-21). Do not make it six.

## Upstream, pinned

Read **only** through `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`. Never a working
tree, never unpinned HEAD.

- `src/extension/rpc.ts` — 848 lines. `registerSubagentRpcBridge` at **`:817`**.
- Event constants at **`:30-32`**:
  - `SUBAGENT_RPC_REQUEST_EVENT = "subagents:rpc:v1:request"`
  - `SUBAGENT_RPC_READY_EVENT = "subagents:rpc:v1:ready"`
  - `SUBAGENT_RPC_REPLY_EVENT_PREFIX = "subagents:rpc:v1:reply:"`
  - `SUBAGENT_RPC_PROTOCOL_VERSION = 1` at `:29`
- `SUBAGENT_RPC_METHODS` at `:34` — **eight**, not seven:
  `["ping", "status", "manage", "spawn", "steer", "interrupt", "stop", "resume"]`.
  The ledger row says seven; it omits `manage`. **The ledger is wrong, the source is right.**
- `SUBAGENT_RPC_MANAGEMENT_ACTIONS` at `:65-73` — `schedule.list`, `schedule.show`,
  `schedule.history`, `schedule.pause`, `schedule.resume`, `schedule.run`, `schedule.delete`.
- `SubagentRpcErrorCode` at `:75-83` — `invalid_request`, `invalid_params`, `unsupported_version`,
  `unsupported_method`, `no_active_session`, `execution_failed`, `not_found`, `invalid_state`.
- Request/reply envelopes at `:37-63`. Fleet types at `:91-110`; bounds at `:112-116`
  (`MAX_FLEET_ENTRIES = 16`, `MAX_FLEET_CANDIDATES = 256`, `MAX_AGENT_LENGTH = 96`,
  `MAX_GOAL_LENGTH = 512`, `MAX_METADATA_LENGTH = 128`).
- Registered from `src/extension/index.ts:529`.

## cyrup side — the seam is LIVE, this is not blocked

The old `extension.rs:9313-9352` citation in `PARITY-GAPS.md` is **DEAD**; that file no longer
exists. Current addresses:

- Registration/subscription block: `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:45-250`
  (`async fn init`). `api.subscribe(&[…])` at `:246`; `register_tool`/`register_command` at `:48`,
  `:142`, `:156`, `:171`, `:200`, `:221`, `:234`.
- **Bus subscribe:** `cyrup_ext::native::InitApi::subscribe_bus(topic)` — `crates/cyrup-ext/src/native.rs:466`.
- **Bus delivery:** `NativeExtension::on_bus_event(&self, topic, payload, ctx)` —
  `crates/cyrup-ext/src/native.rs:523`, dispatched from `crates/cyrup-ext/src/facade.rs:2785`.
  An `Err` is contained and logged; it never stops fan-out.
- **Bus emit:** `HostServices::emit_event(&self, topic, payload)` —
  `crates/cyrup-ext/src/host/services.rs:468` (default no-op), with the **production**
  implementation at `crates/cyrup-session-svc/src/host_services.rs:1522`. Queued by
  `SharedBus::emit` (`crates/cyrup-ext/src/bus.rs:89`) and fanned out at
  `ExtensionHost::deliver_bus_events`. Delivery is **deferred**, not synchronous — a CYRUP-DELTA
  that is documented at `bus.rs` module level and is forced by the WASM store-reentrancy problem.
  Your reply arrives on the next seam boundary, and the test must account for that.
- Existing bus-seam proof: `crates/cyrup-ext/src/tests/payload_and_seam_parity.rs:722`
  (`a_native_extension_receives_inter_extension_bus_events`).

## Do NOT confuse this with SCOPE_12

`background/inspect_rpc/` (4 files, 2 459 LOC, landed `e61ff44`) ports a **different upstream file**
— `runs/background/inspect-rpc.ts` (443 LOC @v0.68.0), a read-only artifact inspector reached
through `/subagents-inspect-rpc` and cyrup's own `inspect` verb. It registers no event bridge and
answers no `ping`/`spawn`/`steer`. PB-8 is untouched by it.

## What each method maps onto in cyrup (verify, do not assume)

Most of the backing machinery already exists and is tested — this bridge is largely a dispatch layer,
which is what makes it genuinely reachable rather than a new subsystem:

- `spawn` → the executor's run-launch path used by the `subagents` tool (`extension/tool/routing.rs`).
- `steer` / `interrupt` / `stop` → the control-channel + steer-inbox machinery.
- `resume` → async-resume (`background/…`); upstream uses `resolveAsyncRunLocation` and
  `reconcileAsyncRun`.
- `status` → the fleet status + `topLevelAsyncCapacity` read; note this is where
  **`UW-21` closes for free** — `background/async_status_snapshot/` is 1 928 tested LOC whose module
  doc at `mod.rs:27-49` names PB-8 as one of its two blockers. Wire it.
- `manage` → the nine `schedule.*` verbs closed by SCOPE_11 / PB-11
  (`background/scheduled_runs/`). Seven of the nine are in the upstream action list.

## Rules

- Upstream reads pinned at `v0.68.0` via `git show`, cited by file and line in code comments.
- Any deliberate divergence gets a `[CYRUP-DELTA]` comment saying what and **why**, in the style the
  crate already uses.
- Workspace stays `cargo fmt` clean and clippy clean under every README gate.
- Baseline before this work: **10414/10414 passing.** Do not regress it.

---

# [AUG] Augmentation — implementation-grade spec

> Produced by the AUGMENT stage. Every `file:line` below was re-read in the tree at
> `/home/user/cyrup` and every upstream line was re-read through
> `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`. Corrections to the seed are called
> out explicitly.

## [AUG] 1 — `alreadyImplemented = false`. Confirmed, hard.

```
$ grep -rn 'subagents:rpc' crates/                                     → 0 hits
$ grep -rni 'rpc_bridge|SubagentRpc|SUBAGENT_RPC' crates/ --include=*.rs → 0 hits
$ grep -rn 'on_bus_event' crates/cyrup-ext-subagents/                   → 0 hits
$ grep -rn 'subscribe_bus' crates/cyrup-ext-subagents/                  → 0 hits
```

There is no `extension/rpc/` directory, no bus subscription anywhere in `cyrup-ext-subagents`, and
`SubagentsExtension` does not override `NativeExtension::on_bus_event` (it takes the
`Ok(())` default at `crates/cyrup-ext/src/native.rs:523-530`). The ONLY `on_bus_event` overrides in
the workspace are the two test doubles in `crates/cyrup-ext/src/tests/payload_and_seam_parity.rs`.

`background/inspect_rpc/` (SCOPE_12) is confirmed present and confirmed NOT this: it is reached
through `route_action`'s `"inspect"` arm (`extension/tool/routing.rs:1160-1179`), renders through
`background::inspect_rpc::respond::render_inspect_reply`, and registers no bus topic. The seed's
"Do NOT confuse this with SCOPE_12" section is correct.

**UW-21 confirmed open.** `grep -rn 'build_async_status_snapshot_for_state|async_status_snapshot_jobs_for_state|encode_async_status_snapshot_widget' crates/ --include=*.rs`
returns hits ONLY inside `background/async_status_snapshot/` itself (its own `pub use` at
`mod.rs:67` and its own unit test at `state.rs:125`). Zero production callers. The module's own
⚠ header at `background/async_status_snapshot/mod.rs:27-48` names this task as the blocker, verbatim
at `:30-31`.

## [AUG] 2 — upstream at v0.68.0, re-verified. Five stale citations in the seed.

`src/extension/rpc.ts` is **848 lines**, as stated. Anchors:

| what | seed says | ACTUAL @v0.68.0 | verdict |
|---|---|---|---|
| `SUBAGENT_RPC_PROTOCOL_VERSION = 1` | `:29` | `:29` | ✅ |
| the three event constants | `:30-32` | `:30-32` | ✅ |
| `SUBAGENT_RPC_METHODS` (eight) | `:34` | `:34` | ✅ |
| request/reply envelopes | `:37-63` | `:37-46` / `:48-63` | ✅ |
| `SUBAGENT_RPC_MANAGEMENT_ACTIONS` (seven) | `:65-73` | `:65-73` | ✅ |
| `SubagentRpcErrorCode` (eight codes) | `:75-83` | **`:77-85`** (`:75` is `SubagentRpcManagementAction`) | ❌ **STALE** |
| fleet types | `:91-110` | **`:92-103` + `:105-112`** | ❌ **STALE** |
| the five bounds | `:112-116` | **`:114-118`** | ❌ **STALE** |
| `registerSubagentRpcBridge` | `:817` | `:817` | ✅ |
| registered from `index.ts` | **`:529`** | **`:759-764`** (`:529` is `executorScheduled(randomUUID(), …)` inside `createScheduledRunManager`) | ❌ **STALE** |
| `dispose` / `emitReady` call sites | not cited | `index.ts:904` (in the `disposers` array) / `index.ts:1186` (inside the `session_start` handler) | ➕ new |

A sixth stale anchor is **in the cyrup tree, not in this task file**:
`crates/cyrup-ext-subagents/src/background/async_status_snapshot/mod.rs:30-31` cites
`extension/rpc.ts:725,749` for the two `buildAsyncStatusSnapshotForState` calls. At v0.68.0 they are
at **`:729` and `:753`**. Fix that citation in the same change that deletes the ⚠ section.

### The load-bearing upstream blocks, quoted

**Registration (`src/extension/index.ts:759-764`)** — note `execute` is `executor.executePublic`,
which is the exact seam cyrup's `SubagentTool::execute` already declares itself to be
(`extension/tool/mod.rs:245-253`):

```ts
	const rpcBridge = registerSubagentRpcBridge({          // :759
		events: pi.events,                                //  :760
		getContext: () => state.lastUiContext,            //  :761
		execute: (id, params, signal, onUpdate, ctx) => executor.executePublic(id, params, signal, onUpdate, ctx),  // :762
		state,                                            //  :763
	});
```

**The bridge itself (`:817-848`)** — ONE listener, ONE reply per request, errors replied not thrown:

```ts
export function registerSubagentRpcBridge(options: RegisterSubagentRpcBridgeOptions): {…} {   // :817
	const fleetKeys: FleetKeyState = { sessionId: null, next: 0, keys: new Map() };            // :821
	const unsubscribe = options.events.on(SUBAGENT_RPC_REQUEST_EVENT, async (raw) => {         // :822
		let request: SubagentRpcRequestEnvelope | undefined;
		try {
			request = parseRequest(raw);                                                       // :825
			const data = await handleRequest(request, options, fleetKeys);                     // :826
			options.events.emit(subagentRpcReplyEvent(request.requestId), {                    // :827
				version: SUBAGENT_RPC_PROTOCOL_VERSION, requestId: request.requestId,
				method: request.method, success: true, data,
			} satisfies SubagentRpcReplyEnvelope);
		} catch (error) {
			const reply = errorReply(request ?? raw, error);                                   // :835
			options.events.emit(subagentRpcReplyEvent(reply.requestId), reply);                // :836
		}
	});
	return {
		emitReady: (ctx) => { options.events.emit(SUBAGENT_RPC_READY_EVENT, pingData(ctx ?? options.getContext())); },  // :841-843
		dispose: () => { if (typeof unsubscribe === "function") unsubscribe(); },              // :844-846
	};
}
```

**`parseRequest` (`:771-787`)** — the ORDER of the three checks is load-bearing and is the whole
`invalid_request` / `unsupported_version` / `unsupported_method` split:

```ts
function parseRequest(raw: unknown): SubagentRpcRequestEnvelope {
	if (!isRecord(raw)) throw new SubagentRpcError("invalid_request", "Subagent RPC request must be an object.");  // :772
	const requestId = assertRequestId(raw.requestId);                                          // :773  ← FIRST
	if (raw.version !== SUBAGENT_RPC_PROTOCOL_VERSION) throw … "unsupported_version" …          // :774-776
	if (typeof raw.method !== "string" || !SUBAGENT_RPC_METHODS.includes(raw.method)) throw … "unsupported_method" …  // :777-779
	return { version: 1, requestId, method, ...(params), ...(source) };                        // :780-786
}
```

`assertRequestId` (`:342-347`): non-empty after trim AND **no `\r` or `\n`** — a newline in a
requestId would forge a second reply topic, which is why it is rejected at the very top.

`safeReplyRequestId` (`:789-795`): the SAME three tests, but returning the literal `"unknown"`
instead of throwing, so a malformed request still gets a reply envelope instead of silence.

**`handleRequest` (`:703-769`)** — dispatch order, and the `ping`-before-context rule:

```ts
	const ctx = options.getContext();
	if (request.method === "ping") return pingData(ctx);                                        // :709  ← ping works with NO context
	if (!ctx) throw new SubagentRpcError("no_active_session", "No active extension context for subagent RPC.");  // :710
	if (request.method === "manage")  return executeChecked(…, manageParams(request.params));   // :712-714
	if (request.method === "spawn")   return executeChecked(…, spawnParams(request.params));    // :715-717
	if (request.method === "status")  { … see below … }                                         // :718-755
	if (request.method === "steer")   return executeChecked(…, steerParams(request.params));    // :756-758
	if (request.method === "interrupt") return executeChecked(…, { action: "interrupt", ...normalizeTargetParams(…) });  // :759-761
	if (request.method === "stop")    return stopAsyncRun(request.params, options, ctx);        // :762-764
	if (request.method === "resume")  return executeChecked(…, resumeParams(request.params));   // :765-767
	throw new SubagentRpcError("unsupported_method", …);                                        // :768
```

**`status` (`:718-755`)** — the two-tier projection, and the UW-21 closure:

```ts
		const statusParams = normalizeStatusParams(request.params);
		let sessionId: string | undefined;
		if (!hasStatusTarget(statusParams)) {                                                   // :721
			try { sessionId = resolveCurrentSessionId(ctx.sessionManager); } catch { /* let the executor produce the canonical error */ }  // :722-726
			if (canUseInMemoryStatus(options.state, sessionId)) {                               // :727
				const fleet = buildFleetStatus(options.state, fleetKeys, sessionId);            // :728
				const asyncSnapshot = buildAsyncStatusSnapshotForState(options.state, sessionId); // :729  ← UW-21
				return { text: inMemoryStatusSummary(fleet), details: { mode: "management", results: [] }, fleet, asyncSnapshot };  // :730-735
			}
		}
		const status = await executeChecked(options, ctx, request.requestId, request.method, { action: "status", ...statusParams });  // :738-744
		sessionId ??= resolveCurrentSessionId(ctx.sessionManager);                              // :745
		return { ...status, fleet: buildFleetStatus(options.state, fleetKeys, sessionId), asyncSnapshot: buildAsyncStatusSnapshotForState(options.state, sessionId) };  // :746-754
```

So **`fleet` and `asyncSnapshot` ride on BOTH tiers** — the in-memory fast path AND the
executor-backed path. Wiring only the fast path leaves UW-21 half-open.

`canUseInMemoryStatus` (`:415-424`) is a FIVE-clause conjunction:
`state && sessionId && state.currentSessionId === sessionId && state.statusProjectionSessionId === sessionId && foregroundControls instanceof Map && asyncJobs instanceof Map`.

**`buildFleetStatus` (`:177-302`)** — the opaque-key state machine. Three properties that MUST
survive:

1. `:183-187` — the key map is **reset whenever the session changes**. `fleet-N` keys are only
   comparable within one session.
2. `:188-191` — with no state, no session, or a session mismatch the answer is the EMPTY fleet
   `{version:1, entries:[], totalActive:0, topLevelAsyncCapacity:{used:0,limit:0}, omitted:0}`
   AND the key map is cleared. This is a **fail-closed session gate**, exactly the class of
   `background/async_status_snapshot/state.rs:47-53`.
3. `:270,297-299` — keys for candidates that are no longer active are **evicted**, so the map does
   not grow without bound across a long session.
   `:273` caps entries at `MAX_FLEET_ENTRIES`; `:197` caps candidates at `MAX_FLEET_CANDIDATES`;
   `:300` reports `omitted = max(0, totalActive - entries.length)` — `totalActive` counts EVERY
   candidate, including ones past the cap.
4. `:276` — a candidate with no displayable `agent`, or a non-safe-integer/negative `startedAt`, is
   **dropped from `entries` but still counted in `totalActive`** (it was counted at `:196`).

**`pingData` (`:440-470`)** — the capability advertisement. Every key here is a PROMISE; anything
advertised and not implemented is a lie the executor must not ship.

**The param normalizers (`:385-559`)** — each is a gate whose refusal text is upstream's:
- `normalizeTargetParams` (`:385-396`) — `{id, runId, dir, index}` only, nothing else.
- `normalizeStatusParams` (`:398-404`) — the four above plus `view`/`lines`.
- `hasStatusTarget` (`:406-413`) — true if ANY of the six is present. `view`/`lines` COUNT as
  targets: `status {view:"fleet"}` must go to the executor, not to the in-memory tier.
- `manageParams` (`:486-512`) — action ∈ the seven; `id` required for all but `schedule.list`;
  `quiet` boolean and only meaningful for `schedule.run`; `id` is `.trim()`ed.
- `spawnParams` (`:514-525`) — **rejects any `action`** (`:518-520`) and **rejects `async: false`**
  (`:521-523`), then forces `async: true` (`:524`). RPC spawn is detached-only.
- `steerParams` (`:527-541`) — non-empty `message`; at least one of `id`/`runId`/`dir`; `mode` ∈
  `steer|follow_up|auto`; `steeringRecovery: false` forced.
- `resumeParams` (`:543-559`) — non-empty `message`; at least one of `id`/`runId`/`dir`;
  `output` non-empty if present; `outputMode` may only be `"file-only"`.
- `assertRecordParams` (`:349-353`) — absent params ⇒ `{}`; a non-object ⇒ `invalid_params`.
- `executeChecked` (`:472-484`) — validates against the tool schema, runs with a FRESH
  `AbortController`, `failIfToolError` (`:380-383`) turns an `isError` result into
  `execution_failed`, and `dataFromToolResult` (`:372-378`) flattens to `{text, details?, isError?}`.
- `stopAsyncRun` (`:561-701`) — the one method that does NOT route through `execute`; see §3.7.

## [AUG] 3 — the cyrup seam map, method by method

Everything below **already exists** in the tree. The bridge is a dispatch + envelope layer, not a
new subsystem. That is precisely why it can be genuinely reachable.

### 3.0 The bus seam (all four halves verified live)

| upstream | cyrup | status |
|---|---|---|
| `pi.events.on(channel, h)` | `InitApi::subscribe_bus(topic)` — `crates/cyrup-ext/src/native.rs:466-468` | ✅ |
| listener invocation | `NativeExtension::on_bus_event(&self, topic, payload, ctx)` — `crates/cyrup-ext/src/native.rs:523-530`, default `Ok(())` | ✅ |
| fan-out | `BusFanout::drain_bus` — `crates/cyrup-ext/src/facade.rs:2719-2760`; native arm dispatches at **`:2785`** with an EVENT-tier `HostCtx` | ✅ |
| `pi.events.emit(channel, data)` | `HostServices::emit_event` — default no-op at `crates/cyrup-ext/src/host/services.rs:468`; **production impl** `crates/cyrup-session-svc/src/host_services.rs:1522-1527` (one line: `bus.emit(topic.to_string(), payload.clone())`) | ✅ |
| the queue | `SharedBus::emit` — `crates/cyrup-ext/src/bus.rs:89-93` | ✅ |
| the bus is attached to the live backend | `host_services.attach_event_bus(Arc::clone(host.bus()))` — `crates/cyrup-session-svc/src/builder.rs:1186`, run BEFORE the native load loop | ✅ |
| extension holds the backend | `NativeExtension::set_host_services` → `native_impl.rs:659-661` → `SubagentExecutor::set_host_services` (`extension/executor/mod.rs:431`); read back with `SubagentExecutor::host_services()` (`mod.rs:437`) | ✅ |

**The deferral is real and the executor must account for it.** `SharedBus::emit` only queues
(`bus.rs:80-93`, with the CYRUP-DELTA stated there). Fan-out happens in
`BusFanout::drain_bus`, which loops up to `MAX_ROUNDS = 64` (`facade.rs:2731`), each round draining
the whole queue and then re-checking. **Consequence, and it is GOOD news:** a reply emitted from
inside `on_bus_event` is picked up by the *next round of the same drain*. A sibling extension
subscribed to the reply topic is therefore reached inside the very same
`host.deliver_bus_events(...)` call as the request. There is no extra pump to arrange.

**`SharedBus` has NO prefix matching** (`subscribers_for` is an exact `t == topic` compare,
`bus.rs:119-130`). So a client subscribes to the FULL reply topic
`subagents:rpc:v1:reply:<requestId>` before it emits, exactly as upstream's
`subagentRpcReplyEvent(requestId)` names it. `SharedBus::subscribe` is `pub` (`bus.rs:46-53`) and
`ExtensionHost::bus()` is `pub` (`facade.rs:1859-1861`), so a client may subscribe dynamically per
request — it does not have to declare every reply topic at `init`. **Say this in the module doc; it
is the single thing a client integrator has to know.**

Existing proof the whole native bus path works end to end:
`crates/cyrup-ext/src/tests/payload_and_seam_parity.rs:722`
(`a_native_extension_receives_inter_extension_bus_events`) — verified at that exact line.

### 3.1 `ping`
`pingData` needs no executor at all. `session` comes from
`SubagentExecutor::host_services()` → `HostServices::session_id()` / `session_file()`
(`crates/cyrup-ext/src/host/services.rs`; live impls at `cyrup-session-svc/src/host_services.rs:1505`
and `:1530`), and `cwd` from the extension's captured `self.cwd`
(`extension/host/mod.rs:44`, the field's own doc explains why it is captured at construction).

### 3.2 `status` — and this is where UW-21 closes
- The executor-backed tier: `SubagentExecutor::control_status_view(cwd, id, dir, child_safe,
  StatusViewSelector{view, lines, index})` — `extension/executor/status.rs:327-334`, returns
  `Result<String, String>`. This is the SAME call `route_control_action`'s `"status"` arm makes
  (`extension/tool/routing.rs:1198-1214`). Routing the RPC through `SubagentTool::execute` with
  `{"action":"status", …}` reaches it and gets the child-safe gate, the authority consult and the
  `id`-first precedence for free.
- The in-memory tier: `SubagentExecutor::fleet_state(cwd, include_history, fleet_inspector_open)` —
  `extension/executor/status.rs:200-205`, returns `crate::tui::fleet_state::FleetState`, cyrup's
  port of upstream's `SubagentState` (the struct is at `tui/fleet_state.rs:460-504`). Pass
  `include_history: false` — upstream's fleet status projects LIVE work only
  (`rpc.ts:199-263` reads `foregroundControls` + `asyncJobs`, never a disk listing), and
  `fleet_state`'s own doc at `status.rs:155-158` says `false` is the always-on-widget/live tier.
- `topLevelAsyncCapacity`:
  `crate::background::active_async_capacity::get_active_async_capacity_snapshot(&session, limit,
  &options)` — `background/active_async_capacity/sweep.rs:113-119`, returning
  `ActiveAsyncCapacitySnapshot { used, limit }` (`key.rs:80-87`). The production precedent for the
  whole read — limit resolution, `SessionId::parse_opt`, and the `{used:0, limit: limit ?? 0}`
  fallback on both the no-session and the `Err` arms — is
  `extension/executor/reports.rs:92-120`. **Copy that block's shape, including its warning that the
  read RECONCILES (it sweeps), so it must not be called from a pure formatter.** `Self::capacity_options`
  is the options builder it uses.
- `asyncSnapshot`: **`crate::background::async_status_snapshot::build_async_status_snapshot_for_state(
  Some(&fleet_state), Some(&session_id), &AsyncStatusSnapshotOptions::default())`** —
  `background/async_status_snapshot/state.rs:73-83`, re-exported at `mod.rs:67`. Its own
  `:31` session gate (`state.rs:47-53`) is STRICT in all three arms and must not be relaxed.
  `AsyncStatusSnapshotOptions` is at `types.rs:287-300`; `generated_at: Some(now)` should be
  threaded from the same clock the rest of the reply uses.
  **Wiring this single call is the whole of UW-21.** After it lands, delete the ⚠ section at
  `mod.rs:27-48` and replace it with the real caller, and fix the `:725,749` → `:729,753` citation.
- `canUseInMemoryStatus`'s five clauses: cyrup has `FleetState::current_session_id`
  (`fleet_state.rs:471`) for clause 3. **There is no `statusProjectionSessionId`** (clause 4) and
  the two `instanceof Map` clauses (5, 6) are unrepresentable in Rust — `foreground_controls` and
  `tracked_jobs` are `Vec`s by type. `[CYRUP-DELTA, unrepresentable]`: the gate becomes
  `state.current_session_id.as_deref() == Some(session.as_str())`, which is clauses 1–3 exactly.

### 3.3 `manage` → the nine `schedule.*` verbs
`ScheduledRunAction::from_wire` — `background/scheduled_runs/tool.rs:95-108` — accepts
`schedule.create|list|show|history|pause|resume|run|run-due|delete`. Upstream's RPC action list has
**seven of the nine** (no `create`, no `run-due`), and that narrowing is upstream's choice, not an
omission: keep the RPC list at exactly the seven at `rpc.ts:65-73`. Dispatch lands in
`route_action`'s `schedule_action` guard arm (`extension/tool/routing.rs:1238-1260`), which already
applies `is_mutating()`'s child-safe refusal (`tool.rs:117-119`).

### 3.4 `spawn`
`SubagentTool::execute` (`extension/tool/mod.rs:173-179`, `impl Tool` at `:147`) with the
normalized object and `"async": true` (`SubagentToolParams`'s field is
`#[serde(rename = "async")]`, `extension/tool/params.rs`). The `action`-must-be-absent guard is
`normalize_public_subagent_execution`'s neighbour — cyrup's own public boundary is
`normalize_public_subagent_execution(action)` at `extension/tool/params.rs:57-70`, called from
`Tool::execute` at `mod.rs:252`, and its doc at `:40-52` already records which of upstream's
`public-execution.ts` clauses are and are not ported. The RPC's `spawn` guard is a SEPARATE,
RPC-local rule (`rpc.ts:518-523`) and belongs in the bridge, not in that function.

### 3.5 `steer`
`route_control_action`'s `"steer"` arm → `SubagentExecutor::control_steer(cwd, run_id_or_id, dir,
message, task, index, mode)` (`extension/tool/routing.rs:1283-1296`). `mode` is validated
downstream by `crate::background::control::SteerDeliveryMode::parse` — the bridge still applies
upstream's own `steer|follow_up|auto` check first so the caller gets the RPC's sentence.
**`steeringRecovery` has NO cyrup analogue** — `grep -rn 'steering_recovery|steeringRecovery' crates/`
returns 0; upstream declares it at `schemas.ts:314` as *"forced false by extension RPC for exact
ownership"*. `[CYRUP-DELTA, unrepresentable]`: cyrup's steer has no pause/revive-after-missed-ack
mode to turn off, so the flag is omitted and the delta comment says exactly that.

### 3.6 `interrupt` / `resume`
`route_control_action`'s `"interrupt"` arm → `control_interrupt(cwd, target)`
(`extension/executor/control.rs:25`), `runId`-first precedence at `routing.rs:1216-1220`.
`"resume"` → `control_resume(cwd, target, message, task, index)`
(`extension/executor/control.rs:111`), `id`-first, with `validate_execution_acceptance` ahead of it
(`routing.rs:1264-1281`).

### 3.7 `stop` — the ONE method upstream does not route through `execute`
Upstream's `stopAsyncRun` (`rpc.ts:561-701`) re-implements the stop path inline: resolve location →
read status → session gate → child resolution → the workflow in-process branch → `reconcileAsyncRun`
→ `deliverStopRequest`.

**Do NOT re-implement it here.** cyrup already ported that whole body, one level down, behind
`route_control_action`'s `"stop"` arm (`extension/tool/routing.rs:1226-1235`) →
`SubagentExecutor::control_stop(cwd, target, dir, child_id)`. Its ingredients are all present and
were checked: `resolve_async_run_id`/`AsyncRunLocation` (`background/run_id_resolver.rs:23`,
`:183`), `resolve_async_status_child` (`background/child_identity.rs:156`),
`is_stoppable_async_status_step` (`:211`), `deliver_stop_request` (`background/control.rs:1051`),
`deliver_child_stop_request` (`:1067`), the workflow child-stop registry
(`extension/executor/workflow_child_stops.rs`) and controllers (`workflow_controllers.rs`).

`[CYRUP-DELTA, mechanism]`: the RPC `stop` routes through the SAME tool arm the model uses, so the
authority consult (`routing.rs:1723-1765`) and the child-safe gate apply to an RPC stop as well.
That is stricter than upstream, and it is the correct direction — an RPC caller must not be a way
around a `Forbid`/`Confirm` authority policy. **Say so in the delta comment.**

`emitChildStopping` (`rpc.ts:595-613`) emits `SUBAGENT_CHILD_STATUS_EVENT`. **cyrup has no
child-status event at all** (`grep -rn 'child-status|SUBAGENT_CHILD_STATUS' crates/ --include=*.rs`
finds only `watchdog::child_status`, an unrelated config codec). `[CYRUP-DELTA]`: `pingData` must
NOT advertise `events.childStatus` / `events.processTerminal` unless they are emitted. cyrup has no
process-terminal artifact at all — already recorded at
`background/active_async_capacity/key.rs:93-99` and `inspect.rs:33`. Advertise only what is emitted.

### 3.8 Error-code mapping
| upstream code | cyrup source |
|---|---|
| `invalid_request` / `unsupported_version` / `unsupported_method` | the bridge's own `parse_request` |
| `invalid_params` | the bridge's own normalizers |
| `no_active_session` | `SubagentExecutor::host_services()` is `None`, or `session_id()` is `None` |
| `execution_failed` | `SubagentTool::execute` returned `Err(ToolError)` — `ToolError.message` is the reply message (`cyrup-core/src/tool.rs:140-173`) |
| `not_found` / `invalid_state` | **NOT separable at this seam.** `control_stop`/`control_resume` return their refusals as `Err(ToolError)` strings with upstream's own sentences, not as typed codes. `[CYRUP-DELTA, mechanism]`: the bridge maps every tool `Err` to `execution_failed` and carries the upstream sentence verbatim in `message`. Do **not** string-match the message to re-derive a code — that would be a second, drifting copy of the classification. State the delta and move on. |

### 3.9 The fleet entry's token shape
Upstream's `TokenUsage` is `{input, output, total, window?, windowPeak?}` (`rpc.ts:126-152`).
cyrup's is `TokenTotals { input: u64, output: u64, total: u64 }`
(`background/telemetry.rs:41-48`), and the two fleet views only carry a scalar
`tokens: Option<u64>` (`tui/fleet_state.rs:195` and `:264`) — there is no per-view
input/output split and no context-window reading anywhere.
`[CYRUP-DELTA, unrepresentable]`: the RPC fleet entry's `tokens` is
`{input: 0, output: 0, total: <the view's scalar>}`, and `window`/`windowPeak` are omitted (upstream
omits them when absent too, `:149-150`). `publicTokens`'s clamping (`:126-152`) still ports — it is
what makes a garbage value a `0` rather than a `NaN` on the wire.

### 3.10 `displayText`
`crate::workflows::display_text::sanitize_display_text` (`workflows/display_text.rs:82`) and
`truncate_display(value, max_utf16_units)` (`:153`) are exact ports of upstream's
`sanitizeDisplayText` / `truncateDisplayText` (re-read at `shared/display-text.ts:33` and `:84`;
`truncateDisplayText` adds NO ellipsis — `previewDisplayText` at `:95` is the one that does, and it
is NOT what `rpc.ts:120-124` calls). Reuse both. Keep the `value.slice(0, 4_096)` pre-slice
(`rpc.ts:122`) — `background/async_status_snapshot/types.rs:325-327` already documents why that
pre-slice is load-bearing rather than an optimisation.

## [AUG] 4 — THE PRODUCTION CALL SITES. This is the deliverable.

Four edits, all in `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs`, all in the
`RegistrationMode::Full` arm (a `ChildSafe` fanout child must register NO RPC surface — it
subscribes to nothing today and must keep subscribing to nothing).

### 4.1 Subscribe — `native_impl.rs`, `async fn init`, Full arm, immediately before the existing `api.subscribe(&[…])` at `:246`

```rust
// PB-8 / pi `registerSubagentRpcBridge({ events: pi.events, … })`
// (`extension/index.ts:759-764` @v0.68.0, impl `extension/rpc.ts:817-848`). Full arm only:
// a ChildSafe fanout child registers no orchestrator surface and must not answer RPC.
api.subscribe_bus(crate::extension::rpc::SUBAGENT_RPC_REQUEST_EVENT);
```

`InitApi::subscribe_bus` is `crates/cyrup-ext/src/native.rs:466`. The host turns the declared topics
into `SharedBus::subscribe` calls at `crates/cyrup-ext/src/facade.rs:532`.

### 4.2 Capture the tool — `native_impl.rs:142-147`

The existing block is:

```rust
api.register_tool(Arc::new(
    SubagentTool::new(self.executor.clone(), self.cwd.clone())
        .with_watchdog(Arc::clone(&self.watchdog))
        .with_description(resolved_description),
));
```

Change it to build the `Arc<SubagentTool>` ONCE, register THAT `Arc`, and stash a clone on the
extension (a `OnceLock<Arc<SubagentTool>>` field on `SubagentsExtension`,
`extension/host/mod.rs:38-92`) so `on_bus_event` dispatches into the **same tool instance the model
uses** — same resolved description, same `allow_mutating_management: true`, same
`DispatchGuard`. That equivalence is upstream's: `options.execute` at `index.ts:762` is
`executor.executePublic`, the identical seam the registered `ToolDefinition.execute` calls at
`index.ts:776`.

**Do NOT call `SubagentsExtension::subagent_tool()` (`extension/host/mod.rs:378-381`) from the
bridge.** It constructs a FRESH tool with default description and its own `DispatchGuard`, and its
doc says plainly it exists for tests and non-`InitApi` callers. Using it would make the RPC path a
parallel universe with its own single-dispatch guard.

### 4.3 Answer — `impl NativeExtension for SubagentsExtension`, a new `on_bus_event`, sited next to `on_event` (after it, before `set_host_services` at `:659`)

```rust
async fn on_bus_event(&self, topic: &str, payload: &serde_json::Value, ctx: &HostCtx)
    -> Result<(), ExtError>
{
    // pi's ONE listener (`extension/rpc.ts:822-838`): parse, handle, reply. A handler fault is
    // REPLIED, never thrown — upstream's try/catch at `:834-837`. Returning `Err` here would be
    // contained and logged by `BusFanout::drain_bus` (`cyrup-ext/src/facade.rs:2740-2750`) and the
    // caller would wait forever for a reply that never comes.
}
```

The reply goes out through
`self.executor.host_services().ok_or(no_active_session)?.emit_event(&reply_topic, &reply_json)`.

### 4.4 Announce — `native_impl.rs`, `on_event`, `HostEvent::SessionStart` arm, after `self.refresh_fleet_status_widget(&ctx.cwd, ctx.has_ui).await;` at **`:412`**

```rust
// pi `rpcBridge.emitReady(ctx)` (`extension/index.ts:1186`, inside its own `session_start`
// handler): a client that attached before this process came up learns the surface is live and
// reads the capability set off the ready payload instead of polling `ping`.
self.emit_rpc_ready().await;
```

`:412` is the last line of the SessionStart arm and is upstream's position (`emitReady` at `:1186`
sits at the tail of pi's `session_start` body, after the herdr bridge and before
`supervisorChannel.start()`).

### 4.5 (Required for `pingData` honesty) — bridge the completion fan-out onto the bus

`pingData` advertises `events.asyncComplete` (`rpc.ts:464`). cyrup's analogue of
`SUBAGENT_ASYNC_COMPLETE_EVENT` is the in-process `CompletionObserver` fan-out
(`background/watch/observer.rs:22-35`, whose doc names that exact upstream event). Add a **fifth**
member to the composite at `extension/executor/notices.rs:454-486` — a
`BusAnnouncingCompletionObserver` that re-emits each `CompletionNotification` on the inter-extension
bus. Register it AFTER `self.wait_completions()` (which must stay first, `:456-462`) and its
position relative to the others is not load-bearing; the composite's ordering doc at `:455-483`
explains which positions ARE. `install_completion_watcher` is called from production at
`native_impl.rs:377`.

Without this, either the ready/ping payload advertises an event nothing emits (a lie), or the
advertisement is dropped and a delegating host has to poll `status` in a loop — which is exactly the
"help agents be more effective in delegate and graph patterns" the mandate names. **Do the wiring.**

### 4.6 What must NOT be done
- **No new crate, no new extension.** The bridge is a module of `cyrup-ext-subagents`.
- **No `#[allow(dead_code)]`, no stub type, no `todo!()`.** Every constant, every method arm and
  every capability key must be reachable from `on_bus_event` or from `emit_rpc_ready`.
- **No `pingData` key that nothing implements.** Drop `childStatus`, `processTerminal`,
  `nonRecoveringSteer` and `launchResolvedExtensions`/`runtimeAcknowledgedExtensions` unless the
  backing behaviour is verified present; each drop gets a `[CYRUP-DELTA]` naming the missing seam.

## [AUG] 5 — the reachability test

**Home: `crates/cyrup-ext-subagents/src/tests/rpc_bridge_integration.rs`**, declared in
`crates/cyrup-ext-subagents/src/tests/mod.rs` (add `mod rpc_bridge_integration;` in alphabetical
position, between `read_only_agent_name_alternation` and `runtime_agent_registration_integration`).

**Why here and not `crates/cyrup-it`:** `cyrup-it`'s targets are gated behind `required-features =
["it"]` (`crates/cyrup-it/Cargo.toml`, THE GATE block) and do **not** run under the merge gate
`cargo nextest run --workspace --features test-fixtures` (README:249). A reachability proof that the
merge gate never executes is not a guard. `crates/cyrup-ext-subagents/src/tests/` runs in the merge
gate and has the exact precedent: `bundled_resources_registration_integration.rs:42-55` builds a real
`ExtensionHost` and `load_native`s the real `SubagentsExtension`.

### The production path the test drives — every hop is real

```
ExtensionHost::new(HostConfig{ mode: Tui, has_ui: true, cwd })          (facade.rs)
  → host.load_native_with_services(Arc::new(SubagentsExtension::with_config_and_cwd(..)), svc)   (facade.rs:370)
      → NativeExtension::set_host_services   (native_impl.rs:659)  ← REAL
      → NativeExtension::init                (native_impl.rs:45)   ← REAL, Full arm
          → InitApi::subscribe_bus(SUBAGENT_RPC_REQUEST_EVENT)     (native.rs:466)  ← REAL
          → host turns it into SharedBus::subscribe                (facade.rs:532)  ← REAL
  → host.bus().subscribe(<observer id>, "subagents:rpc:v1:reply:pb8-1")             ← how a real client attaches
  → host.bus().emit("subagents:rpc:v1:request", <envelope>)        (bus.rs:89)      ← REAL
  → host.deliver_bus_events(&CancelToken::new())                   (facade.rs:1850) ← REAL
      → BusFanout::drain_bus round 1                               (facade.rs:2719) ← REAL
          → SubagentsExtension::on_bus_event                       (facade.rs:2785) ← THE FEATURE
              → SubagentTool::execute / fleet_state / snapshot                       ← REAL
              → HostServices::emit_event → SharedBus::emit                           ← REAL
      → BusFanout::drain_bus round 2 delivers the reply to the observer              ← REAL
```

The only test-authored object is the `HostServices` impl. It must be a **bus-forwarding** one whose
`emit_event` body is the same single line as production
(`cyrup-session-svc/src/host_services.rs:1523-1526`), plus a `session_id()` returning a fixed id and
a `session_file()` returning a path under the tempdir. Put a comment on it citing that production
line so the equivalence is checkable rather than asserted. (`cyrup_ext::host::RecordingServices`,
`cyrup-ext/src/host/services.rs:914`, does **not** override `emit_event`, so it cannot be reused.)

### The tests, and what each one kills

1. **`a_host_drives_ping_over_the_inter_extension_bus_and_gets_a_reply`** — THE proof.
   Asserts the observer saw exactly one delivery on `subagents:rpc:v1:reply:pb8-1`, and that the
   payload has `version == 1`, `requestId == "pb8-1"`, `method == "ping"`, `success == true`,
   `data.methods` == the eight names in upstream order, `data.events.replyPrefix ==
   "subagents:rpc:v1:reply:"`, and **`data.session.sessionId == "<the id the test's HostServices
   returns>"`**.
   *Gutting it fails:* a hardcoded/absent reply fails the `sessionId` assertion, which can only be
   satisfied by actually consulting the live backend through `SubagentExecutor::host_services()`.
   Removing `subscribe_bus` means zero deliveries. Removing the `emit_event` means zero deliveries.
   Replacing `on_bus_event` with `Ok(())` means zero deliveries.

2. **`an_unknown_method_and_a_bad_version_are_replied_not_dropped`** — two requests:
   `{version:1, requestId:"pb8-2", method:"teleport"}` → `success:false`,
   `error.code == "unsupported_method"`; `{version:2, requestId:"pb8-3", method:"ping"}` →
   `error.code == "unsupported_version"`. And a request with `requestId: "bad\nid"` produces a reply
   on `subagents:rpc:v1:reply:unknown` with `error.code == "invalid_request"`.
   *Gutting it fails:* a bridge that returns `Err` from `on_bus_event` instead of replying leaves the
   caller hanging; `drain_bus` contains the `Err` (`facade.rs:2740`) and the test sees no delivery.
   The `unknown` reply topic is the only thing that proves `safeReplyRequestId` (`rpc.ts:789-795`)
   was ported rather than skipped.

3. **`an_rpc_status_carries_the_fleet_and_the_async_status_snapshot`** — **the UW-21 proof.**
   Request `{method:"status"}` with no target. Asserts `data.fleet.version == 1`,
   `data.fleet.entries` is an array, `data.fleet.topLevelAsyncCapacity` has both `used` and `limit`,
   **and `data.asyncSnapshot.kind == "pi-subagents.async-status-snapshot"`** with
   `data.asyncSnapshot.version == ASYNC_STATUS_SNAPSHOT_VERSION`.
   *Gutting it fails:* `asyncSnapshot` is producible ONLY by calling
   `build_async_status_snapshot_for_state`. Drop that call and the key is absent. This assertion is
   the thing that converts 1 928 tested-but-dead LOC into a reachable surface, so it must be an
   assertion on the KIND constant, not on "some object is present".

4. **`an_rpc_status_with_a_target_still_carries_the_fleet_and_snapshot`** — same two keys, but with
   `{method:"status", view:"fleet"}` so `hasStatusTarget` (`rpc.ts:406-413`) is true and the request
   takes the **executor** tier (`rpc.ts:738-754`).
   *Gutting it fails:* wiring the snapshot only into the in-memory fast path — the easy half-job —
   fails this one. Upstream attaches it to both tiers and so must cyrup.

5. **`rpc_spawn_refuses_an_action_and_refuses_a_non_detached_launch`** —
   `{method:"spawn", params:{agent:"x", action:"steer"}}` → `error.code == "invalid_params"` with
   upstream's sentence; `{method:"spawn", params:{agent:"x", async:false}}` → `invalid_params`.
   *Gutting it fails:* dropping either guard turns the RPC surface into a way to run a
   management/control action under a method that claims to launch, or into a blocking foreground
   launch on a fire-and-forget channel.

6. **`the_bridge_announces_ready_on_session_start`** — call the extension's REAL
   `NativeExtension::on_event(&HostEvent::SessionStart{reason:"test".into(),
   previous_session_file:None}, &HostCtx::event(..))` — the same call the host dispatcher makes, and
   the established pattern (`crates/cyrup-it/tests/subagents/fleet_inspector_integration.rs:568-576`)
   — then `host.deliver_bus_events(..)` and assert the observer (subscribed to
   `subagents:rpc:v1:ready`) received one payload whose `methods` and `events` match `ping`'s.
   *Gutting it fails:* remove the `emit_rpc_ready()` line at `native_impl.rs:412` and nothing is
   delivered.

7. **`a_child_safe_registration_answers_no_rpc`** — build the extension with
   `RegistrationMode::ChildSafe`, load it, emit a `ping` request, drain. Assert **zero** deliveries.
   *Gutting it fails:* subscribing from the wrong arm of the `match self.mode` would let a fanout
   child expose the orchestrator's spawn/stop/manage surface on the bus.

8. **`a_completion_is_announced_on_the_bus`** (pairs with §4.5) — install the completion watcher
   through the production `install_completion_watcher`, drop a result file, drain, and assert an
   `asyncComplete` payload reached a subscriber. *Gutting it fails:* remove the fifth composite
   member and `pingData`'s `events.asyncComplete` advertisement becomes a lie the test catches.

### Timing
Delivery is deferred but **not** asynchronous-in-wall-clock: `drain_bus` loops rounds inside the one
`deliver_bus_events(..).await`. No `sleep`, no `tokio::time::timeout`, no polling loop. If a test
needs one of those, the wiring is wrong.

## [AUG] 6 — behaviours that MUST be preserved

1. **Every request produces exactly one reply.** Upstream's `try`/`catch` at `rpc.ts:824-837` has no
   path that returns silently. A bridge that returns `Err` from `on_bus_event` on a bad request is a
   HANG for the caller: `BusFanout::drain_bus` contains and logs the `Err` (`facade.rs:2740-2750`)
   and fan-out continues without a reply. **`on_bus_event` returns `Ok(())` on every request it
   parsed or failed to parse.** Reserve `Err` for "the reply itself could not be emitted".
2. **`requestId` is validated before anything else** (`:773`, `assertRequestId` at `:342-347`): a
   `\r`/`\n` in the id would let a caller forge a reply topic. Rejecting it is a security property,
   not tidiness. The fallback topic on an unusable id is the literal `…reply:unknown` (`:789-795`) —
   never the raw value.
3. **`ping` answers with no session context** (`:709`, ahead of the `!ctx` throw at `:710`). A client
   must be able to discover the surface before a session exists. **Every other method fails closed
   with `no_active_session`** — do not let a missing session silently degrade into an unfiltered
   answer.
4. **`buildFleetStatus`'s session gate is fail-closed** (`:188-191`): no state, no session, or a
   session mismatch ⇒ the EMPTY fleet, and the opaque-key map is CLEARED. Returning unfiltered
   entries when the session does not match would leak one cyrup instance's live children to another
   instance sharing the per-cwd async root. The same rule, independently, on the snapshot side:
   `background/async_status_snapshot/state.rs:47-53` is STRICT in all three arms; its own test
   `the_snapshot_is_empty_when_the_state_session_does_not_match` (`state.rs:115+`) guards it and must
   keep passing.
5. **Fleet keys are opaque and never a run/async identifier** (`rpc.ts:93`'s own doc: *"Opaque key
   for client-side reconciliation; never a run or async identifier"*). `internalKey` —
   `foreground:<runId>:<index>` / `async:<asyncId>[:<index>]` — stays INSIDE the bridge; only
   `fleet-<n>` crosses the wire. Emitting the internal key would hand a bus client run ids it can
   then address directly, bypassing every session gate above.
6. **Fleet bounds hold**: `MAX_FLEET_CANDIDATES = 256` (`:197`), `MAX_FLEET_ENTRIES = 16` (`:273`),
   `MAX_AGENT_LENGTH = 96` / `MAX_GOAL_LENGTH = 512` / `MAX_METADATA_LENGTH = 128` (`:274-285`), and
   the `value.slice(0, 4_096)` pre-slice (`:122`). `totalActive` counts everything (`:196`) so
   `omitted` (`:300`) is truthful.
7. **`spawn` is detached-only** (`:521-524`) and **rejects `action`** (`:518-520`).
8. **`manage` accepts exactly the seven actions** at `:65-73` — NOT all nine cyrup knows. `id` is
   required for six of the seven (`:498-501`).
9. **`resume` accepts only `outputMode: "file-only"`** (`:551-552`).
10. **The authority consult survives.** `route_control_action`'s three-arm gate
    (`extension/tool/routing.rs:1723-1765`) covers `stop` and `steer`. An RPC caller must not be a
    bypass; routing through the tool arm keeps it, and the `Confirm`-with-no-UI arm still refuses
    (`routing.rs:1741-1745`) rather than auto-granting.
11. **The child-safe gate survives.** `RegistrationMode::ChildSafe` registers no bus topic at all, and
    even if it did, `SubagentTool::new_child_safe` sets `allow_mutating_management: false` and every
    mutating arm refuses with upstream's own sentence.
12. **UW-21's snapshot gate is not relaxed to make the RPC easier.** If the session does not match,
    the snapshot is empty. That is the answer.

## [AUG] 7 — files expected to be touched

**New**
- `crates/cyrup-ext-subagents/src/extension/rpc/mod.rs` — module doc (the client contract:
  topics, exact-topic subscription, deferred delivery), the four wire constants, the eight-method
  enum, `subagent_rpc_reply_event(request_id)`, `SubagentRpcBridge` (the `FleetKeyState` owner) and
  `handle_bus_event`.
- `crates/cyrup-ext-subagents/src/extension/rpc/envelope.rs` — request/reply envelopes,
  `SubagentRpcErrorCode`, `parse_request`, `assert_request_id`, `safe_reply_request_id`,
  `error_reply`.
- `crates/cyrup-ext-subagents/src/extension/rpc/params.rs` — `normalize_target_params`,
  `normalize_status_params`, `has_status_target`, `manage_params`, `spawn_params`, `steer_params`,
  `resume_params`, `assert_record_params`.
- `crates/cyrup-ext-subagents/src/extension/rpc/fleet.rs` — `FleetKeyState`, `build_fleet_status`,
  `public_tokens`, `display_text`, `active_state`.
- `crates/cyrup-ext-subagents/src/extension/rpc/ping.rs` — `ping_data`, `session_data`.
- `crates/cyrup-ext-subagents/src/tests/rpc_bridge_integration.rs` — the eight tests in §5.

**Modified**
- `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs` — `init` (`:142-147` tool capture,
  `:246` `subscribe_bus`), `on_event` SessionStart tail (`:412` ready emit), new `on_bus_event`
  (sited after `on_event`, before `set_host_services` at `:659`).
- `crates/cyrup-ext-subagents/src/extension/host/mod.rs` — the `OnceLock<Arc<SubagentTool>>` field on
  `SubagentsExtension` (struct at `:38-92`) and the `SubagentRpcBridge` state it owns.
- `crates/cyrup-ext-subagents/src/extension/mod.rs` — `pub(crate) mod rpc;` beside
  `mod tool;`/`mod wait_tool;`, and any `pub use` the tests need.
- `crates/cyrup-ext-subagents/src/tests/mod.rs` — `mod rpc_bridge_integration;`.
- `crates/cyrup-ext-subagents/src/background/async_status_snapshot/mod.rs` — **delete the ⚠ "no
  production caller" section at `:27-48`**, replace it with the real caller, and fix the stale
  `rpc.ts:725,749` citation at `:30-31` to `:729,753`. UW-21's closure is not complete until that
  paragraph is gone; leaving it standing while wiring the caller is exactly the
  disclosure-instead-of-completion the mandate forbids.
- `crates/cyrup-ext-subagents/src/background/watch/observer.rs` + `.../watch/mod.rs` — the
  `BusAnnouncingCompletionObserver` (§4.5).
- `crates/cyrup-ext-subagents/src/extension/executor/notices.rs` — the fifth composite member at
  `:454-486`.

**Deliberately NOT touched** (so the executor does not collide with a sibling task):
`crates/cyrup-ext/**` (the bus seam is complete — `subscribe_bus`, `on_bus_event`, `drain_bus` and
`emit_event` all exist and all work), `crates/cyrup-session-svc/**` (`attach_event_bus` at
`builder.rs:1186` already runs before the native load loop), `extension/tool/routing.rs`,
`extension/executor/control.rs`, `extension/executor/status.rs`, `background/scheduled_runs/**`,
`background/child_identity.rs`, `background/control.rs`, `background/run_id_resolver.rs`, and
`crates/cyrup-ext-subagents/src/watchdog/**` (UW-4/UW-5's territory).

## [AUG] 8 — the sanity check before calling this done

```
grep -rn 'subagents:rpc' crates/                       # must be > 0, and must include native_impl.rs
grep -rn 'allow(dead_code)' crates/cyrup-ext-subagents/src/extension/rpc/   # must be 0
grep -rn 'todo!\|unimplemented!' crates/cyrup-ext-subagents/src/extension/rpc/  # must be 0
grep -rn 'build_async_status_snapshot_for_state' crates/ --include=*.rs \
  | grep -v async_status_snapshot/                     # must be > 0  ← UW-21
grep -n 'no production caller' crates/cyrup-ext-subagents/src/background/async_status_snapshot/mod.rs  # must be 0
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2
cargo clippy -p cyrup-it --features it --all-targets
cargo nextest run --workspace --features test-fixtures # ≥ 10414 passing, 0 failed
```

---

# [EXEC] What landed

The bridge is live. A host, an editor or a sibling extension can now drive subagents over the
inter-extension bus, and UW-21 is closed — `background/async_status_snapshot/` has real production
callers for BOTH of upstream's two call sites, and its ⚠ "no production caller" section is gone.

## The production call sites

| # | site | what |
|---|---|---|
| 1 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:261` | `api.subscribe_bus(crate::extension::rpc::SUBAGENT_RPC_REQUEST_EVENT)` — `init`, `RegistrationMode::Full` arm ONLY, immediately before the existing `api.subscribe(&[…])`. pi `extension/index.ts:759-764`. |
| 2 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:145-152` | the `Arc<SubagentTool>` is now built ONCE, registered, and the same `Arc` stashed in `SubagentsExtension::rpc_tool` (`extension/host/mod.rs`, a `OnceLock`), so the bridge dispatches into the instance the model uses — same resolved description, same `allow_mutating_management`, same `DispatchGuard`. `subagent_tool()` is deliberately NOT used. |
| 3 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:664` | `NativeExtension::on_bus_event` — sited after `on_event`, before `set_host_services`. Parses, dispatches, and replies through `executor.host_services()?.emit_event(&reply_topic, &reply)`. Returns `Ok(())` on every parsed-or-unparsable request; `Err` is reserved for "the reply itself could not be emitted". |
| 4 | `crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:437` | `self.emit_rpc_ready()` at the tail of the `SessionStart` arm (impl at `extension/host/slash.rs:246`). pi `extension/index.ts:1186`. |
| 5 | `crates/cyrup-ext-subagents/src/extension/executor/notices.rs:485-491` | the FIFTH composite `CompletionObserver` — `BusAnnouncingCompletionObserver` (`background/watch/observer.rs`) — republishes every observed background completion on `subagent:async-complete`. This is what makes `pingData`'s `events.asyncComplete` a promise rather than a lie. Registered after the wait-completion store (which stays first) and before the wait-subscription reconciler (which stays last). |
| 6 | `crates/cyrup-ext-subagents/src/extension/rpc/mod.rs:397` | **UW-21 call site A** — `build_async_status_snapshot_for_state`, in `SubagentRpcBridge::fleet_and_snapshot`, reached from BOTH status tiers (pi `rpc.ts:729` and `:753`). |
| 7 | `crates/cyrup-ext-subagents/src/extension/host/slash.rs:217` (reached from `:320`) | **UW-21 call site B** — `encode_async_status_snapshot_widget`, in `publish_async_status_snapshot_widget`, reached from `refresh_fleet_status_widget` when `ExtMode::Rpc`. pi `tui/render.ts:2999-3002`: in RPC mode the widget reader is a machine, so it gets the machine document. `refresh_fleet_status_widget` gained a `mode` parameter; both production call sites (`native_impl.rs:436`, `:525`) pass `ctx.mode`. |

## New modules

`crates/cyrup-ext-subagents/src/extension/rpc/` — `mod.rs` (wire constants, the eight-method enum,
`subagent_rpc_reply_event`, `SubagentRpcBridge`, `execute_checked`, the two status tiers, the
capacity read), `envelope.rs` (`parse_request`/`assert_request_id`/`safe_reply_request_id`/
`error_reply`), `params.rs` (the eight normalizers), `fleet.rs` (`FleetKeyState`,
`build_fleet_status`, `public_tokens`, `display_text`), `ping.rs` (`ping_data`/`session_data`).

## Reachability test

`crates/cyrup-ext-subagents/src/tests/rpc_bridge_integration.rs` — eight tests, in the crate that the
merge gate actually runs. Real `ExtensionHost`, real `load_native_with_services`, real `init`, real
`host.bus().emit`, real `deliver_bus_events`. The only test-authored object is the `HostServices`
impl, whose `emit_event` body is production's one line.

Each was proven to FAIL against a deliberately gutted implementation, then restored:

| mutation | test that caught it |
|---|---|
| remove `api.subscribe_bus(…)` | `a_host_drives_ping_…` — 0 deliveries |
| gut the snapshot call to `{}` | `an_rpc_status_carries_the_fleet_and_the_async_status_snapshot` |
| attach the snapshot to the in-memory tier only | `an_rpc_status_with_a_target_still_carries_the_fleet_and_snapshot` |
| remove `self.emit_rpc_ready()` | `the_bridge_announces_ready_on_session_start` — 0 deliveries |
| also subscribe from the `ChildSafe` arm | `a_child_safe_registration_answers_no_rpc` |
| drop the fifth composite member | `a_completion_is_announced_on_the_inter_extension_bus` |
| hardcode `ping`'s `session.sessionId` | `a_host_drives_ping_…` |

## CYRUP-DELTAs recorded in code

* `not_found`/`invalid_state` are not emitted and the two variants are ABSENT from the enum — every
  tool `Err` maps to `execution_failed` carrying upstream's sentence verbatim, with no string-match
  to re-derive a narrower code (`rpc/envelope.rs`).
* `stop` routes through the tool arm rather than re-implementing `stopAsyncRun`, so the authority
  consult and the child-safe gate apply to an RPC stop — stricter than upstream (`rpc/mod.rs` doc).
* `steeringRecovery` has no analogue and is omitted (`rpc/params.rs`).
* `TokenUsage.window`/`windowPeak` have no source; the two fleet views carry only a scalar total, so
  a foreground entry's tokens are `{0, 0, total}` (`rpc/fleet.rs`).
* `canUseInMemoryStatus` keeps clauses 1-3; `statusProjectionSessionId` has no field and the two
  `instanceof Map` clauses are discharged by the type system (`rpc/mod.rs`).
* `pingData` DROPS `nonRecoveringSteer`, `launchResolvedExtensions`,
  `runtimeAcknowledgedExtensions`, `processTerminalProof`, `events.childStatus` and
  `events.processTerminal` — each with the missing seam named (`rpc/ping.rs`). The test asserts they
  are absent.
* `state.activeAsyncCapacity` is MEASURED by the dispatcher and handed to the pure projection,
  because the read reconciles (`reports.rs:92-120`'s own rule).
* `job.agents`, `step.label` and `job.activeParallelGroup` have no source (`rpc/fleet.rs`).
* `goal` is a SUPERSET: upstream declares and bounds it but never populates it; cyrup fills it from
  the run's/child's own `description` (`rpc/fleet.rs`).
* The RPC-mode snapshot widget goes into cyrup's single widget slot, where upstream has two
  (`extension/host/slash.rs`).

## Gates

```
cargo fmt --all -- --check                              clean
cargo check --workspace --all-targets                   clean
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2    clean
cargo clippy -p cyrup-it --features it --all-targets    clean (see below)
cargo nextest run --workspace --features test-fixtures  Summary [ 96.238s] 10426 tests run: 10426 passed, 9 skipped
cargo doc --workspace --no-deps --bins                  clean (only the pre-existing cargo#6313 bin/lib filename-collision note)
cargo run -p xtask -- feature-matrix --fast             feature-matrix: 15 combination(s) green
```

### One pre-existing break repaired, one pre-existing failure UNMASKED

`cargo clippy -p cyrup-it --features it --all-targets` did not COMPILE at HEAD:
`crates/cyrup-it/tests/subagents/companions_hostservices_proof.rs:198` builds a `ResultFile`
literal and `background/records.rs:593`'s `schedule_origin` field (added by SUBA-016, present in
HEAD) was missing from it. One line added, with a comment. Neither file is touched by PB-8
otherwise.

Fixing it makes the whole `cyrup-it::subagents` binary build for the first time since that field
landed — and that reveals ONE genuine pre-existing failure in it:
`companions_hostservices_proof::background_completion_injects_a_turn_triggering_message_on_the_real_host_services`
(*"inject_message never fired"*). **It is not PB-8's**: it fails identically with the new fifth
composite observer removed (measured — mutation applied, test run, mutation reverted). PB-8's own
`a_completion_is_announced_on_the_inter_extension_bus` proves the watcher and the observer fan-out
DO run over the same result file, so the break is one stage further on, in the delivery sink, not
in observation. It wants its own row.

Everything else in `cargo run -p xtask -- it` is green: `408/544 tests run: 407 passed` before
nextest's fail-fast cancelled the remainder.


# [FIX] Remediation of the QA findings

Verdict answered: every defect in the QA report is FIXED in code, and every fake test is replaced
by an assertion that discriminates. Each fix below names the mutation that was applied to the
production source, the test that turned red, and the fact that the mutation was reverted.

Gates after the fix: see [FIX] Gates at the end.

---

## [FIX] Defect 1 (major) — the RPC-mode widget hijacked the fleet-status slot

**The finding.** `extension/host/slash.rs`'s `ExtMode::Rpc` branch published the async machine
document into `FLEET_STATUS_WIDGET_KEY` and `return`ed past the human fleet-status refresh. With no
async runs it CLEARED that slot. In `--acp`/`--rpc` with live foreground subagents — the common
case, since `async_status_snapshot_jobs_for_state` reads only `state.tracked_jobs` and never
`state.foreground_controls` — the always-on fleet widget disappeared. A default-on regression.

**The false premise.** The `[CYRUP-DELTA, mechanism]` justified the hijack with *"cyrup publishes
exactly one widget from this extension, this one"*. That is not true: `background/inspect_rpc/mod.rs`
defines `INSPECT_WIDGET_KEY = "subagent-inspect"`, published by this same extension, and
`cyrup_ext::HostServices::set_widget(&self, key, lines, placement)`
(`cyrup-ext/src/host/services.rs:376`) is keyed. The delta is DELETED, because with the port made
faithful there is no divergence left to record.

**The fix — the faithful port is the fix.** Upstream's two keys are distinct at v0.68.0:

| key | constant | upstream site |
|---|---|---|
| `subagent-async` | `WIDGET_KEY` | `src/shared/types.ts:2789` |
| `subagent-fleet-status` | `FLEET_STATUS_WIDGET_KEY` | `src/tui/fleet-status.ts:14` |

`renderWidget` (`src/tui/render.ts:2991-3008`) only ever touches the first — at `:2995` (clear),
`:3000` (the RPC machine document) and `:3007`. It never touches the fleet-status slot.

So:

1. `background/async_status_snapshot/mod.rs` gains `ASYNC_STATUS_SNAPSHOT_WIDGET_KEY =
   "subagent-async"` (pi `WIDGET_KEY`, `shared/types.ts:2789`), with the reason the two slots stay
   separate written on the constant.
2. `publish_async_status_snapshot_widget` writes and clears THAT key.
3. The `ExtMode::Rpc` branch in `refresh_fleet_status_widget` no longer `return`s: upstream's
   `return` leaves `renderWidget`, whose only slot is `WIDGET_KEY`; the fleet-status widget is
   driven by its own tick and is untouched by that branch. The human widget therefore keeps
   working in `--acp` exactly as it did before.
4. `native_impl.rs`'s `SessionShutdown` arm now clears BOTH slots, which is also upstream's
   cleanup block: `fleetStatus?.dispose()` (`extension/index.ts:1063`) takes the fleet-status key
   and `:1098` takes `WIDGET_KEY`. Without this the machine document would outlive its session.
5. **Ordering.** Upstream tests `jobs.length === 0` FIRST (`:2992`), guards only the `setWidget`
   clear inside it with `ctx.hasUI` (`:2995`), and puts the bare `if (!ctx.hasUI) return` AFTER
   that branch (`:2998`). The port now matches. Honest note, written on the function: in cyrup the
   two orders are *provably equivalent* — `:2993-2994`'s `resetWidgetLayoutSession()` and
   `asyncWidgetUpdates.delete(ctx.ui)` belong to upstream's mounted-component path and have no
   cyrup analogue, so `set_widget` fires iff `has_ui && !jobs.is_empty()` either way. The order is
   matched so a future analogue lands in the right arm, and the comment says so rather than
   claiming an observable property.

**Tests added** (`src/tests/rpc_bridge_integration.rs`):

* `the_rpc_mode_machine_document_gets_its_own_slot_and_leaves_the_fleet_widget_alone` — a live
  tracked run, `AgentEnd` in `ExtMode::Rpc` with a UI: the async slot carries the prefixed JSON
  line naming the run, the fleet-status slot carries human rows, and the machine line never
  appears in the human slot.
* `an_empty_rpc_roster_clears_only_the_async_slot` — the empty-roster case, which is the one that
  made this a regression: the clear lands on `subagent-async` and the fleet-status slot is not
  touched at all.
* `tui_mode_publishes_only_the_human_fleet_widget` — the mirror image of upstream's
  `ctx.mode === "rpc"` guard.

To make those possible the harness gained a recording `set_widget` on its `HostServices`, and
`Harness::track_a_live_run`, which writes the `status.json` a real runner writes and lets the
PRODUCTION `JobTracker` tick load it — no hand-built state.

**Mutations run** (production source mutated, test run, mutation reverted):

| mutation | test that failed |
|---|---|
| empty-roster clear sent to `FLEET_STATUS_WIDGET_KEY` | `an_empty_rpc_roster_clears_only_the_async_slot` |
| non-empty publish sent to `FLEET_STATUS_WIDGET_KEY` | `the_rpc_mode_machine_document_gets_its_own_slot_and_leaves_the_fleet_widget_alone` |
| `return` restored in the `ExtMode::Rpc` branch | `the_rpc_mode_machine_document_gets_its_own_slot_and_leaves_the_fleet_widget_alone` |
| `!has_ui` checked before the empty branch | **nothing** — equivalent by construction; see point 5 |

---

## [FIX] Defect 2 (minor) — inline dispatch under the `DrainLatch`

**Not "fixed" by changing the mechanism — recorded honestly, as the task's second option allows.**

It cannot be fixed by detaching: `NativeExtension::on_bus_event` hands out `&self` with no owning
handle, and detaching would also break the guarantee the whole surface is built on — that the
reply is emitted inside the SAME `deliver_bus_events(..)` call, which is what lets a client
round-trip with no pump and nothing to sleep on.

A `[CYRUP-DELTA, mechanism]` now sits on `SubagentsExtension::on_bus_event` stating the REAL
consequence in plain words: upstream's `options.events.on(...)` handler (`rpc.ts:822`) is
fire-and-forget because pi's `EventEmitter` discards the returned promise; cyrup's is awaited by
`BusFanout::drain_bus` (`cyrup-ext/src/facade.rs:2736-2748`) under the RAII `DrainLatch` acquired
at `facade.rs:2724`, so for as long as one RPC method runs, every other extension's bus event in
the same batch waits behind it — and if the drain was entered from `run_command`
(`facade.rs:2150`) the user's slash command does not return until it finishes. The `rpc` module
doc's "Delivery is deferred, but not slow" paragraph now points at that delta instead of leaving
the impression that nothing is paid.

---

## [FIX] Defect 3 (minor) — `params: null` was accepted as `{}`

`assert_record_params` mapped `Some(Value::Null)` to `Ok(Map::new())`. Upstream's
`assertRecordParams` (`rpc.ts:349-353`) returns `{}` only for `undefined`; `parseRequest` (`:784`)
carries an explicit `null` through as a PRESENT value and `isRecord(null)` is false, so upstream
THROWS `invalid_params`. `{"method":"spawn","params":null}` therefore reached
`SubagentTool::execute` in cyrup — taking a single-dispatch slot and a spawn-budget read — where
upstream answers before the tool is entered.

**Fix**: `None => Ok(Map::new())`, and `Some(Value::Null)` falls into the refusal arm with every
other non-record. The `undefined`/`null` distinction is documented on the function as the
load-bearing thing it is.

**Tests**: `params::tests::an_explicit_null_params_is_refused_where_an_absent_one_is_an_empty_record`
(unit) and `an_explicit_null_params_is_refused_on_the_wire` (all seven non-`ping` methods over the
bus, plus the positive control that an ABSENT `params` still succeeds).

**Mutation**: folding `Some(Value::Null)` back into `{}` — both tests failed. Reverted.

---

## [FIX] Defect 5 (minor) — the "in-memory" status tier mutated on a read

The serious half is fixed, and the rest is recorded truthfully.

**The mutation on a read is gone.** `active_async_capacity` called
`get_active_async_capacity_snapshot`, which is a bare alias of `reconcile_active_async_capacity`
(`background/active_async_capacity/sweep.rs:105-119`) and DELETES slots it can prove released. It
now calls `capacity::snapshot_for` (`sweep.rs:25-48`), whose own doc reads *"Counts, does not
reconcile"*. That is also closer to upstream, whose `state.activeAsyncCapacity` (`rpc.ts:301`) is a
plain field read; the sweeping call belongs to the admission path, where it still lives.

**The wasted measurement is gone.** `build_fleet_status`'s fail-closed gate is hoisted into
`fleet::fleet_gate_open`, and `fleet_and_snapshot` asks it BEFORE measuring: a closed gate returns
`empty_fleet()`'s hardcoded `{used: 0, limit: 0}`, so measuring first was I/O for a number
immediately discarded — on a path an external bus client can drive in a loop.

**What remains is recorded, not hidden.** A `[CYRUP-DELTA, mechanism]` on `handle_status` states
that cyrup's fast tier is a MATERIALIZED projection, not a live cache: `fleet_state` builds it per
call, and with `include_history: false` (what both tiers pass) the on-disk async root is not
listed, but each tracked job still has its nested children read from disk
(`extension/executor/status.rs:262`'s `read_nested_children`). So the tier costs one bounded read
per live background run rather than nothing — and, critically, it no longer MUTATES anything.

**Tests**: `fleet::tests::the_hoisted_gate_predicate_agrees_with_the_projection` pins that the
hoisted predicate and the projection's own gate can never disagree, and that only an open gate
publishes a measured capacity. The two `status` integration tests now assert the configured
capacity limit (see Defect 4), which is the value the gate decides whether to publish.

---

## [FIX] Defect 4 (minor) + the fake tests — `extension/rpc/` had no unit tests, and 5 of 8 methods had none at all

`extension/rpc/` now has two `#[cfg(test)]` modules (22 unit tests) and the integration file grew
from 8 tests to 17.

### `fleet.rs` — 13 unit tests

`the_session_gate_is_fail_closed_on_all_three_arms`, `the_hoisted_gate_predicate_agrees_with_the_projection`,
`entry_keys_are_opaque_and_never_carry_the_run_id`,
`keys_are_stable_across_polls_and_evicted_when_the_candidate_goes_away`,
`a_session_change_rotates_the_whole_key_map`,
`the_entry_window_is_bounded_at_sixteen_and_omitted_stays_truthful`,
`candidate_collection_is_capped_at_two_hundred_and_fifty_six`,
`every_display_field_is_bounded_at_its_own_length`,
`display_text_sanitizes_before_it_truncates_and_omits_an_empty_result`,
`an_undisplayable_candidate_is_omitted_rather_than_uncounted`,
`public_tokens_reconciles_a_total_that_disagrees_with_its_parts`,
`a_run_with_parallel_children_contributes_one_candidate_per_child`,
`a_tracked_job_from_another_session_is_not_a_fleet_member`.

The candidate-cap test is built so the cap is OBSERVABLE: the 300 controls carry DESCENDING start
times, so candidates 256..299 are the oldest and would sort to the front of the window if the cap
were removed. With the cap, the window opens on `agent-255`.

**Mutations run — every one caught, every one reverted:**

| mutation (`fleet.rs`) | test that failed |
|---|---|
| entry `key` = `candidate.internal_key` (`foreground:<runId>:<i>`) | `entry_keys_are_opaque_and_never_carry_the_run_id` (+3 more) |
| `entries.len() >= MAX_FLEET_ENTRIES` → `>= 1600` | `the_entry_window_is_bounded_at_sixteen_and_omitted_stays_truthful` |
| fail-closed session-MISMATCH arm deleted | `the_session_gate_is_fail_closed_on_all_three_arms`, `the_hoisted_gate_predicate_agrees_with_the_projection` |
| key-eviction `retain` removed | `keys_are_stable_across_polls_and_evicted_when_the_candidate_goes_away` |
| `truncate_display(&normalized, max_length)` → `normalized` | `every_display_field_is_bounded_at_its_own_length` |
| `MAX_FLEET_CANDIDATES` cap removed | `candidate_collection_is_capped_at_two_hundred_and_fifty_six` |
| whole `build_fleet_status` body → `empty_fleet()` | 4 fleet tests |

### `params.rs` — 9 unit tests, covering all five normalizers

`an_explicit_null_params_is_refused_where_an_absent_one_is_an_empty_record`,
`a_target_normalizer_passes_four_keys_and_drops_everything_else`,
`status_params_carry_view_and_lines_and_both_count_as_targets`,
`manage_accepts_exactly_seven_actions_and_names_them_in_its_refusal`,
`manage_requires_an_id_for_six_of_seven_and_checks_its_type_first`,
`manage_quiet_is_a_boolean_and_only_true_is_forwarded`,
`steer_requires_a_message_a_target_and_a_known_mode`,
`resume_takes_a_message_a_target_and_only_file_only_output`,
`spawn_is_detached_only_and_refuses_any_action_key`.

### The five untested METHODS, now driven end to end over the bus

`rpc_manage_enforces_the_seven_action_allowlist_and_the_id_requirement`,
`rpc_steer_gates_message_target_and_mode_then_dispatches`,
`rpc_resume_gates_message_target_and_output_mode_then_dispatches`,
`rpc_stop_and_interrupt_take_only_target_keys`.

Each proves the method is WIRED, not merely validated: a well-formed request at a target that does
not exist comes back `execution_failed`, which only `SubagentTool::execute` can produce;
`manage schedule.list` comes back with the *scheduled-runs tool's* own refusal sentence, which the
bridge has no way to author. The stop/interrupt test additionally pins the four-key target
whitelist by asserting a smuggled `agent` never appears in the reply, and `stop` threads `childId`
far enough for `validate_stop_child_id` to answer against a REAL tracked target.

**Mutations run — caught and reverted:**

| mutation (`params.rs`) | tests that failed |
|---|---|
| steer mode allowlist disabled | `steer_requires_a_message_a_target_and_a_known_mode`, `rpc_steer_gates_message_target_and_mode_then_dispatches` |
| manage `id` requirement disabled | `manage_requires_an_id_for_six_of_seven_and_checks_its_type_first`, `rpc_manage_enforces_the_seven_action_allowlist_and_the_id_requirement` |
| resume `outputMode` restriction disabled | `resume_takes_a_message_a_target_and_only_file_only_output`, `rpc_resume_gates_message_target_and_output_mode_then_dispatches` |

### The three fake tests, made to discriminate

* `an_rpc_status_carries_the_fleet_and_the_async_status_snapshot` — `is_array()` / `is_number()` /
  `version: 1` are all satisfied by `fleet::empty_fleet()`'s literal. They are replaced by an
  assertion on `topLevelAsyncCapacity`, against a harness config that sets
  `max_active_async_runs_per_session = 7`. `empty_fleet()` reports `{used: 0, limit: 0}`; only a
  projection that passed the session gate and was handed the dispatcher's measurement reports 7.
* `an_rpc_status_with_a_target_still_carries_the_fleet_and_snapshot` — same discriminator on the
  executor tier.
* A new `an_rpc_status_describes_a_live_run_without_leaking_its_id` covers the half
  `empty_fleet()` cannot fake at all: with a live tracked run the reply's summary sentence uses
  pi's SINGULAR noun (*"1 active child"*), `totalActive` is 1, the one entry's `agent` is the run's
  real step agent, its `key` is the opaque `fleet-1`, and the run id appears nowhere in the fleet
  block while the `asyncSnapshot` block does name it.
* `a_child_safe_registration_answers_no_rpc` — an empty-deliveries assertion survives a full
  gutting. It now runs the POSITIVE control first, on the identical envelope, through a `Full`
  harness, and asserts the live backend's session id comes back; only then does it assert the
  `ChildSafe` silence.

---

## [FIX] Gates

```
cargo fmt --all -- --check                              clean (exit 0)
cargo clippy --workspace --all-targets -- -D warnings   clean (exit 0)
cargo clippy -p cyrup-ext-sdk --target wasm32-wasip2    clean (exit 0)
cargo clippy -p cyrup-it --features it --all-targets    clean (exit 0)
cargo doc --workspace --no-deps --bins                  clean (exit 0)
cargo nextest run --workspace --features test-fixtures
     Summary [ 107.611s] 10466 tests run: 10466 passed, 9 skipped
```

**No regression.** The tree stood at 10435 passed / 9 skipped; it now stands at 10466 passed /
9 skipped. The +31 are exactly this remediation's new tests: 13 in `extension/rpc/fleet.rs`, 9 in
`extension/rpc/params.rs`, and 9 added to `tests/rpc_bridge_integration.rs` (8 → 17). Skipped is
unchanged at 9.

No `#[allow(dead_code)]`, no `todo!()`, no new stub. No git command that writes was run. Upstream
was read only through `git -C tmp/pi-subagents show v0.68.0:<path>`.
